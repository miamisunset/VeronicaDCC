//! Procedural node DAG: topology + evaluation order.
//!
//! Pure graph logic lives here (petgraph-backed). Bevy ECS mapping
//! lives in `veronica-scene`; Swift never touches this crate directly.

use petgraph::algo::toposort;
use petgraph::graph::{DiGraph, NodeIndex};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use thiserror::Error;
use veronica_core::NodeId;

/// `version` tag written into every [`GraphSnapshot`].
///
/// Restores reject any other value; Swift persists the same JSON verbatim, so
/// both sides drift-fail loudly instead of misreading each other (ADR-0002).
pub const GRAPH_SNAPSHOT_VERSION: u32 = 1;

/// Name assigned to an operator at creation (ADR-0002).
pub const DEFAULT_OPERATOR_NAME: &str = "Container";

/// Errors for DAG construction and evaluation.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum GraphError {
    /// A dependency edge would introduce a cycle.
    #[error("node graph contains a cycle")]
    Cycle,
    /// Referenced node does not exist.
    #[error("unknown node: {0:?}")]
    UnknownNode(NodeId),
    /// A rename (or snapshot entry) carried an empty or blank name.
    #[error("operator name must not be empty")]
    EmptyName,
    /// A snapshot was written by an incompatible format version.
    #[error("unsupported graph snapshot version: {0}")]
    UnsupportedVersion(u32),
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

    /// Remove a node and its incident edges. Returns whether it was present.
    ///
    /// Repairing the swap-remove bookkeeping keeps [`NodeGraph::evaluation_order`]
    /// coherent after cascade deletes from [`OperatorGraph`].
    #[must_use]
    pub fn remove_node(&mut self, id: NodeId) -> bool {
        let Some(&index) = self.index_by_id.get(&id) else {
            return false;
        };
        // `petgraph` moves the last node into the freed slot; re-point it.
        let last = NodeIndex::new(self.graph.node_count() - 1);
        let moved = if index == last {
            None
        } else {
            Some(self.graph[last])
        };
        self.graph.remove_node(index);
        self.index_by_id.remove(&id);
        if let Some(moved) = moved {
            self.index_by_id.insert(moved, index);
        }
        true
    }

    /// Dependency edges as `(depends_on, dependent)` pairs.
    #[must_use]
    pub fn edges(&self) -> Vec<(NodeId, NodeId)> {
        self.graph
            .edge_indices()
            .filter_map(|edge| self.graph.edge_endpoints(edge))
            .map(|(from, to)| (self.graph[from], self.graph[to]))
            .collect()
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

/// Kind of operator in the procedural graph.
///
/// Slice 1 ships containers only; operator #2 is a pure extension
/// (strict kind validation lives at the FFI boundary, not here).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OperatorKind {
    /// Pure organizer: holds a subnetwork, has no ports, does not cook.
    Container,
}

/// Canvas position in unbounded `f64` coordinates.
///
/// Positions live in Rust and decode as `Double`/`f64` on both sides —
/// never `Float`/`f32` (ADR-0002; pinned by the round-trip test below).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Position {
    /// Horizontal canvas coordinate.
    pub x: f64,
    /// Vertical canvas coordinate.
    pub y: f64,
}

/// The behavior-carrying element of the procedural graph.
///
/// Rust owns operators; Swift renders them and sends intents. `parent` is
/// the containing container's id, or [`None`] at the root.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Operator {
    /// Storage key identifying this operator.
    pub id: NodeId,
    /// Which operator this is (slice 1: always [`OperatorKind::Container`]).
    pub kind: OperatorKind,
    /// Display name; never empty or blank (enforced by rename).
    pub name: String,
    /// Containing container, if any.
    #[serde(default)]
    pub parent: Option<NodeId>,
    /// Canvas position in unbounded `f64` coordinates.
    pub position: Position,
}

impl Operator {
    /// Build an operator from its parts.
    #[must_use]
    pub fn new(
        id: NodeId,
        kind: OperatorKind,
        name: impl Into<String>,
        parent: Option<NodeId>,
        position: Position,
    ) -> Self {
        Self {
            id,
            kind,
            name: name.into(),
            parent,
            position,
        }
    }
}

