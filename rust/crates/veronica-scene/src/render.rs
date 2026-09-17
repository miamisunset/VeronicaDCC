//! Deterministic CPU rasterizer: the test oracle for the demo scene (ADR-0001).
//!
//! Pixels are derived from live Bevy ECS state — the demo-cube angle — so
//! frames provably differ because scene state advanced, not because Swift
//! re-rendered. Since the GPU slice (issue #27)
//! this rasterizer is kept only as the deterministic test oracle behind the
//! frame-publish seam ([`SceneWorld::render_frame`]): integration tests assert
//! GPU frames are visibly beyond flat rasterization by comparing against
//! these bytes. The published pixels themselves now come from
//! `crate::gpu::readback_frame`; the `IOSurface` handoff and Swift are
//! untouched by either source.
//!
//! Format is RGBA8 for the CPU rasterizer below, row-major, non-premultiplied.
//! Frames from the GPU readback path ([`RenderFrame::from_raw_parts`]) hold
//! BGRA8 instead — channel order is producer-specified, see [`RenderFrame`].

use crate::SceneError;

use super::{DEMO_CAMERA_DISTANCE, DEMO_CAMERA_HEIGHT};

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

/// One published frame: fixed-size 4-bytes-per-pixel bytes, row-major,
/// non-premultiplied. Channel order is producer-specified: RGBA8 from
/// [`render_demo_frame`], BGRA8 from the GPU readback path
/// ([`RenderFrame::from_raw_parts`]).
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

    /// Raw frame bytes, row-major (`width * height * 4` long) in the
    /// producer's channel order (see the struct docs).
    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Assemble a published frame from externally rendered BGRA8 bytes.
    ///
    /// Crate-visible constructor for the GPU readback path
    /// (`crate::gpu::readback_frame`), which produces tight row-major bytes
    /// that never pass through the CPU rasterizer below.
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

/// Render the demo scene at `angle_radians` into a fixed-format frame.
///
/// Draws a flat-shaded cube rotated `angle_radians` around Y (plus a fixed
/// X tilt so faces stay visible) over a vertical gradient. Deterministic:
/// the same angle always yields the same bytes.
///
/// # Errors
///
/// Returns [`SceneError::InvalidFrameSize`] when `width` or `height` is zero.
pub fn render_demo_frame(
    angle_radians: f32,
    width: u32,
    height: u32,
) -> Result<RenderFrame, SceneError> {
    if width == 0 || height == 0 {
        return Err(SceneError::InvalidFrameSize { width, height });
    }
    Ok(rasterize_cube(angle_radians, width, height))
}

/// Live viewport camera reduced to what the demo oracle consumes.
///
/// Built by [`SceneWorld::render_camera_frame`](crate::SceneWorld::render_camera_frame)
/// from the camera [`Transform`](bevy_transform::prelude::Transform) the #40
/// ops move plus the [`ViewportPivot`](crate::SceneWorld::pivot): distance
/// drives the projection scale, orbit deltas the view angles, the pivot's
/// camera-space position the screen center — so every nav op is
/// pixel-observable in the oracle without warming up the GPU path.
#[derive(Debug, Clone, Copy)]
pub struct OracleCamera {
    /// Camera-to-pivot distance in world units.
    pub distance: f32,
    /// Orbit azimuth delta from the spawn pose, in radians.
    pub yaw_offset: f32,
    /// Orbit elevation delta from the spawn pose, in radians.
    pub pitch_offset: f32,
    /// Pivot position along the camera-right axis, in world units.
    pub pan_right: f32,
    /// Pivot position along the camera-up axis, in world units.
    pub pan_up: f32,
}

impl OracleCamera {
    /// Spawn-pose view: [`render_demo_frame_with_camera`] with this input
    /// reproduces [`render_demo_frame`] byte-for-byte.
    #[must_use]
    pub fn spawn_default() -> Self {
        Self {
            distance: spawn_distance(),
            yaw_offset: 0.0,
            pitch_offset: 0.0,
            pan_right: 0.0,
            pan_up: 0.0,
        }
    }
}

