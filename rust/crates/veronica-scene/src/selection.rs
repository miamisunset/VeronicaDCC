//! Rust-owned Selection with primvar highlight.
//!
//! [`Selection`] is a world resource holding at most one [`Pick`]: the tap
//! the ID pass resolved (see `pick`), plus the target's polygon total at
//! select time. Nothing here touches the graph — selection is ephemeral UI
//! state, never serialized — and Swift only ever mirrors it read-only (the
//! FFI slice lands in T5).
//!
//! The highlight is primvar-driven: every cooked mesh carries a namespaced
//! `veronica:selection` mask (f32 per vertex, `1.0` on the picked face's
//! vertices) as the canonical record of *what* is selected, plus a
//! [`Mesh::ATTRIBUTE_COLOR`] render binding derived from it that the stock
//! PBR shader reads for free (`VERTEX_COLORS` comes from mesh layout — no
//! material flags, no custom shader, no extra draws, and the cooked batch
//! keeps sharing one [`StandardMaterial`](bevy_pbr::StandardMaterial)).
//! White binding under the white default base is pixel-identity, so clearing
//! the selection restores the unhighlighted frame bit-exactly (pinned by
//! test, not by faith in the multiply).
//!
//! Rendering note: the tint mixes albedo-side (vertex color), not
//! emissive-side as ADR-0007 first sketched. A per-face emissive would need
//! a custom [`Material`](bevy_pbr::Material) duplicating the PBR shader for
//! one highlight — throwaway surface the moment Groups reuse the mask —
//! while the vertex-color binding reuses the single PBR pipeline the beauty
//! pass already warms. The mask stays the source of truth either way, so an
//! emissive binding later would consume the same primvar unchanged.
//!
//! Staleness follows the T2 contract at polygon granularity: the pick
//! survives recooks while its node's polygon count is unchanged and clears
//! on any topology change (enforced in
//! [`SceneWorld::retain_selection_across_recook`], which
//! [`SceneWorld::recook_graph`](crate::SceneWorld::recook_graph) runs after
//! every successful reconciliation).
//!
//! Polygon granularity rides the soup's split vertices via the payload
//! grouping: tinting every member triangle's corners highlights exactly
//! that polygon (a quad pair paints six vertices). Meshes with shared
//! vertices would bleed the tint onto neighbors — accepted for the
//! faces-only MVP scope.

use bevy_asset::Assets;
use bevy_ecs::prelude::*;
use bevy_mesh::{Indices, Mesh, Mesh3d, MeshVertexAttribute, VertexAttributeValues, VertexFormat};
use veronica_core::NodeId;

use crate::{SceneError, SceneWorld, cook::CookedMesh, pick::Pick};

/// Canonical selection-mask channel: namespaced primvar, never a parallel
/// table. The renderer ignores it (custom attribute id); the COLOR binding
/// below is what the shader reads.
pub const SELECTION_MASK_ATTR: &str = "veronica:selection";

/// Attribute id for [`SELECTION_MASK_ATTR`]: first custom slot, so Bevy's
/// own pipeline never binds it.
pub const SELECTION_MASK_ATTRIBUTE: MeshVertexAttribute = MeshVertexAttribute::new(
    SELECTION_MASK_ATTR,
    Mesh::FIRST_AVAILABLE_CUSTOM_ATTRIBUTE,
    VertexFormat::Float32,
);

/// Unselected render binding: white under the white default base.
pub const SELECTION_BASE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

/// Selected-face render binding: safety orange, unmistakable against the
/// gray PBR shading of the default key-lit scene.
pub const SELECTION_TINT: [f32; 4] = [1.0, 0.42, 0.08, 1.0];

/// A stored pick plus the target's polygon total at select time — the two
/// facts the staleness rule compares after each recook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StoredPick {
    /// Source operator of the hit mesh.
    node: NodeId,
    /// Polygon id into the operator's grouping (travels the `Pick.face`
    /// slot; P3 formalizes the wire meaning).
    face: u32,
    /// Polygons the target held when the pick was stored.
    polygons: usize,
}

/// Ephemeral single selection: the resolved tap, if any.
///
/// Defaults to nothing selected. Single-pick MVP: setting a pick replaces
/// the previous one; shift-accumulate is deferred scope.
#[derive(Debug, Default, Resource)]
pub struct Selection {
    /// Stored pick, or `None` when nothing is highlighted.
    pick: Option<StoredPick>,
}

impl Selection {
    /// Currently selected face identity, if any.
    #[must_use]
    pub fn get(&self) -> Option<Pick> {
        self.pick.map(|stored| Pick {
            node: stored.node,
            face: stored.face,
        })
    }
}

/// Triangles in an indexed engine mesh, or `None` when the mesh carries no
/// `U32` index run to map ordinals through.
///
/// `U32`-only is deliberate but narrower than the pick slice (which also
/// unwelds `U16`/non-indexed soup): the handoff always emits `U32`
/// (`mesh.rs:53`), so anything else here is a future source whose picks
/// could never highlight — loud [`SceneError::UnpickableMesh`] at the
/// boundary, not silent support.
#[must_use]
pub fn cooked_triangle_count(mesh: &Mesh) -> Option<usize> {
    match mesh.indices() {
        Some(Indices::U32(indices)) => Some(indices.len() / 3),
        _ => None,
    }
}

/// Paint the unselected base onto `mesh` (CPU side): zero mask plus white
/// render binding, so every cooked mesh enters the world highlight-ready
/// and clearing is a return to this exact state.
pub fn paint_base_mesh(mesh: &mut Mesh) {
    let vertices = mesh.count_vertices();
    mesh.insert_attribute(
        SELECTION_MASK_ATTRIBUTE,
        VertexAttributeValues::Float32(vec![0.0; vertices]),
    );
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_COLOR,
        VertexAttributeValues::Float32x4(vec![SELECTION_BASE; vertices]),
    );
}