/// Serializable mirror of [`Operator`] used inside [`GraphSnapshot`].
///
/// A plain alias today: one format serves persistence and undo (ADR-0002),
/// so the domain and wire shapes are identical by construction.
pub type OperatorSnapshot = Operator;

/// Versioned whole-graph snapshot: the persistence and undo unit.
///
/// Serialized with `camelCase` keys and `default` on additive fields so both
/// sides tolerate forward-compatible growth (ADR-0002).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphSnapshot {
    /// Format version; always [`GRAPH_SNAPSHOT_VERSION`].
    pub version: u32,
    /// Every operator in the graph, sorted by id.
    #[serde(default)]
    pub operators: Vec<OperatorSnapshot>,
    /// Dependency edges; empty in slice 1 (no ports yet) but always present.
    #[serde(default)]
    pub edges: Vec<(NodeId, NodeId)>,
}

/// Procedural operator network: named, positioned operators over the
/// petgraph dependency store.
///
/// Ids are issued by Rust starting at 1; [`NodeId`]`(0)` is never issued
/// (it is the FFI "root parent" sentinel).
#[derive(Debug, Default)]
pub struct OperatorGraph {
    topology: NodeGraph,
    operators: HashMap<NodeId, Operator>,
    next_id: u64,
}

impl OperatorGraph {
    /// Create an empty operator graph; the first issued id is `NodeId(1)`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            topology: NodeGraph::new(),
            operators: HashMap::new(),
            next_id: 1,
        }
    }

    /// Number of operators in the graph.
    #[must_use]
    pub fn len(&self) -> usize {
        self.operators.len()
    }

    /// Whether the graph holds no operators.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.operators.is_empty()
    }

    /// Look up an operator by id.
    #[must_use]
    pub fn operator(&self, id: NodeId) -> Option<&Operator> {
        self.operators.get(&id)
    }

    /// Borrow the underlying dependency store (for evaluation order).
    #[must_use]
    pub fn topology(&self) -> &NodeGraph {
        &self.topology
    }

    /// Create an operator with the default name, returning its fresh id.
    ///
    /// # Errors
    ///
    /// Returns [`GraphError::UnknownNode`] when `parent` names an operator
    /// that does not exist.
    pub fn create_operator(
        &mut self,
        kind: OperatorKind,
        parent: Option<NodeId>,
        position: Position,
    ) -> Result<NodeId, GraphError> {
        if let Some(id) = parent
            && !self.operators.contains_key(&id)
        {
            return Err(GraphError::UnknownNode(id));
        }
        let id = NodeId(self.next_id);
        // `saturating_add` can never reach the `NodeId(0)` sentinel from 1;
        // `.max(1)` states the issuance invariant explicitly.
        self.next_id = self.next_id.saturating_add(1).max(1);
        self.topology.insert_node(id);
        self.operators.insert(
            id,
            Operator::new(id, kind, DEFAULT_OPERATOR_NAME, parent, position),
        );
        Ok(id)
    }

    /// Move an operator to a new canvas position.
    ///
    /// # Errors
    ///
    /// Returns [`GraphError::UnknownNode`] when `id` is not in the graph.
    pub fn move_operator(&mut self, id: NodeId, position: Position) -> Result<(), GraphError> {
        let operator = self
            .operators
            .get_mut(&id)
            .ok_or(GraphError::UnknownNode(id))?;
        operator.position = position;
        Ok(())
    }

    /// Rename an operator.
    ///
    /// # Errors
    ///
    /// Returns [`GraphError::UnknownNode`] when `id` is not in the graph, or
    /// [`GraphError::EmptyName`] when `name` is empty or blank.
    pub fn rename_operator(&mut self, id: NodeId, name: &str) -> Result<(), GraphError> {
        let operator = self
            .operators
            .get_mut(&id)
            .ok_or(GraphError::UnknownNode(id))?;
        if name.trim().is_empty() {
            return Err(GraphError::EmptyName);
        }
        name.clone_into(&mut operator.name);
        Ok(())
    }

    /// Delete an operator and its whole subtree.
    ///
    /// Children are operators whose `parent` chain leads to `id`; the visited
    /// set keeps termination even if parent links ever formed a cycle.
    ///
    /// # Errors
    ///
    /// Returns [`GraphError::UnknownNode`] when `id` is not in the graph.
    pub fn delete_operator(&mut self, id: NodeId) -> Result<(), GraphError> {
        if !self.operators.contains_key(&id) {
            return Err(GraphError::UnknownNode(id));
        }
        let mut stack = vec![id];
        let mut visited = HashSet::new();
        while let Some(current) = stack.pop() {
            if !visited.insert(current) {
                continue;
            }
            let children: Vec<NodeId> = self
                .operators
                .iter()
                .filter(|(_, operator)| operator.parent == Some(current))
                .map(|(child, _)| *child)
                .collect();
            stack.extend(children);
            self.operators.remove(&current);
            // Presence was checked up front; the flag carries no news here.
            let _ = self.topology.remove_node(current);
        }
        Ok(())
    }

    /// Capture the whole graph as a versioned snapshot (undo/persist unit).
    #[must_use]
    pub fn snapshot(&self) -> GraphSnapshot {
        // Sorted by id: `HashMap` order is nondeterministic, and the
        // snapshot→restore→snapshot round trip must be bit-identical.
        let mut operators: Vec<OperatorSnapshot> = self.operators.values().cloned().collect();
        operators.sort_by_key(|operator| operator.id.0);
        GraphSnapshot {
            version: GRAPH_SNAPSHOT_VERSION,
            operators,
            edges: self.topology.edges(),
        }
    }

    /// Replace the whole graph from a snapshot (launch-load path).
    ///
    /// # Errors
    ///
    /// Returns [`GraphError::UnsupportedVersion`] when `snapshot.version` is
    /// not [`GRAPH_SNAPSHOT_VERSION`], or [`GraphError::UnknownNode`] for a
    /// zero id or a dangling parent/edge reference.
    pub fn restore(&mut self, snapshot: GraphSnapshot) -> Result<(), GraphError> {
        if snapshot.version != GRAPH_SNAPSHOT_VERSION {
            return Err(GraphError::UnsupportedVersion(snapshot.version));
        }
        let ids: HashSet<NodeId> = snapshot.operators.iter().map(|o| o.id).collect();
        for operator in &snapshot.operators {
            // `NodeId(0)` is the FFI root-parent sentinel, never an operator.
            if operator.id == NodeId(0) {
                return Err(GraphError::UnknownNode(operator.id));
            }
            if let Some(parent) = operator.parent
                && !ids.contains(&parent)
            {
                return Err(GraphError::UnknownNode(parent));
            }
        }
        for (from, to) in &snapshot.edges {
            if !ids.contains(from) {
                return Err(GraphError::UnknownNode(*from));
            }
            if !ids.contains(to) {
                return Err(GraphError::UnknownNode(*to));
            }
        }
        self.topology = NodeGraph::new();
        self.operators.clear();
        for operator in snapshot.operators {
            self.topology.insert_node(operator.id);
            self.operators.insert(operator.id, operator);
        }
        for (from, to) in snapshot.edges {
            self.topology.add_dependency(from, to)?;
        }
        self.next_id = self
            .operators
            .keys()
            .map(|id| id.0)
            .max()
            .map_or(1, |max| max.saturating_add(1).max(1));
        Ok(())
    }
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

    #[test]
    fn node_removal_keeps_toposort_coherent() {
        let mut graph = NodeGraph::new();
        graph.add_dependency(NodeId(1), NodeId(2)).unwrap();
        graph.add_dependency(NodeId(2), NodeId(3)).unwrap();
        assert!(graph.remove_node(NodeId(2)));
        assert!(!graph.remove_node(NodeId(2)));
        assert!(!graph.remove_node(NodeId(99)));
        assert_eq!(graph.evaluation_order().unwrap().len(), 2);
        assert!(graph.edges().is_empty());
    }
}

