//! Realization: implicit parameters to evaluated vertices, on demand.
//!
//! [`realize`] turns [`ImplicitGeometry`] into [`EvaluatedMesh`] `f64` soup.
//! It uses the same face decomposition as the engine cuboid builder (six
//! faces of four vertices, full `0..1` uvs per face, two triangles per
//! face), so the oracle test in `veronica-scene` pins layout parity vertex
//! for vertex. Realization is pure and total: the implicit form is already
//! validated, so every variant has a realization function and no new error
//! type is needed.

use crate::{
    AttributeData, CubeParams, EvaluatedMesh, ImplicitGeometry, PRIMVAR_NORMAL, PRIMVAR_UV,
};

/// Realize implicit geometry into evaluated vertices.
///
/// The `match` stays exhaustive on purpose: a new [`ImplicitGeometry`]
/// variant breaks compilation here until its realization exists.
#[must_use]
pub fn realize(implicit: &ImplicitGeometry) -> EvaluatedMesh {
    match implicit {
        ImplicitGeometry::Cube(params) => realize_cube(params),
    }
}

/// One cube face: four `(position, normal, uv)` corners.
type Face = [([f64; 3], [f64; 3], [f64; 2]); 4];

/// Realize a cube to 24 vertices (six split faces, normals stay per-face)
/// and 36 indices (two triangles per face).
fn realize_cube(params: &CubeParams) -> EvaluatedMesh {
    let half = [
        params.size[0] / 2.0,
        params.size[1] / 2.0,
        params.size[2] / 2.0,
    ];
    let min = [
        params.center[0] - half[0],
        params.center[1] - half[1],
        params.center[2] - half[2],
    ];
    let max = [
        params.center[0] + half[0],
        params.center[1] + half[1],
        params.center[2] + half[2],
    ];

    // Y-up right-handed, camera looking from +Z toward -Z: same face order,
    // corner order, and uv assignment as the engine cuboid builder.
    let faces: [Face; 6] = [
        // Front (+Z).
        [
            ([min[0], min[1], max[2]], [0.0, 0.0, 1.0], [0.0, 0.0]),
            ([max[0], min[1], max[2]], [0.0, 0.0, 1.0], [1.0, 0.0]),
            ([max[0], max[1], max[2]], [0.0, 0.0, 1.0], [1.0, 1.0]),
            ([min[0], max[1], max[2]], [0.0, 0.0, 1.0], [0.0, 1.0]),
        ],
        // Back (-Z).
        [
            ([min[0], max[1], min[2]], [0.0, 0.0, -1.0], [1.0, 0.0]),
            ([max[0], max[1], min[2]], [0.0, 0.0, -1.0], [0.0, 0.0]),
            ([max[0], min[1], min[2]], [0.0, 0.0, -1.0], [0.0, 1.0]),
            ([min[0], min[1], min[2]], [0.0, 0.0, -1.0], [1.0, 1.0]),
        ],
        // Right (+X).
        [
            ([max[0], min[1], min[2]], [1.0, 0.0, 0.0], [0.0, 0.0]),
            ([max[0], max[1], min[2]], [1.0, 0.0, 0.0], [1.0, 0.0]),
            ([max[0], max[1], max[2]], [1.0, 0.0, 0.0], [1.0, 1.0]),
            ([max[0], min[1], max[2]], [1.0, 0.0, 0.0], [0.0, 1.0]),
        ],
        // Left (-X).
        [
            ([min[0], min[1], max[2]], [-1.0, 0.0, 0.0], [1.0, 0.0]),
            ([min[0], max[1], max[2]], [-1.0, 0.0, 0.0], [0.0, 0.0]),
            ([min[0], max[1], min[2]], [-1.0, 0.0, 0.0], [0.0, 1.0]),
            ([min[0], min[1], min[2]], [-1.0, 0.0, 0.0], [1.0, 1.0]),
        ],
        // Top (+Y).
        [
            ([max[0], max[1], min[2]], [0.0, 1.0, 0.0], [1.0, 0.0]),
            ([min[0], max[1], min[2]], [0.0, 1.0, 0.0], [0.0, 0.0]),
            ([min[0], max[1], max[2]], [0.0, 1.0, 0.0], [0.0, 1.0]),
            ([max[0], max[1], max[2]], [0.0, 1.0, 0.0], [1.0, 1.0]),
        ],
        // Bottom (-Y).
        [
            ([max[0], min[1], max[2]], [0.0, -1.0, 0.0], [0.0, 0.0]),
            ([min[0], min[1], max[2]], [0.0, -1.0, 0.0], [1.0, 0.0]),
            ([min[0], min[1], min[2]], [0.0, -1.0, 0.0], [1.0, 1.0]),
            ([max[0], min[1], min[2]], [0.0, -1.0, 0.0], [0.0, 1.0]),
        ],
    ];

    let mut positions = Vec::with_capacity(24);
    let mut normals = Vec::with_capacity(24);
    let mut uvs = Vec::with_capacity(24);
    let mut indices = Vec::with_capacity(36);
    let mut base: u32 = 0;
    for face in &faces {
        for (position, normal, uv) in face {
            positions.push(*position);
            normals.push(*normal);
            uvs.push(*uv);
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 3, base]);
        base += 4;
    }

    EvaluatedMesh::new(
        positions,
        indices,
        [
            (PRIMVAR_NORMAL.to_owned(), AttributeData::Vec3(normals)),
            (PRIMVAR_UV.to_owned(), AttributeData::Vec2(uvs)),
        ]
        .into_iter()
        .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CubeParams;

    fn bits_eq(actual: [f64; 3], expected: [f64; 3]) {
        for (index, (a, e)) in actual.iter().zip(expected.iter()).enumerate() {
            assert_eq!(a.to_bits(), e.to_bits(), "component {index}");
        }
    }

    #[test]
    fn default_cube_realizes_to_unit_box_at_origin() {
        // Arrange: the documented defaults, 1 m box at the origin.
        // Act.
        let mesh = realize(&ImplicitGeometry::Cube(CubeParams::default()));

        // Assert: split-vertex layout, exact halves, standard channels keyed.
        assert_eq!(mesh.positions.len(), 24);
        assert_eq!(mesh.indices.len(), 36);
        bits_eq(mesh.positions[0], [-0.5, -0.5, 0.5]);
        bits_eq(mesh.positions[2], [0.5, 0.5, 0.5]);
        bits_eq(mesh.positions[23], [0.5, -0.5, -0.5]);
        assert_eq!(&mesh.indices[0..6], &[0, 1, 2, 2, 3, 0]);
        assert_eq!(&mesh.indices[30..36], &[20, 21, 22, 22, 23, 20]);
        assert!(
            matches!(
                mesh.attributes.get(PRIMVAR_NORMAL),
                Some(AttributeData::Vec3(normals)) if normals.len() == 24
            ),
            "normals must travel as a 24-element vec3 channel"
        );
        assert!(
            matches!(
                mesh.attributes.get(PRIMVAR_UV),
                Some(AttributeData::Vec2(uvs)) if uvs.len() == 24
            ),
            "uvs must travel as a 24-element vec2 channel"
        );
    }

    #[test]
    fn realization_carries_size_and_center_verbatim() {
        // Arrange: non-trivial size and offset center (all exactly
        // representable, so bit identity is the requirement).
        let params = CubeParams::new([2.0, 0.5, 3.25], [1.0, -2.0, 10.0]);

        // Act.
        let mesh = realize(&ImplicitGeometry::Cube(params));

        // Assert: front-face min/max corners land exactly.
        bits_eq(mesh.positions[0], [0.0, -2.25, 11.625]);
        bits_eq(mesh.positions[2], [2.0, -1.75, 11.625]);
    }

    #[test]
    fn triangle_winding_agrees_with_stored_normals() {
        // Every triangle's geometric normal must point along its vertices'
        // stored normal: flipped winding renders inside-out.
        let mesh = realize(&ImplicitGeometry::Cube(CubeParams::new(
            [2.0, 3.0, 4.0],
            [5.0, -1.0, 0.5],
        )));
        let normals = match mesh.attributes.get(PRIMVAR_NORMAL) {
            Some(AttributeData::Vec3(normals)) => normals,
            other => panic!("expected vec3 normals, found {other:?}"),
        };
        for triangle in mesh.indices.chunks_exact(3) {
            let [a, b, c] = [triangle[0], triangle[1], triangle[2]];
            let ab = sub(mesh.positions[b as usize], mesh.positions[a as usize]);
            let ac = sub(mesh.positions[c as usize], mesh.positions[a as usize]);
            let geometric = cross(ab, ac);
            let stored = average([
                normals[a as usize],
                normals[b as usize],
                normals[c as usize],
            ]);
            assert!(
                dot(geometric, stored) > 0.0,
                "triangle {triangle:?} winds against its normals"
            );
        }
    }

    fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
        [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
    }

    fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    }

    fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
        a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
    }

    fn average(vectors: [[f64; 3]; 3]) -> [f64; 3] {
        [
            (vectors[0][0] + vectors[1][0] + vectors[2][0]) / 3.0,
            (vectors[0][1] + vectors[1][1] + vectors[2][1]) / 3.0,
            (vectors[0][2] + vectors[1][2] + vectors[2][2]) / 3.0,
        ]
    }
}