impl Default for OracleCamera {
    fn default() -> Self {
        Self::spawn_default()
    }
}

/// Pixel-space projection resolved from an [`OracleCamera`]: what the
/// rasterizer consumes per frame.
#[derive(Debug, Clone, Copy)]
pub struct OracleProjection {
    /// World-units-to-pixels scale.
    pub scale: f32,
    /// Screen center x in pixels (pan-shifted).
    pub center_x: f32,
    /// Screen center y in pixels (pan-shifted).
    pub center_y: f32,
    /// Extra Y rotation from orbit, in radians.
    pub yaw_offset: f32,
    /// Absolute X tilt: [`TILT_X`] plus the orbit elevation delta, in radians.
    pub tilt: f32,
}

/// Resolve camera inputs to a pixel-space projection at `width` x `height`.
///
/// Pure and total: degenerate distances (zero, negative, non-finite — never
/// produced by the camera ops, which no-op those inputs) clamp to a tiny
/// positive range instead of dividing by zero or emitting NaN pixels.
#[must_use]
pub fn oracle_projection(camera: &OracleCamera, width: u32, height: u32) -> OracleProjection {
    let distance = if camera.distance.is_finite() && camera.distance > 0.0 {
        camera.distance
    } else {
        f32::EPSILON
    };
    // Calibrated so the spawn pose reproduces the legacy fixed scale:
    // `spawn_distance() / distance` is exactly 1.0 there, and `x * 1.0 == x`.
    let scale = f32_from_extent(width.min(height)) * 0.30 * (spawn_distance() / distance);
    OracleProjection {
        scale,
        center_x: f32_from_extent(width) * 0.5 - camera.pan_right * scale,
        center_y: f32_from_extent(height) * 0.5 + camera.pan_up * scale,
        yaw_offset: camera.yaw_offset,
        tilt: TILT_X + camera.pitch_offset,
    }
}

/// Render the demo scene at `angle_radians` through the live `camera`.
///
/// Same cube and gradient as [`render_demo_frame`], but the projection
/// derives from the viewport camera via [`oracle_projection`]: dolly changes
/// the scale, orbit the view angles, pan the screen center. Deterministic:
/// the same angle and camera always yield the same bytes.
///
/// # Errors
///
/// Returns [`SceneError::InvalidFrameSize`] when `width` or `height` is zero.
pub fn render_demo_frame_with_camera(
    angle_radians: f32,
    camera: &OracleCamera,
    width: u32,
    height: u32,
) -> Result<RenderFrame, SceneError> {
    if width == 0 || height == 0 {
        return Err(SceneError::InvalidFrameSize { width, height });
    }
    Ok(rasterize_cube_with(
        angle_radians,
        &oracle_projection(camera, width, height),
        width,
        height,
    ))
}

/// Spawn-pose camera-to-pivot distance, derived from the demo spawn constants
/// so the oracle calibration tracks them (single source of truth).
pub(crate) fn spawn_distance() -> f32 {
    DEMO_CAMERA_DISTANCE.hypot(DEMO_CAMERA_HEIGHT)
}

/// Spawn-pose camera elevation, from the same constants.
pub(crate) fn spawn_elevation() -> f32 {
    (DEMO_CAMERA_HEIGHT / DEMO_CAMERA_DISTANCE).atan()
}

/// Infallible core: `width` and `height` are nonzero by construction
/// (checked by [`render_demo_frame`]).
fn rasterize_cube(angle: f32, width: u32, height: u32) -> RenderFrame {
    // Legacy fixed projection, arithmetic preserved exactly so the spawn
    // pose (and every long-pinned threshold measured against it) never
    // drifts: `angle + 0.0 == angle`, `TILT_X + 0.0 == TILT_X`.
    let projection = OracleProjection {
        scale: f32_from_extent(width.min(height)) * 0.30,
        center_x: f32_from_extent(width) * 0.5,
        center_y: f32_from_extent(height) * 0.5,
        yaw_offset: 0.0,
        tilt: TILT_X,
    };
    rasterize_cube_with(angle, &projection, width, height)
}

