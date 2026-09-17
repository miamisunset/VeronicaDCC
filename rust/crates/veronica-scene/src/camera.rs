//! Rust-owned viewport camera: turntable orbit, pan, cursor-pivoted dolly,
//! snap frame-all.
//!
//! The camera is Scene state like everything else, so navigation mutates it
//! here and Swift only sends coarse op deltas over FFI (see the `vrn_viewport_*`
//! entry points). Gesture-to-op mapping lives in Swift; the pivot, the math,
//! and the clamps live here. Orbit is a Houdini-style turntable around the
//! [`ViewportPivot`]: locked Y-up, elevation clamped before the poles, dolly
//! exponential toward the cursor ray.
//!
//! The pivot sits at the scene-bounds center at spawn and after
//! [`SceneWorld::frame_all`] — never per-frame, so panning never fights a
//! recenter and recooks never move the view. Pan translates the camera and
//! the pivot rigidly (Houdini parity); orbit and center-cursor dolly leave
//! the pivot fixed.

use bevy_camera::Projection;
use bevy_ecs::prelude::*;
use bevy_math::bounding::Aabb3d;
use bevy_math::prelude::{Mat4, Vec3};
use bevy_mesh::{Mesh, Mesh3d};
use bevy_transform::prelude::Transform;

use super::{DemoCamera, SceneError, SceneWorld};
use crate::gpu;
use crate::render::{
    OracleCamera, RenderFrame, f32_from_extent, render_demo_frame_with_camera, spawn_elevation,
};

/// Turntable speed: radians of azimuth/elevation per pixel of drag.
const ORBIT_SPEED_RAD_PER_PX: f32 = 0.005;

/// Pan speed: fraction of the camera-to-pivot distance per pixel of drag, so
/// panning feels constant at any distance.
const PAN_SPEED_PER_PX: f32 = 0.0016;

/// Orbit elevation clamp (radians): 89.9 degrees, just shy of the poles where
/// `look_at` with a Y-up degenerates.
const MAX_ELEVATION_RAD: f32 = 89.9_f32.to_radians();

/// Dolly distance clamp, relative to the live scene-bounds diagonal.
const MIN_DISTANCE_SCALE: f32 = 0.05;
/// Dolly distance clamp, relative to the live scene-bounds diagonal.
const MAX_DISTANCE_SCALE: f32 = 50.0;

/// Frame-all margin: fitted distance is scaled up so the bounds sit inside
/// the frame with breathing room instead of touching its edges.
const FRAME_MARGIN: f32 = 1.15;

/// Orbit interest point. Resource (not a component) because exactly one
/// viewport exists and every camera op reads it.
#[derive(Debug, Clone, Copy, Resource)]
pub(super) struct ViewportPivot(pub Vec3);

/// Rotate `offset` (camera position relative to the pivot) by a drag delta.
///
/// Pure turntable math, Bevy-free so tests pin it without a world: positive
/// `dx_px` swings the camera toward +X, positive `dy_px` tilts it up.
#[must_use]
pub(crate) fn orbit_offset(offset: Vec3, horizontal_px: f32, vertical_px: f32) -> Vec3 {
    let radius = offset.length().max(f32::EPSILON);
    let mut azimuth = offset.x.atan2(offset.z);
    let mut elevation = (offset.y / radius).clamp(-1.0, 1.0).asin();
    azimuth += horizontal_px * ORBIT_SPEED_RAD_PER_PX;
    elevation = (elevation + vertical_px * ORBIT_SPEED_RAD_PER_PX)
        .clamp(-MAX_ELEVATION_RAD, MAX_ELEVATION_RAD);
    let (sin_el, cos_el) = elevation.sin_cos();
    Vec3::new(
        radius * cos_el * azimuth.sin(),
        radius * sin_el,
        radius * cos_el * azimuth.cos(),
    )
}

/// Multiplicative distance change for a logarithmic dolly factor:
/// positive `log_factor` zooms in.
#[must_use]
pub(crate) fn dolly_scale(log_factor: f32) -> f32 {
    (-log_factor).exp()
}

