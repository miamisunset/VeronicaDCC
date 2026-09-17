//! Sphere operator parameters: resolution-driven topology.
//!
//! A sphere is the graph's first topology-varying operator: `segments` and
//! `rings` set the triangle budget, so unlike the fixed-topology cube the
//! same node cooks different counts across recooks. That is exactly what
//! the [`crate::selection_survives_recook`] staleness rule keys on —
//! resolution edits clear Selection, radius/center edits retain it.
//!
//! Validity is established at construction ([`SphereParams::new`],
//! [`SphereParams::parse`]): resolution bounds plus positive finite radius.
//! Realization stays pure and total over the validated type, so no new
//! error type is needed downstream.

use std::collections::BTreeMap;
use veronica_graph::ParamValue;

use crate::CookError;
use crate::cube::take_vec3;

/// Parameter key for the sphere's width resolution.
pub const SPHERE_SEGMENTS_KEY: &str = "segments";
/// Parameter key for the sphere's height resolution.
pub const SPHERE_RINGS_KEY: &str = "rings";
/// Parameter key for the sphere's radius in meters.
pub const SPHERE_RADIUS_KEY: &str = "radius";
/// Parameter key for the sphere's center in meters.
pub const SPHERE_CENTER_KEY: &str = "center";

/// Default width resolution: 32 segments (960 triangles at default rings).
pub const DEFAULT_SPHERE_SEGMENTS: u32 = 32;
/// Default height resolution: 16 rings.
pub const DEFAULT_SPHERE_RINGS: u32 = 16;
/// Default radius: 0.5 m, matching the cube's unit extent.
pub const DEFAULT_SPHERE_RADIUS: f64 = 0.5;
/// Default center: the origin.
pub const DEFAULT_SPHERE_CENTER: [f64; 3] = [0.0, 0.0, 0.0];

/// Fewest width segments that still close a solid (a triangular bipyramid).
pub const MIN_SPHERE_SEGMENTS: u32 = 3;
/// Most width segments the tick loop will cook (~16k triangles at max rings).
pub const MAX_SPHERE_SEGMENTS: u32 = 128;
/// Fewest height rings that still close a solid (two pole fans, no quads).
pub const MIN_SPHERE_RINGS: u32 = 2;
/// Most height rings the tick loop will cook.
pub const MAX_SPHERE_RINGS: u32 = 64;

/// Expected shape named by [`CookError::InvalidParameter`] for `segments`.
const EXPECTED_SEGMENTS: &str = "an integer segment count from 3 to 128";
/// Expected shape named by [`CookError::InvalidParameter`] for `rings`.
const EXPECTED_RINGS: &str = "an integer ring count from 2 to 64";
/// Expected shape named by [`CookError::InvalidParameter`] for `radius`.
const EXPECTED_RADIUS: &str = "a positive finite f64 radius";
/// Expected shape named by [`CookError::InvalidParameter`] for `center`.
const EXPECTED_CENTER: &str = "a vec3 of three f64 numbers";

/// Cooked view of a sphere operator's parameters.
///
/// Fields are private on purpose: the constructor invariant (segments in
/// `3..=128`, rings in `2..=64`, radius positive finite) is what makes
/// realization total — no underflow in `rings - 1`, no division by zero in
/// the normal computation. Construction goes through [`SphereParams::new`]
/// or [`SphereParams::parse`]; reads go through the accessors.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SphereParams {
    segments: u32,
    rings: u32,
    radius: f64,
    center: [f64; 3],
}

impl SphereParams {
    /// Build validated parameters from wire-shaped values.
    ///
    /// Shape (variant) mismatches never reach here — [`SphereParams::parse`]
    /// extracts those first. This checks the value domain instead: integer
    /// range for resolutions, positivity and finiteness for radius. Center
    /// travels verbatim like the cube's (shape only, values unchecked).
    ///
    /// # Errors
    ///
    /// Returns [`CookError::InvalidParameter`] naming the key and the
    /// expected domain when a resolution is out of range or the radius is
    /// not a positive finite number.
    pub fn new(
        segments: i64,
        rings: i64,
        radius: f64,
        center: [f64; 3],
    ) -> Result<Self, CookError> {
        Ok(Self {
            segments: check_resolution(
                SPHERE_SEGMENTS_KEY,
                segments,
                MIN_SPHERE_SEGMENTS,
                MAX_SPHERE_SEGMENTS,
                EXPECTED_SEGMENTS,
            )?,
            rings: check_resolution(
                SPHERE_RINGS_KEY,
                rings,
                MIN_SPHERE_RINGS,
                MAX_SPHERE_RINGS,
                EXPECTED_RINGS,
            )?,
            radius: check_radius(radius)?,
            center,
        })
    }