/// Infallible core: `width` and `height` are nonzero by construction
/// (checked by [`render_demo_frame`] and [`render_demo_frame_with_camera`]).
fn rasterize_cube_with(
    angle: f32,
    projection: &OracleProjection,
    width: u32,
    height: u32,
) -> RenderFrame {
    let w = width as usize;
    let h = height as usize;
    let mut pixels = vec![0u8; w * h * FRAME_BYTES_PER_PIXEL];
    paint_background(&mut pixels, w, h);

    // Unit cube corners, rotated around Y by the turntable angle plus the
    // orbit yaw, tilted around X by the orbit-aware tilt so top faces stay
    // visible. Shading uses the object angle alone: orbiting the camera
    // changes which faces are visible, not how the fixed light hits them.
    let (sin_y, cos_y) = (angle + projection.yaw_offset).sin_cos();
    let (obj_sin_y, obj_cos_y) = angle.sin_cos();
    let (sin_x, cos_x) = projection.tilt.sin_cos();
    let mut projected = [[0.0f32; 2]; 8];
    let mut depths = [0.0f32; 8];
    for (index, corner) in CUBE_CORNERS.iter().enumerate() {
        let (x0, y0, z0) = (corner[0], corner[1], corner[2]);
        let x1 = cos_y * x0 + sin_y * z0;
        let z1 = -sin_y * x0 + cos_y * z0;
        let y1 = cos_x * y0 - sin_x * z1;
        let z2 = sin_x * y0 + cos_x * z1;
        depths[index] = z2;
        let scale = projection.scale;
        projected[index] = [
            projection.center_x + x1 * scale,
            projection.center_y - y1 * scale,
        ];
    }

    // Painter's algorithm: far faces first.
    let mut faces: [(usize, f32); 6] = core::array::from_fn(|face| {
        let depth = CUBE_QUADS[face]
            .iter()
            .map(|corner| depths[*corner])
            .sum::<f32>()
            / 4.0;
        (face, depth)
    });
    faces.sort_by(|a, b| a.1.total_cmp(&b.1));

    for (face, _) in faces {
        let shade = shade_for_face(face, obj_sin_y, obj_cos_y);
        let quad = CUBE_QUADS[face];
        let corners = [
            projected[quad[0]],
            projected[quad[1]],
            projected[quad[2]],
            projected[quad[3]],
        ];
        fill_triangle(&mut pixels, w, h, corners[0], corners[1], corners[2], shade);
        fill_triangle(&mut pixels, w, h, corners[0], corners[2], corners[3], shade);
    }

    RenderFrame {
        width,
        height,
        pixels,
    }
}

/// Fixed X tilt (radians) so the cube's top faces stay visible.
const TILT_X: f32 = 0.45;

/// Unit cube corners.
const CUBE_CORNERS: [[f32; 3]; 8] = [
    [-1.0, -1.0, -1.0],
    [1.0, -1.0, -1.0],
    [1.0, 1.0, -1.0],
    [-1.0, 1.0, -1.0],
    [-1.0, -1.0, 1.0],
    [1.0, -1.0, 1.0],
    [1.0, 1.0, 1.0],
    [-1.0, 1.0, 1.0],
];

/// Six faces as corner quads (outward winding before rotation).
const CUBE_QUADS: [[usize; 4]; 6] = [
    [0, 1, 2, 3], // back
    [5, 4, 7, 6], // front
    [4, 0, 3, 7], // left
    [1, 5, 6, 2], // right
    [3, 2, 6, 7], // top
    [4, 5, 1, 0], // bottom
];