/// Clamp `distance` to the scene-scale range around `bounds_diagonal`.
/// Falls back to the current distance when there is no geometry to scale
/// against (empty scene), so dolly stays a no-op rather than degenerating.
#[must_use]
pub(crate) fn clamp_distance(distance: f32, bounds_diagonal: f32) -> f32 {
    if bounds_diagonal > f32::EPSILON {
        distance.clamp(
            MIN_DISTANCE_SCALE * bounds_diagonal,
            MAX_DISTANCE_SCALE * bounds_diagonal,
        )
    } else {
        distance.max(f32::EPSILON)
    }
}

/// Distance that fits `radius` inside a perspective frustum with vertical
/// field of view `fov_y` and `aspect` (width over height), with margin.
#[must_use]
pub(crate) fn fit_distance(radius: f32, fov_y: f32, aspect: f32) -> f32 {
    let half_vertical = fov_y * 0.5;
    let half_horizontal = (half_vertical.tan() * aspect).atan();
    let half_min = half_vertical.min(half_horizontal);
    radius.max(1e-4) / half_min.sin() * FRAME_MARGIN
}

/// Elevation of `offset` above the pivot plane, in radians.
///
/// Degenerate offsets (camera sitting on the pivot — the orbit op no-ops
/// those instead of producing them) read as level rather than NaN.
#[must_use]
pub(crate) fn elevation_of(offset: Vec3, distance: f32) -> f32 {
    (offset.y / distance.max(f32::EPSILON))
        .clamp(-1.0, 1.0)
        .asin()
}

impl SceneWorld {
    /// Axis-aligned bounds of all meshed entities in world space, or `None`
    /// when no mesh contributes a vertex.
    ///
    /// Folds the position attribute through each entity's matrix by hand:
    /// Bevy 0.19 has no mesh-level AABB helper, and the fold keeps the full
    /// transform (rotation from the turntable included) instead of a
    /// center-plus-radius approximation.
    #[must_use]
    pub fn scene_bounds(&mut self) -> Option<Aabb3d> {
        let world = self.app.world_mut();
        let mut query = world.query::<(&Mesh3d, &Transform)>();
        let items: Vec<(_, Mat4)> = query
            .iter(world)
            .map(|(mesh, transform)| (mesh.0.clone(), transform.to_matrix()))
            .collect();
        let meshes = world.resource::<bevy_asset::Assets<Mesh>>();
        let mut extents: Option<(Vec3, Vec3)> = None;
        for (handle, matrix) in items {
            let Some(mesh) = meshes.get(&handle) else {
                continue;
            };
            let Some(bevy_mesh::VertexAttributeValues::Float32x3(positions)) =
                mesh.attribute(Mesh::ATTRIBUTE_POSITION)
            else {
                continue;
            };
            for position in positions {
                let world_pos = matrix.transform_point3(Vec3::from_array(*position));
                extents = Some(match extents {
                    Some((min, max)) => (min.min(world_pos), max.max(world_pos)),
                    None => (world_pos, world_pos),
                });
            }
        }
        extents.map(|(min, max)| Aabb3d::from_min_max(min, max))
    }

    /// Live pivot point. `Vec3::ZERO` before the demo scene spawns.
    #[must_use]
    pub fn pivot(&self) -> Vec3 {
        self.app
            .world()
            .get_resource::<ViewportPivot>()
            .map_or(Vec3::ZERO, |pivot| pivot.0)
    }

    /// Demo-camera translation in world units, or `None` when the camera is
    /// gone. The FFI navigate tests observe motion through this; future
    /// Swift readouts (framing overlays, cursor mapping) can reuse it.
    #[must_use]
    pub fn camera_translation(&mut self) -> Option<[f32; 3]> {
        let world = self.app.world_mut();
        let mut cameras = world.query_filtered::<&Transform, With<DemoCamera>>();
        cameras
            .iter(world)
            .next()
            .map(|transform| transform.translation.to_array())
    }