/// Polygons named by a grouping over `triangles` triangles: one past the
/// highest grouped id (ids run dense from zero per the P1 contract), or
/// the triangle total when the payload carried no map (identity).
///
/// A change detector for the staleness rule, not an identity: any topology
/// edit moves the count, which is all retention compares.
#[must_use]
fn polygon_total(triangles: usize, tri_to_poly: &[u32]) -> usize {
    if tri_to_poly.is_empty() {
        triangles
    } else {
        tri_to_poly.iter().max().map_or(0, |max| *max as usize + 1)
    }
}

/// Write the selection mask plus tint for polygon `face` into `mesh` (CPU side).
///
/// Every member triangle's corners are tinted, so a quad pick paints the
/// whole quad (six vertices on split-vertex soup, with no shared-vertex
/// bleed). An empty grouping reads as identity: `face` names its own
/// triangle, which keeps hand-built and ungrouped cooks pickable.
///
/// Returns the mesh's triangle total for the caller to pair with the
/// polygon total from [`polygon_total`].
///
/// # Errors
///
/// Returns [`SceneError::UnpickableMesh`] when the mesh carries no `U32`
/// index run to map ordinals through, or when the grouping names triangles
/// past the index run; [`SceneError::SelectionFaceOutOfRange`] when `face`
/// names no polygon (the error still reports the triangle total — the
/// variant's wire meaning is P3's to formalize).
pub fn write_selection_mask(
    mesh: &mut Mesh,
    face: u32,
    tri_to_poly: &[u32],
) -> Result<usize, SceneError> {
    let triangles = cooked_triangle_count(mesh).ok_or(SceneError::UnpickableMesh)?;
    if (face as usize) >= polygon_total(triangles, tri_to_poly) {
        return Err(SceneError::SelectionFaceOutOfRange { face, triangles });
    }
    let Some(Indices::U32(indices)) = mesh.indices() else {
        // Unreachable: counted above from the same index run.
        return Err(SceneError::UnpickableMesh);
    };
    // Member ordinals of the polygon, in index order.
    let mut members = Vec::new();
    if tri_to_poly.is_empty() {
        members.push(face as usize);
    } else {
        for (ordinal, poly) in tri_to_poly.iter().enumerate() {
            if *poly == face {
                members.push(ordinal);
            }
        }
    }
    if members.is_empty() {
        // Gapped grouping: the count check passed but no triangle names
        // this polygon — storing it would desync resource and pixels, so
        // reject like any other unnamable face.
        return Err(SceneError::SelectionFaceOutOfRange { face, triangles });
    }
    let mut face_vertices = Vec::with_capacity(members.len() * 3);
    for ordinal in members {
        if ordinal >= triangles {
            // The grouping names more triangles than the index run holds:
            // corrupt map, loud rather than a torn write.
            return Err(SceneError::UnpickableMesh);
        }
        let base = ordinal * 3;
        face_vertices.extend_from_slice(&[
            indices[base] as usize,
            indices[base + 1] as usize,
            indices[base + 2] as usize,
        ]);
    }
    let vertices = mesh.count_vertices();
    if face_vertices.iter().any(|vertex| *vertex >= vertices) {
        // The handoff bounds-checks indices, so this names a bug upstream,
        // not a user error — loud either way, never a torn write.
        return Err(SceneError::UnpickableMesh);
    }
    let mut mask = vec![0.0; vertices];
    let mut colors = vec![SELECTION_BASE; vertices];
    for vertex in face_vertices {
        mask[vertex] = 1.0;
        colors[vertex] = SELECTION_TINT;
    }
    mesh.insert_attribute(
        SELECTION_MASK_ATTRIBUTE,
        VertexAttributeValues::Float32(mask),
    );
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_COLOR,
        VertexAttributeValues::Float32x4(colors),
    );
    Ok(triangles)
}

impl SceneWorld {
    /// Live cooked entity for `node`, if any. Single lookup behind
    /// [`SceneWorld::set_selection`], [`SceneWorld::clear_selection`], and
    /// the recook retention path, so the membership predicate
    /// (`SourceOperator` match over [`CookedMesh`] entities) lives once.
    fn selection_target_entity(&mut self, node: NodeId) -> Option<Entity> {
        self.cooked_entities()
            .into_iter()
            .find_map(|(id, entity)| (id == node).then_some(entity))
    }

    /// Select `pick`: mask plus tint on the target's cooked mesh (CPU and
    /// GPU copies), stored pick in the [`Selection`] resource.
    ///
    /// Single-pick semantics: replaces any previous selection. The next
    /// frames render the picked face tinted; nothing else in the world
    /// moves.
    ///
    /// # Errors
    ///
    /// Returns [`SceneError::NoCookedSelectionTarget`] when no live cooked
    /// entity belongs to `pick.node`, [`SceneError::UnpickableMesh`] when
    /// the target has no mappable index run, and
    /// [`SceneError::SelectionFaceOutOfRange`] when `pick.face` names no
    /// polygon of the target.
    pub fn set_selection(&mut self, pick: Pick) -> Result<(), SceneError> {
        let previous = self.app.world().resource::<Selection>().pick;
        let entity = self
            .selection_target_entity(pick.node)
            .ok_or(SceneError::NoCookedSelectionTarget { node: pick.node })?;
        let polygons = {
            let mut cooked = self
                .app
                .world_mut()
                .get_mut::<CookedMesh>(entity)
                .ok_or(SceneError::NoCookedSelectionTarget { node: pick.node })?;
            // Reborrow through the guard once so the mesh and its grouping
            // borrow disjointly (mask paint plus polygon count, one lookup).
            let cooked: &mut CookedMesh = &mut cooked;
            let triangles = write_selection_mask(&mut cooked.mesh, pick.face, &cooked.tri_to_poly)?;
            polygon_total(triangles, &cooked.tri_to_poly)
        };
        self.refresh_gpu_mesh(entity);
        // Single-pick replace across nodes: the new target paints first (so
        // a failure above leaves the old highlight untouched), then the
        // previous node's mesh returns to base. Same-node replace needs no
        // extra work — the mask rewrite above already repainted the whole
        // mesh. A despawned previous target released its assets with it.
        if let Some(old) = previous
            && old.node != pick.node
            && let Some(old_entity) = self.selection_target_entity(old.node)
        {
            if let Some(mut cooked) = self.app.world_mut().get_mut::<CookedMesh>(old_entity) {
                paint_base_mesh(&mut cooked.mesh);
            }
            self.refresh_gpu_mesh(old_entity);
        }
        self.app.world_mut().resource_mut::<Selection>().pick = Some(StoredPick {
            node: pick.node,
            face: pick.face,
            polygons,
        });
        Ok(())
    }

