//! Slice-2 oracle: ticks publish valid frames whose pixels follow Scene state.
//!
//! A valid published frame has the fixed nonzero extents and a full RGBA8
//! payload; consecutive ticks publish observably different bytes because
//! the Rust-side turntable advanced, not because anyone re-rendered.

use veronica_scene::{FRAME_HEIGHT, FRAME_WIDTH, SceneWorld};

/// Fixed extents are nonzero with a full RGBA8 payload after a tick.
#[test]
fn published_frame_is_valid_after_tick() {
    let mut world = SceneWorld::new_headless();
    let _ = world.spawn_demo_scene();
    world.update();
    let frame = world.render_frame();
    assert_eq!(frame.width(), FRAME_WIDTH);
    assert_eq!(frame.height(), FRAME_HEIGHT);
    assert!(frame.width() > 0 && frame.height() > 0);
    assert_eq!(
        frame.pixels().len(),
        FRAME_WIDTH as usize * FRAME_HEIGHT as usize * 4
    );
}

/// Consecutive ticks publish different pixels: the turntable angle advanced.
#[test]
fn consecutive_ticks_publish_different_pixels() {
    let mut world = SceneWorld::new_headless();
    let _ = world.spawn_demo_scene();
    world.update();
    let before = world.render_frame().pixels().to_vec();
    world.update();
    let after = world.render_frame().pixels().to_vec();
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
    let before = world.render_frame().pixels().to_vec();
    world.update();
    let after = world.render_frame().pixels().to_vec();
    assert_eq!(before, after);
}
