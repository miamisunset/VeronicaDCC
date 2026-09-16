//! Public-API pins for the implicit cook: snapshot JSON in, payloads out.
//!
//! Typed parameters only enter graphs through snapshot restore (the string
//! setter stores text), so these tests drive [`cook`] the way production
//! does — restore a v2 snapshot, then cook.

use veronica_core::NodeId;
use veronica_geometry::{
    CookError, DEFAULT_CUBE_CENTER, DEFAULT_CUBE_SIZE, GeometryPayload, ImplicitGeometry, cook,
};
use veronica_graph::{GraphSnapshot, OperatorGraph, OperatorKind, Position};

/// Restore a v2 snapshot through the public snapshot API.
fn restore(json: &str) -> Result<OperatorGraph, Box<dyn std::error::Error>> {
    let snapshot: GraphSnapshot = serde_json::from_str(json)?;
    let mut graph = OperatorGraph::new();
    graph.restore(snapshot)?;
    Ok(graph)
}

/// Extract one implicit cube's parameters, if that is what the slot holds.
///
/// Returns `None` instead of panicking so the lint-clean helper stays honest;
/// callers assert the shape first, so `None` always surfaces as a named test
/// failure at the assertion above it.
fn implicit_cube_of(
    cooked: &[(NodeId, GeometryPayload)],
    index: usize,
) -> Option<(NodeId, [f64; 3], [f64; 3])> {
    let (id, payload) = cooked.get(index)?;
    let GeometryPayload::Implicit(ImplicitGeometry::Cube(params)) = payload else {
        return None;
    };
    Some((*id, params.size, params.center))
}

/// Bit-exact triple comparison: the cook carries values verbatim, so
/// closeness is the wrong relation — identity is the requirement.
fn assert_triple_bits_eq(actual: [f64; 3], expected: [f64; 3]) {
    for (index, (a, e)) in actual.iter().zip(expected.iter()).enumerate() {
        assert_eq!(a.to_bits(), e.to_bits(), "component {index}");
    }
}

/// Extract the single implicit cube from a one-cube cook.
fn only_cube(cooked: &[(NodeId, GeometryPayload)]) -> Option<(NodeId, [f64; 3], [f64; 3])> {
    assert_eq!(cooked.len(), 1, "expected exactly one cooked payload");
    assert!(
        matches!(
            cooked[0].1,
            GeometryPayload::Implicit(ImplicitGeometry::Cube(_))
        ),
        "cube must cook to the implicit variant"
    );
    implicit_cube_of(cooked, 0)
}

#[test]
fn cube_with_explicit_params_cooks_to_matching_implicit() {
    let graph = restore(
        r#"{"version":2,"operators":[
            {"id":1,"kind":"cube","name":"Box","parent":null,
             "position":{"x":0.0,"y":0.0},
             "parameters":{"size":{"vec3":[2.0,0.5,3.25]},"center":{"vec3":[1.0,2.0,3.0]}}}
        ],"edges":[]}"#,
    )
    .unwrap();
    let Some((id, size, center)) = only_cube(&cook(&graph).unwrap()) else {
        panic!("only_cube asserted the implicit shape above");
    };
    assert_eq!(id, NodeId(1));
    assert_triple_bits_eq(size, [2.0, 0.5, 3.25]);
    assert_triple_bits_eq(center, [1.0, 2.0, 3.0]);
}

#[test]
fn bare_cube_cooks_to_documented_defaults() {
    let mut graph = OperatorGraph::new();
    let id = graph
        .create_operator(OperatorKind::Cube, None, Position { x: 0.0, y: 0.0 })
        .unwrap();
    let Some((cooked_id, size, center)) = only_cube(&cook(&graph).unwrap()) else {
        panic!("only_cube asserted the implicit shape above");
    };
    assert_eq!(cooked_id, id);
    assert_triple_bits_eq(size, DEFAULT_CUBE_SIZE);
    assert_triple_bits_eq(center, DEFAULT_CUBE_CENTER);
}

#[test]
fn mistyped_size_fails_naming_key_and_shape() {
    let graph = restore(
        r#"{"version":2,"operators":[
            {"id":1,"kind":"cube","name":"Box","parent":null,
             "position":{"x":0.0,"y":0.0},
             "parameters":{"size":{"text":"big"}}}
        ],"edges":[]}"#,
    )
    .unwrap();
    let error = cook(&graph).unwrap_err();
    assert_eq!(
        error,
        CookError::InvalidParameter {
            key: "size".to_owned(),
            expected: "a vec3 of three f64 numbers",
        }
    );
    // The message a pipeline developer actually reads.
    assert_eq!(
        error.to_string(),
        "parameter \"size\" must be a vec3 of three f64 numbers"
    );
}

#[test]
fn containers_are_skipped_and_order_is_dependency_first() {
    let graph = restore(
        r#"{"version":2,"operators":[
            {"id":1,"kind":"container","name":"Rig","parent":null,
             "position":{"x":0.0,"y":0.0}},
            {"id":2,"kind":"cube","name":"Base","parent":1,
             "position":{"x":10.0,"y":0.0}},
            {"id":3,"kind":"cube","name":"Top","parent":1,
             "position":{"x":20.0,"y":0.0}}
        ],"edges":[[2,3]]}"#,
    )
    .unwrap();
    let cooked = cook(&graph).unwrap();
    // The container organizes but cooks to nothing; the dependent cube
    // follows its dependency.
    let ids: Vec<u64> = cooked.iter().map(|(id, _)| id.0).collect();
    assert_eq!(ids, vec![2, 3]);
    for (_, payload) in &cooked {
        assert!(
            matches!(
                payload,
                GeometryPayload::Implicit(ImplicitGeometry::Cube(_))
            ),
            "every cooked payload is implicit — no vertices allocated"
        );
    }
}

#[test]
fn cyclic_graph_is_not_cooked() {
    let graph = restore(
        r#"{"version":2,"operators":[
            {"id":1,"kind":"cube","name":"A","parent":null,
             "position":{"x":0.0,"y":0.0}},
            {"id":2,"kind":"cube","name":"B","parent":null,
             "position":{"x":10.0,"y":0.0}}
        ],"edges":[[1,2],[2,1]]}"#,
    )
    .unwrap();
    assert_eq!(cook(&graph), Err(CookError::Cycle));
}

#[test]
fn empty_graph_cooks_to_nothing() {
    assert!(cook(&OperatorGraph::new()).unwrap().is_empty());
}
