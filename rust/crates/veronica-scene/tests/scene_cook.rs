//! End-to-end pin: graph snapshot → [`SceneWorld::cook_graph`] → engine
//! mesh held as observable state, cross-checked against the engine cuboid
//! builder as oracle.
//!
//! The unit tests in `src/cook.rs` pin the scene API contract (tagging,
//! append semantics, loud errors); this file pins the one path the issue
//! asks for — graph to cook to realize to scene — with no step skipped.

use bevy_math::primitives::Cuboid;
use bevy_mesh::{Indices, Mesh, VertexAttributeValues};
use veronica_core::NodeId;
use veronica_graph::{GraphSnapshot, OperatorGraph};
use veronica_scene::{SceneWorld, cooked_triangle_count};

/// One-cube v2 snapshot: non-trivial size plus an offset center, so the
/// oracle comparison must account for the center explicitly.
const CUBE_JSON: &str = r#"{"version":2,"operators":[
    {"id":1,"kind":"cube","name":"Box","parent":null,
     "position":{"x":0.0,"y":0.0},
     "parameters":{"size":{"vec3":[2.0,1.0,4.0]},"center":{"vec3":[0.5,-1.0,2.0]}}}
],"edges":[]}"#;

/// Worst absolute component difference between two position lists.
fn max_abs_diff(mine: &[[f32; 3]], expected: &[[f32; 3]]) -> f32 {
    mine.iter()
        .zip(expected.iter())
        .flat_map(|(a, b)| {
            [
                (a[0] - b[0]).abs(),
                (a[1] - b[1]).abs(),
                (a[2] - b[2]).abs(),
            ]
        })
        .fold(0.0_f32, f32::max)
}

#[test]
fn graph_cube_cooks_into_scene_mesh_matching_engine_oracle() {
    // Arrange: restore the graph from its wire snapshot — the same JSON
    // Swift persists — and build the engine oracle for the same box.
    let snapshot: GraphSnapshot = serde_json::from_str(CUBE_JSON).unwrap();
    let mut graph = OperatorGraph::new();
    graph.restore(snapshot).unwrap();
    let oracle = Mesh::from(Cuboid::new(2.0, 1.0, 4.0));
    let VertexAttributeValues::Float32x3(oracle_positions) =
        oracle.attribute(Mesh::ATTRIBUTE_POSITION).unwrap()
    else {
        panic!("oracle must carry f32 positions");
    };
    let VertexAttributeValues::Float32x3(oracle_normals) =
        oracle.attribute(Mesh::ATTRIBUTE_NORMAL).unwrap()
    else {
        panic!("oracle must carry f32 normals");
    };
    let VertexAttributeValues::Float32x2(oracle_uvs) =
        oracle.attribute(Mesh::ATTRIBUTE_UV_0).unwrap()
    else {
        panic!("oracle must carry f32 uvs");
    };
    let Indices::U32(oracle_indices) = oracle.indices().unwrap() else {
        panic!("oracle must carry u32 indices");
    };

    // Act: the single call under test — graph to scene in one path.
    let mut world = SceneWorld::new_headless();
    let spawned = world.cook_graph(&graph).unwrap();
    assert_eq!(spawned.len(), 1);
    assert_eq!(spawned[0].0, NodeId(1));
    let mesh = world.cooked_mesh(spawned[0].1).unwrap();

    // Assert: same topology the oracle carries.
    let VertexAttributeValues::Float32x3(positions) =
        mesh.attribute(Mesh::ATTRIBUTE_POSITION).unwrap()
    else {
        panic!("cooked mesh must carry f32 positions");
    };
    let VertexAttributeValues::Float32x3(normals) = mesh.attribute(Mesh::ATTRIBUTE_NORMAL).unwrap()
    else {
        panic!("cooked mesh must carry f32 normals");
    };
    let VertexAttributeValues::Float32x2(uvs) = mesh.attribute(Mesh::ATTRIBUTE_UV_0).unwrap()
    else {
        panic!("cooked mesh must carry f32 uvs");
    };
    let Indices::U32(indices) = mesh.indices().unwrap() else {
        panic!("cooked mesh must carry u32 indices");
    };
    assert_eq!(positions.len(), 24);
    assert_eq!(indices.len(), 36);

    // Positions cross the f64→f32 boundary, so closeness (after removing
    // the snapshot center to sit at the origin like the oracle) is the
    // requirement; normals and uvs are exactly representable, so
    // vertex-order identity is the requirement.
    let recentered: Vec<[f32; 3]> = positions
        .iter()
        .map(|p| [p[0] - 0.5, p[1] + 1.0, p[2] - 2.0])
        .collect();
    assert!(max_abs_diff(&recentered, oracle_positions) <= 1e-6);
    assert_eq!(normals, oracle_normals);
    assert_eq!(uvs, oracle_uvs);
    assert_eq!(indices, oracle_indices);
}

/// One-sphere v2 snapshot: 8 segments by 4 rings, so the cooked mesh must
/// hold exactly 2 * 8 * (4 - 1) = 48 triangles.
const SPHERE_JSON: &str = r#"{"version":2,"operators":[
    {"id":1,"kind":"sphere","name":"Ball","parent":null,
     "position":{"x":0.0,"y":0.0},
     "parameters":{"segments":{"integer":8},"rings":{"integer":4},
                   "radius":{"float":1.0},"center":{"vec3":[0.0,0.0,0.0]}}}
],"edges":[]}"#;

#[test]
fn graph_sphere_recooks_into_scene_mesh_with_budgeted_triangles() {
    // Arrange: restore the sphere snapshot the same JSON Swift persists.
    let snapshot: GraphSnapshot = serde_json::from_str(SPHERE_JSON).unwrap();
    let mut graph = OperatorGraph::new();
    graph.restore(snapshot).unwrap();

    // Act: reconcile the scene with the graph — the path every tick takes.
    let mut world = SceneWorld::new_headless();
    let spawned = world.recook_graph(&graph).unwrap();

    // Assert: cooked presence plus the resolution-budgeted triangle count.
    assert_eq!(spawned.len(), 1);
    assert_eq!(spawned[0].0, NodeId(1));
    let mesh = world.cooked_mesh(spawned[0].1).unwrap();
    assert_eq!(cooked_triangle_count(mesh), Some(48));

    // The count alone would pass a wrong-radius or wrong-center sphere,
    // so pin the bounding sphere too: every vertex one unit from the
    // origin. f32 trig output, so tight tolerance — never identity.
    let VertexAttributeValues::Float32x3(positions) =
        mesh.attribute(Mesh::ATTRIBUTE_POSITION).unwrap()
    else {
        panic!("cooked sphere must carry f32 positions");
    };
    assert_eq!(positions.len(), 3 * 48);
    for position in positions {
        let radius = (position[0].powi(2) + position[1].powi(2) + position[2].powi(2)).sqrt();
        let deviation = (radius - 1.0).abs();
        assert!(
            deviation < 1e-4,
            "vertex {position:?} off the unit sphere by {deviation}"
        );
    }
}
