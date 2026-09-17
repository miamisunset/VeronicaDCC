//! GPU slice: ticks publish GPU-rendered frames behind the same seam.
//!
//! A valid published frame has the fixed nonzero extents and a full BGRA8
//! payload. The scene is static (no auto-spin since #46), so consecutive
//! ticks publish identical bytes — pixels follow ECS state, and only nav
//! ops move them. The CPU rasterizer (`render_demo_frame`) is kept only as
//! the deterministic test oracle: GPU frames must be non-uniform and
//! visibly beyond flat rasterization.
//!
//! Pipeline warm-up: early frames may be clear-only while shaders compile on
//! first use, so tests pre-roll bounded ticks (poll-until-non-uniform with a
//! hard cap) before asserting pixel properties.

use veronica_scene::{
    FRAME_HEIGHT, FRAME_WIDTH, MAX_VIEWPORT_EDGE, SceneError, SceneWorld, render_demo_frame,
};

/// Ticks before a test gives up waiting for the lit cube to appear.
///
/// Shader compilation happens on first use; every tick re-renders, so the
/// first non-uniform frame proves the GPU path is live.
const MAX_WARMUP_TICKS: u32 = 240;

/// True when at least one pixel differs from the first: a clear-only frame is
/// perfectly uniform, so this proves scene content reached the pixels.
fn is_non_uniform(pixels: &[u8]) -> bool {
    let Some(first) = pixels.chunks_exact(4).next() else {
        return false;
    };
    pixels.chunks_exact(4).any(|pixel| pixel != first)
}

/// Tick until the GPU publishes a non-uniform frame, then return it.
///
/// Fails the test when no non-uniform frame appears within
/// [`MAX_WARMUP_TICKS`] ticks; that means the GPU path never produced scene
/// content.
fn render_when_ready(world: &mut SceneWorld) -> Vec<u8> {
    let mut last = Vec::new();
    let mut ready = false;
    for _ in 0..MAX_WARMUP_TICKS {
        world.update();
        match world.render_frame() {
            Ok(frame) if is_non_uniform(frame.pixels()) => {
                last = frame.pixels().to_vec();
                ready = true;
                break;
            }
            Ok(frame) => last = frame.pixels().to_vec(),
            Err(_) => {}
        }
    }
    assert!(
        ready,
        "GPU frame never showed the lit cube after {MAX_WARMUP_TICKS} ticks"
    );
    last
}

/// Fixed extents are nonzero with a full BGRA8 payload after a tick, and the
/// pixels are GPU-rendered scene content: non-uniform and different from the
/// CPU oracle's flat rasterization at the same cube angle.
#[test]
fn published_frame_is_valid_after_tick() {
    let mut world = SceneWorld::new_headless();
    let _ = world.spawn_demo_scene();
    let pixels = render_when_ready(&mut world);
    assert_eq!(
        pixels.len(),
        FRAME_WIDTH as usize * FRAME_HEIGHT as usize * 4
    );
    assert!(is_non_uniform(&pixels));
    let oracle = render_demo_frame(world.cube_angle_y(), FRAME_WIDTH, FRAME_HEIGHT).unwrap();
    assert_ne!(
        pixels,
        oracle.pixels(),
        "GPU frame must be visibly beyond the flat CPU rasterization"
    );
}

/// The lit cube reads achromatic: white material under a white key light
/// through `AgX` must be near-gray, never the magenta canary.
/// Regression pin for #27 — without the `tonemapping_luts` cargo feature the
/// `AgX` LUT resolves to a 1x1 magenta placeholder (`lut_placeholder`) and the
/// cube publishes `(255, 0, 255)`.
#[test]
fn lit_cube_is_achromatic_not_magenta() {
    let mut world = SceneWorld::new_headless();
    let _ = world.spawn_demo_scene();
    let pixels = render_when_ready(&mut world);
    // Brightest pixel: the lit cube face is far brighter than the dark clear
    // color, whatever the cube angle.
    let brightest = pixels
        .chunks_exact(4)
        .max_by_key(|pixel| u16::from(pixel[0]) + u16::from(pixel[1]) + u16::from(pixel[2]))
        .expect("frame holds pixels");
    let [b, g, r, a] = [brightest[0], brightest[1], brightest[2], brightest[3]];
    assert_eq!(a, 255, "opaque PBR output, got {brightest:?}");
    assert!(
        r > 150 && g > 150 && b > 150,
        "lit face must be bright, got [{b}, {g}, {r}]"
    );
    for (first, second) in [(r, g), (g, b), (r, b)] {
        assert!(
            first.abs_diff(second) <= 12,
            "white cube must read achromatic, got [{b}, {g}, {r}]"
        );
    }
}

/// Consecutive ticks publish identical pixels: the static scene holds still,
/// so pixels follow ECS state and only nav ops move them.
#[test]
fn consecutive_ticks_publish_identical_pixels() {
    let mut world = SceneWorld::new_headless();
    let _ = world.spawn_demo_scene();
    let before = render_when_ready(&mut world);
    world.update();
    let after = world
        .render_frame()
        .expect("GPU readback stays live after warm-up")
        .pixels()
        .to_vec();
    assert_eq!(before, after);
}

/// The pixel source tracks the world tick counter, not wall time: ticks
/// advance while the cube angle holds at its spawn orientation.
#[test]
fn cube_angle_holds_still_while_ticks_advance() {
    let mut world = SceneWorld::new_headless();
    let _ = world.spawn_demo_scene();
    assert_eq!(world.tick_count(), 0);
    world.update();
    assert_eq!(world.cube_angle_y().to_bits(), 0.0f32.to_bits());
    assert_eq!(world.tick_count(), 1);
}

