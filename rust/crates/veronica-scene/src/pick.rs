//! On-demand GPU ID pass resolving tap NDC to face identity (ADR-0007, T3).
//!
//! [`SceneWorld::resolve_pick`] answers one question — which face is under
//! this tap — with two transient GPU passes on render layer 1, invisible to
//! the beauty camera on layer 0:
//!
//! 1. **Entity pass.** Every cooked mesh renders once more, each in a flat
//!    [`slot_color`] (entity slot as 24-bit RGB, hit alpha), reusing the
//!    cooked GPU mesh handles — no new geometry. The tap texel names the
//!    operator, or the tap misses.
//! 2. **Face pass.** Only on an entity hit: the hit operator's cooked mesh
//!    is unwelded into [`id_mesh_from_cooked`] (triangle *k* becomes three
//!    private vertices all carrying `encode_face_ordinal(k)`) and rendered
//!    under a white unlit material. Same tap texel resolves the triangle.
//!
//! Each pass ticks until its target shows painted pixels ([`frame_painted`],
//! bounded by [`PICK_PASS_UPDATE_BUDGET`]): a fresh transient camera needs
//! warmup frames (view setup, pipeline compile) during which the target
//! holds only clear values, and reading the tap texel then would misreport
//! a hit as a background tap. The loop absorbs slow shader compiles instead
//! of misreading; exhausting the budget means real breakage (a non-empty
//! scene always paints somewhere), so it fails loudly instead of returning
//! a lying miss.
//!
//! Everything spawned or uploaded (ID camera, ID target, overlays, transient
//! materials and meshes) is torn down before return, on success and on
//! failure alike — between picks the world holds exactly what it held
//! before, so the per-tick path renders no differently than without picking.
//! Per-tap cost is a few extra frames per pass plus full-target readbacks,
//! independent of triangle count; CPU work is one mesh unweld per hit entity.
//!
//! **Exactness chain** (each link pinned by the center-tap GPU test — a break
//! anywhere fails loudly instead of mis-picking): white unlit materials emit
//! vertex color verbatim (Bevy multiplies material base by vertex color, and
//! white is the identity); `fog_enabled: false` (fog would blend by depth);
//! [`Tonemapping::None`] (any curve would remap the bytes);
//! [`DebandDither::Disabled`]; an `Rgba8Unorm` target (no sRGB encode on
//! write); linear-exact `k/255` colors (the unorm8 quantize rounds back to
//! `k`); transparent-black clear (background reads as the T2 miss pixel).
//!
//! **MSAA rule and its limit.** The target is multisampled (Bevy default),
//! so silhouette pixels blend ID color with clear — including a partial
//! alpha. A hit requires full-coverage alpha
//! ([`HIT_ALPHA`][veronica_geometry::HIT_ALPHA]); anything else is a miss,
//! so a silhouette-edge tap deselects instead of mis-highlighting.
//!
//! The gate does not cover texels straddling two opaque faces: MSAA resolve
//! averages their RGB while alpha stays full, and the blended triple can
//! decode to a third ordinal. Out-of-range blends deselect (the entity slot
//! lookup and [`face_in_range`] admit only attributable identities), but an
//! in-range blend aliases to a real — wrong — face. Pixel-perfect seam taps
//! are rare at MVP mesh sizes, and per-target single-sample rendering is not
//! available in Bevy (sample count is a global resource; toggling it would
//! rebuild every pipeline including beauty's), so this stays a documented
//! limitation until dense-mesh picking earns its own design.

use crate::{SceneError, SceneWorld, gpu, render::FRAME_BYTES_PER_PIXEL};
use bevy_asset::{Assets, Handle, RenderAssetUsages};
use bevy_camera::{
    Camera, Camera3d, CameraOutputMode, ClearColorConfig, Hdr, Projection, RenderTarget,
    visibility::{RenderLayers, Visibility},
};
use bevy_color::{Color, LinearRgba};
use bevy_core_pipeline::tonemapping::{DebandDither, Tonemapping};
use bevy_ecs::prelude::*;
use bevy_image::Image;
use bevy_mesh::{Indices, Mesh, Mesh3d, PrimitiveTopology, VertexAttributeValues};
use bevy_pbr::{MeshMaterial3d, StandardMaterial};
use bevy_render::render_resource::{TextureFormat, TextureUsages};
use bevy_transform::prelude::Transform;
use veronica_core::NodeId;
use veronica_geometry::{HIT_ALPHA, MAX_FACE_ORDINAL, decode_face_ordinal, encode_face_ordinal};

/// Render layer carrying the transient pick overlays plus the ID camera.
///
/// The beauty camera stays on the default layer 0, so pick geometry never
/// touches the presented frame — and the ID camera never sees beauty
/// geometry either, so its target holds only ID colors and clear.
const PICK_RENDER_LAYER: usize = 1;

/// Maximum pick-pass updates waiting for painted pixels before the pass is
/// declared broken (loud) instead of missed (silent).
///
/// Calibrated with margin: a fresh transient camera lands its first painted
/// frame on update 3 on dev hardware (update 1: output clear only, update 2:
/// clear color without draws). Slow shader compiles take longer, and the
/// readiness loop below absorbs them instead of misreading an unpainted
/// frame as a background tap.
const PICK_PASS_UPDATE_BUDGET: u32 = 10;

/// A resolved tap: the operator whose mesh was hit, and which triangle of
/// its realized mesh (glossary `Face`, ADR-0007 identity contract).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pick {
    /// Source operator of the hit mesh.
    pub node: NodeId,
    /// Triangle ordinal into the operator's realized mesh.
    pub face: u32,
}

/// Map NDC to the tap pixel, or `None` when the tap addresses no pixel.
///
/// NDC `(-1, -1)` is the bottom-left texel, `(1, 1)` the top-right (texture
/// row 0 is the top row; NDC +Y is up). Non-finite coordinates and points
/// outside the `[-1, 1]` square address nothing — the caller treats them as
/// a miss (deselect), never an error.
#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the NDC guard bounds both scales to [0, extent]; `as` is exact in range"
)]
pub(crate) fn ndc_to_pixel(ndc_x: f32, ndc_y: f32, width: u32, height: u32) -> Option<(u32, u32)> {
    if !ndc_x.is_finite() || !ndc_y.is_finite() {
        return None;
    }
    if !(-1.0..=1.0).contains(&ndc_x) || !(-1.0..=1.0).contains(&ndc_y) {
        return None;
    }
    // f64 intermediates: extents cap at 2048, far below 2^53, so the scale
    // is exact and only `floor` discards.
    let column = ((f64::from(ndc_x) + 1.0) * 0.5 * f64::from(width)).floor() as u32;
    let row = ((1.0 - f64::from(ndc_y)) * 0.5 * f64::from(height)).floor() as u32;
    // The exact-edge case (`ndc == 1`) lands one past; fold it onto the
    // last texel.
    let column = column.min(width.saturating_sub(1));
    let row = row.min(height.saturating_sub(1));
    Some((column, row))
}