#[cfg(test)]
mod operator_tests {
    use super::*;

    fn position(x: f64, y: f64) -> Position {
        Position { x, y }
    }

    #[test]
    fn create_issues_ids_from_one_with_default_name() {
        let mut graph = OperatorGraph::new();
        assert!(graph.is_empty());
        let first = graph
            .create_operator(OperatorKind::Container, None, position(1.0, 2.0))
            .unwrap();
        let second = graph
            .create_operator(OperatorKind::Container, Some(first), position(3.0, 4.0))
            .unwrap();
        assert_eq!(first, NodeId(1));
        assert_eq!(second, NodeId(2));
        assert_eq!(graph.len(), 2);
        let operator = graph.operator(first).unwrap();
        assert_eq!(operator.name, DEFAULT_OPERATOR_NAME);
        assert_eq!(operator.kind, OperatorKind::Container);
        assert_eq!(operator.parent, None);
        assert_eq!(graph.operator(second).unwrap().parent, Some(first));
    }

    #[test]
    fn create_rejects_unknown_parent() {
        let mut graph = OperatorGraph::new();
        assert_eq!(
            graph.create_operator(OperatorKind::Container, Some(NodeId(9)), position(0.0, 0.0)),
            Err(GraphError::UnknownNode(NodeId(9)))
        );
        assert!(graph.is_empty());
    }

