//! Render handoff: pipeline-precision mesh to engine mesh.
//!
//! The single documented `f64`→`f32` conversion lives here (ADR-0005):
//! everything upstream works in `f64`, and narrowing happens in exactly one
//! place, pinned by tests. Standard channels bind by name — the pipeline
//! `normal` key to [`Mesh::ATTRIBUTE_NORMAL`], the `uv` key to
//! [`Mesh::ATTRIBUTE_UV_0`]. Custom namespaced channels stay in
//! [`EvaluatedMesh`]: the engine attribute id needs a `&'static str`, so
//! shader-side binding for customs is later work, not silent dropping.

use bevy_asset::RenderAssetUsages;
use bevy_mesh::{Indices, Mesh, PrimitiveTopology};
use veronica_geometry::{AttributeData, EvaluatedMesh, PRIMVAR_NORMAL, PRIMVAR_UV};

use crate::SceneError;

/// Convert an evaluated pipeline mesh into a render-ready engine mesh.
///
/// Standard channels are bound by name and validated against the vertex
/// count; indices are bounds-checked because [`EvaluatedMesh::new`] is
/// public and hand-built meshes can dangle. Custom channels are preserved
/// upstream, not uploaded.
///
/// # Errors
///
/// Returns [`SceneError::MissingAttribute`] when a standard channel is
/// absent, [`SceneError::AttributeShape`] when it has the wrong shape,
/// [`SceneError::AttributeLength`] when its length differs from the vertex
/// count, and [`SceneError::IndexOutOfBounds`] when an index names no
/// vertex.
pub fn render_mesh_from_evaluated(evaluated: &EvaluatedMesh) -> Result<Mesh, SceneError> {
    let vertex_count = evaluated.positions.len();
    let normals = take_vec3_channel(evaluated, PRIMVAR_NORMAL, vertex_count)?;
    let uvs = take_vec2_channel(evaluated, PRIMVAR_UV, vertex_count)?;
    let bad_index = evaluated
        .indices
        .iter()
        .find(|index| (**index as usize) >= vertex_count);
    if let Some(index) = bad_index {
        return Err(SceneError::IndexOutOfBounds {
            index: *index,
            vertex_count,
        });
    }

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, to_f32x3(&evaluated.positions));
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, to_f32x3(&normals));
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, to_f32x2(&uvs));
    mesh.insert_indices(Indices::U32(evaluated.indices.clone()));
    Ok(mesh)
}

/// Read one mandatory vec3 channel, failing loudly on absence, wrong shape,
/// or length mismatch (never coerce, never silently substitute).
fn take_vec3_channel(
    evaluated: &EvaluatedMesh,
    name: &str,
    vertex_count: usize,
) -> Result<Vec<[f64; 3]>, SceneError> {
    match evaluated.attributes.get(name) {
        None => Err(SceneError::MissingAttribute {
            name: name.to_owned(),
        }),
        Some(AttributeData::Vec3(channel)) if channel.len() == vertex_count => Ok(channel.clone()),
        Some(AttributeData::Vec3(channel)) => Err(SceneError::AttributeLength {
            name: name.to_owned(),
            expected: vertex_count,
            actual: channel.len(),
        }),
        Some(_) => Err(SceneError::AttributeShape {
            name: name.to_owned(),
            expected: "a vec3 channel",
        }),
    }
}

/// Read one mandatory vec2 channel; same loudness contract as vec3.
fn take_vec2_channel(
    evaluated: &EvaluatedMesh,
    name: &str,
    vertex_count: usize,
) -> Result<Vec<[f64; 2]>, SceneError> {
    match evaluated.attributes.get(name) {
        None => Err(SceneError::MissingAttribute {
            name: name.to_owned(),
        }),
        Some(AttributeData::Vec2(channel)) if channel.len() == vertex_count => Ok(channel.clone()),
        Some(AttributeData::Vec2(channel)) => Err(SceneError::AttributeLength {
            name: name.to_owned(),
            expected: vertex_count,
            actual: channel.len(),
        }),
        Some(_) => Err(SceneError::AttributeShape {
            name: name.to_owned(),
            expected: "a vec2 channel",
        }),
    }
}

/// Narrow one position list to engine precision: the boundary conversion.
#[allow(
    clippy::cast_possible_truncation,
    reason = "narrowing is the specified handoff behavior (ADR-0005), pinned by precision_loss_happens_only_at_handoff"
)]
fn to_f32x3(values: &[[f64; 3]]) -> Vec<[f32; 3]> {
    values
        .iter()
        .map(|v| [v[0] as f32, v[1] as f32, v[2] as f32])
        .collect()
}

/// Narrow one uv list to engine precision: the boundary conversion.
#[allow(
    clippy::cast_possible_truncation,
    reason = "narrowing is the specified handoff behavior (ADR-0005), pinned by precision_loss_happens_only_at_handoff"
)]
fn to_f32x2(values: &[[f64; 2]]) -> Vec<[f32; 2]> {
    values.iter().map(|v| [v[0] as f32, v[1] as f32]).collect()
}