/// Linear-exact channel value for one ID-buffer byte.
///
/// `byte/255` in `f32` needs ~16 significant bits against the 24-bit
/// mantissa, so the unorm8 quantize on write rounds back to `byte` exactly —
/// the property the center-tap GPU test pins end to end.
#[must_use]
fn linear_channel(byte: u8) -> f32 {
    f32::from(byte) / 255.0
}

/// Largest triangle total the ID space addresses.
///
/// Ordinals run `0..=MAX_FACE_ORDINAL`, so a mesh holds one more triangle
/// than the max ordinal. Counts and ordinals differ by one — [`u24_checked`]
/// below bounds ordinals, this bounds totals.
const MAX_PICK_TRIANGLES: usize = MAX_FACE_ORDINAL as usize + 1;

/// Narrow an ordinal into the 24-bit ID space, failing loudly past it.
///
/// Ordinal-bound, not count-bound: entity slots are indices (`0..=MAX` is
/// valid), while triangle totals allow one more (see
/// [`MAX_PICK_TRIANGLES`]).
///
/// # Errors
///
/// Returns [`SceneError::PickSpaceExhausted`] when `ordinal` exceeds
/// [`MAX_FACE_ORDINAL`].
fn u24_checked(ordinal: usize) -> Result<u32, SceneError> {
    u32::try_from(ordinal)
        .ok()
        .filter(|value| *value <= MAX_FACE_ORDINAL)
        .ok_or(SceneError::PickSpaceExhausted { count: ordinal })
}

/// Whether a triangle total fits the ID space: `0..=MAX_PICK_TRIANGLES`
/// triangles carry encodable ordinals.
#[must_use]
fn triangle_total_fits(count: usize) -> bool {
    count <= MAX_PICK_TRIANGLES
}

/// Exact linear color for one entity slot.
///
/// Channels go through [`linear_channel`]; alpha is always full coverage
/// (see the module-level MSAA rule).
///
/// # Errors
///
/// Returns [`SceneError::PickSpaceExhausted`] when `slot` exceeds the
/// 24-bit range.
fn slot_color(slot: usize) -> Result<Color, SceneError> {
    let ordinal = u24_checked(slot)?;
    let rgb =
        encode_face_ordinal(ordinal).map_err(|_| SceneError::PickSpaceExhausted { count: slot })?;
    Ok(Color::LinearRgba(LinearRgba::new(
        linear_channel(rgb[0]),
        linear_channel(rgb[1]),
        linear_channel(rgb[2]),
        1.0,
    )))
}

/// Build the transient per-face ID mesh for one cooked mesh.
///
/// Unwelded triangle soup: triangle *k* becomes three private vertices all
/// carrying `encode_face_ordinal(k)` as linear-exact vertex colors, so
/// interpolation is constant across the face even where the source mesh
/// shares vertices. Positions come straight from the cooked mesh (same
/// coverage as the beauty pass, pixel for pixel); normals and uvs are
/// dropped — the unlit ID material reads neither.
///
/// Accepts indexed `U32`/`U16` triangle lists and non-indexed position soup
/// (consecutive triples); anything else fails loudly instead of guessing.
///
/// # Errors
///
/// Returns [`SceneError::MissingAttribute`] when positions are absent,
/// [`SceneError::UnpickableMesh`] for non-triangle topology or a ragged
/// index/vertex soup, [`SceneError::PickSpaceExhausted`] past the 24-bit
/// range, and [`SceneError::IndexOutOfBounds`] when an index names no vertex.
///
/// Note: a present-but-misshaped position channel needs no arm — Bevy's
/// `insert_attribute` panics on format mismatch for standard attributes, so
/// a live mesh either carries `Float32x3` positions or none at all.
fn id_mesh_from_cooked(mesh: &Mesh) -> Result<Mesh, SceneError> {
    if mesh.primitive_topology() != PrimitiveTopology::TriangleList {
        return Err(SceneError::UnpickableMesh);
    }
    let Some(VertexAttributeValues::Float32x3(positions)) =
        mesh.attribute(Mesh::ATTRIBUTE_POSITION)
    else {
        return Err(SceneError::MissingAttribute {
            name: Mesh::ATTRIBUTE_POSITION.name.to_owned(),
        });
    };
    let triangles: Vec<[u32; 3]> = match mesh.indices() {
        Some(Indices::U32(indices)) => {
            if indices.len() % 3 != 0 {
                return Err(SceneError::UnpickableMesh);
            }
            indices
                .chunks_exact(3)
                .map(|corners| [corners[0], corners[1], corners[2]])
                .collect()
        }
        Some(Indices::U16(indices)) => {
            if indices.len() % 3 != 0 {
                return Err(SceneError::UnpickableMesh);
            }
            indices
                .chunks_exact(3)
                .map(|corners| {
                    [
                        u32::from(corners[0]),
                        u32::from(corners[1]),
                        u32::from(corners[2]),
                    ]
                })
                .collect()
        }
        None => {
            if positions.len() % 3 != 0 {
                return Err(SceneError::UnpickableMesh);
            }
            let vertex_count =
                u32::try_from(positions.len()).map_err(|_| SceneError::PickSpaceExhausted {
                    count: positions.len(),
                })?;
            (0..vertex_count)
                .step_by(3)
                .map(|base| [base, base + 1, base + 2])
                .collect()
        }
    };
    if !triangle_total_fits(triangles.len()) {
        return Err(SceneError::PickSpaceExhausted {
            count: triangles.len(),
        });
    }
    let vertex_count = triangles.len() * 3;
    let mut id_positions = Vec::with_capacity(vertex_count);
    let mut id_colors = Vec::with_capacity(vertex_count);
    for (order, triangle) in triangles.iter().enumerate() {
        let ordinal = u32::try_from(order).map_err(|_| SceneError::PickSpaceExhausted {
            count: triangles.len(),
        })?;
        debug_assert!(
            ordinal <= MAX_FACE_ORDINAL,
            "count pre-check bounds every ordinal"
        );
        let rgb = encode_face_ordinal(ordinal).map_err(|_| SceneError::PickSpaceExhausted {
            count: triangles.len(),
        })?;
        let color = [
            linear_channel(rgb[0]),
            linear_channel(rgb[1]),
            linear_channel(rgb[2]),
            1.0,
        ];
        for index in triangle {
            let position = positions
                .get(*index as usize)
                .ok_or(SceneError::IndexOutOfBounds {
                    index: *index,
                    vertex_count: positions.len(),
                })?;
            id_positions.push(*position);
            id_colors.push(color);
        }
    }
    let mut id_mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    id_mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, id_positions);
    id_mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, id_colors);
    Ok(id_mesh)
}