    /// Camera entity's [`Transform`].
    ///
    /// # Errors
    ///
    /// Returns [`SceneError::NoViewportCamera`] when the demo camera is gone.
    fn viewport_camera(&mut self) -> Result<Entity, SceneError> {
        let world = self.app.world_mut();
        let mut cameras = world.query_filtered::<Entity, With<DemoCamera>>();
        cameras
            .iter(world)
            .next()
            .ok_or(SceneError::NoViewportCamera)
    }

    /// Orbit the camera around the pivot by a drag delta in pixels.
    ///
    /// Turntable: azimuth/elevation deltas, Y-up locked, elevation clamped
    /// to ±89.9 degrees. Non-finite deltas are a no-op, never NaN poison.
    ///
    /// # Errors
    ///
    /// Returns [`SceneError::NoViewportCamera`] when the demo camera is gone.
    pub fn orbit_camera(&mut self, horizontal_px: f32, vertical_px: f32) -> Result<(), SceneError> {
        if !horizontal_px.is_finite() || !vertical_px.is_finite() {
            return Ok(());
        }
        let pivot = self.pivot();
        let camera = self.viewport_camera()?;
        let world = self.app.world_mut();
        let mut transform = world
            .get_mut::<Transform>(camera)
            .ok_or(SceneError::NoViewportCamera)?;
        let offset = transform.translation - pivot;
        if offset.length() <= f32::EPSILON {
            return Ok(());
        }
        transform.translation = pivot + orbit_offset(offset, horizontal_px, vertical_px);
        transform.look_at(pivot, Vec3::Y);
        Ok(())
    }

    /// Pan the camera and the pivot rigidly by a drag delta in pixels.
    ///
    /// Positive `dx_px` moves the view left (the scene follows the cursor),
    /// positive `dy_px` moves it up; the step scales with pivot distance so
    /// panning feels constant at any range. Non-finite deltas are a no-op.
    ///
    /// # Errors
    ///
    /// Returns [`SceneError::NoViewportCamera`] when the demo camera is gone.
    pub fn pan_camera(&mut self, horizontal_px: f32, vertical_px: f32) -> Result<(), SceneError> {
        if !horizontal_px.is_finite() || !vertical_px.is_finite() {
            return Ok(());
        }
        let pivot = self.pivot();
        let camera = self.viewport_camera()?;
        let world = self.app.world_mut();
        let mut transform = world
            .get_mut::<Transform>(camera)
            .ok_or(SceneError::NoViewportCamera)?;
        let offset = transform.translation - pivot;
        let step = offset.length() * PAN_SPEED_PER_PX;
        let right = transform.rotation * Vec3::X;
        let up = transform.rotation * Vec3::Y;
        let delta = right * (-horizontal_px * step) + up * (vertical_px * step);
        transform.translation += delta;
        world.resource_mut::<ViewportPivot>().0 += delta;
        Ok(())
    }

