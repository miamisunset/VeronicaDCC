//! Face-ordinal identity contract for sub-element picking (ADR-0007, T2).
//!
//! Identity is a face ordinal: triangle *k* of an operator's realized mesh
//! (glossary `Face`). The on-demand GPU ID pass (T3) rasterizes each triangle
//! in its 24-bit RGB-encoded ordinal and reads back the tapped pixel; this
//! module owns the CPU-side codec both ends must agree on, plus the
//! count-change staleness rule deciding whether a stored Pick survives a
//! recook. Pure functions only — no GPU needed.
//!
//! Miss handling: the RGB channel carries ordinals only, so a miss cannot
//! live inside it without stealing a valid identity. The hit flag travels in
//! alpha instead — [`decode_pick_pixel`] maps alpha-zero pixels to `None`
//! and every other pixel to its ordinal — keeping the full 24-bit range
//! (including 0 and max-u24) round-trippable.

use thiserror::Error;

/// Bits of face-ordinal identity carried in one ID-buffer pixel (R8G8B8).
pub const FACE_ID_BITS: u32 = 24;

/// Largest encodable face ordinal: the whole 24-bit range is valid identity,
/// so the boundary is exactly max-u24.
pub const MAX_FACE_ORDINAL: u32 = 0x00FF_FFFF;

/// Alpha marking a hit pixel in the ID-buffer readback.
pub const HIT_ALPHA: u8 = 255;

/// Alpha marking a miss pixel in the ID-buffer readback. The pass clears to
/// this, so background taps (and taps on a failed render) read back as an
/// explicit miss rather than as face zero.
pub const MISS_ALPHA: u8 = 0;

/// Explicit miss encoding: any RGB with [`MISS_ALPHA`]. Distinct from every
/// valid identity by construction — valid identities always carry
/// [`HIT_ALPHA`] (see [`decode_pick_pixel`]).
pub const MISS_PIXEL: [u8; 4] = [0, 0, 0, MISS_ALPHA];

/// Domain errors for face-identity encoding.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum FaceIdError {
    /// The ordinal exceeds the 24-bit ID-buffer range, so no RGB triple can
    /// hold it. Surfaces loudly (rather than aliasing onto another face via
    /// truncation) so an oversized mesh fails its pick instead of
    /// mis-highlighting.
    #[error("face ordinal {ordinal} exceeds the 24-bit range (max {MAX_FACE_ORDINAL})")]
    OrdinalOutOfRange {
        /// The rejected ordinal.
        ordinal: u32,
    },
}

/// Encode a face ordinal as 24-bit RGB, R carrying the high byte.
///
/// Both ends of the ID pass (rasterize in T3, read back here) must agree on
/// this byte order; the pin tests below lock it.
///
/// # Errors
///
/// Returns [`FaceIdError::OrdinalOutOfRange`] when `ordinal` exceeds
/// [`MAX_FACE_ORDINAL`].
///
/// # Examples
///
/// ```
/// # use veronica_geometry::{decode_face_ordinal, encode_face_ordinal};
/// let rgb = encode_face_ordinal(0x12_3456)?;
/// assert_eq!(rgb, [0x12, 0x34, 0x56]);
/// assert_eq!(decode_face_ordinal(rgb), 0x12_3456);
/// # Ok::<(), veronica_geometry::FaceIdError>(())
/// ```
/// (`Result` is already `#[must_use]`, so no attribute here.)
pub const fn encode_face_ordinal(ordinal: u32) -> Result<[u8; 3], FaceIdError> {
    if ordinal > MAX_FACE_ORDINAL {
        return Err(FaceIdError::OrdinalOutOfRange { ordinal });
    }
    // Big-endian bytes without narrowing casts: `to_be_bytes` keeps the
    // full value visible, and slicing drops the always-zero high byte.
    let bytes = ordinal.to_be_bytes();
    Ok([bytes[1], bytes[2], bytes[3]])
}

/// Decode a 24-bit RGB triple back to its face ordinal.
///
/// Total: every triple is a valid ordinal (miss travels in alpha, never in
/// RGB), so this is the exact inverse of [`encode_face_ordinal`] over
/// `0..=MAX_FACE_ORDINAL`.
#[must_use]
pub const fn decode_face_ordinal(rgb: [u8; 3]) -> u32 {
    u32::from_be_bytes([0, rgb[0], rgb[1], rgb[2]])
}

/// Resolve one read-back ID pixel (RGBA) to a face ordinal, or `None` for a
/// miss.
///
/// Any nonzero alpha is a hit — only [`MISS_ALPHA`] (the pass clear value)
/// means "tapped nothing". In particular RGB black with hit alpha is face
/// zero, not a miss: alpha alone carries the miss flag.
#[must_use]
pub const fn decode_pick_pixel(pixel: [u8; 4]) -> Option<u32> {
    if pixel[3] == MISS_ALPHA {
        None
    } else {
        Some(decode_face_ordinal([pixel[0], pixel[1], pixel[2]]))
    }
}