/// Whether one read-back ID pixel is a full-coverage hit.
///
/// MSAA blends silhouette pixels with the clear color, including a partial
/// alpha — so only full-coverage alpha counts as a hit. Partial coverage is
/// a miss (deselect), never a mis-pick: see the module docs. RGB black with
/// hit alpha is face zero, not a miss (miss travels in alpha alone, per the
/// T2 codec).
#[must_use]
fn is_full_coverage_hit(pixel: [u8; 4]) -> bool {
    pixel[3] == HIT_ALPHA
}

/// Whether any pixel differs from the pick clear (transparent black).
///
/// The readiness signal for a pick pass: a non-empty scene always paints
/// somewhere once its pipelines are compiled, so an all-clear frame means
/// "not rendered yet", never "background tap".
#[must_use]
fn frame_painted(frame: &[u8]) -> bool {
    const PICK_CLEAR: [u8; 4] = [0, 0, 0, 0];
    frame.chunks_exact(4).any(|pixel| pixel != PICK_CLEAR)
}

/// Read the tap texel out of a tight row-major frame, or `None` when the
/// coordinates address no texel.
///
/// `(pixel_x, pixel_y)` come from [`ndc_to_pixel`] for the same extents, so
/// the index is always in bounds in practice — but the math stays checked
/// anyway: an out-of-range tap degrades to a miss rather than panicking.
fn pick_pixel(frame: &[u8], pixel_x: u32, pixel_y: u32, width: u32) -> Option<[u8; 4]> {
    let row_start = (pixel_x as usize)
        .checked_add((pixel_y as usize).checked_mul(width as usize)?)?
        .checked_mul(FRAME_BYTES_PER_PIXEL)?;
    let end = row_start.checked_add(FRAME_BYTES_PER_PIXEL)?;
    frame.get(row_start..end)?.try_into().ok()
}

/// Whether a decoded face ordinal names a real triangle of a mesh with
/// `triangle_total` triangles.
///
/// Backstop against MSAA seam blends (see the module docs): a texel
/// straddling two opaque faces resolves both at full alpha, and the blended
/// triple can decode outside the mesh. That pixel must deselect — never mint
/// an impossible identity for the selection slice to choke on. (A blend that
/// lands inside the range still aliases to a nearby face; see the module
/// docs for why that stays a documented limitation.)
#[must_use]
fn face_in_range(face: u32, triangle_total: usize) -> bool {
    (face as usize) < triangle_total
}

/// One cooked mesh eligible for the entity pass: everything the transient
/// overlays need, snapshotted up front so the pass never re-queries live
/// entities mid-flight.
#[derive(Debug, Clone)]
struct PickableMesh {
    /// Source operator: the identity half of a resolved pick.
    node: NodeId,
    /// Cooked GPU mesh: template for the entity-pass overlay plus the face
    /// pass's position source. The overlay uploads a COLOR-stripped clone
    /// (see `begin_pick_pass`), never this handle directly.
    mesh: Handle<Mesh>,
    /// World transform, copied so overlays cover the beauty mesh exactly.
    transform: Transform,
}

/// One transient asset uploaded for a pick pass, removed at teardown.
#[derive(Debug)]
enum PickAsset {
    /// An uploaded ID mesh (face pass).
    Mesh(Handle<Mesh>),
    /// A flat slot material (entity pass) or the white face material.
    Material(Handle<StandardMaterial>),
}

/// Transient handles owned by one pick: every entity spawned and every asset
/// uploaded for the two ID passes. [`SceneWorld::cleanup_pick_pass`] consumes
/// this on all paths, so a pick never leaks entities or assets into the
/// tick world.
#[derive(Debug)]
struct PickPass {
    /// The layer-1 ID camera.
    camera: Entity,
    /// Entity-pass overlays (drained and replaced by the face overlay).
    overlays: Vec<Entity>,
    /// The `Rgba8Unorm` ID target both passes render into.
    target: Handle<Image>,
    /// Transient uploads to remove at teardown.
    assets: Vec<PickAsset>,
    /// Slot-indexed pickables: overlay *i* paints slot *i*.
    pickables: Vec<PickableMesh>,
}

impl SceneWorld {
    /// Resolve a tap in NDC to the face under it, or `None` for a miss.
    ///
    /// Two transient GPU passes (see the module docs): an entity pass over
    /// all cooked meshes, then a face pass over the hit operator's unwelded
    /// ID mesh. Each pass ticks until its target shows painted pixels
    /// (bounded warmup — never a misread unpainted frame). Everything built
    /// is torn down before return, so plain ticks cost nothing. The ID
    /// camera clones the live viewport camera's transform and projection, so
    /// orbiting then tapping hits the face now under the tap.
    ///
    /// Non-finite or out-of-range NDC, an empty scene, a background pixel,
    /// a partial-coverage (MSAA edge) pixel, and an out-of-range blend all
    /// resolve to `Ok(None)`: a tap that cannot be attributed to a face is
    /// a deselect. (The one exception is an in-range MSAA seam blend, which
    /// aliases to a nearby face — documented limitation, see module docs.)
    ///
    /// Runs [`SceneWorld::update`] internally — call off the Swift
    /// `MainActor` thread like every tick (the FFI slice owns this rule).
    ///
    /// # Errors
    ///
    /// Returns [`SceneError::NoViewportCamera`] when the viewport camera is
    /// gone, [`SceneError::PickPassNotReady`] when a pass shows no painted
    /// pixel within its update budget, [`SceneError::UnpickableMesh`] /
    /// [`SceneError::PickSpaceExhausted`] / handoff errors when a cooked mesh
    /// cannot be pick-meshed, or the staging, poll, and map [`SceneError`]
    /// variants when the copy-back fails.
    pub fn resolve_pick(&mut self, ndc_x: f32, ndc_y: f32) -> Result<Option<Pick>, SceneError> {
        let (width, height) = self.viewport_size();
        let Some((pixel_x, pixel_y)) = ndc_to_pixel(ndc_x, ndc_y, width, height) else {
            return Ok(None);
        };
        let cooked = self.cooked_entities();
        let pickables = self.pickable_meshes(&cooked);
        if pickables.is_empty() {
            // No meshed operators: background always misses, with no GPU
            // work at all (deterministic even without an adapter).
            return Ok(None);
        }
        let mut pass = self.begin_pick_pass(pickables)?;
        let result = self.finish_pick_pass(&mut pass, pixel_x, pixel_y);
        self.cleanup_pick_pass(pass);
        result
    }

    /// Cooked entities reduced to what the entity pass needs: node id, GPU
    /// mesh handle, and world transform. Entities without a render mesh are
    /// skipped — slots index this list, never the raw cook order.
    fn pickable_meshes(&mut self, cooked: &[(NodeId, Entity)]) -> Vec<PickableMesh> {
        let mut pickables = Vec::with_capacity(cooked.len());
        for (node, entity) in cooked {
            let world = self.app.world();
            let mesh = world.get::<Mesh3d>(*entity).map(|handle| handle.0.clone());
            let transform = world.get::<Transform>(*entity).copied().unwrap_or_default();
            if let Some(mesh) = mesh {
                pickables.push(PickableMesh {
                    node: *node,
                    mesh,
                    transform,
                });
            }
        }
        pickables
    }

