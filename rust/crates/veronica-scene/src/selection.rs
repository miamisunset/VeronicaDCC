//! Rust-owned Selection with primvar highlight.
//!
//! [`Selection`] is a world resource holding at most one [`Pick`]: the tap
//! the ID pass resolved (see `pick`), plus the target's triangle total at
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
//! Staleness follows the T2 contract: the pick survives recooks while its
//! node's triangle count is unchanged and clears on any topology change
//! (enforced in [`SceneWorld::retain_selection_across_recook`], which
//! [`SceneWorld::recook_graph`](crate::SceneWorld::recook_graph) runs after
//! every successful reconciliation).
//!
//! Face granularity rides the soup's split vertices: implicit geometry
//! realizes with one vertex run per face, so tinting a face's three vertices
//! highlights exactly that face. Meshes with shared vertices would bleed the
//! tint onto neighbors — accepted for the faces-only MVP scope.

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

/// A stored pick plus the target's triangle total at select time — the two
/// facts the staleness rule compares after each recook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StoredPick {
    /// Source operator of the hit mesh.
    node: NodeId,
    /// Triangle ordinal into the operator's realized mesh.
    face: u32,
    /// Triangles the target held when the pick was stored.
    triangles: usize,
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

/// Write the selection mask plus tint for `face` into `mesh` (CPU side).
///
/// Returns the mesh's triangle total for the caller to store alongside the
/// pick. Only the face's own vertices are touched, so the highlight is
/// exactly face-granular on split-vertex geometry.
///
/// # Errors
///
/// Returns [`SceneError::UnpickableMesh`] when the mesh carries no `U32`
/// index run to map the ordinal through, and
/// [`SceneError::SelectionFaceOutOfRange`] when `face` names no triangle.
pub fn write_selection_mask(mesh: &mut Mesh, face: u32) -> Result<usize, SceneError> {
    let triangles = cooked_triangle_count(mesh).ok_or(SceneError::UnpickableMesh)?;
    if (face as usize) >= triangles {
        return Err(SceneError::SelectionFaceOutOfRange { face, triangles });
    }
    let Some(Indices::U32(indices)) = mesh.indices() else {
        // Unreachable: counted above from the same index run.
        return Err(SceneError::UnpickableMesh);
    };
    let base = (face as usize) * 3;
    let face_vertices = [
        indices[base] as usize,
        indices[base + 1] as usize,
        indices[base + 2] as usize,
    ];
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
    /// triangle of the target.
    pub fn set_selection(&mut self, pick: Pick) -> Result<(), SceneError> {
        let entity = self
            .selection_target_entity(pick.node)
            .ok_or(SceneError::NoCookedSelectionTarget { node: pick.node })?;
        let triangles = {
            let mut cooked = self
                .app
                .world_mut()
                .get_mut::<CookedMesh>(entity)
                .ok_or(SceneError::NoCookedSelectionTarget { node: pick.node })?;
            write_selection_mask(&mut cooked.0, pick.face)?
        };
        self.refresh_gpu_mesh(entity);
        self.app.world_mut().resource_mut::<Selection>().pick = Some(StoredPick {
            node: pick.node,
            face: pick.face,
            triangles,
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
                    paint_base_mesh(&mut cooked.0);
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
    /// recook: same node alive with an unchanged triangle count keeps its
    /// highlight on the same face; a missing node or changed count clears
    /// the pick (T2 staleness rule, enforced here rather than trusted).
    pub(crate) fn retain_selection_across_recook(&mut self) {
        let Some(stored) = self.app.world().resource::<Selection>().pick else {
            return;
        };
        let pick = Pick {
            node: stored.node,
            face: stored.face,
        };
        let unchanged = self.cooked_triangle_total(stored.node) == Some(stored.triangles);
        let restored = unchanged && self.set_selection(pick).is_ok();
        if !restored {
            self.app.world_mut().resource_mut::<Selection>().pick = None;
        }
    }

    /// Live triangle total for `node`'s cooked mesh, or `None` when the
    /// node has no cooked entity or no mappable index run.
    fn cooked_triangle_total(&mut self, node: NodeId) -> Option<usize> {
        let entity = self.selection_target_entity(node)?;
        cooked_triangle_count(self.cooked_mesh(entity)?)
    }

    /// Copy the mask plus COLOR binding from the CPU [`CookedMesh`] onto
    /// the entity's uploaded GPU mesh, so the next frames render the fresh
    /// paint. Missing channels or handles are skipped: every paint path
    /// runs before its refresh, and the pixel tests prove the sync.
    fn refresh_gpu_mesh(&mut self, entity: Entity) {
        let world = self.app.world();
        let mask = world
            .get::<CookedMesh>(entity)
            .and_then(|cooked| cooked.0.attribute(SELECTION_MASK_ATTRIBUTE).cloned());
        let colors = world
            .get::<CookedMesh>(entity)
            .and_then(|cooked| cooked.0.attribute(Mesh::ATTRIBUTE_COLOR).cloned());
        let handle = world.get::<Mesh3d>(entity).map(|mesh| mesh.0.clone());
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
         "position":{"x":0.0,"y":0.0},"parameters":{}}
    ],"edges":[]}"#;

    /// Beauty-pass clear color at the frame center before the cube draws.
    const BEAUTY_CLEAR_BG: [u8; 4] = [47, 44, 43, 255];

    fn cube_graph() -> OperatorGraph {
        let snapshot: GraphSnapshot = serde_json::from_str(CUBE_JSON).unwrap();
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
        // Arrange + act: face 1 uses vertices 1, 2, 3.
        let mut mesh = two_triangle_mesh();
        paint_base_mesh(&mut mesh);
        let triangles = write_selection_mask(&mut mesh, 1).unwrap();

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
            write_selection_mask(&mut mesh, 2),
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

        // Assert: resource mirrors the pick, mask marks exactly face 0's
        // vertices (indices 0, 1, 2 for the first triangle).
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
        assert_eq!(mask.iter().filter(|v| **v == 1f32.to_bits()).count(), 3);
        assert_eq!(&mask[..3], &[1f32.to_bits(); 3]);
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

        // Assert: single-pick semantics — the old face's vertices are base
        // again, exactly one face worth of vertices is marked.
        assert_eq!(
            world.selection(),
            Some(Pick {
                node: NodeId(1),
                face: 5
            })
        );
        let (_, entity) = world.cooked_entities()[0];
        let mask = mask_of(world.cooked_mesh(entity).unwrap());
        assert_eq!(mask.iter().filter(|v| **v == 1f32.to_bits()).count(), 3);
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

        // Assert: the pick survives on the same face with a fresh mask.
        assert_eq!(
            world.selection(),
            Some(Pick {
                node: NodeId(1),
                face: 0
            })
        );
        let (_, entity) = world.cooked_entities()[0];
        let mask = mask_of(world.cooked_mesh(entity).unwrap());
        assert_eq!(mask.iter().filter(|v| **v == 1f32.to_bits()).count(), 3);
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
}
