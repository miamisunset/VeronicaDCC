//! Public-API pins for realization and the render handoff: snapshot JSON
//! in, engine mesh out, with the engine cuboid builder as oracle.
//!
//! The oracle lives here — not in `veronica-geometry` — so the geometry
//! crate stays Bevy-free all the way down including dev-dependencies.

use bevy_math::primitives::Cuboid;
use bevy_mesh::{Indices, Mesh, PrimitiveTopology, VertexAttributeValues};
use veronica_geometry::{
    AttributeData, EvaluatedMesh, GeometryPayload, ImplicitGeometry, PRIMVAR_NORMAL, PRIMVAR_UV,
    cook, realize,
};
use veronica_graph::{GraphSnapshot, OperatorGraph};
use veronica_scene::{SceneError, render_mesh_from_evaluated};

/// One-cube v2 snapshot: non-trivial size plus an offset center, so the
/// oracle comparison must account for the center explicitly.
const CUBE_JSON: &str = r#"{"version":2,"operators":[
    {"id":1,"kind":"cube","name":"Box","parent":null,
     "position":{"x":0.0,"y":0.0},
     "parameters":{"size":{"vec3":[2.0,1.0,4.0]},"center":{"vec3":[0.5,-1.0,2.0]}}}
],"edges":[]}"#;

/// Restore a v2 snapshot, cook it, and realize the single implicit cube.
///
/// Returns `None` instead of panicking so the lint-clean helper stays
/// honest; callers unwrap in the test body, where the failure surfaces as
/// a named test failure.
fn realized_cube() -> Option<EvaluatedMesh> {
    let snapshot: GraphSnapshot = serde_json::from_str(CUBE_JSON).ok()?;
    let mut graph = OperatorGraph::new();
    graph.restore(snapshot).ok()?;
    let cooked = cook(&graph).ok()?;
    if cooked.len() != 1 {
        return None;
    }
    let GeometryPayload::Implicit(implicit) = &cooked[0].1 else {
        return None;
    };
    Some(realize(implicit))
}

/// Engine oracle lists: positions, normals, uvs, indices.
type OracleLists = (Vec<[f32; 3]>, Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<u32>);

/// Extract the oracle's position/normal/uv/index lists, failing the lookup
/// (not the test) when the engine shape surprises us.
fn oracle_lists(mesh: &Mesh) -> Option<OracleLists> {
    let positions = match mesh.attribute(Mesh::ATTRIBUTE_POSITION)? {
        VertexAttributeValues::Float32x3(values) => values.clone(),
        _ => return None,
    };
    let normals = match mesh.attribute(Mesh::ATTRIBUTE_NORMAL)? {
        VertexAttributeValues::Float32x3(values) => values.clone(),
        _ => return None,
    };
    let uvs = match mesh.attribute(Mesh::ATTRIBUTE_UV_0)? {
        VertexAttributeValues::Float32x2(values) => values.clone(),
        _ => return None,
    };
    let indices = match mesh.indices()? {
        Indices::U32(values) => values.clone(),
        Indices::U16(_) => return None,
    };
    Some((positions, normals, uvs, indices))
}

/// Worst absolute component difference between pipeline positions and
/// engine positions: the only relation allowed across the precision
/// boundary is closeness, never identity.
fn max_abs_diff(mine: &[[f64; 3]], oracle: &[[f32; 3]]) -> f64 {
    mine.iter()
        .zip(oracle.iter())
        .flat_map(|(a, b)| {
            [
                (a[0] - f64::from(b[0])).abs(),
                (a[1] - f64::from(b[1])).abs(),
                (a[2] - f64::from(b[2])).abs(),
            ]
        })
        .fold(0.0, f64::max)
}

/// Bit patterns of pipeline values narrowed the way the handoff narrows
/// them: exact comparison must happen in `f32` land, never across it.
#[allow(
    clippy::cast_possible_truncation,
    reason = "test oracle mirrors the specified f64->f32 handoff (ADR-0005) to pin its output bits"
)]
fn narrowed_bits(values: &[[f64; 3]]) -> Vec<[u32; 3]> {
    values
        .iter()
        .map(|v| {
            [
                (v[0] as f32).to_bits(),
                (v[1] as f32).to_bits(),
                (v[2] as f32).to_bits(),
            ]
        })
        .collect()
}

fn f32_bits(values: &[[f32; 3]]) -> Vec<[u32; 3]> {
    values
        .iter()
        .map(|v| [v[0].to_bits(), v[1].to_bits(), v[2].to_bits()])
        .collect()
}

