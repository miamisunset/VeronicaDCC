//! Cube operator parameters: per-axis size plus independent center.
//!
//! Units are meters in Y-up right-handed space (project glossary `Unit`).

use std::collections::BTreeMap;
use veronica_graph::ParamValue;

use crate::CookError;

/// Parameter key for the cube's per-axis size.
pub const CUBE_SIZE_KEY: &str = "size";
/// Parameter key for the cube's center.
pub const CUBE_CENTER_KEY: &str = "center";
/// Default size: a 1 m box on every axis (Houdini Box parity).
pub const DEFAULT_CUBE_SIZE: [f64; 3] = [1.0, 1.0, 1.0];
/// Default center: the origin.
pub const DEFAULT_CUBE_CENTER: [f64; 3] = [0.0, 0.0, 0.0];

/// Expected shape named by [`CookError::InvalidParameter`] for cube vectors.
const EXPECTED_VEC3: &str = "a vec3 of three f64 numbers";

/// Cooked view of a [`veronica_graph::OperatorKind::Cube`] operator's parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CubeParams {
    /// Full extents per axis in meters (not half-extents).
    pub size: [f64; 3],
    /// Box center in meters.
    pub center: [f64; 3],
}

impl CubeParams {
    /// Build parameters from explicit size and center.
    #[must_use]
    pub const fn new(size: [f64; 3], center: [f64; 3]) -> Self {
        Self { size, center }
    }

    /// Parse parameters from an operator's typed map.
    ///
    /// Absent keys fall back to the documented defaults
    /// ([`DEFAULT_CUBE_SIZE`], [`DEFAULT_CUBE_CENTER`]); unknown keys are
    /// ignored so additive growth stays forward-tolerant. Values travel
    /// verbatim — including non-finite floats. Shape is validated, values
    /// are not: range checks belong to future validation, not to parsing.
    ///
    /// # Errors
    ///
    /// Returns [`CookError::InvalidParameter`] naming the key and the
    /// expected shape when `size` or `center` is present but not a
    /// [`ParamValue::Vec3`].
    pub fn parse(parameters: &BTreeMap<String, ParamValue>) -> Result<Self, CookError> {
        Ok(Self {
            size: take_vec3(parameters, CUBE_SIZE_KEY, DEFAULT_CUBE_SIZE)?,
            center: take_vec3(parameters, CUBE_CENTER_KEY, DEFAULT_CUBE_CENTER)?,
        })
    }
}

impl Default for CubeParams {
    /// The documented defaults: 1 m box at the origin.
    fn default() -> Self {
        Self {
            size: DEFAULT_CUBE_SIZE,
            center: DEFAULT_CUBE_CENTER,
        }
    }
}

/// Read one optional triple, defaulting when absent and failing loudly when
/// present-but-mistyped (never coerce, never silently substitute).
fn take_vec3(
    parameters: &BTreeMap<String, ParamValue>,
    key: &str,
    default: [f64; 3],
) -> Result<[f64; 3], CookError> {
    match parameters.get(key) {
        None => Ok(default),
        Some(ParamValue::Vec3(triple)) => Ok(*triple),
        Some(_) => Err(CookError::InvalidParameter {
            key: key.to_owned(),
            expected: EXPECTED_VEC3,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(pairs: &[(&str, ParamValue)]) -> BTreeMap<String, ParamValue> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect()
    }

    /// Bit-exact triple comparison: parsing must carry values verbatim, so
    /// closeness is the wrong relation — identity is the requirement.
    fn assert_triple_bits_eq(actual: [f64; 3], expected: [f64; 3]) {
        for (index, (a, e)) in actual.iter().zip(expected.iter()).enumerate() {
            assert_eq!(a.to_bits(), e.to_bits(), "component {index}");
        }
    }

    #[test]
    fn absent_keys_fall_back_to_documented_defaults() {
        assert_eq!(
            CubeParams::parse(&params(&[])).unwrap(),
            CubeParams::default()
        );
        assert_triple_bits_eq(CubeParams::default().size, DEFAULT_CUBE_SIZE);
        assert_triple_bits_eq(CubeParams::default().center, DEFAULT_CUBE_CENTER);
    }

    #[test]
    fn explicit_triples_are_carried_verbatim() {
        let parsed = CubeParams::parse(&params(&[
            ("size", ParamValue::Vec3([2.0, 0.5, 3.25])),
            ("center", ParamValue::Vec3([0.0, -0.0, 10.0])),
        ]))
        .unwrap();
        assert_triple_bits_eq(parsed.size, [2.0, 0.5, 3.25]);
        // `-0.0 == 0.0` under `PartialEq`; the cook must not normalize it away.
        assert_eq!(parsed.center[1].to_bits(), (-0.0_f64).to_bits());
    }

    #[test]
    fn each_wrong_variant_fails_naming_key_and_shape() {
        for key in [CUBE_SIZE_KEY, CUBE_CENTER_KEY] {
            for wrong in [
                ParamValue::Float(1.0),
                ParamValue::Integer(1),
                ParamValue::Text("1".to_owned()),
                ParamValue::Flag(true),
            ] {
                assert_eq!(
                    CubeParams::parse(&params(&[(key, wrong)])),
                    Err(CookError::InvalidParameter {
                        key: key.to_owned(),
                        expected: EXPECTED_VEC3,
                    }),
                    "{key} must reject every non-vec3 variant"
                );
            }
        }
    }

    #[test]
    fn unknown_keys_are_ignored_for_forward_tolerance() {
        let parsed = CubeParams::parse(&params(&[
            ("size", ParamValue::Vec3([2.0, 2.0, 2.0])),
            ("bevel", ParamValue::Float(0.1)),
        ]))
        .unwrap();
        assert_triple_bits_eq(parsed.size, [2.0, 2.0, 2.0]);
        assert_triple_bits_eq(parsed.center, DEFAULT_CUBE_CENTER);
    }

    #[test]
    fn non_finite_values_are_carried_not_validated() {
        // Shape validation is parsing's job; value validation is a future
        // range check's. NaN/±inf pass through verbatim.
        let parsed = CubeParams::parse(&params(&[(
            "size",
            ParamValue::Vec3([f64::NAN, f64::INFINITY, 1.0]),
        )]))
        .unwrap();
        assert!(parsed.size[0].is_nan());
        assert_eq!(parsed.size[1].to_bits(), f64::INFINITY.to_bits());
    }
}