    /// Clear the selection: base paint on the previously selected mesh (when
    /// it still lives — a despawned target released its assets with it) and
    /// the resource back to nothing selected.
    ///
    /// Idempotent: clearing with no selection is a no-op. After the next
    /// frames the viewport shows the unhighlighted scene bit-exactly.
    pub fn clear_selection(&mut self) {
        let stored = self.app.world().resource::<Selection>().pick;
        if let Some(stored) = stored {
            let entity = self.selection_target_entity(stored.node);
            if let Some(entity) = entity {
                if let Some(mut cooked) = self.app.world_mut().get_mut::<CookedMesh>(entity) {
                    paint_base_mesh(&mut cooked.mesh);
                }
                self.refresh_gpu_mesh(entity);
            }
        }
        self.app.world_mut().resource_mut::<Selection>().pick = None;
    }

    /// Currently selected face identity, if any. Read-only mirror for Swift
    /// (the FFI slice in T5 calls this, never the resource directly).
    #[must_use]
    pub fn selection(&self) -> Option<Pick> {
        self.app.world().resource::<Selection>().get()
    }

    /// Re-apply the stored pick onto freshly cooked entities after a
    /// recook: same node alive with an unchanged polygon count keeps its
    /// highlight on the same polygon; a missing node or changed count clears
    /// the pick (T2 staleness rule at polygon granularity, enforced here
    /// rather than trusted).
    pub(crate) fn retain_selection_across_recook(&mut self) {
        let Some(stored) = self.app.world().resource::<Selection>().pick else {
            return;
        };
        let pick = Pick {
            node: stored.node,
            face: stored.face,
        };
        let unchanged = self.cooked_polygon_total(stored.node) == Some(stored.polygons);
        let restored = unchanged && self.set_selection(pick).is_ok();
        if !restored {
            self.app.world_mut().resource_mut::<Selection>().pick = None;
        }
    }

    /// Live polygon total for `node`'s cooked mesh, or `None` when the
    /// node has no cooked entity or no mappable index run.
    fn cooked_polygon_total(&mut self, node: NodeId) -> Option<usize> {
        let entity = self.selection_target_entity(node)?;
        let mesh = self.cooked_mesh(entity)?;
        let triangles = cooked_triangle_count(mesh)?;
        Some(polygon_total(triangles, self.cooked_polygons(entity)?))
    }

    /// Copy the mask plus COLOR binding from the CPU [`CookedMesh`] onto
    /// the entity's uploaded GPU mesh, so the next frames render the fresh
    /// paint. Missing channels or handles are skipped: every paint path
    /// runs before its refresh, and the pixel tests prove the sync.
    fn refresh_gpu_mesh(&mut self, entity: Entity) {
        let world = self.app.world();
        let mask = world
            .get::<CookedMesh>(entity)
            .and_then(|cooked| cooked.mesh.attribute(SELECTION_MASK_ATTRIBUTE).cloned());
        let colors = world
            .get::<CookedMesh>(entity)
            .and_then(|cooked| cooked.mesh.attribute(Mesh::ATTRIBUTE_COLOR).cloned());
        let handle = world.get::<Mesh3d>(entity).map(|mesh| mesh.0.clone());
        // Every paint path runs before its refresh on a live cooked entity,
        // so a missing channel or handle names a broken invariant — loud in
        // debug, pixel-proven in release (the clear/highlight tests fail on
        // any desync).
        debug_assert!(
            mask.is_some() && colors.is_some() && handle.is_some(),
            "refresh ran on an entity without painted channels or a GPU handle"
        );
        if let (Some(mask), Some(colors), Some(handle)) = (mask, colors, handle)
            && let Some(mut gpu) = self
                .app
                .world_mut()
                .resource_mut::<Assets<Mesh>>()
                .get_mut(&handle)
        {
            gpu.insert_attribute(SELECTION_MASK_ATTRIBUTE, mask);
            gpu.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_asset::RenderAssetUsages;
    use bevy_mesh::PrimitiveTopology;
    use veronica_graph::{GraphSnapshot, OperatorGraph, ParamValue};

    use crate::render::RenderFrame;

    /// Default cube snapshot: one unit cube at the origin.
    const CUBE_JSON: &str = r#"{"version":2,"operators":[
        {"id":1,"kind":"cube","name":"Box","parent":null,
         "position":{"x":0.0,"y":0.0},
         "parameters":{}}
    ],"edges":[]}"#;

