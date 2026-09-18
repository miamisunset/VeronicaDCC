//! Geometry payloads: what flows on graph edges.
//!
//! Two variants (ADR-0005). [`ImplicitGeometry`] carries parameters only —
//! no vertices are allocated — so parameter-only downstream nodes never pay
//! for topology. [`EvaluatedMesh`] is the realized form: our own `f64` soup
//! with a named attribute map, produced on demand when a consumer needs
//! topology. It is deliberately not the engine mesh type, which is `f32`
//! and would pull render types into the Bevy-free crates.

use std::collections::BTreeMap;

use crate::{CubeParams, SphereParams};

/// Standard per-vertex normal channel in the attribute map.
pub const PRIMVAR_NORMAL: &str = "normal";
/// Standard per-vertex uv channel in the attribute map.
pub const PRIMVAR_UV: &str = "uv";

/// The cook output (and realization input) travelling on graph edges.
#[derive(Debug, Clone, PartialEq)]
pub enum GeometryPayload {
    /// Parameters only; no vertices allocated.
    Implicit(ImplicitGeometry),
    /// Realized vertices plus named channels.
    Evaluated(EvaluatedMesh),
}

/// Lightweight, unevaluated geometry: the operator's parameters, verbatim.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ImplicitGeometry {
    /// A cube described by size and center; realization is downstream's job.
    Cube(CubeParams),
    /// A sphere described by resolution, radius, and center; realization is
    /// downstream's job. No operator cooks this yet (graph kind lands in
    /// the S2 slice); the variant exists so realization and its contract
    /// tests pin the ordinal layout first.
    Sphere(SphereParams),
}

/// Realized mesh in pipeline precision: positions plus indexed topology and
/// a named channel map (glossary `Primvar`).
///
/// Standard channels live in the map under [`PRIMVAR_NORMAL`] and
/// [`PRIMVAR_UV`]; custom channels use namespaced keys (e.g.
/// `"veronica:wetness"`) so future producer nodes cannot collide with
/// standards. All data is `f64`; the render handoff converts once.
///
/// [`EvaluatedMesh::tri_to_poly`] groups render triangles into modeling
/// polygons (quads stay whole): entry `t` names the polygon owning triangle
/// `t`, where triangle `t` is `indices[3 * t..3 * t + 3]` in emission order.
/// An empty map means ungrouped (hand-built meshes); [`realize`](crate::realize)
/// always populates it. The render handoff ignores the map — the engine
/// mesh stays a triangle list — so grouping travels upstream only.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EvaluatedMesh {
    /// Vertex positions in meters, Y-up right-handed.
    pub positions: Vec<[f64; 3]>,
    /// Triangle indices into [`EvaluatedMesh::positions`].
    pub indices: Vec<u32>,
    /// Named per-vertex channels, standard and custom.
    pub attributes: BTreeMap<String, AttributeData>,
    /// One polygon id per triangle, in index order.
    pub tri_to_poly: Vec<u32>,
}

/// One named per-vertex channel.
#[derive(Debug, Clone, PartialEq)]
pub enum AttributeData {
    /// Scalar channel (masks, weights).
    Float(Vec<f64>),
    /// 2-vector channel (uvs).
    Vec2(Vec<[f64; 2]>),
    /// 3-vector channel (normals, colors-as-data).
    Vec3(Vec<[f64; 3]>),
}

impl EvaluatedMesh {
    /// Create a mesh from its parts. The polygon map starts empty
    /// (ungrouped); producers that know their topology attach it with
    /// [`EvaluatedMesh::with_triangle_polygons`].
    #[must_use]
    pub fn new(
        positions: Vec<[f64; 3]>,
        indices: Vec<u32>,
        attributes: BTreeMap<String, AttributeData>,
    ) -> Self {
        Self {
            positions,
            indices,
            attributes,
            tri_to_poly: Vec::new(),
        }
    }

    /// Attach the triangle-to-polygon map: one polygon id per triangle in
    /// index order. Consumers (pick, mask, retention) rely on positional
    /// correspondence, so debug builds assert the map length equals the
    /// triangle count (`indices.len() / 3`); release builds trust it for
    /// zero cost.
    #[must_use]
    pub fn with_triangle_polygons(mut self, tri_to_poly: Vec<u32>) -> Self {
        debug_assert_eq!(
            tri_to_poly.len(),
            self.indices.len() / 3,
            "polygon map must name every triangle exactly once"
        );
        self.tri_to_poly = tri_to_poly;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn implicit_carries_params_with_no_vertex_storage() {
        // By construction: the implicit variant holds parameters only, so
        // cooking one can never allocate vertices. This pins the shape the
        // acceptance criterion means by "implicit".
        let payload = GeometryPayload::Implicit(ImplicitGeometry::Cube(CubeParams::default()));
        assert!(
            matches!(
                payload,
                GeometryPayload::Implicit(ImplicitGeometry::Cube(_))
            ),
            "cube must cook to the implicit variant"
        );
    }

    #[test]
    fn evaluated_mesh_holds_named_channels() {
        let mut attributes = BTreeMap::new();
        attributes.insert(PRIMVAR_UV.to_owned(), AttributeData::Vec2(vec![[0.0, 0.0]]));
        let mesh = EvaluatedMesh::new(vec![[0.0, 0.0, 0.0]], vec![0], attributes);
        assert!(mesh.attributes.contains_key(PRIMVAR_UV));
        assert!(!mesh.attributes.contains_key(PRIMVAR_NORMAL));
    }
}