    /// Clone a cooked mesh minus its COLOR binding for the entity-pass
    /// overlay: cooked meshes carry the selection COLOR channel, and the
    /// stock shader multiplies slot base by vertex color, so the overlay
    /// must not inherit it (a highlighted face would decode `slot × tint`).
    ///
    /// # Errors
    ///
    /// Returns [`SceneError::UnpickableMesh`] when the handle resolves to
    /// no uploaded mesh. Unreachable single-threaded — the handle came
    /// from a live entity in this same call stack, and nothing despawns or
    /// evicts between — but loud rather than a guess.
    fn stripped_overlay_mesh(&self, handle: &Handle<Mesh>) -> Result<Mesh, SceneError> {
        let assets = self.app.world().resource::<Assets<Mesh>>();
        let Some(source) = assets.get(handle) else {
            return Err(SceneError::UnpickableMesh);
        };
        let mut clone = source.clone();
        clone.remove_attribute(Mesh::ATTRIBUTE_COLOR);
        Ok(clone)
    }

    /// Spawn the transient entity pass: ID target, ID camera cloning the
    /// live camera, one flat overlay per pickable.
    ///
    /// All fallible work (camera snapshot, slot range checks) runs before
    /// the first spawn, so a returned pass is always complete and
    /// teardown-safe.
    fn begin_pick_pass(&mut self, pickables: Vec<PickableMesh>) -> Result<PickPass, SceneError> {
        let (eye, projection) = {
            let camera = self.viewport_camera()?;
            let world = self.app.world();
            let eye = world
                .get::<Transform>(camera)
                .ok_or(SceneError::NoViewportCamera)?
                .to_owned();
            let projection = world
                .get::<Projection>(camera)
                .ok_or(SceneError::NoViewportCamera)?
                .clone();
            (eye, projection)
        };
        let mut colors = Vec::with_capacity(pickables.len());
        for slot in 0..pickables.len() {
            colors.push(slot_color(slot)?);
        }
        let (width, height) = self.viewport_size();
        let target = {
            let mut images = self.app.world_mut().resource_mut::<Assets<Image>>();
            let mut image =
                Image::new_target_texture(width, height, TextureFormat::Rgba8Unorm, None);
            image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
            images.add(image)
        };
        let camera = self
            .app
            .world_mut()
            .spawn((
                Camera3d::default(),
                Camera {
                    // Transparent black everywhere a clear can apply: the
                    // main clear (background reads back as the T2 miss
                    // pixel) and the output-mode clear (an unrendered first
                    // frame must read as "not ready", not as painted — the
                    // default gray would trip the readiness signal).
                    clear_color: ClearColorConfig::Custom(Color::LinearRgba(LinearRgba::new(
                        0.0, 0.0, 0.0, 0.0,
                    ))),
                    output_mode: CameraOutputMode::Write {
                        blend_state: None,
                        clear_color: ClearColorConfig::Custom(Color::LinearRgba(LinearRgba::new(
                            0.0, 0.0, 0.0, 0.0,
                        ))),
                    },
                    ..Default::default()
                },
                RenderTarget::from(target.clone()),
                projection,
                eye,
                RenderLayers::layer(PICK_RENDER_LAYER),
                Tonemapping::None,
                DebandDither::Disabled,
            ))
            .id();
        // Strip the auto-required `Hdr` marker: an HDR camera strands its
        // render in the intermediate texture when `Tonemapping::None` skips
        // the blit (leaving the default clear behind), while an LDR camera
        // renders straight into the target with in-shader tonemapping —
        // which `None` disables, emitting the ID colors verbatim.
        self.app.world_mut().entity_mut(camera).remove::<Hdr>();
        let mut pass = PickPass {
            camera,
            overlays: Vec::with_capacity(pickables.len()),
            target,
            assets: Vec::with_capacity(pickables.len() * 2 + 2),
            pickables,
        };
        // One flat overlay per pickable. The overlay mesh is a COLOR-stripped
        // clone of the cooked mesh, never the cooked handle itself: cooked
        // meshes carry the selection COLOR binding (white base, orange on a
        // picked face), and the stock shader multiplies slot base by vertex
        // color — reusing the handle would decode `slot × tint` on a
        // highlighted face instead of the slot. Stripping keeps slot pixels
        // exact regardless of highlight state.
        let mut overlays = Vec::with_capacity(pass.pickables.len());
        for (pickable, color) in pass.pickables.iter().zip(colors) {
            let stripped = self.stripped_overlay_mesh(&pickable.mesh)?;
            let handle = self
                .app
                .world_mut()
                .resource_mut::<Assets<Mesh>>()
                .add(stripped);
            pass.assets.push(PickAsset::Mesh(handle.clone()));
            overlays.push((handle, pickable.transform, color));
        }
        for (mesh, transform, color) in overlays {
            let material = self
                .app
                .world_mut()
                .resource_mut::<Assets<StandardMaterial>>()
                .add(StandardMaterial {
                    base_color: color,
                    unlit: true,
                    fog_enabled: false,
                    ..Default::default()
                });
            pass.assets.push(PickAsset::Material(material.clone()));
            let overlay = self
                .app
                .world_mut()
                .spawn((
                    Mesh3d(mesh),
                    MeshMaterial3d(material),
                    transform,
                    Visibility::Visible,
                    RenderLayers::layer(PICK_RENDER_LAYER),
                ))
                .id();
            pass.overlays.push(overlay);
        }
        Ok(pass)
    }

