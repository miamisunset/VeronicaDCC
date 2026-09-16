//! GPU slice: ticks publish GPU-rendered frames behind the same seam.
//!
//! A valid published frame has the fixed nonzero extents and a full RGBA8
//! payload; consecutive ticks publish observably different bytes because
//! the Rust-side turntable advanced, not because anyone re-rendered. The CPU
//! rasterizer (`render_demo_frame`) is kept only as the deterministic test
//! oracle: GPU frames must be non-uniform and visibly beyond flat
//! rasterization.
//!
//! Pipeline warm-up: early frames may be clear-only while shaders compile on
//! first use, so tests pre-roll bounded ticks (poll-until-non-uniform with a
//! hard cap) before asserting pixel properties.

use veronica_scene::{FRAME_HEIGHT, FRAME_WIDTH, SceneWorld, render_demo_frame};

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

/// Fixed extents are nonzero with a full RGBA8 payload after a tick, and the
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
