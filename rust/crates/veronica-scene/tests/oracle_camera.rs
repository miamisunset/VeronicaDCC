//! Oracle honors the viewport camera: nav ops move oracle pixels.
//!
//! The CPU rasterizer (`render_demo_frame`) is the deterministic test oracle
//! behind the frame-publish seam. Before issue #44 it projected with a fixed
//! scale from the turntable angle only, so no nav op could ever move its
//! pixels. [`SceneWorld::render_camera_frame`] renders the same cube through
//! the live viewport camera instead: distance drives the projection scale,
//! orbit the view angles, pan the screen center — so every #40 camera op is
//! pixel-observable without warming up the GPU path.

use veronica_scene::{SceneError, SceneWorld};

/// Pixels brighter than a lit-face threshold (sum over R+G+B).
///
/// The oracle is RGBA8: the lit cube face reads warm (~195,126,69, sum ~390
/// at full intensity) while the dark gradient background sums to ~20-53, so
/// 150 separates lit geometry from background with wide margin.
fn lit_pixel_count(pixels: &[u8]) -> usize {
    pixels
        .chunks_exact(4)
        .filter(|pixel| u16::from(pixel[0]) + u16::from(pixel[1]) + u16::from(pixel[2]) > 150)
        .count()
}

/// Dolly-in grows the oracle cube: halving the distance quadruples the lit
/// area, so the lit count must grow and the bytes must differ.
#[test]
fn dolly_in_grows_the_oracle_cube() {
    let mut world = SceneWorld::new_headless();
    let _ = world.spawn_demo_scene();
    let before = world
        .render_camera_frame()
        .expect("oracle renders from the live camera");
    let before_lit = lit_pixel_count(before.pixels()).max(1);
    world
        .dolly_camera(std::f32::consts::LN_2, (0.0, 0.0))
        .expect("dolly works");
    let after = world
        .render_camera_frame()
        .expect("oracle stays live after dolly");
    assert_ne!(
        before.pixels(),
        after.pixels(),
        "halving the distance must move oracle pixels"
    );
    let after_lit = lit_pixel_count(after.pixels());
    assert!(
        after_lit > before_lit,
        "a 2x zoom must grow the lit area, went {before_lit} -> {after_lit} lit pixels"
    );
}

/// Frame-all refits the oracle image: fitting moves the camera nearer, so
/// the lit cube covers more pixels and the bytes differ.
#[test]
fn frame_all_refits_the_oracle_image() {
    let mut world = SceneWorld::new_headless();
    let _ = world.spawn_demo_scene();
    let before = world
        .render_camera_frame()
        .expect("oracle renders from the live camera");
    let before_lit = lit_pixel_count(before.pixels()).max(1);
    world.frame_all().expect("frame-all works");
    let reframed = world
        .render_camera_frame()
        .expect("oracle stays live after frame-all");
    assert_ne!(
        before.pixels(),
        reframed.pixels(),
        "the frame-all refit must move oracle pixels"
    );
    let reframed_lit = lit_pixel_count(reframed.pixels());
    assert!(
        reframed_lit > before_lit,
        "fitting must grow the lit area, went {before_lit} -> {reframed_lit} lit pixels"
    );
}

/// Orbit swings the view angles, so the projected cube must move.
#[test]
fn orbit_moves_the_oracle_cube() {
    let mut world = SceneWorld::new_headless();
    let _ = world.spawn_demo_scene();
    let before = world
        .render_camera_frame()
        .expect("oracle renders from the live camera");
    world.orbit_camera(120.0, 40.0).expect("orbit works");
    let after = world
        .render_camera_frame()
        .expect("oracle stays live after orbit");
    assert_ne!(
        before.pixels(),
        after.pixels(),
        "a 120x40px orbit must move oracle pixels"
    );
}

/// Pan shifts the screen center, so the projected cube must move.
#[test]
fn pan_moves_the_oracle_cube() {
    let mut world = SceneWorld::new_headless();
    let _ = world.spawn_demo_scene();
    let before = world
        .render_camera_frame()
        .expect("oracle renders from the live camera");
    world.pan_camera(100.0, 0.0).expect("pan works");
    let after = world
        .render_camera_frame()
        .expect("oracle stays live after pan");
    assert_ne!(
        before.pixels(),
        after.pixels(),
        "a 100px pan must move oracle pixels"
    );
}

/// No camera, no oracle: without a demo scene the render errors with the
/// existing missing-camera kind (matched on kind, never on message text).
#[test]
fn oracle_without_camera_is_no_viewport_camera() {
    let mut world = SceneWorld::new_headless();
    assert!(
        matches!(
            world.render_camera_frame(),
            Err(SceneError::NoViewportCamera)
        ),
        "the oracle needs the live camera"
    );
}