    #[test]
    fn move_updates_position_and_rejects_unknown_id() {
        let mut graph = OperatorGraph::new();
        let id = graph
            .create_operator(OperatorKind::Container, None, position(0.0, 0.0))
            .unwrap();
        graph.move_operator(id, position(120.0, 80.0)).unwrap();
        assert_eq!(graph.operator(id).unwrap().position, position(120.0, 80.0));
        assert_eq!(
            graph.move_operator(NodeId(99), position(0.0, 0.0)),
            Err(GraphError::UnknownNode(NodeId(99)))
        );
    }

    #[test]
    fn rename_accepts_names_and_rejects_empty_or_blank() {
        let mut graph = OperatorGraph::new();
        let id = graph
            .create_operator(OperatorKind::Container, None, position(0.0, 0.0))
            .unwrap();
        graph.rename_operator(id, "Hero").unwrap();
        assert_eq!(graph.operator(id).unwrap().name, "Hero");
        assert_eq!(graph.rename_operator(id, ""), Err(GraphError::EmptyName));
        assert_eq!(graph.rename_operator(id, "   "), Err(GraphError::EmptyName));
        assert_eq!(
            graph.rename_operator(NodeId(99), "Hero"),
            Err(GraphError::UnknownNode(NodeId(99)))
        );
        // Failed renames leave the last good name in place.
        assert_eq!(graph.operator(id).unwrap().name, "Hero");
    }

    #[test]
    fn delete_cascades_the_subtree() {
        let mut graph = OperatorGraph::new();
        let root = graph
            .create_operator(OperatorKind::Container, None, position(0.0, 0.0))
            .unwrap();
        let child = graph
            .create_operator(OperatorKind::Container, Some(root), position(1.0, 1.0))
            .unwrap();
        let grandchild = graph
            .create_operator(OperatorKind::Container, Some(child), position(2.0, 2.0))
            .unwrap();
        let survivor = graph
            .create_operator(OperatorKind::Container, None, position(9.0, 9.0))
            .unwrap();
        graph.delete_operator(root).unwrap();
        assert!(graph.operator(root).is_none());
        assert!(graph.operator(child).is_none());
        assert!(graph.operator(grandchild).is_none());
        assert!(graph.operator(survivor).is_some());
        assert_eq!(graph.len(), 1);
        assert_eq!(
            graph.delete_operator(root),
            Err(GraphError::UnknownNode(root))
        );
    }

    #[test]
    fn restore_rejects_unsupported_version() {
        let mut graph = OperatorGraph::new();
        let snapshot = GraphSnapshot {
            version: 2,
            ..GraphSnapshot::default()
        };
        assert_eq!(
            graph.restore(snapshot),
            Err(GraphError::UnsupportedVersion(2))
        );
        assert!(graph.is_empty());
    }

