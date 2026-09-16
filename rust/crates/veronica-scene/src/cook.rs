//! Graph-to-scene cook path: operators in, render-ready meshes out.
//!
//! [`SceneWorld::cook_graph`] runs the full pipeline — [`cook`], [`realize`],
//! [`render_mesh_from_evaluated`] — and holds each resulting engine mesh as a
//! [`CookedMesh`] component on its own entity, tagged with the source
//! operator's [`NodeId`]. Containers cook to nothing, so they contribute no
//! entities; a graph with no geometry operators yields an empty mapping.
//! Cooking appends: previously cooked entities are left untouched.

use bevy_ecs::prelude::*;
use bevy_mesh::Mesh;
use veronica_core::NodeId;
use veronica_geometry::{GeometryPayload, cook, realize};
use veronica_graph::OperatorGraph;

use crate::{SceneError, SceneWorld, render_mesh_from_evaluated};

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
    /// also carries [`SourceOperator`] with the source operator id.
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
        let mut spawned = Vec::with_capacity(cooked.len());
        for (id, payload) in &cooked {
            let evaluated = match payload {
                GeometryPayload::Implicit(implicit) => realize(implicit),
                GeometryPayload::Evaluated(mesh) => mesh.clone(),
            };
            let mesh = render_mesh_from_evaluated(&evaluated)?;
            let entity = self
                .app
                .world_mut()
                .spawn((SourceOperator(*id), CookedMesh(mesh)))
                .id();
            spawned.push((*id, entity));
        }
        Ok(spawned)
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
}
