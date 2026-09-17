//! Graph-to-scene cook path: operators in, render-ready meshes out.
//!
//! [`SceneWorld::cook_graph`] runs the full pipeline — [`cook`], [`realize`],
//! [`render_mesh_from_evaluated`] — uploads each resulting engine mesh to the
//! GPU asset store, and holds it as a [`CookedMesh`] component on its own
//! entity, tagged with the source operator's [`NodeId`]. Alongside the mesh
//! each entity carries the render bundle ([`Mesh3d`], a shared default
//! material, identity [`Transform`], [`Visibility`]) plus [`SceneTag`], so
//! cooked geometry renders, frames, and counts like any other scene content.
//! Containers cook to nothing, so they contribute no entities; a graph with
//! no geometry operators yields an empty mapping. Cooking appends:
//! previously cooked entities are left untouched — reconciliation lives in
//! [`SceneWorld::recook_graph`].

use bevy_asset::{Assets, Handle};
use bevy_camera::visibility::Visibility;
use bevy_ecs::prelude::*;
use bevy_mesh::{Mesh, Mesh3d};
use bevy_pbr::{MeshMaterial3d, StandardMaterial};
use bevy_transform::prelude::Transform;
use veronica_core::NodeId;
use veronica_geometry::{GeometryPayload, cook, realize};
use veronica_graph::OperatorGraph;

use crate::{SceneError, SceneTag, SceneWorld, render_mesh_from_evaluated};

/// One stale cooked entity plus its render-bundle handles for cleanup.
///
/// Handles are optional: membership is defined by [`CookedMesh`], and a
/// future cooked entity may carry no render bundle — it must still be
/// despawned rather than leak.
type StaleCooked = (
    Entity,
    Option<Handle<Mesh>>,
    Option<Handle<StandardMaterial>>,
);

/// Links a cooked-mesh entity back to the graph operator that produced it.
///
/// A dedicated component — not [`crate::MeshTag`] — because operator
/// [`NodeId`]s and scene [`veronica_core::MeshId`]s are different id spaces,
/// and mixing them would lie about provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Component)]
pub struct SourceOperator(
    /// Id of the graph operator this entity's mesh was cooked from.
    pub NodeId,
);

/// Render-ready mesh cooked from one graph operator.
///
/// Stored as a component so the mesh is observable ECS state — queryable,
/// inspectable, and ready for the render slice to bind — rather than a
/// value buried in a Rust-side map.
#[derive(Debug, Component)]
pub struct CookedMesh(
    /// Engine mesh produced by the realize-plus-handoff path.
    pub Mesh,
);

impl SceneWorld {
    /// Cook every geometry operator in `graph` and hold the resulting
    /// meshes as [`CookedMesh`] components.
    ///
    /// Returns `(operator, entity)` pairs in cook order (dependency-first),
    /// so callers observe topological order without re-sorting. Each entity
    /// also carries [`SourceOperator`] with the source operator id plus the
    /// render bundle ([`Mesh3d`], shared default material, identity
    /// [`Transform`], [`Visibility`]) so it draws, frames, and counts.
    ///
    /// All payloads convert before any entity spawns, so a handoff failure
    /// leaves the world untouched instead of stranding earlier cooks.
    /// Appends: previously cooked entities are left untouched — call
    /// [`SceneWorld::recook_graph`] for clear-and-respawn reconciliation.
    ///
    /// # Errors
    ///
    /// Returns [`SceneError::Cook`] when the graph fails to cook (cycle,
    /// unknown operator, mistyped parameter), or the handoff errors
    /// ([`SceneError::MissingAttribute`], [`SceneError::AttributeShape`],
    /// [`SceneError::AttributeLength`], [`SceneError::IndexOutOfBounds`])
    /// when a payload fails validation at the scene boundary.
    pub fn cook_graph(
        &mut self,
        graph: &OperatorGraph,
    ) -> Result<Vec<(NodeId, Entity)>, SceneError> {
        let cooked = cook(graph)?;
        // Convert everything first: the world gains entities only once every
        // payload has survived the handoff.
        let mut meshes = Vec::with_capacity(cooked.len());
        for (id, payload) in cooked {
            let evaluated = match payload {
                GeometryPayload::Implicit(implicit) => realize(&implicit),
                GeometryPayload::Evaluated(mesh) => mesh,
            };
            meshes.push((id, render_mesh_from_evaluated(&evaluated)?));
        }
        // Nothing to upload or spawn: return before touching the asset
        // store, so an empty cook (container-only or empty graph) creates
        // no orphan material handle.
        if meshes.is_empty() {
            return Ok(Vec::new());
        }
        // Upload once per cook: one GPU mesh per operator plus a single
        // default material shared by the whole batch.
        let handles: Vec<Handle<Mesh>> = {
            let world = self.app.world_mut();
            let mut assets = world.resource_mut::<Assets<Mesh>>();
            let mut handles = Vec::with_capacity(meshes.len());
            for (_, mesh) in &meshes {
                handles.push(assets.add(mesh.clone()));
            }
            handles
        };
        let material = {
            let world = self.app.world_mut();
            let mut materials = world.resource_mut::<Assets<StandardMaterial>>();
            materials.add(StandardMaterial::default())
        };
        let mut spawned = Vec::with_capacity(meshes.len());
        for ((id, mesh), handle) in meshes.into_iter().zip(handles) {
            let entity = self
                .app
                .world_mut()
                .spawn((
                    SceneTag,
                    SourceOperator(id),
                    CookedMesh(mesh),
                    Mesh3d(handle),
                    MeshMaterial3d(material.clone()),
                    Transform::default(),
                    Visibility::default(),
                ))
                .id();
            spawned.push((id, entity));
        }
        Ok(spawned)
    }