/// Widen engine values to pipeline precision: widening is exact, so
/// bit identity across it is the requirement (no narrowing anywhere in
/// test code outside [`narrowed_bits`]).
fn widened_bits3(values: &[[f32; 3]]) -> Vec<[u64; 3]> {
    values
        .iter()
        .map(|v| {
            [
                f64::from(v[0]).to_bits(),
                f64::from(v[1]).to_bits(),
                f64::from(v[2]).to_bits(),
            ]
        })
        .collect()
}

/// Widen engine uvs to pipeline precision; same exactness contract.
fn widened_bits2(values: &[[f32; 2]]) -> Vec<[u64; 2]> {
    values
        .iter()
        .map(|v| [f64::from(v[0]).to_bits(), f64::from(v[1]).to_bits()])
        .collect()
}

fn pipeline_bits3(values: &[[f64; 3]]) -> Vec<[u64; 3]> {
    values
        .iter()
        .map(|v| [v[0].to_bits(), v[1].to_bits(), v[2].to_bits()])
        .collect()
}

fn pipeline_bits2(values: &[[f64; 2]]) -> Vec<[u64; 2]> {
    values
        .iter()
        .map(|v| [v[0].to_bits(), v[1].to_bits()])
        .collect()
}

#[test]
fn realized_cube_matches_engine_cuboid_oracle() {
    // Arrange: pipeline realization of the offset cube, engine cuboid of
    // the same size at the origin (the engine builder takes no center).
    let mesh = realized_cube().unwrap();
    let oracle = Mesh::from(Cuboid::new(2.0, 1.0, 4.0));
    let Some((oracle_positions, oracle_normals, oracle_uvs, oracle_indices)) =
        oracle_lists(&oracle)
    else {
        panic!("engine cuboid must carry position/normal/uv/indices");
    };

    // Act: recenter pipeline positions so both boxes sit at the origin.
    let recentered: Vec<[f64; 3]> = mesh
        .positions
        .iter()
        .map(|p| [p[0] - 0.5, p[1] + 1.0, p[2] - 2.0])
        .collect();

    // Assert: same topology counts, close positions, identical channels.
    // Normals and uvs are exactly representable in both precisions, so
    // widened bit identity is the requirement; positions cross the
    // precision boundary, so closeness is the requirement there.
    assert_eq!(mesh.positions.len(), 24);
    assert_eq!(mesh.indices.len(), 36);
    assert!(max_abs_diff(&recentered, &oracle_positions) <= 1e-6);
    let Some(AttributeData::Vec3(normals)) = mesh.attributes.get(PRIMVAR_NORMAL) else {
        panic!("normals must travel as a vec3 channel");
    };
    let mut mine_normals = pipeline_bits3(normals);
    let mut expected_normals = widened_bits3(&oracle_normals);
    mine_normals.sort_unstable();
    expected_normals.sort_unstable();
    assert_eq!(mine_normals, expected_normals);
    let Some(AttributeData::Vec2(uvs)) = mesh.attributes.get(PRIMVAR_UV) else {
        panic!("uvs must travel as a vec2 channel");
    };
    let mut mine_uvs = pipeline_bits2(uvs);
    let mut expected_uvs = widened_bits2(&oracle_uvs);
    mine_uvs.sort_unstable();
    expected_uvs.sort_unstable();
    assert_eq!(mine_uvs.len(), 24);
    assert_eq!(mine_uvs, expected_uvs);
    assert_eq!(mesh.indices, oracle_indices);
}

#[test]
fn converted_mesh_carries_channels_bound_by_name() {
    // Arrange + act: full pipeline to the engine mesh.
    let mesh = realized_cube().unwrap();
    let converted = render_mesh_from_evaluated(&mesh).unwrap();

    // Assert: topology, positions, and both standard channels read back.
    assert_eq!(
        converted.primitive_topology(),
        PrimitiveTopology::TriangleList
    );
    assert_eq!(converted.count_vertices(), 24);
    let positions = match converted.attribute(Mesh::ATTRIBUTE_POSITION) {
        Some(VertexAttributeValues::Float32x3(values)) => values.clone(),
        other => panic!("expected f32 positions, found {other:?}"),
    };
    assert_eq!(f32_bits(&positions), narrowed_bits(&mesh.positions));
    let normals = match converted.attribute(Mesh::ATTRIBUTE_NORMAL) {
        Some(VertexAttributeValues::Float32x3(values)) => values.clone(),
        other => panic!("expected f32 normals, found {other:?}"),
    };
    assert_eq!(
        normals[0].map(f32::to_bits),
        [0.0, 0.0, 1.0].map(f32::to_bits)
    );
    let uvs = match converted.attribute(Mesh::ATTRIBUTE_UV_0) {
        Some(VertexAttributeValues::Float32x2(values)) => values.clone(),
        other => panic!("expected f32 uvs, found {other:?}"),
    };
    assert_eq!(uvs[0].map(f32::to_bits), [0.0, 0.0].map(f32::to_bits));
    assert_eq!(uvs[1].map(f32::to_bits), [1.0, 0.0].map(f32::to_bits));
    assert_eq!(uvs[2].map(f32::to_bits), [1.0, 1.0].map(f32::to_bits));
    assert_eq!(uvs[3].map(f32::to_bits), [0.0, 1.0].map(f32::to_bits));
    assert_eq!(
        converted.indices(),
        Some(&Indices::U32(mesh.indices.clone()))
    );
}