/// Outward face normals before rotation, matching [`CUBE_QUADS`].
const FACE_NORMALS: [[f32; 3]; 6] = [
    [0.0, 0.0, -1.0],
    [0.0, 0.0, 1.0],
    [-1.0, 0.0, 0.0],
    [1.0, 0.0, 0.0],
    [0.0, 1.0, 0.0],
    [0.0, -1.0, 0.0],
];

/// Key light direction (already normalized).
const LIGHT_DIR: [f32; 3] = [-0.42, 0.75, 0.51];

/// Flat-shade one face: rotate its normal by the turntable angle, then
/// Lambert against the key light over a warm base color.
fn shade_for_face(face: usize, sin_y: f32, cos_y: f32) -> [u8; 3] {
    let normal = FACE_NORMALS[face];
    let rotated = [
        cos_y * normal[0] + sin_y * normal[2],
        normal[1],
        -sin_y * normal[0] + cos_y * normal[2],
    ];
    let luminance =
        (rotated[0] * LIGHT_DIR[0] + rotated[1] * LIGHT_DIR[1] + rotated[2] * LIGHT_DIR[2])
            .clamp(0.0, 1.0);
    let intensity = 0.25 + 0.75 * luminance;
    [
        channel(0.85 * intensity),
        channel(0.55 * intensity),
        channel(0.30 * intensity),
    ]
}

/// Cast a `u32` frame extent to `f32`.
///
/// Exact for every realistic frame: extents below 2^24 convert losslessly
/// and slice 2 fixes them at 512x320.
#[allow(
    clippy::cast_precision_loss,
    reason = "frame extents are fixed small constants, far below 2^24"
)]
pub(crate) fn f32_from_extent(value: u32) -> f32 {
    value as f32
}

/// Cast a sub-2^24 pixel index to `f32`.
///
/// Exact: all frame coordinates are far below 2^24, where `usize`→`f32`
/// is lossless.
#[allow(
    clippy::cast_precision_loss,
    reason = "pixel indices stay far below 2^24, where the cast is exact"
)]
fn f32_from_index(value: usize) -> f32 {
    value as f32
}

/// Cast a clamped non-negative coordinate to a pixel index.
///
/// Callers clamp into `0..2^24` first (`floor().max(0.0)` on frame-bounded
/// geometry), so neither sign nor magnitude is lost.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "inputs are clamped non-negative and far below 2^24"
)]
fn index_from_coord(value: f32) -> usize {
    value as usize
}

/// Scale a 0.0–1.0 channel to a byte.
///
/// The clamp plus round keeps the value inside `0..=255`, so the cast is
/// exact; the pedantic lint cannot see the invariant.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to 0.0..=255.0 and rounded before casting"
)]
fn channel(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}
/// Scale a 0.0–255.0 gradient value to a byte (background path).
///
/// The clamp plus round keeps the value inside `0..=255`, so the cast is
/// exact; the pedantic lint cannot see the invariant.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to 0.0..=255.0 and rounded before casting"
)]
fn background_channel(value: f32) -> u8 {
    value.clamp(0.0, 255.0).round() as u8
}

/// Dark vertical gradient background.
fn paint_background(pixels: &mut [u8], width: usize, height: usize) {
    let height_f = f32_from_index(height);
    for y in 0..height {
        let t = f32_from_index(y) / height_f;
        let top = [13.0, 17.0, 23.0];
        let bottom = [5.0, 6.0, 9.0];
        for x in 0..width {
            let offset = (y * width + x) * FRAME_BYTES_PER_PIXEL;
            for channel_index in 0..3 {
                let value = top[channel_index] + (bottom[channel_index] - top[channel_index]) * t;
                pixels[offset + channel_index] = background_channel(value);
            }
            pixels[offset + 3] = 255;
        }
    }
}