    /// Reconcile the scene with `graph`: clear-and-respawn.
    ///
    /// Despawns every prior [`SourceOperator`] entity (with its uploaded GPU
    /// mesh and batch material) and respawns the fresh cook, so the live
    /// cooked set always mirrors the graph exactly. Correct-by-construction:
    /// no mesh diffing, no stale entities.
    ///
    /// Failure-atomic: the stale set is collected before cooking, and
    /// [`SceneWorld::cook_graph`] spawns only after every payload validates,
    /// so a cook error returns with the previous cooked entities (and their
    /// assets) still alive.
    ///
    /// # Errors
    ///
    /// Same [`SceneError`] variants as [`SceneWorld::cook_graph`].
    pub fn recook_graph(
        &mut self,
        graph: &OperatorGraph,
    ) -> Result<Vec<(NodeId, Entity)>, SceneError> {
        // Stale set uses the same membership predicate as
        // [`SceneWorld::cooked_entities`] (`With<CookedMesh>`): render-bundle
        // handles are looked up optionally, so a future cooked entity
        // without the bundle is still cleaned up instead of leaking.
        let stale_entities: Vec<Entity> = {
            let world = self.app.world_mut();
            let mut cooked = world.query_filtered::<Entity, With<CookedMesh>>();
            cooked.iter(world).collect()
        };
        let stale: Vec<StaleCooked> = {
            let world = self.app.world_mut();
            stale_entities
                .into_iter()
                .map(|entity| {
                    let mesh = world.get::<Mesh3d>(entity).map(|handle| handle.0.clone());
                    let material = world
                        .get::<MeshMaterial3d<StandardMaterial>>(entity)
                        .map(|handle| handle.0.clone());
                    (entity, mesh, material)
                })
                .collect()
        };
        let spawned = self.cook_graph(graph)?;
        {
            let world = self.app.world_mut();
            for (entity, mesh, material) in stale {
                if let Some(mesh) = mesh {
                    world.resource_mut::<Assets<Mesh>>().remove(&mesh);
                }
                if let Some(material) = material {
                    world
                        .resource_mut::<Assets<StandardMaterial>>()
                        .remove(&material);
                }
                world.despawn(entity);
            }
        }
        Ok(spawned)
    }

    /// `(operator, entity)` pairs currently holding live cooked meshes.
    ///
    /// Takes `&mut self` because some Bevy versions require mutable world
    /// access to construct a query, even though nothing is mutated.
    #[must_use]
    pub fn cooked_entities(&mut self) -> Vec<(NodeId, Entity)> {
        let mut state = self
            .app
            .world_mut()
            .query_filtered::<(Entity, &SourceOperator), With<CookedMesh>>();
        state
            .iter(self.app.world())
            .map(|(entity, source)| (source.0, entity))
            .collect()
    }