    /// Default sphere snapshot: 0.5 m radius at the origin, no parameter
    /// overrides (absent keys exercise the documented parse defaults).
    const SPHERE_JSON: &str = r#"{"version":2,"operators":[
        {"id":1,"kind":"sphere","name":"Ball","parent":null,
         "position":{"x":0.0,"y":0.0},
         "parameters":{}}
    ],"edges":[]}"#;

    /// Beauty-pass clear color at the frame center before the cube draws.
    const BEAUTY_CLEAR_BG: [u8; 4] = [47, 44, 43, 255];

    fn cube_graph() -> OperatorGraph {
        let snapshot: GraphSnapshot = serde_json::from_str(CUBE_JSON).unwrap();
        let mut graph = OperatorGraph::new();
        graph.restore(snapshot).unwrap();
        graph
    }

    /// Two cubes side by side: node 1 left, node 2 right.
    const TWO_CUBE_JSON: &str = r#"{"version":2,"operators":[
        {"id":1,"kind":"cube","name":"Left","parent":null,
         "position":{"x":0.0,"y":0.0},
         "parameters":{"center":{"vec3":[-1.5,0.0,0.0]}}},
        {"id":2,"kind":"cube","name":"Right","parent":null,
         "position":{"x":0.0,"y":0.0},
         "parameters":{"center":{"vec3":[1.5,0.0,0.0]}}}
    ],"edges":[]}"#;

    fn two_cube_graph() -> OperatorGraph {
        let snapshot: GraphSnapshot = serde_json::from_str(TWO_CUBE_JSON).unwrap();
        let mut graph = OperatorGraph::new();
        graph.restore(snapshot).unwrap();
        graph
    }

    /// Base scene plus one default cube, recooked in.
    fn cube_world() -> SceneWorld {
        let mut world = SceneWorld::new_headless();
        let _ = world.spawn_base_scene();
        world.recook_graph(&cube_graph()).unwrap();
        world
    }

    /// Cooked world under test: base scene plus one default sphere.
    fn sphere_world() -> SceneWorld {
        let snapshot: GraphSnapshot = serde_json::from_str(SPHERE_JSON).unwrap();
        let mut graph = OperatorGraph::new();
        graph.restore(snapshot).unwrap();
        let mut world = SceneWorld::new_headless();
        let _ = world.spawn_base_scene();
        world.recook_graph(&graph).unwrap();
        world
    }

    /// Minimum-resolution sphere snapshot: segments 5, rings 2, 10
    /// triangles — faces big enough for pixel-meaningful highlight tests.
    const SPHERE_MIN_JSON: &str = r#"{"version":2,"operators":[
        {"id":1,"kind":"sphere","name":"Ball","parent":null,
         "position":{"x":0.0,"y":0.0},
         "parameters":{"segments":{"integer":5},"rings":{"integer":2}}}
    ],"edges":[]}"#;

    /// Minimum-resolution sphere with doubled radius (same 10 triangles).
    const SPHERE_MIN_RADIUS_JSON: &str = r#"{"version":2,"operators":[
        {"id":1,"kind":"sphere","name":"Ball","parent":null,
         "position":{"x":0.0,"y":0.0},
         "parameters":{"segments":{"integer":5},"rings":{"integer":2},
                        "radius":{"float":1.0}}}
    ],"edges":[]}"#;

    /// Coarsened sphere: segments 8, rings 2, 16 triangles — a real
    /// triangle-count change against the minimum-resolution cook.
    const SPHERE_COARSE_JSON: &str = r#"{"version":2,"operators":[
        {"id":1,"kind":"sphere","name":"Ball","parent":null,
         "position":{"x":0.0,"y":0.0},
         "parameters":{"segments":{"integer":8},"rings":{"integer":2}}}
    ],"edges":[]}"#;

    /// Quad-band sphere: segments 5, rings 3 — 20 triangles in 15 polygons
    /// (10 fan singles plus 5 quad pairs), so polygon granularity is
    /// observable: the south fan owns polygons 0..5, the first quad band
    /// owns polygon 5 (triangles 5 and 6).
    const SPHERE_QUAD_JSON: &str = r#"{"version":2,"operators":[
        {"id":1,"kind":"sphere","name":"Ball","parent":null,
         "position":{"x":0.0,"y":0.0},
         "parameters":{"segments":{"integer":5},"rings":{"integer":3}}}
    ],"edges":[]}"#;

    /// Quad-band sphere with doubled radius (same 15 polygons).
    const SPHERE_QUAD_RADIUS_JSON: &str = r#"{"version":2,"operators":[
        {"id":1,"kind":"sphere","name":"Ball","parent":null,
         "position":{"x":0.0,"y":0.0},
         "parameters":{"segments":{"integer":5},"rings":{"integer":3},
                        "radius":{"float":1.0}}}
    ],"edges":[]}"#;

    /// Operator graph restored from `json`.
    fn sphere_graph(json: &str) -> OperatorGraph {
        let snapshot: GraphSnapshot = serde_json::from_str(json).unwrap();
        let mut graph = OperatorGraph::new();
        graph.restore(snapshot).unwrap();
        graph
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

    fn mask_of(mesh: &Mesh) -> Vec<u32> {
        match mesh.attribute(SELECTION_MASK_ATTRIBUTE) {
            // Bit-exact compare downstream: mask values are copied literals,
            // never computed, so bits are the honest assertion (and dodge
            // `float_cmp` the same way `realize` tests do).
            Some(VertexAttributeValues::Float32(mask)) => {
                mask.iter().map(|v| v.to_bits()).collect()
            }
            other => panic!("mask channel must be f32, found {other:?}"),
        }
    }

    fn colors_of(mesh: &Mesh) -> Vec<[u32; 4]> {
        match mesh.attribute(Mesh::ATTRIBUTE_COLOR) {
            Some(VertexAttributeValues::Float32x4(colors)) => colors
                .iter()
                .map(|c| {
                    [
                        c[0].to_bits(),
                        c[1].to_bits(),
                        c[2].to_bits(),
                        c[3].to_bits(),
                    ]
                })
                .collect(),
            other => panic!("color binding must be float4, found {other:?}"),
        }
    }

    /// Bit patterns of the constant bindings, for exact test comparison.
    fn bits4(color: [f32; 4]) -> [u32; 4] {
        [
            color[0].to_bits(),
            color[1].to_bits(),
            color[2].to_bits(),
            color[3].to_bits(),
        ]
    }

    /// Tick until the beauty pipeline draws the cube, then return the frame.
    fn warm_beauty_until_cube(world: &mut SceneWorld) -> RenderFrame {
        let (width, height) = world.viewport_size();
        let offset = ((height / 2 * width + width / 2) * 4) as usize;
        for _ in 0..30 {
            world.update();
            // Same discipline as the pick readiness loop: a missing GPU
            // image means "not uploaded yet" (wait), anything else failing
            // is loud, never swallowed.
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

    /// Render the settled frame: two updates plus readback so asset
    /// re-uploads (mask repaints) have propagated to the target. Same
    /// wait-discipline as the warmup: a missing GPU image is "not yet",
    /// anything else failing is loud.
    fn settled_frame(world: &mut SceneWorld) -> RenderFrame {
        world.update();
        world.update();
        match world.render_frame() {
            Ok(frame) => frame,
            Err(SceneError::NoGpuImage) => {
                world.update();
                world.render_frame().unwrap()
            }
            Err(error) => panic!("settled copy-back failed: {error}"),
        }
    }

    #[test]
    fn mask_channel_is_namespaced_and_renderer_invisible() {
        // Assert: the canonical mask travels under the veronica namespace
        // on a custom slot Bevy's own pipeline never binds.
        assert_eq!(SELECTION_MASK_ATTR, "veronica:selection");
        assert_eq!(SELECTION_MASK_ATTRIBUTE.name, "veronica:selection");
        assert_eq!(
            format!("{SELECTION_MASK_ATTRIBUTE:?}"),
            format!(
                "{:?}",
                MeshVertexAttribute::new(
                    "veronica:selection",
                    Mesh::FIRST_AVAILABLE_CUSTOM_ATTRIBUTE,
                    VertexFormat::Float32,
                )
            )
        );
    }

    #[test]
    fn base_paint_zeroes_mask_and_whites_binding() {
        // Arrange + act.
        let mut mesh = two_triangle_mesh();
        paint_base_mesh(&mut mesh);

        // Assert: zero mask, white binding, full vertex coverage.
        assert_eq!(mask_of(&mesh), vec![0f32.to_bits(); 4]);
        assert_eq!(colors_of(&mesh), vec![bits4(SELECTION_BASE); 4]);
    }

    #[test]
    fn mask_writer_tints_only_hit_face_vertices() {
        // Arrange + act: face 1 uses vertices 1, 2, 3 (explicit identity
        // grouping: two triangles, two polygons).
        let mut mesh = two_triangle_mesh();
        paint_base_mesh(&mut mesh);
        let triangles = write_selection_mask(&mut mesh, 1, &[0, 1]).unwrap();

        // Assert: total reported, mask set exactly on the hit face, tint
        // exactly on its vertices, base everywhere else.
        assert_eq!(triangles, 2);
        assert_eq!(
            mask_of(&mesh),
            vec![
                0f32.to_bits(),
                1f32.to_bits(),
                1f32.to_bits(),
                1f32.to_bits()
            ]
        );
        assert_eq!(
            colors_of(&mesh),
            vec![
                bits4(SELECTION_BASE),
                bits4(SELECTION_TINT),
                bits4(SELECTION_TINT),
                bits4(SELECTION_TINT)
            ]
        );
    }

    #[test]
    fn out_of_range_face_fails_loudly() {
        // Arrange.
        let mut mesh = two_triangle_mesh();
        paint_base_mesh(&mut mesh);

        // Act + assert: the error names the face and the total, and the
        // mesh keeps its base paint (no torn write).
        assert_eq!(
            write_selection_mask(&mut mesh, 2, &[0, 1]),
            Err(SceneError::SelectionFaceOutOfRange {
                face: 2,
                triangles: 2
            })
        );
        assert_eq!(
            SceneError::SelectionFaceOutOfRange {
                face: 2,
                triangles: 2
            }
            .to_string(),
            "selection face 2 is out of range for 2 triangles"
        );
        assert_eq!(mask_of(&mesh), vec![0f32.to_bits(); 4]);
    }

    #[test]
    fn gapped_grouping_fails_loudly_without_torn_write() {
        // Arrange: grouping [0,0,2,2] names no polygon 1 (corrupt map —
        // no producer emits gaps, so the plant is direct).
        let mut mesh = two_triangle_mesh();
        paint_base_mesh(&mut mesh);

        // Act + assert: the missing polygon rejects with the mesh
        // untouched, never an empty highlight.
        assert_eq!(
            write_selection_mask(&mut mesh, 1, &[0, 0, 2, 2]),
            Err(SceneError::SelectionFaceOutOfRange {
                face: 1,
                triangles: 2
            })
        );
        assert_eq!(mask_of(&mesh), vec![0f32.to_bits(); 4]);
    }

    #[test]
    fn set_selection_missing_node_fails_loudly() {
        // Arrange: base scene only, nothing cooked.
        let mut world = SceneWorld::new_headless();
        let _ = world.spawn_base_scene();

        // Act + assert.
        let error = world
            .set_selection(Pick {
                node: NodeId(1),
                face: 0,
            })
            .unwrap_err();
        assert_eq!(
            error,
            SceneError::NoCookedSelectionTarget { node: NodeId(1) }
        );
        assert_eq!(
            error.to_string(),
            "no cooked mesh for selection target NodeId(1)"
        );
        assert_eq!(world.selection(), None);
    }

    #[test]
    fn set_then_clear_round_trips_resource_and_mask() {
        // Arrange.
        let mut world = cube_world();

        // Act: select face 0 of the cube (12 triangles).
        world
            .set_selection(Pick {
                node: NodeId(1),
                face: 0,
            })
            .unwrap();

        // Assert: resource mirrors the pick, mask marks exactly polygon 0's
        // vertices (triangles 0 and 1 share the face's four corners).
        assert_eq!(
            world.selection(),
            Some(Pick {
                node: NodeId(1),
                face: 0
            })
        );
        let (_, entity) = world.cooked_entities()[0];
        let mesh = world.cooked_mesh(entity).unwrap();
        let mask = mask_of(mesh);
        assert_eq!(mask.iter().filter(|v| **v == 1f32.to_bits()).count(), 4);
        assert_eq!(&mask[..4], &[1f32.to_bits(); 4]);
        assert_eq!(colors_of(mesh)[0], bits4(SELECTION_TINT));

        // Act: clear, then clear again (idempotent no-op).
        world.clear_selection();
        world.clear_selection();

        // Assert: resource empty, mask back to zero, binding back to white.
        assert_eq!(world.selection(), None);
        let mesh = world.cooked_mesh(entity).unwrap();
        assert_eq!(mask_of(mesh), vec![0f32.to_bits(); mesh.count_vertices()]);
        assert!(colors_of(mesh).iter().all(|c| *c == bits4(SELECTION_BASE)));
    }

    #[test]
    fn set_selection_replaces_previous_pick() {
        // Arrange: face 0 selected.
        let mut world = cube_world();
        world
            .set_selection(Pick {
                node: NodeId(1),
                face: 0,
            })
            .unwrap();

        // Act: select face 5 instead.
        world
            .set_selection(Pick {
                node: NodeId(1),
                face: 5,
            })
            .unwrap();

        // Assert: single-pick semantics — the old polygon's vertices are
        // base again, exactly one polygon (two triangles sharing four
        // corners) worth of mask is marked.
        assert_eq!(
            world.selection(),
            Some(Pick {
                node: NodeId(1),
                face: 5
            })
        );
        let (_, entity) = world.cooked_entities()[0];
        let mask = mask_of(world.cooked_mesh(entity).unwrap());
        assert_eq!(mask.iter().filter(|v| **v == 1f32.to_bits()).count(), 4);
        assert_eq!(&mask[..3], &[0f32.to_bits(); 3]);
    }

    #[test]
    fn recook_same_topology_keeps_selection() {
        // Arrange: face 0 selected on the cube.
        let mut world = cube_world();
        world
            .set_selection(Pick {
                node: NodeId(1),
                face: 0,
            })
            .unwrap();

        // Act: reconcile against the unchanged graph.
        world.recook_graph(&cube_graph()).unwrap();

        // Assert: the pick survives on the same polygon with a fresh mask
        // (polygon 0 covers two triangles sharing four corners).
        assert_eq!(
            world.selection(),
            Some(Pick {
                node: NodeId(1),
                face: 0
            })
        );
        let (_, entity) = world.cooked_entities()[0];
        let mask = mask_of(world.cooked_mesh(entity).unwrap());
        assert_eq!(mask.iter().filter(|v| **v == 1f32.to_bits()).count(), 4);
    }

    #[test]
    fn parameter_recook_keeps_highlight_on_same_face() {
        // Arrange: face 0 selected; resize keeps the 12-triangle topology.
        let mut world = cube_world();
        world
            .set_selection(Pick {
                node: NodeId(1),
                face: 0,
            })
            .unwrap();
        let mut resized = cube_graph();
        resized
            .set_parameter_value(NodeId(1), "size", ParamValue::Vec3([3.0, 3.0, 3.0]))
            .unwrap();

        // Act.
        world.recook_graph(&resized).unwrap();

        // Assert: same face still picked — resizing never loses the pick.
        assert_eq!(
            world.selection(),
            Some(Pick {
                node: NodeId(1),
                face: 0
            })
        );
    }

    #[test]
    fn recook_without_node_clears_selection() {
        // Arrange: face 0 selected on the cube.
        let mut world = cube_world();
        world
            .set_selection(Pick {
                node: NodeId(1),
                face: 0,
            })
            .unwrap();

        // Act: reconcile against a graph without the cube (topology gone).
        world.recook_graph(&OperatorGraph::new()).unwrap();

        // Assert: the pick clears rather than pointing at nothing.
        assert_eq!(world.selection(), None);
        assert!(world.cooked_entities().is_empty());
    }

    #[test]
    fn set_selection_changes_viewport_pixels() {
        // Arrange: warm beauty pipeline, unhighlighted frame.
        let mut world = cube_world();
        let plain = warm_beauty_until_cube(&mut world);

        // Act: select the front face (face 0 covers the frame center).
        world
            .set_selection(Pick {
                node: NodeId(1),
                face: 0,
            })
            .unwrap();
        let highlighted = settled_frame(&mut world);

        // Assert: the highlight is pixel-visible.
        assert_ne!(
            highlighted.pixels(),
            plain.pixels(),
            "selecting a face must change viewport pixels"
        );
        let (width, height) = world.viewport_size();
        let center = ((height / 2 * width + width / 2) * 4) as usize;
        assert_ne!(
            highlighted.pixels().get(center..center + 4),
            plain.pixels().get(center..center + 4),
            "the front-face highlight must repaint the frame center"
        );
    }

    #[test]
    fn clear_selection_restores_frame_bit_exact() {
        // Arrange: warm frame, then a highlighted frame.
        let mut world = cube_world();
        let plain = warm_beauty_until_cube(&mut world);
        world
            .set_selection(Pick {
                node: NodeId(1),
                face: 0,
            })
            .unwrap();
        let highlighted = settled_frame(&mut world);
        assert_ne!(highlighted.pixels(), plain.pixels());

        // Act: clear and settle.
        world.clear_selection();
        let restored = settled_frame(&mut world);

        // Assert: bit-exact return to the unhighlighted frame.
        assert_eq!(
            restored.pixels(),
            plain.pixels(),
            "clearing must restore the unhighlighted frame exactly"
        );
    }

    #[test]
    fn sphere_pick_paints_highlight_pixels() {
        // Arrange: warm beauty frame on the default sphere. The warmup
        // helper waits for a non-clear center pixel, which the centered
        // sphere satisfies like the cube does.
        let mut world = sphere_world();
        let plain = warm_beauty_until_cube(&mut world);

        // Act: resolve a real center-tap pick, then select it — the same
        // two calls the FFI pick makes back to back.
        let pick = world
            .resolve_pick(0.0, 0.0)
            .unwrap()
            .expect("center tap must hit the sphere");
        world.set_selection(pick).unwrap();
        let highlighted = settled_frame(&mut world);

        // Assert: the painted face changes the presented frame.
        assert_ne!(
            highlighted.pixels(),
            plain.pixels(),
            "selecting a sphere face must tint the frame"
        );
    }

    #[test]
    fn recook_same_topology_keeps_highlight_pixels() {
        // Arrange: highlighted frame on the cube.
        let mut world = cube_world();
        warm_beauty_until_cube(&mut world);
        world
            .set_selection(Pick {
                node: NodeId(1),
                face: 0,
            })
            .unwrap();
        let highlighted = settled_frame(&mut world);

        // Act: reconcile against the unchanged graph and settle.
        world.recook_graph(&cube_graph()).unwrap();
        let recooked = settled_frame(&mut world);

        // Assert: the same face highlights the same pixels — the recook
        // neither lost the pick nor repainted it elsewhere.
        assert_eq!(
            recooked.pixels(),
            highlighted.pixels(),
            "same-topology recook must keep the highlight pixel-identical"
        );
    }

    #[test]
    fn cross_node_replace_clears_previous_highlight() {
        // Arrange: two cubes cooked, face 0 of node 1 selected.
        let graph = two_cube_graph();
        let mut world = SceneWorld::new_headless();
        let _ = world.spawn_base_scene();
        world.recook_graph(&graph).unwrap();
        world
            .set_selection(Pick {
                node: NodeId(1),
                face: 0,
            })
            .unwrap();

        // Act: move the pick to node 2.
        world
            .set_selection(Pick {
                node: NodeId(2),
                face: 1,
            })
            .unwrap();

        // Assert: single-pick replace — node 1's mesh is fully base again,
        // node 2 carries exactly one polygon of mask (two triangles sharing
        // four corners), resource reports node 2.
        assert_eq!(
            world.selection(),
            Some(Pick {
                node: NodeId(2),
                face: 1
            })
        );
        let mut masks = world
            .cooked_entities()
            .into_iter()
            .map(|(id, entity)| {
                (
                    id,
                    mask_of(world.cooked_mesh(entity).unwrap())
                        .into_iter()
                        .filter(|v| *v == 1f32.to_bits())
                        .count(),
                )
            })
            .collect::<Vec<_>>();
        masks.sort_by_key(|(id, _)| id.0);
        assert_eq!(masks, vec![(NodeId(1), 0), (NodeId(2), 4)]);
    }

    #[test]
    fn re_tap_highlighted_face_resolves_same_pick() {
        // Arrange: warm pipeline, front face selected (tint live on the
        // cooked mesh and its GPU upload).
        let mut world = cube_world();
        warm_beauty_until_cube(&mut world);
        world
            .set_selection(Pick {
                node: NodeId(1),
                face: 0,
            })
            .unwrap();

        // Act: tap the frame center again — the highlighted face's pixels
        // must still decode to their slot, not `slot × tint`.
        let pick = world.resolve_pick(0.0, 0.0).unwrap();

        // Assert: same node; face 0 or its quad sibling 1 (the center texel
        // lands on either front triangle).
        let pick = pick.expect("center tap on the cube must hit");
        assert_eq!(pick.node, NodeId(1));
        assert!(
            pick.face <= 1,
            "center tap must resolve to a front face, got {}",
            pick.face
        );
    }

    #[test]
    fn sphere_radius_recook_retains_highlight_on_same_face() {
        // Arrange: polygon 0 selected on the minimum-resolution sphere (10
        // fan singles, so the mask is exactly one polygon of three
        // vertices — the 1:1 fan case).
        let mut world = sphere_world();
        world.recook_graph(&sphere_graph(SPHERE_MIN_JSON)).unwrap();
        world
            .set_selection(Pick {
                node: NodeId(1),
                face: 0,
            })
            .unwrap();

        // Act: recook with doubled radius — same node, same count.
        world
            .recook_graph(&sphere_graph(SPHERE_MIN_RADIUS_JSON))
            .unwrap();

        // Assert: the pick survives on the same polygon ordinal with
        // exactly one polygon of mask — the T4 retention half, pinned
        // through a real geometry change no fixed-topology operator can
        // produce.
        assert_eq!(
            world.selection(),
            Some(Pick {
                node: NodeId(1),
                face: 0
            })
        );
        let entity = world
            .cooked_entities()
            .into_iter()
            .find_map(|(id, entity)| (id == NodeId(1)).then_some(entity))
            .expect("sphere still cooked");
        let painted = mask_of(world.cooked_mesh(entity).unwrap())
            .into_iter()
            .filter(|v| *v == 1f32.to_bits())
            .count();
        assert_eq!(painted, 3, "exactly one face stays masked");
    }

    #[test]
    fn sphere_resolution_recook_clears_highlight_on_real_count_change() {
        // Arrange: face 0 selected on the minimum-resolution sphere.
        let mut world = sphere_world();
        world.recook_graph(&sphere_graph(SPHERE_MIN_JSON)).unwrap();
        world
            .set_selection(Pick {
                node: NodeId(1),
                face: 0,
            })
            .unwrap();
        assert_eq!(
            world.selection(),
            Some(Pick {
                node: NodeId(1),
                face: 0
            })
        );

        // Act: recook with more segments — same node, 10 triangles become
        // 16, a real count change (no forged totals).
        world
            .recook_graph(&sphere_graph(SPHERE_COARSE_JSON))
            .unwrap();

        // Assert: the stale pick clears rather than surviving on a mesh
        // whose topology it no longer describes — the case
        // `changed_count_clears_selection_on_recook` could only forge with
        // the fixed-topology Cube.
        assert_eq!(world.selection(), None);
    }

    #[test]
    fn mask_writer_expands_grouped_polygon_to_all_member_triangles() {
        // Arrange: two triangles sharing polygon 7 (grouped path, no cook).
        let mut mesh = two_triangle_mesh();
        paint_base_mesh(&mut mesh);
        let triangles = write_selection_mask(&mut mesh, 7, &[7, 7]).unwrap();

        // Assert: both triangles' vertices tinted — the whole polygon
        // paints, with no bleed possible past the member set.
        assert_eq!(triangles, 2);
        assert_eq!(mask_of(&mesh), vec![1f32.to_bits(); 4]);
        assert_eq!(colors_of(&mesh), vec![bits4(SELECTION_TINT); 4]);
    }

    #[test]
    fn sphere_fan_pick_paints_exactly_one_triangle() {
        // Arrange: minimum-resolution sphere, all pole fans (polygon 0 is
        // triangle 0, 1:1).
        let mut world = sphere_world();
        world.recook_graph(&sphere_graph(SPHERE_MIN_JSON)).unwrap();
        world
            .set_selection(Pick {
                node: NodeId(1),
                face: 0,
            })
            .unwrap();

        // Assert: exactly one triangle of mask — the fan single.
        let entity = world
            .cooked_entities()
            .into_iter()
            .find_map(|(id, entity)| (id == NodeId(1)).then_some(entity))
            .expect("sphere still cooked");
        let painted = mask_of(world.cooked_mesh(entity).unwrap())
            .into_iter()
            .filter(|v| *v == 1f32.to_bits())
            .count();
        assert_eq!(painted, 3, "a fan pick paints one triangle");
    }

    #[test]
    fn sphere_quad_pick_paints_both_triangles() {
        // Arrange: default sphere (32 segments put the first quad-band
        // polygon at id 32, covering triangles 32 and 33).
        let mut world = sphere_world();
        world
            .set_selection(Pick {
                node: NodeId(1),
                face: 32,
            })
            .unwrap();

        // Assert: both member triangles masked — six vertices, and the
        // neighboring fan single (triangle 31) untouched.
        let entity = world
            .cooked_entities()
            .into_iter()
            .find_map(|(id, entity)| (id == NodeId(1)).then_some(entity))
            .expect("sphere still cooked");
        let mask = mask_of(world.cooked_mesh(entity).unwrap());
        let painted = mask.iter().filter(|v| **v == 1f32.to_bits()).count();
        assert_eq!(painted, 6, "a quad pick paints the whole quad");
    }

    #[test]
    fn sphere_quad_radius_recook_retains_whole_quad() {
        // Arrange: polygon 5 selected on the quad-band sphere (first quad
        // pair: triangles 5 and 6, six vertices masked).
        let mut world = sphere_world();
        world.recook_graph(&sphere_graph(SPHERE_QUAD_JSON)).unwrap();
        world
            .set_selection(Pick {
                node: NodeId(1),
                face: 5,
            })
            .unwrap();

        // Act: recook with doubled radius — same node, same 15 polygons.
        world
            .recook_graph(&sphere_graph(SPHERE_QUAD_RADIUS_JSON))
            .unwrap();

        // Assert: the pick survives on the same polygon with the whole
        // quad still masked — the retain half at polygon granularity.
        assert_eq!(
            world.selection(),
            Some(Pick {
                node: NodeId(1),
                face: 5
            })
        );
        let entity = world
            .cooked_entities()
            .into_iter()
            .find_map(|(id, entity)| (id == NodeId(1)).then_some(entity))
            .expect("sphere still cooked");
        let painted = mask_of(world.cooked_mesh(entity).unwrap())
            .into_iter()
            .filter(|v| *v == 1f32.to_bits())
            .count();
        assert_eq!(painted, 6, "the retained quad stays fully masked");
    }

    #[test]
    fn sphere_quad_resolution_recook_clears_on_polygon_count_change() {
        // Arrange: polygon 5 selected on the quad-band sphere (15 polygons).
        let mut world = sphere_world();
        world.recook_graph(&sphere_graph(SPHERE_QUAD_JSON)).unwrap();
        world
            .set_selection(Pick {
                node: NodeId(1),
                face: 5,
            })
            .unwrap();
        assert_eq!(
            world.selection(),
            Some(Pick {
                node: NodeId(1),
                face: 5
            })
        );

        // Act: recook coarser — same node, 15 polygons become 16, a real
        // polygon-count change (20 triangles become 16 alongside).
        world
            .recook_graph(&sphere_graph(SPHERE_COARSE_JSON))
            .unwrap();

        // Assert: the stale pick clears rather than surviving on a mesh
        // whose grouping it no longer describes — the clear half at
        // polygon granularity.
        assert_eq!(world.selection(), None);
    }

    #[test]
    fn changed_count_clears_selection_on_recook() {
        // Arrange: face 0 selected, then the stored total is forged stale
        // (same-module plant: the fixed-topology Cube operator cannot
        // produce a real count change, so the predicate is pinned directly).
        let mut world = cube_world();
        world
            .set_selection(Pick {
                node: NodeId(1),
                face: 0,
            })
            .unwrap();
        world.app.world_mut().resource_mut::<Selection>().pick = Some(StoredPick {
            node: NodeId(1),
            face: 0,
            polygons: 999,
        });

        // Act: reconcile against the unchanged graph.
        world.recook_graph(&cube_graph()).unwrap();

        // Assert: the stale pick clears rather than surviving on a mesh
        // whose topology it no longer describes.
        assert_eq!(world.selection(), None);
    }

    #[test]
    fn unrestorable_face_clears_selection_on_recook() {
        // Arrange: stored face forged past the mesh (retention re-resolves
        // through `set_selection`, which must reject it). The forged
        // polygon total matches the live cube (6 polygons) so the test
        // still exercises the rejection path, not the count-mismatch path.
        let mut world = cube_world();
        world
            .set_selection(Pick {
                node: NodeId(1),
                face: 0,
            })
            .unwrap();
        world.app.world_mut().resource_mut::<Selection>().pick = Some(StoredPick {
            node: NodeId(1),
            face: 40,
            polygons: 6,
        });

        // Act.
        world.recook_graph(&cube_graph()).unwrap();

        // Assert: cleared — retention restores, never invents.
        assert_eq!(world.selection(), None);
    }
}
