//! Realization: implicit parameters to evaluated vertices, on demand.
//!
//! [`realize`] turns [`ImplicitGeometry`] into [`EvaluatedMesh`] `f64` soup.
//! It uses the same face decomposition as the engine cuboid builder (six
//! faces of four vertices, full `0..1` uvs per face, two triangles per
//! face), so the oracle test in `veronica-scene` pins layout parity vertex
//! for vertex. Realization is pure and total: the implicit form is already
//! validated (cube shapes at parse, sphere domains at construction), so
//! every variant has a realization function and no new error type is needed.

use std::f64::consts::{PI, TAU};

use crate::{
    AttributeData, CubeParams, EvaluatedMesh, ImplicitGeometry, PRIMVAR_NORMAL, PRIMVAR_UV,
    SphereParams,
};

/// Realize implicit geometry into evaluated vertices.
///
/// The `match` stays exhaustive on purpose: a new [`ImplicitGeometry`]
/// variant breaks compilation here until its realization exists.
#[must_use]
pub fn realize(implicit: &ImplicitGeometry) -> EvaluatedMesh {
    match implicit {
        ImplicitGeometry::Cube(params) => realize_cube(params),
        ImplicitGeometry::Sphere(params) => realize_sphere(params),
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

/// One split-vertex soup corner: position, outward normal, and uv.
type Corner = ([f64; 3], [f64; 3], [f64; 2]);

/// Realize a sphere to `3 * triangle_count` split vertices with sequential
/// indices (fully unwelded soup: every triangle owns its three vertices, so
/// face ordinals are exactly triangle order).
///
/// Emission order is the ordinal contract, south pole to north pole: the
/// south pole fan (`segments` triangles), then each quad band ring by ring
/// (two triangles per quad), then the north pole fan. Ring `j` sits at
/// polar angle `PI * j / rings`; quad bands span rings `1..rings - 1`, so
/// the minimum `(3, 2)` sphere is two bare pole fans. Quads emit
/// `(upper, lower, lower_next)` and `(upper, lower_next, upper_next)` —
/// counterclockwise seen from outside, matching the stored radial normals.
/// Poles are written literally from center and radius, never through
/// `sin(PI)`, which is nonzero in floating point.
///
/// Total over the type: [`SphereParams`] guarantees `rings >= 2` (every
/// range still emits) and small counts (every counter stays far from
/// `u32::MAX`).
fn realize_sphere(params: &SphereParams) -> EvaluatedMesh {
    let segments = params.segments();
    let rings = params.rings();
    let radius = params.radius();
    let [cx, cy, cz] = params.center();
    let triangles = params.triangle_count();

    // Ring vertex: latitude line `j` (`0` = north pole line, `rings` =
    // south pole line), longitude step `i` (`segments` wraps the seam, so
    // `i + 1 == segments` duplicates the seam vertex by construction).
    let ring_vertex = |j: u32, i: u32| -> Corner {
        let theta = PI * f64::from(j) / f64::from(rings);
        let phi = TAU * f64::from(i) / f64::from(segments);
        let (sin_theta, cos_theta) = theta.sin_cos();
        let (sin_phi, cos_phi) = phi.sin_cos();
        let offset = [
            radius * sin_theta * sin_phi,
            radius * cos_theta,
            radius * sin_theta * cos_phi,
        ];
        (
            [cx + offset[0], cy + offset[1], cz + offset[2]],
            [offset[0] / radius, offset[1] / radius, offset[2] / radius],
            [
                f64::from(i) / f64::from(segments),
                f64::from(j) / f64::from(rings),
            ],
        )
    };

    let mut positions = Vec::with_capacity(3 * triangles as usize);
    let mut normals = Vec::with_capacity(3 * triangles as usize);
    let mut uvs = Vec::with_capacity(3 * triangles as usize);
    let mut indices = Vec::with_capacity(3 * triangles as usize);
    let mut next: u32 = 0;
    let mut push_triangle = |a: Corner, b: Corner, c: Corner| {
        for (position, normal, uv) in [a, b, c] {
            positions.push(position);
            normals.push(normal);
            uvs.push(uv);
        }
        indices.extend_from_slice(&[next, next + 1, next + 2]);
        next += 3;
    };

    // South pole fan: ordinals `0..segments`, joined to the adjacent
    // (southernmost) ring.
    let south_pole: Corner = ([cx, cy - radius, cz], [0.0, -1.0, 0.0], [0.0, 0.0]);
    for i in 0..segments {
        push_triangle(
            south_pole,
            ring_vertex(rings - 1, i + 1),
            ring_vertex(rings - 1, i),
        );
    }

    // Quad bands, ring by ring. Empty when `rings == 2` (bare fans).
    for j in 1..rings - 1 {
        for i in 0..segments {
            let upper = ring_vertex(j, i);
            let upper_next = ring_vertex(j, i + 1);
            let lower = ring_vertex(j + 1, i);
            let lower_next = ring_vertex(j + 1, i + 1);
            push_triangle(upper, lower, lower_next);
            push_triangle(upper, lower_next, upper_next);
        }
    }

    // North pole fan: the closing `segments` ordinals, joined to the
    // adjacent (northernmost) ring.
    let north_pole: Corner = ([cx, cy + radius, cz], [0.0, 1.0, 0.0], [1.0, 1.0]);
    for i in 0..segments {
        push_triangle(north_pole, ring_vertex(1, i), ring_vertex(1, i + 1));
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
    use crate::{CubeParams, DEFAULT_SPHERE_CENTER, DEFAULT_SPHERE_SEGMENTS};

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

    fn sphere_indices(segments: i64, rings: i64, radius: f64) -> Vec<u32> {
        realize(&ImplicitGeometry::Sphere(
            SphereParams::new(segments, rings, radius, DEFAULT_SPHERE_CENTER).unwrap(),
        ))
        .indices
    }

    #[test]
    fn default_sphere_realizes_to_documented_counts_and_layout() {
        // Arrange: the documented defaults through the dispatch arm.
        // Act.
        let mesh = realize(&ImplicitGeometry::Sphere(SphereParams::default()));

        // Assert: fully unwelded soup — 960 triangles, 3 private vertices
        // and 3 indices each, standard channels keyed per vertex.
        assert_eq!(mesh.indices.len(), 3 * 960);
        assert_eq!(mesh.positions.len(), 3 * 960);
        assert_eq!(&mesh.indices[0..6], &[0, 1, 2, 3, 4, 5]);
        assert_eq!(mesh.indices[mesh.indices.len() - 1], 2_879);
        assert!(
            matches!(
                mesh.attributes.get(PRIMVAR_NORMAL),
                Some(AttributeData::Vec3(normals)) if normals.len() == 2_880
            ),
            "normals must travel as a per-vertex vec3 channel"
        );
        assert!(
            matches!(
                mesh.attributes.get(PRIMVAR_UV),
                Some(AttributeData::Vec2(uvs)) if uvs.len() == 2_880
            ),
            "uvs must travel as a per-vertex vec2 channel"
        );
    }

    #[test]
    fn realized_counts_match_triangle_budget_everywhere() {
        // Arrange/Act/Assert: soup layout means positions and indices both
        // scale as 3x the budget, minimum through maximum.
        for (segments, rings) in [(3, 2), (4, 3), (8, 8), (32, 16), (128, 64)] {
            let params = SphereParams::new(segments, rings, 0.5, DEFAULT_SPHERE_CENTER).unwrap();
            let mesh = realize(&ImplicitGeometry::Sphere(params));
            let budgeted = 3 * params.triangle_count();
            assert_eq!(
                u32::try_from(mesh.indices.len()).unwrap(),
                budgeted,
                "segments={segments} rings={rings}"
            );
            assert_eq!(
                u32::try_from(mesh.positions.len()).unwrap(),
                budgeted,
                "segments={segments} rings={rings}"
            );
        }
    }

    #[test]
    fn poles_open_and_close_the_ordinal_sequence() {
        // Arrange: the minimum sphere (two bare fans) at default radius.
        // Act.
        let mesh = realize(&ImplicitGeometry::Sphere(
            SphereParams::new(3, 2, 0.5, DEFAULT_SPHERE_CENTER).unwrap(),
        ));

        // Assert: the first emitted vertex is the literal south pole, and
        // the first vertex of the last triangle is the literal north pole —
        // written, never computed through sin(PI).
        bits_eq(mesh.positions[0], [0.0, -0.5, 0.0]);
        let last = mesh.positions.len() - 3;
        bits_eq(mesh.positions[last], [0.0, 0.5, 0.0]);
        assert_eq!(mesh.indices.len(), 18);
    }

    #[test]
    fn sphere_vertices_lie_on_the_radius() {
        // Arrange: an off-center sphere (exercises the center offset).
        // Act.
        let mesh = realize(&ImplicitGeometry::Sphere(
            SphereParams::new(12, 8, 2.0, [1.0, -1.0, 0.5]).unwrap(),
        ));

        // Assert: every vertex sits one radius from the center. Exact
        // equality is the wrong relation for sin/cos output — tight
        // tolerance is the requirement.
        for position in &mesh.positions {
            let distance = (position[0] - 1.0)
                .hypot(position[1] + 1.0)
                .hypot(position[2] - 0.5);
            let deviation = (distance - 2.0).abs();
            assert!(
                deviation < 1e-12,
                "vertex {position:?} off the sphere by {deviation}"
            );
        }
    }

    #[test]
    fn sphere_winding_agrees_with_stored_normals() {
        // Every triangle's geometric normal must point along its vertices'
        // stored radial normal: flipped winding renders inside-out.
        let mesh = realize(&ImplicitGeometry::Sphere(
            SphereParams::new(12, 8, 2.0, [1.0, -1.0, 0.5]).unwrap(),
        ));
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

    #[test]
    fn realization_is_deterministic_and_radius_stable() {
        // Arrange: the same implicit sphere cooked twice, plus a radius
        // recook of the same node (the retain half of the staleness rule).
        // Act.
        let first = realize(&ImplicitGeometry::Sphere(SphereParams::default()));
        let second = realize(&ImplicitGeometry::Sphere(SphereParams::default()));
        let resized = realize(&ImplicitGeometry::Sphere(
            SphereParams::new(32, 16, 2.0, DEFAULT_SPHERE_CENTER).unwrap(),
        ));

        // Assert: bit-identical across recooks; a radius change moves
        // vertices but keeps every ordinal (indices untouched).
        assert_eq!(first, second, "realize must be a pure function");
        assert_eq!(
            first.indices, resized.indices,
            "radius edits must preserve face ordinals"
        );
        assert_ne!(
            first.positions, resized.positions,
            "radius edits must move vertices"
        );

        // A center change is the same retain case through translation.
        let moved = realize(&ImplicitGeometry::Sphere(
            SphereParams::new(32, 16, 0.5, [4.0, -2.0, 1.0]).unwrap(),
        ));
        assert_eq!(
            first.indices, moved.indices,
            "center edits must preserve face ordinals"
        );
        assert_ne!(
            first.positions, moved.positions,
            "center edits must move vertices"
        );
    }

    #[test]
    fn pole_fans_cap_their_own_hemisphere() {
        // Regression: the fans once joined each pole to the far ring,
        // cutting diameter-spanning fins through the interior and leaving
        // both polar caps open. Counts, radii, and winding all passed on
        // the miswired mesh — only hemisphere membership catches it.
        //
        // Arrange: the default sphere; south fan owns the first
        // `segments` ordinals, north fan the last `segments`.
        let mesh = realize(&ImplicitGeometry::Sphere(SphereParams::default()));
        let fan_triangles = DEFAULT_SPHERE_SEGMENTS as usize;

        // Act/Assert: every non-pole fan vertex sits on its pole's half.
        // Each fan triangle leads with its pole vertex, so the ring
        // vertices are the second and third of every triple.
        for triangle in mesh.indices.chunks_exact(3).take(fan_triangles) {
            for index in [triangle[1], triangle[2]] {
                assert!(
                    mesh.positions[index as usize][1] < 0.0,
                    "south fan reaches the northern hemisphere"
                );
            }
        }
        for triangle in mesh.indices.chunks_exact(3).rev().take(fan_triangles) {
            for index in [triangle[1], triangle[2]] {
                assert!(
                    mesh.positions[index as usize][1] > 0.0,
                    "north fan reaches the southern hemisphere"
                );
            }
        }
    }

    #[test]
    fn resolution_change_renumbers_ordinals() {
        // Arrange: the same node with one more segment (the clear half of
        // the staleness rule: the count changes, so the pick must drop).
        // Act/Assert: the ordinal sequence is no longer comparable.
        assert_ne!(
            sphere_indices(32, 16, 0.5),
            sphere_indices(33, 16, 0.5),
            "a segment edit must renumber faces"
        );
        assert_ne!(
            sphere_indices(32, 16, 0.5),
            sphere_indices(32, 17, 0.5),
            "a ring edit must renumber faces"
        );
    }
}