    /// Borrow the cooked mesh held by `entity`, if it is alive and cooked.
    #[must_use]
    pub fn cooked_mesh(&self, entity: Entity) -> Option<&Mesh> {
        self.app
            .world()
            .get::<CookedMesh>(entity)
            .map(|held| &held.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use veronica_graph::{GraphSnapshot, OperatorKind, Position};

    /// One-cube v2 snapshot: non-trivial size plus an offset center.
    const CUBE_JSON: &str = r#"{"version":2,"operators":[
        {"id":1,"kind":"cube","name":"Box","parent":null,
         "position":{"x":0.0,"y":0.0},
         "parameters":{"size":{"vec3":[2.0,1.0,4.0]},"center":{"vec3":[0.5,-1.0,2.0]}}}
    ],"edges":[]}"#;

    fn cube_graph() -> OperatorGraph {
        let snapshot: GraphSnapshot = serde_json::from_str(CUBE_JSON).unwrap();
        let mut graph = OperatorGraph::new();
        graph.restore(snapshot).unwrap();
        graph
    }

    #[test]
    fn cook_graph_holds_mesh_as_observable_state() {
        // Arrange: one cube plus one organizer (which cooks to nothing).
        let mut world = SceneWorld::new_headless();
        let mut graph = cube_graph();
        graph
            .create_operator(OperatorKind::Container, None, Position { x: 0.0, y: 0.0 })
            .unwrap();

        // Act.
        let spawned = world.cook_graph(&graph).unwrap();

        // Assert: one entity, tagged with the cube's operator id, holding a
        // complete mesh (24 verts, 36 indices, both standard channels).
        assert_eq!(spawned.len(), 1);
        let (id, entity) = spawned[0];
        assert_eq!(id, NodeId(1));
        assert_eq!(
            world.app.world().get::<SourceOperator>(entity),
            Some(&SourceOperator(id))
        );
        let mesh = world.cooked_mesh(entity).unwrap();
        assert_eq!(mesh.count_vertices(), 24);
        assert!(mesh.attribute(Mesh::ATTRIBUTE_NORMAL).is_some());
        assert!(mesh.attribute(Mesh::ATTRIBUTE_UV_0).is_some());
        // Assert: the render bundle — GPU handle resolving in the asset
        // store, identity transform, scene membership for entity counts.
        let handle = world
            .app
            .world()
            .get::<Mesh3d>(entity)
            .expect("cooked entity carries its GPU handle")
            .0
            .clone();
        assert!(
            world
                .app
                .world()
                .resource::<bevy_asset::Assets<Mesh>>()
                .get(&handle)
                .is_some(),
            "the uploaded mesh must resolve in the asset store"
        );
        assert!(
            world
                .app
                .world()
                .get::<MeshMaterial3d<StandardMaterial>>(entity)
                .is_some(),
            "cooked entity carries a material"
        );
        assert_eq!(
            world.app.world().get::<Transform>(entity),
            Some(&Transform::default())
        );
        assert!(world.app.world().get::<SceneTag>(entity).is_some());
        assert_eq!(world.cooked_entities(), vec![(id, entity)]);
    }

    #[test]
    fn cook_graph_appends_without_touching_previous_cooks() {
        // Arrange + act: cook the same graph twice.
        let mut world = SceneWorld::new_headless();
        let graph = cube_graph();
        let first = world.cook_graph(&graph).unwrap();
        let second = world.cook_graph(&graph).unwrap();

        // Assert: two distinct live entities, both still holding meshes.
        assert_ne!(first[0].1, second[0].1);
        assert!(world.cooked_mesh(first[0].1).is_some());
        assert!(world.cooked_mesh(second[0].1).is_some());
    }

    #[test]
    fn container_only_graph_cooks_to_empty_mapping() {
        // Arrange: organizers cook to nothing by design.
        let mut world = SceneWorld::new_headless();
        let mut graph = OperatorGraph::new();
        graph
            .create_operator(OperatorKind::Container, None, Position { x: 0.0, y: 0.0 })
            .unwrap();

        // Act + assert.
        assert!(world.cook_graph(&graph).unwrap().is_empty());
    }

