//! Frame types and the viewport sizing contract for the GPU readback path.
//!
//! Pixels published by [`SceneWorld::render_frame`](crate::SceneWorld::render_frame)
//! come off the GPU (`crate::gpu::readback_frame`) as tight row-major BGRA8
//! bytes: channel order is producer-specified, see [`RenderFrame`]. The
//! deterministic CPU rasterizer oracle that used to live here was deleted
//! (issue #51) — GPU viability is proven by the UI pixel test, so Rust pins
//! cooked-mesh presence, attributes, and entity reconciliation instead of
//! pixel bytes.

use crate::SceneError;

/// Fixed initial frame width in pixels (see [`MAX_VIEWPORT_EDGE`]).
pub const FRAME_WIDTH: u32 = 512;
/// Fixed initial frame height in pixels (see [`MAX_VIEWPORT_EDGE`]).
pub const FRAME_HEIGHT: u32 = 320;
/// Long-edge cap for the live viewport: `set_viewport_size` accepts any
/// nonzero `width` x `height` with `max(width, height) <= MAX_VIEWPORT_EDGE`.
///
/// The bound keeps one padded staging buffer comfortably in GPU memory
/// (2048 x 2048 x 4 B ≈ 16 MiB before row padding) while covering every
/// realistic Retina pane. Raise it only with a memory rationale in tow.
pub const MAX_VIEWPORT_EDGE: u32 = 2048;
/// Bytes per pixel in the published frames.
pub const FRAME_BYTES_PER_PIXEL: usize = 4;

/// One published frame: fixed-size 4-bytes-per-pixel BGRA8 bytes from the
/// GPU readback path ([`RenderFrame::from_raw_parts`]), row-major,
/// non-premultiplied.
#[derive(Debug, Clone)]
pub struct RenderFrame {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl RenderFrame {
    /// Frame width in pixels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Frame height in pixels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Raw BGRA8 frame bytes, row-major (`width * height * 4` long).
    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Assemble a published frame from externally rendered BGRA8 bytes.
    ///
    /// Crate-visible constructor for the GPU readback path
    /// (`crate::gpu::readback_frame`), which produces tight row-major bytes.
    ///
    /// # Errors
    ///
    /// Returns [`SceneError::FrameLengthMismatch`] when `pixels` is not exactly
    /// `width * height * 4` bytes long.
    pub(crate) fn from_raw_parts(
        width: u32,
        height: u32,
        pixels: Vec<u8>,
    ) -> Result<RenderFrame, SceneError> {
        let expected = width as usize * height as usize * FRAME_BYTES_PER_PIXEL;
        if pixels.len() != expected {
            return Err(SceneError::FrameLengthMismatch {
                expected,
                actual: pixels.len(),
            });
        }
        Ok(RenderFrame {
            width,
            height,
            pixels,
        })
    }
}

/// Cast a `u32` frame extent to `f32`.
///
/// Exact for every realistic frame: extents below 2^24 convert losslessly
/// and the live viewport caps them at [`MAX_VIEWPORT_EDGE`].
#[allow(
    clippy::cast_precision_loss,
    reason = "frame extents are capped small constants, far below 2^24"
)]
pub(crate) fn f32_from_extent(value: u32) -> f32 {
    value as f32
}