    /// Parse parameters from an operator's typed map.
    ///
    /// Absent keys fall back to the documented defaults; unknown keys are
    /// ignored so additive growth stays forward-tolerant. Present-but-wrong
    /// values fail loudly: mistyped variants name the expected wire shape,
    /// out-of-domain values name the expected range.
    ///
    /// # Errors
    ///
    /// Returns [`CookError::InvalidParameter`] naming the key and the
    /// expected shape or domain when any present value is unusable.
    pub fn parse(parameters: &BTreeMap<String, ParamValue>) -> Result<Self, CookError> {
        Self::new(
            take_integer(
                parameters,
                SPHERE_SEGMENTS_KEY,
                DEFAULT_SPHERE_SEGMENTS,
                EXPECTED_SEGMENTS,
            )?,
            take_integer(
                parameters,
                SPHERE_RINGS_KEY,
                DEFAULT_SPHERE_RINGS,
                EXPECTED_RINGS,
            )?,
            take_float(
                parameters,
                SPHERE_RADIUS_KEY,
                DEFAULT_SPHERE_RADIUS,
                EXPECTED_RADIUS,
            )?,
            take_vec3(
                parameters,
                SPHERE_CENTER_KEY,
                DEFAULT_SPHERE_CENTER,
                EXPECTED_CENTER,
            )?,
        )
    }

    /// Width resolution: longitude steps around the equator.
    #[must_use]
    pub fn segments(&self) -> u32 {
        self.segments
    }

    /// Height resolution: latitude lines from pole to pole, inclusive.
    #[must_use]
    pub fn rings(&self) -> u32 {
        self.rings
    }

    /// Sphere radius in meters; always positive finite by construction.
    #[must_use]
    pub fn radius(&self) -> f64 {
        self.radius
    }

    /// Sphere center in meters, carried verbatim.
    #[must_use]
    pub fn center(&self) -> [f64; 3] {
        self.center
    }

    /// Triangle budget for these parameters: two pole fans of `segments`
    /// plus `rings - 2` quad bands of `2 * segments`, i.e.
    /// `2 * segments * (rings - 1)`.
    ///
    /// Total over the type: the constructor invariant keeps `rings >= 2`
    /// (no underflow) and the product under `2 * 128 * 63` (no overflow).
    #[must_use]
    pub const fn triangle_count(&self) -> u32 {
        2 * self.segments * (self.rings - 1)
    }
}

impl Default for SphereParams {
    /// The documented defaults: 32 × 16 resolution, 0.5 m radius, origin.
    fn default() -> Self {
        Self {
            segments: DEFAULT_SPHERE_SEGMENTS,
            rings: DEFAULT_SPHERE_RINGS,
            radius: DEFAULT_SPHERE_RADIUS,
            center: DEFAULT_SPHERE_CENTER,
        }
    }
}

/// Check one resolution against its closed range. Wire-exact: the `i64`
/// converts with [`u32::try_from`] (never `as`), so negatives and huge
/// values fail as invalid parameters rather than wrapping.
fn check_resolution(
    key: &str,
    raw: i64,
    min: u32,
    max: u32,
    expected: &'static str,
) -> Result<u32, CookError> {
    match u32::try_from(raw) {
        Ok(value) if (min..=max).contains(&value) => Ok(value),
        _ => Err(CookError::InvalidParameter {
            key: key.to_owned(),
            expected,
        }),
    }
}

/// Check the radius: realization divides by it for normals, so unlike the
/// cube's verbatim-carried size it must be positive finite — a zero or
/// non-finite radius would cook NaN normals into every downstream pass.
fn check_radius(radius: f64) -> Result<f64, CookError> {
    if radius.is_finite() && radius > 0.0 {
        Ok(radius)
    } else {
        Err(CookError::InvalidParameter {
            key: SPHERE_RADIUS_KEY.to_owned(),
            expected: EXPECTED_RADIUS,
        })
    }
}