/// Whether a stored face pick survives a recook (ADR-0007): identity is the
/// face ordinal into the operator's realized mesh, so equal triangle counts
/// retain the pick while any count change invalidates it — a pick must never
/// silently point at the wrong face after a topology change.
#[must_use]
pub const fn selection_survives_recook(
    previous_triangle_count: usize,
    current_triangle_count: usize,
) -> bool {
    previous_triangle_count == current_triangle_count
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CubeParams, ImplicitGeometry, realize};

    #[test]
    fn face_id_carries_exactly_24_bits() {
        assert_eq!(FACE_ID_BITS, 24);
        assert_eq!(MAX_FACE_ORDINAL, 0x00FF_FFFF);
    }

    #[test]
    fn ordinal_zero_round_trips() {
        // Arrange: the lowest valid identity.
        // Act.
        let rgb = encode_face_ordinal(0).unwrap();

        // Assert: black is a valid encoding here (miss lives in alpha).
        assert_eq!(rgb, [0, 0, 0]);
        assert_eq!(decode_face_ordinal(rgb), 0);
    }

    #[test]
    fn ordinal_max_u24_round_trips() {
        // Arrange: the highest valid identity.
        // Act.
        let rgb = encode_face_ordinal(MAX_FACE_ORDINAL).unwrap();

        // Assert.
        assert_eq!(rgb, [0xFF, 0xFF, 0xFF]);
        assert_eq!(decode_face_ordinal(rgb), MAX_FACE_ORDINAL);
    }

    #[test]
    fn interior_ordinals_round_trip() {
        // Byte-boundary walk plus an arbitrary interior value: each byte
        // lane must survive the trip independently.
        for ordinal in [1, 0xFF, 0x100, 0xFFFF, 0x01_0000, 0x12_3456, 0xAB_CD_EF] {
            // Act.
            let rgb = encode_face_ordinal(ordinal).unwrap();

            // Assert.
            assert_eq!(decode_face_ordinal(rgb), ordinal, "ordinal {ordinal:#X}");
        }
    }

    #[test]
    fn ordinal_above_u24_is_rejected_not_truncated() {
        // Arrange: one past the encodable range.
        // Act.
        let result = encode_face_ordinal(MAX_FACE_ORDINAL + 1);

        // Assert: loud failure naming the ordinal, never silent aliasing.
        assert_eq!(
            result,
            Err(FaceIdError::OrdinalOutOfRange {
                ordinal: MAX_FACE_ORDINAL + 1
            })
        );
        assert_eq!(
            result.unwrap_err().to_string(),
            "face ordinal 16777216 exceeds the 24-bit range (max 16777215)"
        );
    }

    #[test]
    fn miss_pixel_decodes_to_none() {
        assert_eq!(decode_pick_pixel(MISS_PIXEL), None);
        assert_eq!(decode_pick_pixel([0x12, 0x34, 0x56, MISS_ALPHA]), None);
    }

    #[test]
    fn black_with_hit_alpha_is_face_zero_not_miss() {
        // The disambiguation the alpha-carries-miss design exists for: RGB
        // black alone must never read as a miss, or face zero would be
        // unpickable.
        assert_eq!(decode_pick_pixel([0, 0, 0, HIT_ALPHA]), Some(0));
    }

    #[test]
    fn hit_pixels_resolve_to_their_encoded_ordinal() {
        for ordinal in [0, 1, 0x12_3456, MAX_FACE_ORDINAL] {
            let rgb = encode_face_ordinal(ordinal).unwrap();
            let pixel = [rgb[0], rgb[1], rgb[2], HIT_ALPHA];
            assert_ne!(pixel, MISS_PIXEL, "hit must differ from miss");
            assert_eq!(decode_pick_pixel(pixel), Some(ordinal));
        }
    }

    #[test]
    fn realize_output_order_is_deterministic_across_recooks() {
        // Arrange: same parameters cooked twice, plus parameters whose
        // values differ (a resize recook must not reorder topology).
        let first = realize(&ImplicitGeometry::Cube(CubeParams::default()));
        let second = realize(&ImplicitGeometry::Cube(CubeParams::default()));
        let resized = realize(&ImplicitGeometry::Cube(CubeParams::new(
            [2.0, 3.0, 4.0],
            [5.0, -1.0, 0.5],
        )));

        // Assert: triangle order pinned — ordinals survive recooks.
        assert_eq!(first.indices, second.indices);
        assert_eq!(first.indices, resized.indices);
        // Sanity: the resize actually moved vertices, so the order pin is
        // load-bearing rather than vacuous.
        assert_ne!(first.positions, resized.positions);
    }

    #[test]
    fn same_triangle_count_retains_identity() {
        assert!(selection_survives_recook(12, 12));
        assert!(selection_survives_recook(0, 0));
    }

    #[test]
    fn changed_triangle_count_invalidates_identity() {
        assert!(!selection_survives_recook(12, 10));
        assert!(!selection_survives_recook(10, 12));
        assert!(!selection_survives_recook(12, 0));
    }
}
