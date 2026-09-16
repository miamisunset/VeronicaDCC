//! GPU slice: ticks publish GPU-rendered frames behind the same seam.
//!
//! A valid published frame has the fixed nonzero extents and a full BGRA8
//! payload; consecutive ticks publish observably different bytes because
//! the Rust-side turntable advanced, not because anyone re-rendered. The CPU
//! rasterizer (`render_demo_frame`) is kept only as the deterministic test
//! oracle: GPU frames must be non-uniform and visibly beyond flat
//! rasterization.
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
/// CPU oracle's flat rasterization at the same turntable angle.
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
    let oracle = render_demo_frame(world.spin_angle(), FRAME_WIDTH, FRAME_HEIGHT).unwrap();
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
    // color, whatever the turntable angle.
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

/// Consecutive ticks publish different pixels: the turntable angle advanced
/// through the GPU path.
#[test]
fn consecutive_ticks_publish_different_pixels() {
    let mut world = SceneWorld::new_headless();
    let _ = world.spawn_demo_scene();
    let before = render_when_ready(&mut world);
    world.update();
    let after = world
        .render_frame()
        .expect("GPU readback stays live after warm-up")
        .pixels()
        .to_vec();
    assert_ne!(before, after);
}

/// The pixel source tracks the world tick counter, not wall time.
#[test]
fn frame_angle_follows_tick_count() {
    let mut world = SceneWorld::new_headless();
    let _ = world.spawn_demo_scene();
    assert_eq!(world.tick_count(), 0);
    world.update();
    assert!(world.spin_angle() > 0.0);
}

/// Re-targeting mid-life propagates to the published frame: extents follow,
/// the turntable stays alive across the switch, and the lit cube still reads
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

    // The turntable survived the switch: consecutive ticks still differ.
    world.update();
    let after = world
        .render_frame()
        .expect("readback stays live after resize")
        .pixels()
        .to_vec();
    assert_ne!(frame.pixels(), after.as_slice());

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
    assert_eq!(world.spin_angle().to_bits(), 0.0f32.to_bits());
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