    #[test]
    fn mistyped_parameter_fails_loudly_as_cook_error() {
        // Arrange: size must be a vec3; text is a loud failure, not a default.
        let mut world = SceneWorld::new_headless();
        let mut graph = cube_graph();
        graph.set_parameter(NodeId(1), "size", "big").unwrap();

        // Act + assert.
        let error = world.cook_graph(&graph).unwrap_err();
        assert!(matches!(error, SceneError::Cook(_)));
        assert_eq!(
            error.to_string(),
            "parameter \"size\" must be a vec3 of three f64 numbers"
        );
    }

    #[test]
    fn cooked_mesh_is_none_for_unknown_or_uncooked_entities() {
        // Arrange: a dead entity and a live but uncooked one.
        let mut world = SceneWorld::new_headless();
        let dead = world.app.world_mut().spawn_empty().id();
        world.app.world_mut().despawn(dead);
        let bare = world.app.world_mut().spawn_empty().id();

        // Act + assert.
        assert!(world.cooked_mesh(dead).is_none());
        assert!(world.cooked_mesh(bare).is_none());
    }

    #[test]
    fn recook_replaces_prior_cooked_entities() {
        // Arrange: one cube cooked into the scene.
        let mut world = SceneWorld::new_headless();
        let graph = cube_graph();
        let first = world.recook_graph(&graph).unwrap();
        assert_eq!(first.len(), 1);

        // Act: recook the unchanged graph.
        let second = world.recook_graph(&graph).unwrap();

        // Assert: clear-and-respawn — the old entity is dead, the new one
        // is live, and exactly one cooked entity remains.
        assert_eq!(second.len(), 1);
        assert_ne!(first[0].1, second[0].1);
        assert!(world.app.world().get_entity(first[0].1).is_err());
        assert!(world.cooked_mesh(second[0].1).is_some());
        assert_eq!(world.cooked_entities(), vec![(NodeId(1), second[0].1)]);
    }

    #[test]
    fn recook_empty_graph_clears_cooked() {
        // Arrange: cooked cube in the scene.
        let mut world = SceneWorld::new_headless();
        let graph = cube_graph();
        world.recook_graph(&graph).unwrap();
        assert_eq!(world.cooked_entities().len(), 1);

        // Act: reconcile against an empty graph.
        let empty = OperatorGraph::new();
        let spawned = world.recook_graph(&empty).unwrap();

        // Assert: nothing cooked, nothing left behind.
        assert!(spawned.is_empty());
        assert!(world.cooked_entities().is_empty());
    }

    #[test]
    fn recook_failure_keeps_prior_cooked() {
        // Arrange: a good cook in the scene.
        let mut world = SceneWorld::new_headless();
        let graph = cube_graph();
        let good = world.recook_graph(&graph).unwrap();

        // Act: reconcile against a graph whose parameter is mistyped.
        let mut broken = cube_graph();
        broken.set_parameter(NodeId(1), "size", "big").unwrap();
        let error = world.recook_graph(&broken).unwrap_err();

        // Assert: loud cook error, and the previous cooked entity (plus its
        // uploaded mesh) survives the failed recook.
        assert!(matches!(error, SceneError::Cook(_)));
        assert!(world.cooked_mesh(good[0].1).is_some());
        assert_eq!(world.cooked_entities(), vec![(NodeId(1), good[0].1)]);
    }

    #[test]
    fn recook_drops_uploaded_assets_with_the_entities() {
        // Arrange: one cook resident.
        let mut world = SceneWorld::new_headless();
        let graph = cube_graph();
        let first = world.recook_graph(&graph).unwrap();
        let stale_handle = world
            .app
            .world()
            .get::<Mesh3d>(first[0].1)
            .expect("cooked entity carries its GPU handle")
            .0
            .clone();

        // Act: recook; the stale entity and its assets go away.
        world.recook_graph(&graph).unwrap();

        // Assert: the old GPU mesh no longer resolves.
        assert!(
            world
                .app
                .world()
                .resource::<Assets<Mesh>>()
                .get(&stale_handle)
                .is_none(),
            "despawned cooks must release their uploaded meshes"
        );
    }
}