/// Read one optional integer, defaulting when absent and failing loudly when
/// present-but-mistyped (never coerce, never silently substitute). Range is
/// [`SphereParams::new`]'s job, not this helper's.
fn take_integer(
    parameters: &BTreeMap<String, ParamValue>,
    key: &str,
    default: u32,
    expected: &'static str,
) -> Result<i64, CookError> {
    match parameters.get(key) {
        None => Ok(i64::from(default)),
        Some(ParamValue::Integer(value)) => Ok(*value),
        Some(_) => Err(CookError::InvalidParameter {
            key: key.to_owned(),
            expected,
        }),
    }
}

/// Read one optional float, defaulting when absent and failing loudly when
/// present-but-mistyped (never coerce, never silently substitute).
fn take_float(
    parameters: &BTreeMap<String, ParamValue>,
    key: &str,
    default: f64,
    expected: &'static str,
) -> Result<f64, CookError> {
    match parameters.get(key) {
        None => Ok(default),
        Some(ParamValue::Float(value)) => Ok(*value),
        Some(_) => Err(CookError::InvalidParameter {
            key: key.to_owned(),
            expected,
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

    fn expect_invalid(result: &Result<SphereParams, CookError>, key: &str, expected: &'static str) {
        assert_eq!(
            result,
            &Err(CookError::InvalidParameter {
                key: key.to_owned(),
                expected,
            }),
            "{key} must fail naming its domain"
        );
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
        // Arrange: an empty map.
        // Act.
        let parsed = SphereParams::parse(&params(&[])).unwrap();

        // Assert: every default lands exactly.
        assert_eq!(parsed, SphereParams::default());
        assert_eq!(parsed.segments(), DEFAULT_SPHERE_SEGMENTS);
        assert_eq!(parsed.rings(), DEFAULT_SPHERE_RINGS);
        assert_eq!(parsed.radius().to_bits(), DEFAULT_SPHERE_RADIUS.to_bits());
        assert_triple_bits_eq(parsed.center(), DEFAULT_SPHERE_CENTER);
    }

    #[test]
    fn explicit_values_are_carried_verbatim() {
        // Arrange: every key present with a boundary-legal value.
        // Act.
        let parsed = SphereParams::parse(&params(&[
            ("segments", ParamValue::Integer(3)),
            ("rings", ParamValue::Integer(64)),
            ("radius", ParamValue::Float(2.5)),
            ("center", ParamValue::Vec3([1.0, -0.0, -3.0])),
        ]))
        .unwrap();

        // Assert: integers land exactly, floats bit-identical (`-0.0` must
        // survive — `PartialEq` cannot tell it from `0.0`).
        assert_eq!(parsed.segments(), 3);
        assert_eq!(parsed.rings(), 64);
        assert_eq!(parsed.radius().to_bits(), 2.5_f64.to_bits());
        assert_eq!(parsed.center()[1].to_bits(), (-0.0_f64).to_bits());
    }

    #[test]
    fn each_wrong_variant_fails_naming_key_and_shape() {
        // Arrange/Act/Assert: every non-matching variant per key.
        for wrong in [
            ParamValue::Float(1.0),
            ParamValue::Text("32".to_owned()),
            ParamValue::Flag(true),
            ParamValue::Vec3([32.0, 16.0, 1.0]),
        ] {
            expect_invalid(
                &SphereParams::parse(&params(&[(SPHERE_SEGMENTS_KEY, wrong.clone())])),
                SPHERE_SEGMENTS_KEY,
                EXPECTED_SEGMENTS,
            );
            expect_invalid(
                &SphereParams::parse(&params(&[(SPHERE_RINGS_KEY, wrong)])),
                SPHERE_RINGS_KEY,
                EXPECTED_RINGS,
            );
        }
        for wrong in [
            ParamValue::Integer(1),
            ParamValue::Text("0.5".to_owned()),
            ParamValue::Flag(false),
        ] {
            expect_invalid(
                &SphereParams::parse(&params(&[(SPHERE_RADIUS_KEY, wrong.clone())])),
                SPHERE_RADIUS_KEY,
                EXPECTED_RADIUS,
            );
        }
        for wrong in [
            ParamValue::Float(0.5),
            ParamValue::Integer(1),
            ParamValue::Text("origin".to_owned()),
            ParamValue::Flag(false),
        ] {
            expect_invalid(
                &SphereParams::parse(&params(&[(SPHERE_CENTER_KEY, wrong)])),
                SPHERE_CENTER_KEY,
                EXPECTED_CENTER,
            );
        }
    }

    #[test]
    fn unknown_keys_are_ignored_for_forward_tolerance() {
        // Arrange: a future key alongside valid values.
        // Act.
        let parsed = SphereParams::parse(&params(&[
            ("segments", ParamValue::Integer(8)),
            ("smooth_shading", ParamValue::Flag(true)),
        ]))
        .unwrap();

        // Assert: known keys parse, the stranger passes through silently.
        assert_eq!(parsed.segments(), 8);
        assert_eq!(parsed.rings(), DEFAULT_SPHERE_RINGS);
    }

    #[test]
    fn resolution_bounds_reject_outsiders() {
        // Arrange/Act/Assert: below-minimum, above-maximum, negative, and
        // huge values all fail naming the range; the four corners parse.
        for segments in [0, 1, 2, 129, 1_000_000, -3, i64::MIN, i64::MAX] {
            expect_invalid(
                &SphereParams::parse(&params(&[(
                    SPHERE_SEGMENTS_KEY,
                    ParamValue::Integer(segments),
                )])),
                SPHERE_SEGMENTS_KEY,
                EXPECTED_SEGMENTS,
            );
        }
        for rings in [0, 1, 65, 1_000_000, -2, i64::MIN, i64::MAX] {
            expect_invalid(
                &SphereParams::parse(&params(&[(SPHERE_RINGS_KEY, ParamValue::Integer(rings))])),
                SPHERE_RINGS_KEY,
                EXPECTED_RINGS,
            );
        }
        for (segments, rings) in [(3, 2), (128, 64), (3, 64), (128, 2)] {
            let parsed = SphereParams::parse(&params(&[
                ("segments", ParamValue::Integer(segments)),
                ("rings", ParamValue::Integer(rings)),
            ]))
            .unwrap();
            assert_eq!(parsed.segments(), u32::try_from(segments).unwrap());
            assert_eq!(parsed.rings(), u32::try_from(rings).unwrap());
        }
    }

    #[test]
    fn non_positive_or_non_finite_radius_is_rejected() {
        // Arrange/Act/Assert: realization divides by the radius for
        // normals, so zero, negative, and non-finite radii fail at the
        // boundary instead of cooking NaN normals downstream.
        for radius in [0.0, -0.5, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            expect_invalid(
                &SphereParams::parse(&params(&[(SPHERE_RADIUS_KEY, ParamValue::Float(radius))])),
                SPHERE_RADIUS_KEY,
                EXPECTED_RADIUS,
            );
        }
        // The smallest positive subnormal still parses: finiteness and
        // sign are the contract, magnitude is the caller's business.
        let parsed = SphereParams::parse(&params(&[(
            SPHERE_RADIUS_KEY,
            ParamValue::Float(f64::MIN_POSITIVE),
        )]))
        .unwrap();
        assert_eq!(parsed.radius().to_bits(), f64::MIN_POSITIVE.to_bits());
    }

    #[test]
    fn triangle_count_matches_two_fans_plus_quad_bands() {
        // Arrange: corners plus the defaults.
        // Act/Assert: 2 * segments * (rings - 1), with the minimum (3, 2)
        // cooking two bare pole fans and nothing else.
        for (segments, rings, expected) in [
            (3, 2, 6),
            (4, 3, 16),
            (8, 8, 112),
            (32, 16, 960),
            (128, 64, 16_128),
        ] {
            let params = SphereParams::new(segments, rings, 0.5, DEFAULT_SPHERE_CENTER).unwrap();
            assert_eq!(
                params.triangle_count(),
                expected,
                "segments={segments} rings={rings}"
            );
        }
    }

    #[test]
    fn constructor_rejects_exactly_what_parse_rejects() {
        // Arrange/Act/Assert: `new` is the single validation gate, so its
        // domain matches `parse`'s — including the radius floor.
        expect_invalid(
            &SphereParams::new(2, 16, 0.5, DEFAULT_SPHERE_CENTER),
            SPHERE_SEGMENTS_KEY,
            EXPECTED_SEGMENTS,
        );
        expect_invalid(
            &SphereParams::new(32, 1, 0.5, DEFAULT_SPHERE_CENTER),
            SPHERE_RINGS_KEY,
            EXPECTED_RINGS,
        );
        expect_invalid(
            &SphereParams::new(32, 16, 0.0, DEFAULT_SPHERE_CENTER),
            SPHERE_RADIUS_KEY,
            EXPECTED_RADIUS,
        );
    }
}
