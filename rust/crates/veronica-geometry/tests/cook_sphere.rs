//! Public-API pins for the sphere cook: snapshot JSON in, payloads out.
//!
//! Typed parameters only enter graphs through snapshot restore (the string
//! setter stores text), so these tests drive [`cook`] the way production
//! does — restore a v2 snapshot, then cook. Bare operators carry no
//! parameters at all ([`veronica_graph`] starts them empty), so the
//! documented defaults surface through [`SphereParams::parse`], never
//! through the graph core.

use veronica_core::NodeId;
use veronica_geometry::{
    CookError, DEFAULT_SPHERE_CENTER, DEFAULT_SPHERE_RADIUS, DEFAULT_SPHERE_RINGS,
    DEFAULT_SPHERE_SEGMENTS, GeometryPayload, ImplicitGeometry, SphereParams, cook,
};
use veronica_graph::{GraphSnapshot, OperatorGraph, OperatorKind, Position};

/// Restore a v2 snapshot through the public snapshot API.
fn restore(json: &str) -> Result<OperatorGraph, Box<dyn std::error::Error>> {
    let snapshot: GraphSnapshot = serde_json::from_str(json)?;
    let mut graph = OperatorGraph::new();
    graph.restore(snapshot)?;
    Ok(graph)
}

/// Extract one implicit sphere's parameters, if that is what the slot holds.
///
/// Returns `None` instead of panicking so the lint-clean helper stays honest;
/// callers assert the shape first, so `None` always surfaces as a named test
/// failure at the assertion above it.
fn implicit_sphere_of(
    cooked: &[(NodeId, GeometryPayload)],
    index: usize,
) -> Option<(NodeId, SphereParams)> {
    let (id, payload) = cooked.get(index)?;
    let GeometryPayload::Implicit(ImplicitGeometry::Sphere(params)) = payload else {
        return None;
    };
    Some((*id, *params))
}

/// Extract the single implicit sphere from a one-sphere cook.
fn only_sphere(cooked: &[(NodeId, GeometryPayload)]) -> Option<(NodeId, SphereParams)> {
    assert_eq!(cooked.len(), 1, "expected exactly one cooked payload");
    assert!(
        matches!(
            cooked[0].1,
            GeometryPayload::Implicit(ImplicitGeometry::Sphere(_))
        ),
        "sphere must cook to the implicit variant"
    );
    implicit_sphere_of(cooked, 0)
}

#[test]
fn sphere_with_explicit_params_cooks_to_matching_implicit() {
    let graph = restore(
        r#"{"version":2,"operators":[
            {"id":1,"kind":"sphere","name":"Ball","parent":null,
             "position":{"x":0.0,"y":0.0},
             "parameters":{"segments":{"integer":8},"rings":{"integer":4},
                           "radius":{"float":2.0},"center":{"vec3":[1.0,2.0,3.0]}}}
        ],"edges":[]}"#,
    )
    .unwrap();
    let Some((id, params)) = only_sphere(&cook(&graph).unwrap()) else {
        panic!("only_sphere asserted the implicit shape above");
    };
    assert_eq!(id, NodeId(1));
    assert_eq!(params.segments(), 8);
    assert_eq!(params.rings(), 4);
    assert_eq!(params.radius().to_bits(), 2.0_f64.to_bits());
    assert_eq!(params.triangle_count(), 2 * 8 * 3);
}

#[test]
fn bare_sphere_cooks_to_documented_defaults() {
    // A created operator carries no parameters; the cook must still
    // produce the documented sphere.
    let mut graph = OperatorGraph::new();
    let id = graph
        .create_operator(OperatorKind::Sphere, None, Position { x: 0.0, y: 0.0 })
        .unwrap();
    let Some((cooked_id, params)) = only_sphere(&cook(&graph).unwrap()) else {
        panic!("only_sphere asserted the implicit shape above");
    };
    assert_eq!(cooked_id, id);
    assert_eq!(params.segments(), DEFAULT_SPHERE_SEGMENTS);
    assert_eq!(params.rings(), DEFAULT_SPHERE_RINGS);
    assert_eq!(params.radius().to_bits(), DEFAULT_SPHERE_RADIUS.to_bits());
    for (index, (actual, expected)) in params
        .center()
        .iter()
        .zip(DEFAULT_SPHERE_CENTER.iter())
        .enumerate()
    {
        assert_eq!(actual.to_bits(), expected.to_bits(), "component {index}");
    }
}