    #[test]
    fn restore_rejects_zero_ids_and_dangling_parents() {
        let mut graph = OperatorGraph::new();
        let zero = GraphSnapshot {
            version: GRAPH_SNAPSHOT_VERSION,
            operators: vec![Operator::new(
                NodeId(0),
                OperatorKind::Container,
                "Zero",
                None,
                position(0.0, 0.0),
            )],
            edges: vec![],
        };
        assert_eq!(graph.restore(zero), Err(GraphError::UnknownNode(NodeId(0))));

        let dangling = GraphSnapshot {
            version: GRAPH_SNAPSHOT_VERSION,
            operators: vec![Operator::new(
                NodeId(1),
                OperatorKind::Container,
                "Orphan",
                Some(NodeId(7)),
                position(0.0, 0.0),
            )],
            edges: vec![],
        };
        assert_eq!(
            graph.restore(dangling),
            Err(GraphError::UnknownNode(NodeId(7)))
        );
        assert!(graph.is_empty());
    }

    #[test]
    fn snapshot_orders_operators_by_id() {
        let mut graph = OperatorGraph::new();
        let b = graph
            .create_operator(OperatorKind::Container, None, position(0.0, 0.0))
            .unwrap();
        let a = graph
            .create_operator(OperatorKind::Container, None, position(0.0, 0.0))
            .unwrap();
        graph.delete_operator(a).unwrap();
        let c = graph
            .create_operator(OperatorKind::Container, None, position(0.0, 0.0))
            .unwrap();
        let ids: Vec<u64> = graph.snapshot().operators.iter().map(|o| o.id.0).collect();
        assert_eq!(ids, vec![b.0, c.0]);
    }

    /// Property-style pin (ADR-0002): snapshot→restore→snapshot must preserve
    /// every position bit, including values JSON text must round-trip exactly
    /// (subnormals, `-0.0`, large magnitudes, `PI`).
    #[test]
    fn f64_positions_survive_snapshot_restore_bit_identical() {
        const TRICKY: [f64; 10] = [
            0.0,
            -0.0,
            1.0,
            -1.0,
            std::f64::consts::PI,
            120.0,
            1e300,
            5e-324,
            2.225_073_858_507_201_4e-308,
            1.797_693_134_862_315_7e308,
        ];
        let mut graph = OperatorGraph::new();
        for (index, value) in TRICKY.iter().enumerate() {
            let parent = if index == 0 {
                None
            } else {
                Some(NodeId(index as u64))
            };
            graph
                .create_operator(OperatorKind::Container, parent, position(*value, -*value))
                .unwrap();
        }
        let before = serde_json::to_string(&graph.snapshot()).unwrap();
        let mut revived = OperatorGraph::new();
        let parsed: GraphSnapshot = serde_json::from_str(&before).unwrap();
        revived.restore(parsed).unwrap();
        let after = serde_json::to_string(&revived.snapshot()).unwrap();
        assert_eq!(before, after);
        for (original, round_tripped) in graph
            .snapshot()
            .operators
            .iter()
            .zip(revived.snapshot().operators.iter())
        {
            assert_eq!(
                original.position.x.to_bits(),
                round_tripped.position.x.to_bits()
            );
            assert_eq!(
                original.position.y.to_bits(),
                round_tripped.position.y.to_bits()
            );
        }
    }

    #[test]
    fn restore_continues_id_sequence_past_max() {
        let mut graph = OperatorGraph::new();
        graph
            .create_operator(OperatorKind::Container, None, position(0.0, 0.0))
            .unwrap();
        graph
            .create_operator(OperatorKind::Container, None, position(0.0, 0.0))
            .unwrap();
        let snapshot = graph.snapshot();
        let mut revived = OperatorGraph::new();
        revived.restore(snapshot).unwrap();
        let next = revived
            .create_operator(OperatorKind::Container, None, position(0.0, 0.0))
            .unwrap();
        assert_eq!(next, NodeId(3));
    }
}