/// Re-targeting mid-life propagates to the published frame: extents follow
/// while the static scene holds still, and the lit cube still reads
/// achromatic at the new size. Zero and over-cap requests error.
#[test]
fn viewport_resize_propagates_mid_life() {
    use veronica_scene::FRAME_BYTES_PER_PIXEL;
    let mut world = SceneWorld::new_headless();
    let _ = world.spawn_demo_scene();
    let _ = render_when_ready(&mut world);
    assert_eq!(world.viewport_size(), (FRAME_WIDTH, FRAME_HEIGHT));

    // Invalid sizes error before touching GPU state.
    for (width, height) in [(0, 200), (320, 0), (MAX_VIEWPORT_EDGE + 1, 100)] {
        assert!(
            matches!(
                world.set_viewport_size(width, height),
                Err(SceneError::InvalidViewportSize { .. })
            ),
            "extents {width}x{height} must be rejected"
        );
    }
    assert_eq!(world.viewport_size(), (FRAME_WIDTH, FRAME_HEIGHT));

    // Re-target and tick: the published frame carries the new extents.
    world.set_viewport_size(384, 256).unwrap();
    assert_eq!(world.viewport_size(), (384, 256));
    world.update();
    let frame = world
        .render_frame()
        .expect("readback stays live across resize");
    assert_eq!((frame.width(), frame.height()), (384, 256));
    assert_eq!(
        frame.pixels().len(),
        384_usize * 256 * FRAME_BYTES_PER_PIXEL
    );

    // Stillness survives the switch: after one settle tick past the
    // re-target (staging rebuild), the static scene publishes identical
    // bytes at the new extents.
    world.update();
    let _ = world.render_frame().expect("readback settles after resize");
    world.update();
    let after = world
        .render_frame()
        .expect("readback stays live after resize")
        .pixels()
        .to_vec();
    world.update();
    let steady_after = world
        .render_frame()
        .expect("readback stays live")
        .pixels()
        .to_vec();
    assert_eq!(after.as_slice(), steady_after.as_slice());

    // Brightest-pixel achromatic pin, re-checked at the new size.
    let brightest = after
        .chunks_exact(4)
        .max_by_key(|pixel| u16::from(pixel[0]) + u16::from(pixel[1]) + u16::from(pixel[2]))
        .expect("frame holds pixels");
    let [blue, green, red, alpha] = [brightest[0], brightest[1], brightest[2], brightest[3]];
    assert_eq!(alpha, 255, "opaque PBR output, got {brightest:?}");
    assert!(
        red > 150 && green > 150 && blue > 150,
        "lit face must be bright, got [{blue}, {green}, {red}]"
    );
    for (first, second) in [(red, green), (green, blue), (red, blue)] {
        assert!(
            first.abs_diff(second) <= 12,
            "white cube must read achromatic, got [{blue}, {green}, {red}]"
        );
    }

    // Idempotent no-op: re-requesting the live size keeps publishing it.
    world.set_viewport_size(384, 256).unwrap();
    world.update();
    let steady = world
        .render_frame()
        .expect("idempotent re-target stays live");
    assert_eq!((steady.width(), steady.height()), (384, 256));
}

/// No cube, no motion: with no `DemoCube` alive the published angle is zero
/// and consecutive ticks publish identical frames. Pixels follow ECS state.
#[test]
fn frames_hold_still_without_a_cube() {
    let mut world = SceneWorld::new_headless();
    world.update();
    // Exact bit comparison: the cubeless path returns the `0.0` constant
    // with no float arithmetic in between.
    assert_eq!(world.cube_angle_y().to_bits(), 0.0f32.to_bits());
    let before = world
        .render_frame()
        .expect("readback works with no demo scene")
        .pixels()
        .to_vec();
    world.update();
    let after = world
        .render_frame()
        .expect("readback works with no demo scene")
        .pixels()
        .to_vec();
    assert_eq!(before, after);
}

/// Frame-all visibly reframes through the readback seam: fitting moves the
/// camera nearer, so the lit cube covers substantially more pixels. The
/// scene is static, so the before/after pair isolates the refit exactly —
/// the 4.7→2.6 distance fit grows the lit area ~3×.
#[test]
fn frame_all_reframes_the_published_image() {
    /// Pixels brighter than a lit face threshold (sum over B+G+R).
    fn lit_pixel_count(pixels: &[u8]) -> usize {
        pixels
            .chunks_exact(4)
            .filter(|pixel| u16::from(pixel[0]) + u16::from(pixel[1]) + u16::from(pixel[2]) > 450)
            .count()
    }

    let mut world = SceneWorld::new_headless();
    let _ = world.spawn_demo_scene();
    let before = render_when_ready(&mut world);
    world.frame_all().expect("frame-all works");
    world.update();
    let reframed = world
        .render_frame()
        .expect("readback stays live after frame-all");
    assert_eq!(
        (reframed.width(), reframed.height()),
        (FRAME_WIDTH, FRAME_HEIGHT)
    );
    let before_lit = lit_pixel_count(&before).max(1);
    let reframed_lit = lit_pixel_count(reframed.pixels());
    assert!(
        reframed_lit >= before_lit + before_lit / 2,
        "fitting must grow the lit area ~3x, went {before_lit} -> {reframed_lit} lit pixels"
    );
}
