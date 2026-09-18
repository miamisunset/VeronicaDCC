//! Wire-version pins (ADR-0002): v1 is rejected, v2 is the previous wire
//! (restores with the polygon migration applied by the selection owner),
//! v3 is the golden fixture. Both language tracks verify the same files;
//! if the JSON drifts, both gates fail.
//!
//! The Swift twin loads the same files repo-relative via `#filePath`.

use veronica_core::NodeId;
use veronica_graph::{GRAPH_SNAPSHOT_VERSION, GraphError, GraphSnapshot, OperatorKind, ParamValue};

/// The v1 fixture no longer restores: versions are rejected, not migrated.
#[test]
fn v1_fixture_is_rejected_not_migrated() {
    let stale: GraphSnapshot =
        serde_json::from_str(include_str!("fixtures/graph-v1.json")).unwrap();
    assert_eq!(stale.version, 1);
    assert_ne!(stale.version, GRAPH_SNAPSHOT_VERSION);

    let mut graph = veronica_graph::OperatorGraph::new();
    assert_eq!(graph.restore(stale), Err(GraphError::UnsupportedVersion(1)));
    assert!(graph.is_empty());
}

/// The v2 fixture decodes into typed parameters, including one operator
/// without a parameters key (additive reads tolerate it). v2 is the
/// previous wire: it pins version 2 (not current) and still restores —
/// the polygon migration lives with the selection owner, never the graph.
#[test]
fn v2_fixture_decodes_typed_parameters() {
    let snapshot: GraphSnapshot =
        serde_json::from_str(include_str!("fixtures/graph-v2.json")).unwrap();

    assert_eq!(snapshot.version, 2);
    assert_ne!(snapshot.version, GRAPH_SNAPSHOT_VERSION);
    assert!(snapshot.edges.is_empty());
    assert_eq!(snapshot.operators.len(), 2);

    let hero = &snapshot.operators[0];
    assert_eq!(hero.id, NodeId(7));
    assert_eq!(hero.kind, OperatorKind::Container);
    assert_eq!(hero.name, "Hero");
    assert_eq!(hero.parent, None);
    assert_eq!(hero.position.x.to_bits(), 120.0_f64.to_bits());
    assert_eq!(hero.position.y.to_bits(), 80.0_f64.to_bits());
    assert_eq!(
        hero.parameters["label"],
        ParamValue::Text("Hero".to_owned())
    );
    assert_eq!(hero.parameters["opacity"], ParamValue::Float(0.5));
    assert_eq!(hero.parameters["count"], ParamValue::Integer(3));
    assert_eq!(hero.parameters["visible"], ParamValue::Flag(true));
    assert_eq!(hero.parameters["size"], ParamValue::Vec3([1.0, 2.0, 3.0]));
    // The file triple survives bit-identical, not just `==`.
    assert!(
        matches!(hero.parameters["size"], ParamValue::Vec3(_)),
        "size must decode as a triple"
    );
    if let ParamValue::Vec3(triple) = &hero.parameters["size"] {
        assert_eq!(triple[0].to_bits(), 1.0_f64.to_bits());
        assert_eq!(triple[1].to_bits(), 2.0_f64.to_bits());
        assert_eq!(triple[2].to_bits(), 3.0_f64.to_bits());
    }

    let sidekick = &snapshot.operators[1];
    assert_eq!(sidekick.id, NodeId(8));
    assert_eq!(sidekick.parent, Some(NodeId(7)));
    assert!(sidekick.parameters.is_empty());

    // The decoded graph restores cleanly (previous-wire acceptance).
    let mut graph = veronica_graph::OperatorGraph::new();
    graph.restore(snapshot).unwrap();
    assert_eq!(graph.len(), 2);
}

/// The v3 fixture is the current golden: same shape as v2, current
/// version, restores cleanly. Typed-parameter decode stays pinned by the
/// v2 test above; this one pins the wire tag both tracks agree on.
#[test]
fn v3_fixture_is_the_current_golden() {
    let snapshot: GraphSnapshot =
        serde_json::from_str(include_str!("fixtures/graph-v3.json")).unwrap();

    assert_eq!(snapshot.version, GRAPH_SNAPSHOT_VERSION);
    assert_eq!(snapshot.version, 3);
    assert!(snapshot.edges.is_empty());
    assert_eq!(snapshot.operators.len(), 2);

    let mut graph = veronica_graph::OperatorGraph::new();
    graph.restore(snapshot).unwrap();
    assert_eq!(graph.len(), 2);
}

/// The v2 fixture re-serializes without loss: decode→encode is stable.
#[test]
fn v2_fixture_reflate_is_stable() {
    let first: GraphSnapshot =
        serde_json::from_str(include_str!("fixtures/graph-v2.json")).unwrap();
    let reflated: GraphSnapshot =
        serde_json::from_str(&serde_json::to_string(&first).unwrap()).unwrap();
    assert_eq!(first, reflated);
}