    /// Run both passes to a pick: entity frame first, then — on a hit — the
    /// face frame for the hit operator's ID mesh. Tap texels index out of
    /// the painted frames.
    fn finish_pick_pass(
        &mut self,
        pass: &mut PickPass,
        pixel_x: u32,
        pixel_y: u32,
    ) -> Result<Option<Pick>, SceneError> {
        let (width, height) = self.viewport_size();
        let entity_frame = self.update_until_painted(&pass.target, width, height)?;
        let Some(entity_pixel) = pick_pixel(&entity_frame, pixel_x, pixel_y, width) else {
            return Ok(None);
        };
        if !is_full_coverage_hit(entity_pixel) {
            // Background, or a partial-coverage MSAA edge pixel: a miss
            // (see the module-level MSAA rule).
            return Ok(None);
        }
        let slot = decode_face_ordinal([entity_pixel[0], entity_pixel[1], entity_pixel[2]]);
        let Some(pickable) = pass.pickables.get(slot as usize).cloned() else {
            // A full-coverage pixel no overlay painted: unattributable, so a
            // miss rather than a guess. Unreachable by construction (slots
            // are painted from this same list); the GPU integration tests
            // pin exactness, so any systematic breakage fails loudly there
            // instead of mis-highlighting here.
            return Ok(None);
        };
        for overlay in pass.overlays.drain(..) {
            let _ = self.app.world_mut().despawn(overlay);
        }
        let id_mesh = {
            let world = self.app.world();
            let assets = world.resource::<Assets<Mesh>>();
            let Some(cpu_mesh) = assets.get(&pickable.mesh) else {
                // Uploaded at cook time and never removed mid-pick (nothing
                // despawns during the two updates above): degrading to a
                // miss keeps the impossible case a deselect, not a failure.
                return Ok(None);
            };
            id_mesh_from_cooked(cpu_mesh)?
        };
        // Triangle total for the range backstop below: the ID mesh is
        // non-indexed soup by construction, so positions come in triples.
        // Read before the upload moves the mesh.
        let Some(VertexAttributeValues::Float32x3(positions)) =
            id_mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            // Unreachable by construction; the safe direction on the
            // impossible path is to bail to a miss.
            return Ok(None);
        };
        let face_total = positions.len() / 3;
        let id_handle = self
            .app
            .world_mut()
            .resource_mut::<Assets<Mesh>>()
            .add(id_mesh);
        pass.assets.push(PickAsset::Mesh(id_handle.clone()));
        let white = self
            .app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial {
                base_color: Color::WHITE,
                unlit: true,
                fog_enabled: false,
                ..Default::default()
            });
        pass.assets.push(PickAsset::Material(white.clone()));
        let overlay = self
            .app
            .world_mut()
            .spawn((
                Mesh3d(id_handle),
                MeshMaterial3d(white),
                pickable.transform,
                Visibility::Visible,
                RenderLayers::layer(PICK_RENDER_LAYER),
            ))
            .id();
        pass.overlays.push(overlay);
        let face_frame = self.update_until_painted(&pass.target, width, height)?;
        let Some(face_pixel) = pick_pixel(&face_frame, pixel_x, pixel_y, width) else {
            return Ok(None);
        };
        if !is_full_coverage_hit(face_pixel) {
            // Same MSAA rule as the entity pass: a blended edge pixel
            // carries a blended ordinal, so it is a miss, not a guess.
            return Ok(None);
        }
        let face = decode_face_ordinal([face_pixel[0], face_pixel[1], face_pixel[2]]);
        // Range backstop: a seam-blended triple can decode past the mesh.
        if !face_in_range(face, face_total) {
            return Ok(None);
        }
        Ok(Some(Pick {
            node: pickable.node,
            face,
        }))
    }

    /// Tick until the ID target shows painted pixels, then return the frame.
    ///
    /// A fresh transient camera needs warmup frames (view setup, pipeline
    /// compile) during which the target holds only clear values — decoding
    /// the tap texel then would misreport a hit as a background tap. Any
    /// painted pixel proves the pass rendered; the tap texel is decoded
    /// afterwards. [`SceneError::NoGpuImage`] (target not uploaded yet) reads
    /// as not-ready, like an all-clear frame; every other error returns
    /// immediately — the loop must never mask real copy-back faults.
    /// Exhausting [`PICK_PASS_UPDATE_BUDGET`] means real breakage (a
    /// non-empty scene always paints somewhere), so it fails loudly instead
    /// of returning a lying miss.
    ///
    /// # Errors
    ///
    /// Returns [`SceneError::PickPassNotReady`] on budget exhaustion, or the
    /// staging, poll, and map [`SceneError`] variants when the copy-back
    /// fails.
    fn update_until_painted(
        &mut self,
        target: &Handle<Image>,
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>, SceneError> {
        let mut attempts = 0;
        loop {
            self.update();
            attempts += 1;
            match gpu::readback_target_bytes(&mut self.app, target, width, height) {
                Ok(frame) if frame_painted(&frame) => return Ok(frame),
                Ok(_) | Err(SceneError::NoGpuImage) => {
                    if attempts >= PICK_PASS_UPDATE_BUDGET {
                        return Err(SceneError::PickPassNotReady { attempts });
                    }
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// Tear down everything one pick built: despawn the ID camera and every
    /// overlay, remove the ID target plus all transient meshes and
    /// materials. Runs on success and failure alike — a pick never leaks
    /// entities or assets into the tick world.
    fn cleanup_pick_pass(&mut self, pass: PickPass) {
        let world = self.app.world_mut();
        let _ = world.despawn(pass.camera);
        for overlay in pass.overlays {
            let _ = world.despawn(overlay);
        }
        world.resource_mut::<Assets<Image>>().remove(&pass.target);
        for asset in pass.assets {
            match asset {
                PickAsset::Mesh(handle) => {
                    world.resource_mut::<Assets<Mesh>>().remove(&handle);
                }
                PickAsset::Material(handle) => {
                    world
                        .resource_mut::<Assets<StandardMaterial>>()
                        .remove(&handle);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RenderFrame;
    use crate::gpu::GpuFrameTarget;
    use veronica_core::NodeId;
    use veronica_geometry::{HIT_ALPHA, MISS_ALPHA, MISS_PIXEL};
    use veronica_graph::{GraphSnapshot, OperatorGraph};

    /// Bit-exact `f32` comparison: ID colors and unwelded positions are
    /// carried verbatim (no arithmetic), so closeness is the wrong
    /// relation — identity is the requirement.
    fn assert_f32_bits_eq(actual: f32, expected: f32) {
        assert_eq!(actual.to_bits(), expected.to_bits());
    }

    /// Bit-exact `[f32; N]` comparison, same rationale as above.
    fn assert_f32_array_bits_eq<const N: usize>(actual: [f32; N], expected: [f32; N]) {
        for (index, (a, e)) in actual.iter().zip(expected.iter()).enumerate() {
            assert_eq!(a.to_bits(), e.to_bits(), "lane {index}");
        }
    }
    /// Default cube at the origin: 1 m box, no parameter overrides (the
    /// absent keys exercise the documented parse defaults).
    const CUBE_JSON: &str = r#"{"version":2,"operators":[
        {"id":1,"kind":"cube","name":"Box","parent":null,
         "position":{"x":0.0,"y":0.0},
         "parameters":{}}
    ],"edges":[]}"#;

    /// Default cube 1.5 m right of the origin (x-orientation probe).
    const CUBE_RIGHT_JSON: &str = r#"{"version":2,"operators":[
        {"id":1,"kind":"cube","name":"Box","parent":null,
         "position":{"x":0.0,"y":0.0},
         "parameters":{"center":{"vec3":[1.5,0.0,0.0]}}}
    ],"edges":[]}"#;

    /// Default cube 1.0 m above the origin (y-orientation probe).
    const CUBE_UP_JSON: &str = r#"{"version":2,"operators":[
        {"id":1,"kind":"cube","name":"Box","parent":null,
         "position":{"x":0.0,"y":0.0},
         "parameters":{"center":{"vec3":[0.0,1.0,0.0]}}}
    ],"edges":[]}"#;

    /// Cooked world under test: base scene plus one cube from `json`.
    fn cooked_world(json: &str) -> SceneWorld {
        let snapshot: GraphSnapshot = serde_json::from_str(json).unwrap();
        let mut graph = OperatorGraph::new();
        graph.restore(snapshot).unwrap();
        let mut world = SceneWorld::new_headless();
        let _ = world.spawn_base_scene();
        world.recook_graph(&graph).unwrap();
        world
    }

    /// Cooked world under test: base scene plus one default cube.
    fn cube_world() -> SceneWorld {
        cooked_world(CUBE_JSON)
    }

    /// Hand-built two-triangle mesh: four vertices, `U32` indices.
    fn two_triangle_mesh() -> Mesh {
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        );
        mesh.insert_attribute(
            Mesh::ATTRIBUTE_POSITION,
            vec![
                [0.0f32, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
            ],
        );
        mesh.insert_indices(Indices::U32(vec![0, 1, 2, 1, 2, 3]));
        mesh
    }

    #[test]
    fn ndc_center_maps_to_frame_center() {
        assert_eq!(ndc_to_pixel(0.0, 0.0, 512, 320), Some((256, 160)));
    }

    #[test]
    fn ndc_corners_map_to_edge_texels() {
        // NDC (-1, -1) is bottom-left: row 0 is the top row, so bottom NDC
        // lands on the last row.
        assert_eq!(ndc_to_pixel(-1.0, -1.0, 512, 320), Some((0, 319)));
        assert_eq!(ndc_to_pixel(1.0, 1.0, 512, 320), Some((511, 0)));
        assert_eq!(ndc_to_pixel(-1.0, 1.0, 512, 320), Some((0, 0)));
        assert_eq!(ndc_to_pixel(1.0, -1.0, 512, 320), Some((511, 319)));
    }

    #[test]
    fn ndc_outside_square_is_miss() {
        for (x, y) in [(1.5, 0.0), (0.0, -1.5), (2.0, 2.0), (-1.01, 0.5)] {
            assert_eq!(ndc_to_pixel(x, y, 512, 320), None, "ndc ({x}, {y})");
        }
    }

    #[test]
    fn ndc_non_finite_is_miss() {
        for (x, y) in [
            (f32::NAN, 0.0),
            (0.0, f32::NAN),
            (f32::INFINITY, 0.0),
            (0.0, f32::NEG_INFINITY),
        ] {
            assert_eq!(ndc_to_pixel(x, y, 512, 320), None);
        }
    }

    #[test]
    fn slot_zero_is_black_full_alpha() {
        let Color::LinearRgba(color) = slot_color(0).unwrap() else {
            panic!("slot colors are linear rgba");
        };
        assert_f32_array_bits_eq(
            [color.red, color.green, color.blue, color.alpha],
            [0.0, 0.0, 0.0, 1.0],
        );
    }

    #[test]
    fn slot_color_carries_ordinal_bytes() {
        // Arrange: an interior ordinal plus the top of the range.
        // Act + assert: each RGB lane holds its byte over 255, exactly.
        for (slot, rgb) in [
            (0x12_3456_usize, [0x12_u8, 0x34, 0x56]),
            (0x00FF_FFFF, [0xFF, 0xFF, 0xFF]),
        ] {
            let Color::LinearRgba(color) = slot_color(slot).unwrap() else {
                panic!("slot colors are linear rgba");
            };
            assert_f32_bits_eq(color.red, f32::from(rgb[0]) / 255.0);
            assert_f32_bits_eq(color.green, f32::from(rgb[1]) / 255.0);
            assert_f32_bits_eq(color.blue, f32::from(rgb[2]) / 255.0);
            assert_f32_bits_eq(color.alpha, 1.0);
        }
    }

    #[test]
    fn count_and_ordinal_bounds_differ_by_one() {
        // Ordinals run `0..=MAX` (slot indices); totals run one further —
        // `MAX + 1` triangles carry exactly the encodable ordinals.
        assert!(triangle_total_fits(0));
        assert!(triangle_total_fits(12));
        assert!(triangle_total_fits(MAX_FACE_ORDINAL as usize + 1));
        assert!(!triangle_total_fits(MAX_FACE_ORDINAL as usize + 2));
        assert_eq!(
            u24_checked(MAX_FACE_ORDINAL as usize).unwrap(),
            MAX_FACE_ORDINAL
        );
        assert_eq!(
            u24_checked(MAX_FACE_ORDINAL as usize + 1),
            Err(SceneError::PickSpaceExhausted {
                count: MAX_FACE_ORDINAL as usize + 1
            })
        );
    }

    #[test]
    fn face_range_backstop_admits_only_real_triangles() {
        // A 12-triangle mesh: ordinals 0–11 resolve, 12+ deselect (MSAA
        // seam blends decoding past the mesh must never mint identities).
        assert!(face_in_range(0, 12));
        assert!(face_in_range(11, 12));
        assert!(!face_in_range(12, 12));
        assert!(!face_in_range(u32::MAX, 12));
        assert!(!face_in_range(0, 0));
    }

    #[test]
    fn slot_past_u24_fails_loudly() {
        // Arrange: one past the encodable range.
        // Act + assert: loud exhaustion naming the count, never truncation.
        assert_eq!(
            slot_color(0x0100_0000),
            Err(SceneError::PickSpaceExhausted { count: 0x0100_0000 })
        );
        assert_eq!(
            u24_checked(0x0100_0000),
            Err(SceneError::PickSpaceExhausted { count: 0x0100_0000 })
        );
        assert_eq!(
            SceneError::PickSpaceExhausted { count: 7 }.to_string(),
            "pick identity space exhausted: 7 items exceed the 24-bit range"
        );
    }

    #[test]
    fn indexed_u32_unwelds_with_ordinal_colors() {
        // Act.
        let id = id_mesh_from_cooked(&two_triangle_mesh()).unwrap();

        // Assert: six private vertices (two per source triangle, shared
        // vertex 1 and 2 duplicated), triangle 0 black, triangle 1 blue 1
        // (big-endian codec: ordinal 1 is RGB [0, 0, 1]).
        let positions: &[[f32; 3]] = match id.attribute(Mesh::ATTRIBUTE_POSITION).unwrap() {
            VertexAttributeValues::Float32x3(positions) => positions,
            _ => panic!("ID positions stay Float32x3"),
        };
        assert_eq!(positions.len(), 6);
        assert_f32_array_bits_eq(positions[0], [0.0, 0.0, 0.0]);
        assert_f32_array_bits_eq(positions[3], [1.0, 0.0, 0.0]);
        let colors: &[[f32; 4]] = match id.attribute(Mesh::ATTRIBUTE_COLOR).unwrap() {
            VertexAttributeValues::Float32x4(colors) => colors,
            _ => panic!("ID colors are Float32x4"),
        };
        assert_eq!(colors.len(), 6);
        for color in &colors[0..3] {
            assert_f32_array_bits_eq(*color, [0.0, 0.0, 0.0, 1.0]);
        }
        for color in &colors[3..6] {
            assert_f32_array_bits_eq(*color, [0.0, 0.0, 1.0 / 255.0, 1.0]);
        }
        // Assert: non-indexed soup, triangle topology, nothing else bound.
        assert!(id.indices().is_none());
        assert_eq!(id.primitive_topology(), PrimitiveTopology::TriangleList);
        assert!(id.attribute(Mesh::ATTRIBUTE_NORMAL).is_none());
    }

    #[test]
    fn indexed_u16_is_supported() {
        // Arrange: the same triangles as `U16`.
        let mut mesh = two_triangle_mesh();
        mesh.insert_indices(Indices::U16(vec![0, 1, 2, 1, 2, 3]));

        // Act + assert: identical unweld to the `U32` path.
        let id = id_mesh_from_cooked(&mesh).unwrap();
        let colors = match id.attribute(Mesh::ATTRIBUTE_COLOR).unwrap() {
            VertexAttributeValues::Float32x4(colors) => colors.clone(),
            _ => panic!("ID colors are Float32x4"),
        };
        assert_eq!(colors.len(), 6);
        assert_f32_array_bits_eq(colors[5], [0.0, 0.0, 1.0 / 255.0, 1.0]);
    }

    #[test]
    fn non_indexed_soup_takes_consecutive_triples() {
        // Arrange: three bare vertices, no indices.
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        );
        mesh.insert_attribute(
            Mesh::ATTRIBUTE_POSITION,
            vec![[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        );

        // Act + assert: one triangle, ordinal zero.
        let id = id_mesh_from_cooked(&mesh).unwrap();
        let colors = match id.attribute(Mesh::ATTRIBUTE_COLOR).unwrap() {
            VertexAttributeValues::Float32x4(colors) => colors.clone(),
            _ => panic!("ID colors are Float32x4"),
        };
        assert_eq!(colors.len(), 3);
        for color in &colors {
            assert_f32_array_bits_eq(*color, [0.0, 0.0, 0.0, 1.0]);
        }
    }

    #[test]
    fn non_triangle_topology_is_rejected() {
        // Arrange: a strip (ordinal semantics undefined on strips).
        let mut strip = Mesh::new(
            PrimitiveTopology::TriangleStrip,
            RenderAssetUsages::default(),
        );
        strip.insert_attribute(
            Mesh::ATTRIBUTE_POSITION,
            vec![[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        );

        // Act + assert.
        assert_eq!(
            id_mesh_from_cooked(&strip).unwrap_err(),
            SceneError::UnpickableMesh
        );
    }

    #[test]
    fn ragged_soups_are_rejected() {
        // Arrange: four indices (not whole triangles), four bare vertices.
        let mut ragged_indices = two_triangle_mesh();
        ragged_indices.insert_indices(Indices::U32(vec![0, 1, 2, 3]));
        let mut ragged_soup = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        );
        ragged_soup.insert_attribute(
            Mesh::ATTRIBUTE_POSITION,
            vec![
                [0.0f32, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
            ],
        );

        // Act + assert.
        assert_eq!(
            id_mesh_from_cooked(&ragged_indices).unwrap_err(),
            SceneError::UnpickableMesh
        );
        assert_eq!(
            id_mesh_from_cooked(&ragged_soup).unwrap_err(),
            SceneError::UnpickableMesh
        );
    }

    #[test]
    fn missing_positions_fail_loudly() {
        // Arrange: no positions at all. (A present-but-misshaped channel
        // needs no test: Bevy's `insert_attribute` panics on format
        // mismatch for standard attributes, so it is unconstructible.)
        let bare = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        );

        // Act + assert: the handoff error, naming the channel.
        assert_eq!(
            id_mesh_from_cooked(&bare).unwrap_err(),
            SceneError::MissingAttribute {
                name: "Vertex_Position".to_owned(),
            }
        );
    }

    #[test]
    fn dangling_index_fails_loudly() {
        // Arrange: index 9 names no vertex of the four.
        let mut mesh = two_triangle_mesh();
        mesh.insert_indices(Indices::U32(vec![0, 1, 9]));

        // Act + assert.
        assert_eq!(
            id_mesh_from_cooked(&mesh).unwrap_err(),
            SceneError::IndexOutOfBounds {
                index: 9,
                vertex_count: 4,
            }
        );
    }

    #[test]
    fn empty_scene_pick_is_deterministic_miss() {
        // Arrange: base scene only — no cooked operators, no GPU work.
        let mut world = SceneWorld::new_headless();
        let _ = world.spawn_base_scene();

        // Act + assert.
        assert_eq!(world.resolve_pick(0.0, 0.0), Ok(None));
    }

    #[test]
    fn out_of_range_ndc_skips_camera() {
        // Arrange: no base scene at all (no camera to snapshot).
        let mut world = SceneWorld::new_headless();

        // Act + assert: miss, not `NoViewportCamera` — the NDC guard runs
        // before any camera or GPU work.
        assert_eq!(world.resolve_pick(1.5, 0.0), Ok(None));
        assert_eq!(world.resolve_pick(0.0, f32::NAN), Ok(None));
    }

    #[test]
    fn cooked_without_camera_is_an_error() {
        // Arrange: a cooked cube but no base scene (camera gone).
        let snapshot: GraphSnapshot = serde_json::from_str(CUBE_JSON).unwrap();
        let mut graph = OperatorGraph::new();
        graph.restore(snapshot).unwrap();
        let mut world = SceneWorld::new_headless();
        world.recook_graph(&graph).unwrap();

        // Act + assert.
        assert_eq!(
            world.resolve_pick(0.0, 0.0),
            Err(SceneError::NoViewportCamera)
        );
    }

    #[test]
    fn center_tap_hits_cube_front_face() {
        // Arrange: default cube at the origin under the default camera.
        let mut world = cube_world();

        // Act.
        let pick = world.resolve_pick(0.0, 0.0).unwrap();

        // Assert: node 1, front-face triangle (ordinals 0–1). The exact
        // ordinal pins the whole exactness chain plus depth (a broken chain
        // decodes garbage; missing depth resolves the back face, 2–3).
        let Some(pick) = pick else {
            panic!("center tap must hit the cube");
        };
        assert_eq!(pick.node, NodeId(1));
        assert!(pick.face <= 1, "front face, got {}", pick.face);
    }

    #[test]
    fn corner_tap_misses() {
        // Arrange: default cube at the origin.
        let mut world = cube_world();

        // Act + assert: the far corner is background (clear alpha = miss).
        assert_eq!(world.resolve_pick(-0.95, 0.9), Ok(None));
        assert_eq!(world.resolve_pick(0.95, -0.9), Ok(None));
    }

    #[test]
    fn offset_cube_pins_x_orientation() {
        // Arrange: cube 1.5 m right of the origin (projects right of
        // center under the default camera).
        let mut world = cooked_world(CUBE_RIGHT_JSON);

        // Act + assert: right side hits node 1, mirrored left side misses
        // — a flipped NDC-x mapping would invert exactly this.
        let hit = world.resolve_pick(0.5, 0.0).unwrap();
        let Some(hit) = hit else {
            panic!("right-side tap must hit the offset cube");
        };
        assert_eq!(hit.node, NodeId(1));
        assert_eq!(world.resolve_pick(-0.5, 0.0), Ok(None));
    }

    #[test]
    fn offset_cube_pins_y_orientation() {
        // Arrange: cube 1.0 m above the origin (projects above center).
        let mut world = cooked_world(CUBE_UP_JSON);

        // Act + assert: upper tap hits node 1, mirrored lower tap misses
        // — a flipped NDC-y mapping would invert exactly this.
        let hit = world.resolve_pick(0.0, 0.5).unwrap();
        let Some(hit) = hit else {
            panic!("upper tap must hit the offset cube");
        };
        assert_eq!(hit.node, NodeId(1));
        assert_eq!(world.resolve_pick(0.0, -0.5), Ok(None));
    }

    #[test]
    fn painted_frame_detection_and_texel_indexing() {
        // Arrange: an all-clear frame plus one with a single painted texel.
        let clear = vec![0u8; 8 * 4];
        let mut painted = clear.clone();
        painted[5 * 4..6 * 4].copy_from_slice(&[0x12, 0x34, 0x56, HIT_ALPHA]);

        // Act + assert.
        assert!(!frame_painted(&clear));
        assert!(!frame_painted(&[]));
        assert!(frame_painted(&painted));
        assert_eq!(
            pick_pixel(&painted, 5, 0, 8),
            Some([0x12, 0x34, 0x56, HIT_ALPHA])
        );
        assert_eq!(pick_pixel(&painted, 0, 0, 8), Some([0, 0, 0, 0]));
        assert_eq!(pick_pixel(&painted, 8, 0, 8), None);
        assert_eq!(pick_pixel(&painted, 0, 1, 8), None);
    }

    #[test]
    fn quarter_orbit_tap_hits_side_face() {
        // Arrange: default cube; swing the camera 90 degrees toward +X
        // (pi/2 at 0.005 rad/px) so the right face (+X, ordinals 4–5)
        // centers under the tap.
        let mut world = cube_world();
        world
            .orbit_camera(std::f32::consts::FRAC_PI_2 / 0.005, 0.0)
            .unwrap();

        // Act.
        let pick = world.resolve_pick(0.0, 0.0).unwrap();

        // Assert: same node, side face — the resolve honors the moved
        // camera, not the spawn pose.
        let Some(pick) = pick else {
            panic!("center tap must hit the orbited cube");
        };
        assert_eq!(pick.node, NodeId(1));
        assert!(
            pick.face == 4 || pick.face == 5,
            "right face, got {}",
            pick.face
        );
    }

    #[test]
    fn half_orbit_tap_hits_back_face() {
        // Arrange: swing 180 degrees (pi at 0.005 rad/px).
        let mut world = cube_world();
        world
            .orbit_camera(std::f32::consts::PI / 0.005, 0.0)
            .unwrap();

        // Act.
        let pick = world.resolve_pick(0.0, 0.0).unwrap();

        // Assert: back face (ordinals 2–3), same node.
        let Some(pick) = pick else {
            panic!("center tap must hit the turned cube");
        };
        assert_eq!(pick.node, NodeId(1));
        assert!(
            pick.face == 2 || pick.face == 3,
            "back face, got {}",
            pick.face
        );
    }
    /// Beauty clear bytes (BGRA) when nothing draws: Bevy's default
    /// `ClearColor` (`srgb_u8(43, 44, 47)`) through the `Bgra8UnormSrgb`
    /// target. Test-only, fails loudly if the default ever changes.
    const BEAUTY_CLEAR_BG: [u8; 4] = [47, 44, 43, 255];

    /// Tick until the beauty pipeline draws the cube (bounded): a fresh
    /// world needs warmup frames for pipeline compile, and capturing a
    /// "before" frame cold would compare an empty frame against a drawn one.
    fn warm_beauty_until_cube(world: &mut SceneWorld) -> RenderFrame {
        let (width, height) = world.viewport_size();
        let offset = ((height / 2 * width + width / 2) * 4) as usize;
        for _ in 0..30 {
            world.update();
            // Same discipline as the production readiness loop: a missing
            // GPU image means "not uploaded yet" (wait), anything else
            // failing is loud, never swallowed.
            let frame = match world.render_frame() {
                Ok(frame) => frame,
                Err(SceneError::NoGpuImage) => continue,
                Err(error) => panic!("beauty warmup copy-back failed: {error}"),
            };
            if frame.pixels().get(offset..offset + 4) != Some(&BEAUTY_CLEAR_BG[..]) {
                return frame;
            }
        }
        panic!("beauty pipeline drew no cube within 30 updates");
    }

    #[test]
    fn per_tick_world_untouched_by_pick() {
        // Arrange: steady-state cube world (beauty pipeline warmed until
        // the cube draws), with the published frame captured.
        let mut world = cube_world();
        let frame_before = warm_beauty_until_cube(&mut world);
        let target_before = world
            .app
            .world()
            .resource::<GpuFrameTarget>()
            .handle
            .clone();
        let entities_before = world.entity_count();
        let cooked_before = world.cooked_entities();
        let camera_before = world.camera_translation();

        // Act: a real pick (must hit, or the comparison proves nothing).
        let pick = world.resolve_pick(0.0, 0.0).unwrap();
        assert!(pick.is_some(), "the pick under test must hit");

        // Assert: same entities, same cooked set, same camera, same target
        // handle, and bit-identical published pixels — the transient passes
        // leaked nothing into the tick world.
        assert_eq!(world.entity_count(), entities_before);
        assert_eq!(world.cooked_entities(), cooked_before);
        assert_eq!(world.camera_translation(), camera_before);
        assert_eq!(
            world
                .app
                .world()
                .resource::<GpuFrameTarget>()
                .handle
                .clone(),
            target_before
        );
        let frame_after = world.render_frame().unwrap();
        assert_eq!(frame_after.pixels(), frame_before.pixels());
    }

    #[test]
    fn coverage_gate_accepts_only_full_alpha() {
        // Full coverage hits — including RGB black, which is face zero,
        // not a miss.
        assert!(is_full_coverage_hit([0x12, 0x34, 0x56, HIT_ALPHA]));
        assert!(is_full_coverage_hit([0, 0, 0, HIT_ALPHA]));
        // Miss encodings and MSAA-blended edge pixels (partial alpha over
        // background or over an ID color) are misses.
        assert!(!is_full_coverage_hit(MISS_PIXEL));
        assert!(!is_full_coverage_hit([0x12, 0x34, 0x56, MISS_ALPHA]));
        assert!(!is_full_coverage_hit([0, 0, 0, 128]));
        assert!(!is_full_coverage_hit([0x7F, 0, 0, 200]));
        assert_eq!(HIT_ALPHA, 255);
        assert_eq!(MISS_ALPHA, 0);
    }
}