#[test]
#[allow(
    clippy::cast_possible_truncation,
    reason = "asserting the specified f64->f32 handoff output bit-for-bit (ADR-0005) requires spelling the narrowing"
)]
fn precision_loss_happens_only_at_handoff() {
    // Arrange: a size whose half is not representable in f32.
    let params = veronica_geometry::CubeParams::new([0.1, 1.0, 1.0], [0.0, 0.0, 0.0]);
    let mesh = realize(&ImplicitGeometry::Cube(params));

    // Assert: pipeline precision holds the exact f64 half (division by two
    // is exact), and the engine mesh holds exactly the f32 narrowing.
    assert_eq!(mesh.positions[0][0].to_bits(), (-0.05_f64).to_bits());
    let converted = render_mesh_from_evaluated(&mesh).unwrap();
    let positions = match converted.attribute(Mesh::ATTRIBUTE_POSITION) {
        Some(VertexAttributeValues::Float32x3(values)) => values.clone(),
        other => panic!("expected f32 positions, found {other:?}"),
    };
    assert_eq!(positions[0][0].to_bits(), (-0.05_f64 as f32).to_bits());
    assert_ne!(f64::from(positions[0][0]).to_bits(), (-0.05_f64).to_bits());
}

#[test]
fn missing_normal_names_the_channel() {
    // Arrange: realized cube with normals removed.
    let mut mesh = realize(&ImplicitGeometry::Cube(
        veronica_geometry::CubeParams::default(),
    ));
    mesh.attributes.remove(PRIMVAR_NORMAL);

    // Act + assert.
    assert_eq!(
        render_mesh_from_evaluated(&mesh),
        Err(SceneError::MissingAttribute {
            name: "normal".to_owned(),
        })
    );
}

#[test]
fn wrong_channel_shape_names_expected() {
    // Arrange: normals stored as scalars instead of vec3.
    let mut mesh = realize(&ImplicitGeometry::Cube(
        veronica_geometry::CubeParams::default(),
    ));
    mesh.attributes.insert(
        PRIMVAR_NORMAL.to_owned(),
        AttributeData::Float(vec![0.0; 24]),
    );

    // Act + assert.
    assert_eq!(
        render_mesh_from_evaluated(&mesh),
        Err(SceneError::AttributeShape {
            name: "normal".to_owned(),
            expected: "a vec3 channel",
        })
    );
}

#[test]
fn short_normals_name_counts() {
    // Arrange: normals shorter than the vertex list (the vec3 length arm
    // the other tests leave uncovered).
    let mut mesh = realize(&ImplicitGeometry::Cube(
        veronica_geometry::CubeParams::default(),
    ));
    mesh.attributes.insert(
        PRIMVAR_NORMAL.to_owned(),
        AttributeData::Vec3(vec![[0.0, 0.0, 1.0]; 3]),
    );

    // Act + assert.
    assert_eq!(
        render_mesh_from_evaluated(&mesh),
        Err(SceneError::AttributeLength {
            name: "normal".to_owned(),
            expected: 24,
            actual: 3,
        })
    );
}

#[test]
fn mistyped_uv_names_expected() {
    // Arrange: uvs stored as scalars instead of vec2.
    let mut mesh = realize(&ImplicitGeometry::Cube(
        veronica_geometry::CubeParams::default(),
    ));
    mesh.attributes
        .insert(PRIMVAR_UV.to_owned(), AttributeData::Float(vec![0.0; 24]));

    // Act + assert.
    assert_eq!(
        render_mesh_from_evaluated(&mesh),
        Err(SceneError::AttributeShape {
            name: "uv".to_owned(),
            expected: "a vec2 channel",
        })
    );
}

#[test]
fn short_channel_names_counts() {
    // Arrange: uvs shorter than the vertex list.
    let mut mesh = realize(&ImplicitGeometry::Cube(
        veronica_geometry::CubeParams::default(),
    ));
    mesh.attributes.insert(
        PRIMVAR_UV.to_owned(),
        AttributeData::Vec2(vec![[0.0, 0.0]; 3]),
    );

    // Act + assert.
    assert_eq!(
        render_mesh_from_evaluated(&mesh),
        Err(SceneError::AttributeLength {
            name: "uv".to_owned(),
            expected: 24,
            actual: 3,
        })
    );
}