    /// Dolly toward the cursor by a logarithmic factor at `cursor_ndc`.
    ///
    /// Positive `log_factor` zooms in: distance scales by `exp(-log_factor)`
    /// and clamps to 0.05–50× the scene-bounds diagonal. The pivot slides
    /// toward the cursor ray's pivot-depth point proportionally, so the
    /// world point under the cursor stays put (a centered cursor leaves the
    /// pivot exactly fixed). Non-finite inputs are a no-op.
    ///
    /// # Errors
    ///
    /// Returns [`SceneError::NoViewportCamera`] when the demo camera is gone.
    pub fn dolly_camera(
        &mut self,
        log_factor: f32,
        cursor_ndc: (f32, f32),
    ) -> Result<(), SceneError> {
        if !log_factor.is_finite() || !cursor_ndc.0.is_finite() || !cursor_ndc.1.is_finite() {
            return Ok(());
        }
        let pivot = self.pivot();
        let camera = self.viewport_camera()?;
        // Snapshot everything read-only first: the `Mut<Transform>` guard
        // below cannot coexist with any other world borrow.
        let world = self.app.world();
        let eye = world
            .get::<Transform>(camera)
            .ok_or(SceneError::NoViewportCamera)?
            .translation;
        let fov = world
            .get::<Projection>(camera)
            .and_then(|projection| match projection {
                Projection::Perspective(perspective) => Some(perspective.fov),
                _ => None,
            })
            .unwrap_or(std::f32::consts::FRAC_PI_4);
        let viewport = world.resource::<gpu::GpuViewportSize>();
        let aspect = f32_from_extent(viewport.width) / f32_from_extent(viewport.height.max(1));
        let offset = eye - pivot;
        let distance = offset.length();
        if distance <= f32::EPSILON {
            return Ok(());
        }
        let diagonal = self
            .scene_bounds()
            .map_or(distance, |bounds| (bounds.max - bounds.min).length());
        let new_distance = clamp_distance(distance * dolly_scale(log_factor), diagonal);
        let to_eye = offset / distance;
        let to_pivot = -to_eye;
        let right = to_pivot.cross(Vec3::Y).normalize_or(Vec3::X);
        let up = right.cross(to_pivot);
        let tan_half = (fov * 0.5).tan();
        let anchor = eye
            + to_pivot * distance
            + (right * (cursor_ndc.0 * tan_half * aspect) + up * (cursor_ndc.1 * tan_half))
                * distance;
        let shifted_pivot = pivot + (anchor - pivot) * (1.0 - new_distance / distance);
        let world = self.app.world_mut();
        let mut transform = world
            .get_mut::<Transform>(camera)
            .ok_or(SceneError::NoViewportCamera)?;
        transform.translation = shifted_pivot + to_eye * new_distance;
        transform.look_at(shifted_pivot, Vec3::Y);
        world.resource_mut::<ViewportPivot>().0 = shifted_pivot;
        Ok(())
    }

    /// Frame the whole scene: pivot to the bounds center, distance to fit.
    ///
    /// Snap, not animated; preserves the current view direction. A scene
    /// with no meshed entities is a no-op (`Ok`, nothing to frame).
    ///
    /// # Errors
    ///
    /// Returns [`SceneError::NoViewportCamera`] when the demo camera is gone.
    pub fn frame_all(&mut self) -> Result<(), SceneError> {
        let camera = self.viewport_camera()?;
        let Some(bounds) = self.scene_bounds() else {
            return Ok(());
        };
        let center = Vec3::from((bounds.min + bounds.max) * 0.5);
        let radius = (bounds.max - bounds.min).length() * 0.5;
        let world = self.app.world_mut();
        let fov = world
            .get::<Projection>(camera)
            .and_then(|projection| match projection {
                Projection::Perspective(perspective) => Some(perspective.fov),
                _ => None,
            })
            .unwrap_or(std::f32::consts::FRAC_PI_4);
        let size = world.resource::<gpu::GpuViewportSize>();
        let aspect = f32_from_extent(size.width) / f32_from_extent(size.height.max(1));
        let mut transform = world
            .get_mut::<Transform>(camera)
            .ok_or(SceneError::NoViewportCamera)?;
        let direction = (transform.translation - center).normalize_or(Vec3::Z);
        transform.translation = center + direction * fit_distance(radius, fov, aspect);
        transform.look_at(center, Vec3::Y);
        world.resource_mut::<ViewportPivot>().0 = center;
        Ok(())
    }