/// Fill one triangle with a solid color via edge functions.
fn fill_triangle(
    pixels: &mut [u8],
    width: usize,
    height: usize,
    a: [f32; 2],
    b: [f32; 2],
    c: [f32; 2],
    color: [u8; 3],
) {
    let width_f = f32_from_index(width);
    let height_f = f32_from_index(height);
    let min_x = index_from_coord(a[0].min(b[0]).min(c[0]).floor().max(0.0));
    let max_x = index_from_coord(a[0].max(b[0]).max(c[0]).ceil().min(width_f));
    let min_y = index_from_coord(a[1].min(b[1]).min(c[1]).floor().max(0.0));
    let max_y = index_from_coord(a[1].max(b[1]).max(c[1]).ceil().min(height_f));
    let area = edge(a, b, c);
    if area == 0.0 {
        return;
    }
    for y in min_y..max_y {
        for x in min_x..max_x {
            let point = [f32_from_index(x) + 0.5, f32_from_index(y) + 0.5];
            let inside = if area > 0.0 {
                edge(a, b, point) >= 0.0 && edge(b, c, point) >= 0.0 && edge(c, a, point) >= 0.0
            } else {
                edge(a, b, point) <= 0.0 && edge(b, c, point) <= 0.0 && edge(c, a, point) <= 0.0
            };
            if inside {
                let offset = (y * width + x) * FRAME_BYTES_PER_PIXEL;
                pixels[offset..offset + 3].copy_from_slice(&color);
                pixels[offset + 3] = 255;
            }
        }
    }
}