#[test]
fn snapshot_round_trip_carries_all_four_params() {
    // Restore with explicit params, re-emit the snapshot, and cook the
    // re-restored graph: the wire must carry every sphere param losslessly.
    let graph = restore(
        r#"{"version":2,"operators":[
            {"id":1,"kind":"sphere","name":"Ball","parent":null,
             "position":{"x":0.0,"y":0.0},
             "parameters":{"segments":{"integer":8},"rings":{"integer":4},
                           "radius":{"float":2.0},"center":{"vec3":[1.0,2.0,3.0]}}}
        ],"edges":[]}"#,
    )
    .unwrap();
    let json = serde_json::to_string(&graph.snapshot()).unwrap();
    assert!(
        json.contains(r#""kind":"sphere""#),
        "snapshot must carry the sphere kind, got: {json}"
    );
    for fragment in [
        r#""segments":{"integer":8}"#,
        r#""rings":{"integer":4}"#,
        r#""radius":{"float":2.0}"#,
        r#""center":{"vec3":[1.0,2.0,3.0]}"#,
    ] {
        assert!(
            json.contains(fragment),
            "snapshot must carry {fragment}, got: {json}"
        );
    }
    let mut second = OperatorGraph::new();
    second
        .restore(serde_json::from_str(&json).unwrap())
        .unwrap();
    let Some((_, params)) = only_sphere(&cook(&second).unwrap()) else {
        panic!("only_sphere asserted the implicit shape above");
    };
    assert_eq!((params.segments(), params.rings()), (8, 4));
}

#[test]
fn mistyped_segments_fail_naming_key_and_shape() {
    let graph = restore(
        r#"{"version":2,"operators":[
            {"id":1,"kind":"sphere","name":"Ball","parent":null,
             "position":{"x":0.0,"y":0.0},
             "parameters":{"segments":{"text":"many"}}}
        ],"edges":[]}"#,
    )
    .unwrap();
    let error = cook(&graph).unwrap_err();
    assert_eq!(
        error,
        CookError::InvalidParameter {
            key: "segments".to_owned(),
            expected: "an integer segment count from 3 to 128",
        }
    );
    // The message a pipeline developer actually reads.
    assert_eq!(
        error.to_string(),
        "parameter \"segments\" must be an integer segment count from 3 to 128"
    );
}

#[test]
fn out_of_domain_resolution_fails_at_cook_time() {
    // Externally-crafted snapshots bypass field editors, so the cook is
    // the backstop: a degenerate resolution fails here, never downstream.
    let graph = restore(
        r#"{"version":2,"operators":[
            {"id":1,"kind":"sphere","name":"Ball","parent":null,
             "position":{"x":0.0,"y":0.0},
             "parameters":{"segments":{"integer":2}}}
        ],"edges":[]}"#,
    )
    .unwrap();
    assert_eq!(
        cook(&graph).unwrap_err(),
        CookError::InvalidParameter {
            key: "segments".to_owned(),
            expected: "an integer segment count from 3 to 128",
        }
    );
}

#[test]
fn sphere_cooks_alongside_cubes_in_dependency_order() {
    let graph = restore(
        r#"{"version":2,"operators":[
            {"id":1,"kind":"cube","name":"Base","parent":null,
             "position":{"x":0.0,"y":0.0}},
            {"id":2,"kind":"sphere","name":"Ball","parent":null,
             "position":{"x":10.0,"y":0.0}}
        ],"edges":[[1,2]]}"#,
    )
    .unwrap();
    let cooked = cook(&graph).unwrap();
    let ids: Vec<u64> = cooked.iter().map(|(id, _)| id.0).collect();
    assert_eq!(ids, vec![1, 2]);
    assert!(
        matches!(
            cooked[0].1,
            GeometryPayload::Implicit(ImplicitGeometry::Cube(_))
        ),
        "first payload stays a cube"
    );
    let Some((_, params)) = implicit_sphere_of(&cooked, 1) else {
        panic!("second payload must be the implicit sphere");
    };
    // Absent keys: the mixed graph still gets sphere defaults.
    assert_eq!(params.segments(), DEFAULT_SPHERE_SEGMENTS);
}