    /// Render the CPU oracle through the live viewport camera.
    ///
    /// Same cube as [`render_demo_frame`](crate::render::render_demo_frame),
    /// but the projection derives from the camera the orbit/pan/dolly ops
    /// move: distance drives the scale, orbit the view angles, the pivot's
    /// camera-space position the screen center — so every nav op is
    /// pixel-observable without warming up the GPU path. Reads the turntable
    /// angle and viewport extents live, so consecutive ticks still differ.
    ///
    /// # Errors
    ///
    /// Returns [`SceneError::NoViewportCamera`] when the demo camera is gone.
    pub fn render_camera_frame(&mut self) -> Result<RenderFrame, SceneError> {
        let camera = self.viewport_camera()?;
        let world = self.app.world();
        let transform = world
            .get::<Transform>(camera)
            .ok_or(SceneError::NoViewportCamera)?;
        let pivot = self.pivot();
        let offset = transform.translation - pivot;
        let distance = offset.length();
        let view = OracleCamera {
            distance,
            // The spawn pose sits on +Z, so its azimuth is exactly 0 and the
            // live azimuth already is the orbit delta.
            yaw_offset: offset.x.atan2(offset.z),
            pitch_offset: elevation_of(offset, distance) - spawn_elevation(),
            pan_right: pivot.dot(transform.rotation * Vec3::X),
            pan_up: pivot.dot(transform.rotation * Vec3::Y),
        };
        let (width, height) = self.viewport_size();
        render_demo_frame_with_camera(self.spin_angle(), &view, width, height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::{FRAME_HEIGHT, FRAME_WIDTH};

    /// Camera-to-pivot distance in a spawned world.
    fn camera_distance(world: &mut SceneWorld) -> f32 {
        let camera = world.viewport_camera().expect("demo camera exists");
        let translation = world
            .app
            .world()
            .get::<Transform>(camera)
            .expect("camera has a transform")
            .translation;
        (translation - world.pivot()).length()
    }

    #[test]
    fn orbit_keeps_distance_and_moves_position() {
        let mut world = SceneWorld::new_headless();
        let _ = world.spawn_demo_scene();
        let before = camera_distance(&mut world);
        world.orbit_camera(120.0, 40.0).expect("orbit works");
        let after = camera_distance(&mut world);
        assert!(
            (before - after).abs() < 1e-4,
            "orbit must preserve radius, went {before} -> {after}"
        );
        let camera = world.viewport_camera().expect("demo camera exists");
        let translation = world
            .app
            .world()
            .get::<Transform>(camera)
            .expect("camera has a transform")
            .translation;
        assert!(
            (translation.x - 0.0).abs() > 0.1,
            "a 120px drag must swing the camera off +Z, got {translation:?}"
        );
    }

    #[test]
    fn orbit_leaves_pivot_fixed() {
        let mut world = SceneWorld::new_headless();
        let _ = world.spawn_demo_scene();
        let pivot = world.pivot();
        world.orbit_camera(200.0, 100.0).expect("orbit works");
        assert_eq!(world.pivot(), pivot);
    }

    #[test]
    fn orbit_clamps_elevation_before_the_pole() {
        let mut world = SceneWorld::new_headless();
        let _ = world.spawn_demo_scene();
        for vertical_px in [100_000.0, -100_000.0] {
            world.orbit_camera(0.0, vertical_px).expect("orbit works");
            let camera = world.viewport_camera().expect("demo camera exists");
            let translation = world
                .app
                .world()
                .get::<Transform>(camera)
                .expect("camera has a transform")
                .translation;
            let offset = translation - world.pivot();
            let sine = offset.y / offset.length();
            assert!(
                sine.is_finite() && sine.abs() <= 1.0,
                "elevation must stay valid, got sine {sine}"
            );
            assert!(
                (sine.abs() - 1.0).abs() < 1e-3,
                "a huge vertical drag must park at ±89.9 degrees, got sine {sine}"
            );
        }
    }

    #[test]
    fn orbit_quarter_turn_matches_pure_math() {
        // Arrange: camera straight out on +Z at unit distance.
        let offset = Vec3::new(0.0, 0.0, 2.0);
        // Act: the pixel count of a 90-degree swing.
        let turned = orbit_offset(
            offset,
            std::f32::consts::FRAC_PI_2 / ORBIT_SPEED_RAD_PER_PX,
            0.0,
        );
        // Assert: parked on +X at the same radius, no Y drift.
        assert!((turned.length() - 2.0).abs() < 1e-5);
        assert!((turned.x - 2.0).abs() < 1e-4, "got {turned:?}");
        assert!(turned.y.abs() < 1e-5, "got {turned:?}");
        assert!(turned.z.abs() < 1e-4, "got {turned:?}");
    }

    #[test]
    fn non_finite_deltas_are_noops() {
        let mut world = SceneWorld::new_headless();
        let _ = world.spawn_demo_scene();
        let camera = world.viewport_camera().expect("demo camera exists");
        let before = *world
            .app
            .world()
            .get::<Transform>(camera)
            .expect("camera has a transform");
        world
            .orbit_camera(f32::NAN, 0.0)
            .expect("NaN orbit is a noop");
        world.pan_camera(0.0, f32::INFINITY).expect("pan is a noop");
        world
            .dolly_camera(f32::NAN, (0.0, 0.0))
            .expect("NaN dolly is a noop");
        let after = *world
            .app
            .world()
            .get::<Transform>(camera)
            .expect("camera has a transform");
        assert_eq!(before.translation, after.translation);
        assert_eq!(before.rotation, after.rotation);
    }

    #[test]
    fn pan_translates_camera_and_pivot_rigidly() {
        let mut world = SceneWorld::new_headless();
        let _ = world.spawn_demo_scene();
        let camera = world.viewport_camera().expect("demo camera exists");
        let before_t = world
            .app
            .world()
            .get::<Transform>(camera)
            .expect("camera has a transform")
            .translation;
        let before_p = world.pivot();
        world.pan_camera(100.0, 0.0).expect("pan works");
        let after_t = world
            .app
            .world()
            .get::<Transform>(camera)
            .expect("camera has a transform")
            .translation;
        let after_p = world.pivot();
        assert_eq!(after_t - before_t, after_p - before_p);
        assert!(
            (after_t - before_t).length() > 1e-3,
            "a 100px pan must move the rig"
        );
    }

    #[test]
    fn centered_dolly_halves_distance_and_holds_pivot() {
        let mut world = SceneWorld::new_headless();
        let _ = world.spawn_demo_scene();
        let before = camera_distance(&mut world);
        let pivot = world.pivot();
        world
            .dolly_camera(std::f32::consts::LN_2, (0.0, 0.0))
            .expect("dolly works");
        let after = camera_distance(&mut world);
        assert!(
            (after - before * 0.5).abs() < 1e-3,
            "ln2 must halve the distance, went {before} -> {after}"
        );
        assert!(
            (world.pivot() - pivot).length() < 1e-6,
            "centered dolly must hold the pivot, drifted {pivot:?} -> {:?}",
            world.pivot()
        );
    }

    #[test]
    fn off_center_dolly_pulls_pivot_toward_cursor() {
        let mut world = SceneWorld::new_headless();
        let _ = world.spawn_demo_scene();
        let pivot = world.pivot();
        world.dolly_camera(1.0, (1.0, 0.0)).expect("dolly works");
        assert!(
            world.pivot().x > pivot.x,
            "a right-edge dolly must pull the pivot right, went {pivot:?} -> {:?}",
            world.pivot()
        );
    }

    #[test]
    fn dolly_clamps_to_scene_scale() {
        let mut world = SceneWorld::new_headless();
        let _ = world.spawn_demo_scene();
        let diagonal = world
            .scene_bounds()
            .map(|bounds| (bounds.max - bounds.min).length())
            .expect("demo cube has bounds");
        world.dolly_camera(100.0, (0.0, 0.0)).expect("dolly works");
        assert!((camera_distance(&mut world) - 0.05 * diagonal).abs() < 1e-3);
        world.dolly_camera(-200.0, (0.0, 0.0)).expect("dolly works");
        assert!((camera_distance(&mut world) - 50.0 * diagonal).abs() < 1e-2);
    }

    #[test]
    fn frame_all_fits_the_cube() {
        let mut world = SceneWorld::new_headless();
        let _ = world.spawn_demo_scene();
        world.frame_all().expect("frame-all works");
        // Assert: pivot back at the cube center, distance at the analytic fit
        // (r = sqrt(3)/2, fov 45 deg, aspect 512/320, margin 1.15 ~= 2.60).
        assert!((world.pivot() - Vec3::ZERO).length() < 1e-4);
        let distance = camera_distance(&mut world);
        let aspect = f32_from_extent(FRAME_WIDTH) / f32_from_extent(FRAME_HEIGHT);
        let expected = fit_distance(3.0_f32.sqrt() * 0.5, std::f32::consts::FRAC_PI_4, aspect);
        assert!(
            (distance - expected).abs() < 0.05,
            "framed distance {distance} must match the analytic fit {expected}"
        );
        // Assert: the cube actually projects inside the frame — NDC of its
        // center within [-1, 1] on both axes.
        let camera = world.viewport_camera().expect("demo camera exists");
        let translation = world
            .app
            .world()
            .get::<Transform>(camera)
            .expect("camera has a transform")
            .translation;
        let view_dir = (world.pivot() - translation).normalize();
        let right = view_dir.cross(Vec3::Y).normalize();
        let up = right.cross(view_dir);
        let to_center = world.pivot() - translation;
        let depth = to_center.dot(view_dir);
        let half_height = depth * (std::f32::consts::FRAC_PI_8).tan();
        let ndc_y = to_center.dot(up) / half_height;
        let ndc_x = to_center.dot(right) / (half_height * aspect);
        assert!(ndc_x.abs() <= 1.0 && ndc_y.abs() <= 1.0);
    }

    #[test]
    fn frame_all_without_geometry_is_a_noop() {
        let mut world = SceneWorld::new_headless();
        let ids = world.spawn_demo_scene();
        world.update();
        world.app.world_mut().despawn(ids.cube);
        let camera = world.viewport_camera().expect("demo camera exists");
        let before = *world
            .app
            .world()
            .get::<Transform>(camera)
            .expect("camera has a transform");
        world.frame_all().expect("empty frame-all is Ok");
        let after = *world
            .app
            .world()
            .get::<Transform>(camera)
            .expect("camera has a transform");
        assert_eq!(before.translation, after.translation);
    }

    #[test]
    fn missing_camera_is_an_error() {
        let mut world = SceneWorld::new_headless();
        assert_eq!(
            world.orbit_camera(10.0, 0.0),
            Err(SceneError::NoViewportCamera)
        );
        assert_eq!(
            world.pan_camera(10.0, 0.0),
            Err(SceneError::NoViewportCamera)
        );
        assert_eq!(
            world.dolly_camera(1.0, (0.0, 0.0)),
            Err(SceneError::NoViewportCamera)
        );
        assert_eq!(world.frame_all(), Err(SceneError::NoViewportCamera));
    }

    #[test]
    fn spawn_seats_pivot_at_bounds_center() {
        let mut world = SceneWorld::new_headless();
        let _ = world.spawn_demo_scene();
        let bounds = world.scene_bounds().expect("demo cube has bounds");
        let center = Vec3::from((bounds.min + bounds.max) * 0.5);
        assert_eq!(world.pivot(), center);
    }

    #[test]
    fn ticks_never_recenter_the_pivot() {
        let mut world = SceneWorld::new_headless();
        let _ = world.spawn_demo_scene();
        world.pan_camera(50.0, 25.0).expect("pan works");
        let pivot = world.pivot();
        for _ in 0..5 {
            world.update();
        }
        assert_eq!(world.pivot(), pivot);
    }

    #[test]
    fn scene_bounds_cover_the_demo_cube() {
        let mut world = SceneWorld::new_headless();
        let _ = world.spawn_demo_scene();
        let bounds = world.scene_bounds().expect("cube has bounds");
        assert!((bounds.min.x + 0.5).abs() < 1e-4, "got {bounds:?}");
        assert!((bounds.max.x - 0.5).abs() < 1e-4, "got {bounds:?}");
    }
}