/// Signed edge function for points `a` → `b` evaluated at `p`.
fn edge(a: [f32; 2], b: [f32; 2], p: [f32; 2]) -> f32 {
    (p[0] - a[0]) * (b[1] - a[1]) - (p[1] - a[1]) * (b[0] - a[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_extents_are_an_error() {
        assert!(matches!(
            render_demo_frame(0.0, 0, FRAME_HEIGHT),
            Err(SceneError::InvalidFrameSize { width: 0, .. })
        ));
        assert!(matches!(
            render_demo_frame(0.0, FRAME_WIDTH, 0),
            Err(SceneError::InvalidFrameSize { height: 0, .. })
        ));
    }

    #[test]
    fn frame_layout_is_rgba8_row_major() {
        let frame = render_demo_frame(0.0, FRAME_WIDTH, FRAME_HEIGHT).unwrap();
        assert_eq!(frame.width(), FRAME_WIDTH);
        assert_eq!(frame.height(), FRAME_HEIGHT);
        assert_eq!(
            frame.pixels().len(),
            FRAME_WIDTH as usize * FRAME_HEIGHT as usize * FRAME_BYTES_PER_PIXEL
        );
    }

    #[test]
    fn same_angle_rasterizes_identical_bytes() {
        let first = render_demo_frame(0.35, 128, 96).unwrap();
        let second = render_demo_frame(0.35, 128, 96).unwrap();
        assert_eq!(first.pixels(), second.pixels());
    }

    #[test]
    fn angle_step_changes_pixels() {
        let before = render_demo_frame(0.0, 128, 96).unwrap();
        let after = render_demo_frame(0.02, 128, 96).unwrap();
        assert_ne!(before.pixels(), after.pixels());
    }

    #[test]
    fn cube_covers_center_with_opaque_lit_pixels() {
        let frame = render_demo_frame(0.0, FRAME_WIDTH, FRAME_HEIGHT).unwrap();
        let center = (FRAME_HEIGHT as usize / 2 * FRAME_WIDTH as usize + FRAME_WIDTH as usize / 2)
            * FRAME_BYTES_PER_PIXEL;
        let pixel = &frame.pixels()[center..center + FRAME_BYTES_PER_PIXEL];
        assert_eq!(pixel[3], 255);
        // Lit cube face is far brighter than the dark background gradient.
        assert!(pixel[0] > 40 || pixel[1] > 40 || pixel[2] > 40);
    }

    #[test]
    fn spawn_default_reproduces_legacy_bytes() {
        let legacy = render_demo_frame(0.35, 128, 96).unwrap();
        let through_camera =
            render_demo_frame_with_camera(0.35, &OracleCamera::spawn_default(), 128, 96).unwrap();
        assert_eq!(legacy.pixels(), through_camera.pixels());
    }

    #[test]
    fn with_camera_zero_extents_are_an_error() {
        assert!(matches!(
            render_demo_frame_with_camera(0.0, &OracleCamera::spawn_default(), 0, FRAME_HEIGHT),
            Err(SceneError::InvalidFrameSize { width: 0, .. })
        ));
        assert!(matches!(
            render_demo_frame_with_camera(0.0, &OracleCamera::spawn_default(), FRAME_WIDTH, 0),
            Err(SceneError::InvalidFrameSize { height: 0, .. })
        ));
    }

    #[test]
    fn halving_distance_doubles_projection_scale() {
        let near = oracle_projection(
            &OracleCamera {
                distance: spawn_distance() * 0.5,
                ..OracleCamera::spawn_default()
            },
            128,
            96,
        );
        let far = oracle_projection(&OracleCamera::spawn_default(), 128, 96);
        assert!(
            (near.scale - far.scale * 2.0).abs() < 1e-3,
            "a 2x dolly-in must double the scale, went {} -> {}",
            far.scale,
            near.scale
        );
    }

    #[test]
    fn degenerate_distances_clamp_to_finite_scale() {
        for distance in [0.0, -2.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let projection = oracle_projection(
                &OracleCamera {
                    distance,
                    ..OracleCamera::spawn_default()
                },
                128,
                96,
            );
            assert!(
                projection.scale.is_finite() && projection.scale > 0.0,
                "distance {distance} must clamp to a finite positive scale"
            );
            assert!(
                projection.center_x.is_finite() && projection.center_y.is_finite(),
                "distance {distance} must keep a finite center"
            );
        }
    }

    #[test]
    fn distance_change_moves_oracle_pixels() {
        let spawn = OracleCamera::spawn_default();
        let before = render_demo_frame_with_camera(0.35, &spawn, 128, 96).unwrap();
        let after = render_demo_frame_with_camera(
            0.35,
            &OracleCamera {
                distance: spawn.distance * 0.5,
                ..spawn
            },
            128,
            96,
        )
        .unwrap();
        assert_ne!(before.pixels(), after.pixels());
    }

    #[test]
    fn yaw_offset_moves_oracle_pixels() {
        let spawn = OracleCamera::spawn_default();
        let before = render_demo_frame_with_camera(0.35, &spawn, 128, 96).unwrap();
        let after = render_demo_frame_with_camera(
            0.35,
            &OracleCamera {
                yaw_offset: 0.6,
                ..spawn
            },
            128,
            96,
        )
        .unwrap();
        assert_ne!(before.pixels(), after.pixels());
    }

    #[test]
    fn pitch_offset_moves_oracle_pixels() {
        let spawn = OracleCamera::spawn_default();
        let before = render_demo_frame_with_camera(0.35, &spawn, 128, 96).unwrap();
        let after = render_demo_frame_with_camera(
            0.35,
            &OracleCamera {
                pitch_offset: 0.2,
                ..spawn
            },
            128,
            96,
        )
        .unwrap();
        assert_ne!(before.pixels(), after.pixels());
    }

    #[test]
    fn pan_shift_moves_oracle_pixels() {
        let spawn = OracleCamera::spawn_default();
        let before = render_demo_frame_with_camera(0.35, &spawn, 128, 96).unwrap();
        let after = render_demo_frame_with_camera(
            0.35,
            &OracleCamera {
                pan_right: 0.75,
                pan_up: -0.5,
                ..spawn
            },
            128,
            96,
        )
        .unwrap();
        assert_ne!(before.pixels(), after.pixels());
    }
}
