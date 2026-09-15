//! Golden-fixture pin (ADR-0002): the repo ships one canonical v1 snapshot,
//! and both language tracks decode it. If this JSON drifts, both gates fail.
//!
//! The Swift twin loads the same file repo-relative via `#filePath`.

use veronica_core::NodeId;
use veronica_graph::{GRAPH_SNAPSHOT_VERSION, GraphSnapshot, OperatorKind};

/// The ADR-0002 `graph-v1.json` example decodes into exactly one container.
#[test]
fn golden_fixture_decodes_to_the_adr_example() {
    let snapshot: GraphSnapshot =
        serde_json::from_str(include_str!("fixtures/graph-v1.json")).unwrap();

    assert_eq!(snapshot.version, GRAPH_SNAPSHOT_VERSION);
    assert_eq!(snapshot.version, 1);
    assert!(snapshot.edges.is_empty());
    assert_eq!(snapshot.operators.len(), 1);

    let operator = &snapshot.operators[0];
    assert_eq!(operator.id, NodeId(7));
    assert_eq!(operator.kind, OperatorKind::Container);
    assert_eq!(operator.name, "Hero");
    assert_eq!(operator.parent, None);
    assert_eq!(operator.position.x.to_bits(), 120.0_f64.to_bits());
    assert_eq!(operator.position.y.to_bits(), 80.0_f64.to_bits());
}

/// The fixture re-serializes without loss: decode→encode is stable.
#[test]
fn golden_fixture_reflate_is_stable() {
    let first: GraphSnapshot =
        serde_json::from_str(include_str!("fixtures/graph-v1.json")).unwrap();
    let reflated: GraphSnapshot =
        serde_json::from_str(&serde_json::to_string(&first).unwrap()).unwrap();
    assert_eq!(first, reflated);
}
