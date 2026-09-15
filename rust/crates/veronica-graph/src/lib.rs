//! Procedural node DAG: topology + evaluation order.
//!
//! Pure graph logic lives here (petgraph-backed). Bevy ECS mapping
//! lives in `veronica-scene`; Swift never touches this crate directly.

use petgraph::algo::toposort;
use petgraph::graph::{DiGraph, NodeIndex};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use veronica_core::NodeId;

/// Errors for DAG construction and evaluation.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum GraphError {
    /// A dependency edge would introduce a cycle.
    #[error("node graph contains a cycle")]
    Cycle,
    /// Referenced node does not exist.
    #[error("unknown node: {0:?}")]
    UnknownNode(NodeId),
}

/// A procedural node DAG. Edges point from dependency to dependent.
#[derive(Debug, Default)]
pub struct NodeGraph {
    graph: DiGraph<NodeId, ()>,
    index_by_id: std::collections::HashMap<NodeId, NodeIndex>,
}

impl NodeGraph {
    /// Create an empty graph.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a node if absent and return its [`NodeId`].
    pub fn insert_node(&mut self, id: NodeId) -> NodeId {
        if let Some(&index) = self.index_by_id.get(&id) {
            let _ = index;
            return id;
        }
        let index = self.graph.add_node(id);
        self.index_by_id.insert(id, index);
        id
    }

    /// Add a dependency edge `depends_on -> dependent`.
    ///
    /// # Errors
    ///
    /// Currently infallible; returns `Ok` after inserting both endpoints.
    /// The `Result` is reserved for future validation (e.g. unknown nodes
    /// once insertion becomes explicit).
    pub fn add_dependency(
        &mut self,
        depends_on: NodeId,
        dependent: NodeId,
    ) -> Result<(), GraphError> {
        self.insert_node(depends_on);
        self.insert_node(dependent);
        let from = self.index_by_id[&depends_on];
        let to = self.index_by_id[&dependent];
        self.graph.add_edge(from, to, ());
        Ok(())
    }

    /// Return node ids in dependency-first evaluation order.
    ///
    /// # Errors
    ///
    /// Returns [`GraphError::Cycle`] when the graph contains a dependency cycle.
    #[must_use = "evaluation order is returned, not applied — feed it to the evaluator"]
    pub fn evaluation_order(&self) -> Result<Vec<NodeId>, GraphError> {
        toposort(&self.graph, None)
            .map(|order| order.into_iter().map(|i| self.graph[i]).collect())
            .map_err(|_| GraphError::Cycle)
    }
}

/// Serializable snapshot of DAG topology (for undo/redo + persistence).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphTopology {
    /// Node ids in the graph.
    pub nodes: Vec<NodeId>,
    /// `(depends_on, dependent)` edges.
    pub edges: Vec<(NodeId, NodeId)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluation_is_dependency_first() {
        let mut graph = NodeGraph::new();
        graph.add_dependency(NodeId(1), NodeId(2)).unwrap();
        graph.add_dependency(NodeId(2), NodeId(3)).unwrap();
        assert_eq!(
            graph.evaluation_order().unwrap(),
            vec![NodeId(1), NodeId(2), NodeId(3)]
        );
    }

    #[test]
    fn cycle_is_an_error() {
        let mut graph = NodeGraph::new();
        graph.add_dependency(NodeId(1), NodeId(2)).unwrap();
        graph.add_dependency(NodeId(2), NodeId(1)).unwrap();
        assert_eq!(graph.evaluation_order(), Err(GraphError::Cycle));
    }
}
