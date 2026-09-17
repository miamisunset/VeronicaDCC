//! Headless Bevy scene: ECS world mirroring the procedural DAG output.
//!
//! Runs headless with an explicit GPU plugin set (see `gpu`): no window, an
//! offscreen `Bgra8UnormSrgb` target, synchronous CPU readback. Never call
//! `App::run()` on the Swift `MainActor` thread — drive via
//! [`SceneWorld::update`] from a background thread or a Swift-driven tick.
//!
//! The world owns a small Rust-built base scene (viewport camera plus key
//! light). Graph content arrives through the tick recook loop: the FFI
//! context cooks its [`OperatorGraph`](veronica_graph::OperatorGraph) into
//! render-bound entities (see `cook`) whenever the graph epoch moved since
//! the last cook, so an empty graph renders camera plus light only.

use bevy_app::{App, Update};
use bevy_asset::Assets;
use bevy_camera::{Camera, Camera3d, RenderTarget};
use bevy_core_pipeline::CorePipelinePlugin;
use bevy_diagnostic::FrameCountPlugin;
use bevy_ecs::prelude::*;
use bevy_image::{Image, ImagePlugin};
use bevy_light::{DirectionalLight, LightPlugin};
use bevy_math::prelude::Vec3;
use bevy_mesh::MeshPlugin;
use bevy_pbr::PbrPlugin;
use bevy_render::RenderPlugin;
use bevy_time::TimePlugin;
use bevy_transform::prelude::Transform;
use bevy_window::{ExitCondition, WindowPlugin};
use thiserror::Error;
use veronica_core::{BoneTransform, MeshId, MorphWeights};

mod camera;
mod cook;
mod gpu;
mod mesh;
mod pick;
mod render;

pub use cook::{CookedMesh, SourceOperator};
pub use mesh::render_mesh_from_evaluated;
pub use pick::Pick;
pub use render::{
    FRAME_BYTES_PER_PIXEL, FRAME_HEIGHT, FRAME_WIDTH, MAX_VIEWPORT_EDGE, RenderFrame,
};

/// Errors for scene operations.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum SceneError {
    /// Requested entity does not exist.
    #[error("unknown scene entity")]
    UnknownEntity,
    /// A standard attribute channel is absent from the evaluated mesh.
    #[error("missing attribute \"{name}\"")]
    MissingAttribute {
        /// Channel key that was looked up.
        name: String,
    },
    /// A standard attribute channel has the wrong shape.
    #[error("attribute \"{name}\" must be {expected}")]
    AttributeShape {
        /// Channel key that was looked up.
        name: String,
        /// Shape the handoff requires (e.g. "a vec3 channel").
        expected: &'static str,
    },
    /// A standard attribute channel's length differs from the vertex count.
    #[error("attribute \"{name}\" has {actual} elements for {expected} vertices")]
    AttributeLength {
        /// Channel key that was looked up.
        name: String,
        /// Vertex count the channel must match.
        expected: usize,
        /// Elements the channel actually holds.
        actual: usize,
    },
    /// A triangle index names no vertex.
    #[error("index {index} is out of bounds for {vertex_count} vertices")]
    IndexOutOfBounds {
        /// Offending index value.
        index: u32,
        /// Vertices the mesh actually holds.
        vertex_count: usize,
    },
    /// The graph failed to cook before any mesh reached the scene.
    #[error(transparent)]
    Cook(#[from] veronica_geometry::CookError),
    /// The requested viewport size is zero or exceeds the
    /// [`MAX_VIEWPORT_EDGE`](crate::render::MAX_VIEWPORT_EDGE) long-edge cap.
    ///
    /// This guards the live GPU target (staging-buffer memory is bounded
    /// by the cap).
    #[error(
        "invalid viewport size {width}x{height}: extents must be nonzero with longest edge <= 2048"
    )]
    InvalidViewportSize {
        /// Requested width in pixels.
        width: u32,
        /// Requested height in pixels.
        height: u32,
    },
    /// The GPU render target has no uploaded image yet; tick the world first.
    #[error("render target has no GPU image yet")]
    NoGpuImage,
    /// The viewport camera is missing; the base scene was never spawned or
    /// its camera was despawned. Navigation ops need it alive.
    #[error("viewport camera is missing")]
    NoViewportCamera,
    /// The staging stride does not fit in a `u32`.
    #[error("frame staging stride does not fit in u32")]
    StagingStrideOverflow,
    /// The GPU device poll failed during frame readback.
    #[error("GPU device poll failed during frame readback")]
    DevicePollFailed,
    /// The staging buffer map callback never ran.
    #[error("staging buffer map callback never ran")]
    MapCallbackLost,
    /// The staging buffer map failed.
    #[error("staging buffer map failed")]
    MapFailed,
    /// Raw bytes do not fill a `width` x `height` BGRA8 frame.
    #[error("raw frame has {actual} bytes, expected {expected}")]
    FrameLengthMismatch {
        /// Bytes the frame layout requires.
        expected: usize,
        /// Bytes actually supplied.
        actual: usize,
    },
    /// A cooked mesh cannot be pick-meshed: non-triangle topology or a
    /// vertex soup that is not whole triangles.
    #[error("cooked mesh is not pickable as triangles")]
    UnpickableMesh,
    /// A pick count exceeds the 24-bit ID-buffer range.
    #[error("pick identity space exhausted: {count} items exceed the 24-bit range")]
    PickSpaceExhausted {
        /// Items needing distinct IDs (triangles or entity slots).
        count: usize,
    },
    /// A pick pass showed no painted pixel within its update budget.
    #[error("pick pass produced no painted pixel after {attempts} updates")]
    PickPassNotReady {
        /// Updates waited before giving up.
        attempts: u32,
    },
}

/// Bevy component mirroring [`MorphWeights`] for one mesh entity.
#[derive(Debug, Clone, Component, Default)]
pub struct MorphWeightComponent {
    /// Current blend weights.
    pub weights: Vec<f32>,
}

/// Bevy component mirroring [`BoneTransform`] for one bone entity.
#[derive(Debug, Clone, Component)]
pub struct BoneTransformComponent {
    /// Current bone transform.
    pub transform: BoneTransform,
}

/// Bevy tag linking an entity back to its [`MeshId`].
/// (Kept here so `veronica-core` stays Bevy-free.)
#[derive(Debug, Clone, Copy, Component)]
pub struct MeshTag(pub MeshId);

/// Tags every entity belonging to the Rust-owned scene.
///
/// Bevy itself owns internal entities (schedules and friends), so raw world
/// counts are meaningless to Swift. [`SceneWorld::entity_count`] counts only
/// entities carrying this marker: the viewport camera, the light, and the
/// cooked graph meshes.
#[derive(Debug, Clone, Copy, Default, Component)]
pub struct SceneTag;

/// Marker for the viewport camera. Carries the real render components
/// ([`Camera3d`](bevy_camera::Camera3d), [`Camera`](bevy_camera::Camera),
/// [`RenderTarget`](bevy_camera::RenderTarget), [`Transform`]) pointed at the
/// offscreen target; the marker preserves the
/// [`SceneWorld::entity_count`] semantics.
#[derive(Debug, Clone, Copy, Default, Component)]
pub struct ViewportCamera;

/// Marker for the scene key light. Carries a real
/// [`DirectionalLight`](bevy_light::DirectionalLight) plus [`Transform`].
/// See [`ViewportCamera`].
#[derive(Debug, Clone, Copy, Default, Component)]
pub struct SceneLight;

/// Entity ids of the Rust-owned base scene, for future render wiring.
#[derive(Debug, Clone, Copy)]
pub struct SceneIds {
    /// Camera entity.
    pub camera: Entity,
    /// Light entity.
    pub light: Entity,
}

/// Ticks elapsed since creation. Incremented by [`count_ticks`] each update.
#[derive(Debug, Default, Resource)]
struct TickCount(u64);

/// Viewport camera height above the ground plane, looking at the origin.
const VIEWPORT_CAMERA_HEIGHT: f32 = 1.5;

/// Viewport camera distance from the origin along +Z.
const VIEWPORT_CAMERA_DISTANCE: f32 = 4.5;

/// Scene directional-light position; the light shines toward the origin.
const SCENE_LIGHT_OFFSET_X: f32 = 2.0;
/// Scene directional-light position; the light shines toward the origin.
const SCENE_LIGHT_OFFSET_Y: f32 = 4.0;
/// Scene directional-light position; the light shines toward the origin.
const SCENE_LIGHT_OFFSET_Z: f32 = 3.0;

/// Advance [`TickCount`] once per schedule run.
fn count_ticks(mut count: ResMut<TickCount>) {
    count.0 = count.0.saturating_add(1);
}

/// Headless scene world. Owns the Bevy [`App`] plus id lookup tables.
#[derive(Debug, Default)]
pub struct SceneWorld {
    app: App,
}

impl SceneWorld {
    /// Create a headless GPU app: no window, offscreen target, manual ticks.
    ///
    /// Builds the explicit plugin set documented in `gpu` (order matters:
    /// task pools, frame counter, time, transform, assets, windowless window
    /// plugin, renderer, image, mesh, camera, light, core pipeline, PBR),
    /// finalizes it once with `finish` + `cleanup`, then creates the
    /// offscreen render target. Requires Metal; Bevy panics during plugin
    /// init when no adapter exists (deliberate, see `gpu`).
    #[must_use]
    pub fn new_headless() -> Self {
        let mut app = App::new();
        app.add_plugins((
            bevy_app::TaskPoolPlugin::default(),
            FrameCountPlugin,
            TimePlugin,
            bevy_transform::TransformPlugin,
            bevy_asset::AssetPlugin::default(),
            WindowPlugin {
                primary_window: None,
                exit_condition: ExitCondition::DontExit,
                ..Default::default()
            },
            RenderPlugin::default(),
            ImagePlugin::default(),
            MeshPlugin,
            bevy_camera::CameraPlugin,
            LightPlugin,
            CorePipelinePlugin,
            PbrPlugin::default(),
        ));
        app.init_resource::<TickCount>()
            .add_systems(Update, count_ticks);
        app.finish();
        app.cleanup();
        let target = {
            let mut images = app.world_mut().resource_mut::<Assets<Image>>();
            gpu::create_frame_target(&mut images)
        };
        app.world_mut()
            .insert_resource(gpu::GpuFrameTarget { handle: target });
        app.world_mut().insert_resource(gpu::GpuViewportSize {
            width: render::FRAME_WIDTH,
            height: render::FRAME_HEIGHT,
        });
        Self { app }
    }

    /// Spawn the Rust-owned base scene: viewport camera plus key light,
    /// both carrying [`SceneTag`] so [`SceneWorld::entity_count`] sees
    /// exactly them.
    ///
    /// Alongside the markers each entity carries real render components: the
    /// camera gets [`Camera3d`](bevy_camera::Camera3d) plus a
    /// [`Camera`](bevy_camera::Camera) pointed at the offscreen target, and
    /// the light a [`DirectionalLight`](bevy_light::DirectionalLight).
    /// Graph content is not spawned here — it arrives through the tick
    /// recook loop (see `cook`), so an empty graph renders camera plus
    /// light only.
    #[must_use]
    pub fn spawn_base_scene(&mut self) -> SceneIds {
        let target = self
            .app
            .world()
            .resource::<gpu::GpuFrameTarget>()
            .handle
            .clone();
        let world = self.app.world_mut();
        let camera = world
            .spawn((
                SceneTag,
                ViewportCamera,
                Camera3d::default(),
                Camera::default(),
                RenderTarget::from(target),
                Transform::from_xyz(0.0, VIEWPORT_CAMERA_HEIGHT, VIEWPORT_CAMERA_DISTANCE)
                    .looking_at(Vec3::ZERO, Vec3::Y),
            ))
            .id();
        let light = world
            .spawn((
                SceneTag,
                SceneLight,
                DirectionalLight::default(),
                Transform::from_xyz(
                    SCENE_LIGHT_OFFSET_X,
                    SCENE_LIGHT_OFFSET_Y,
                    SCENE_LIGHT_OFFSET_Z,
                )
                .looking_at(Vec3::ZERO, Vec3::Y),
            ))
            .id();
        let pivot = self.scene_bounds().map_or(Vec3::ZERO, |bounds| {
            Vec3::from((bounds.min + bounds.max) * 0.5)
        });
        self.app
            .world_mut()
            .insert_resource(camera::ViewportPivot(pivot));
        SceneIds { camera, light }
    }

    /// Ticks elapsed since creation.
    #[must_use]
    pub fn tick_count(&self) -> u64 {
        self.app.world().resource::<TickCount>().0
    }

    /// Live viewport extents in pixels (initially `FRAME_WIDTH` x
    /// `FRAME_HEIGHT`, re-targeted by [`SceneWorld::set_viewport_size`]).
    #[must_use]
    pub fn viewport_size(&self) -> (u32, u32) {
        let size = self.app.world().resource::<gpu::GpuViewportSize>();
        (size.width, size.height)
    }

    /// Re-target the offscreen viewport to `width` x `height` pixels.
    ///
    /// Recreates the offscreen [`Image`] target, re-points the viewport camera
    /// at it, and drops the staging buffer (lazily rebuilt at the new size
    /// on the next [`SceneWorld::render_frame`]). Idempotent: requesting
    /// the current size is a no-op that touches neither the target nor the
    /// camera. The Bevy camera aspect follows the target texture
    /// automatically, so no projection code runs here.
    ///
    /// # Errors
    ///
    /// Returns [`SceneError::InvalidViewportSize`] when either extent is
    /// zero or the longest edge exceeds
    /// [`MAX_VIEWPORT_EDGE`](crate::render::MAX_VIEWPORT_EDGE).
    pub fn set_viewport_size(&mut self, width: u32, height: u32) -> Result<(), SceneError> {
        use render::MAX_VIEWPORT_EDGE;
        if width == 0 || height == 0 || width.max(height) > MAX_VIEWPORT_EDGE {
            return Err(SceneError::InvalidViewportSize { width, height });
        }
        if self.viewport_size() == (width, height) {
            return Ok(());
        }
        let old_handle = self
            .app
            .world()
            .resource::<gpu::GpuFrameTarget>()
            .handle
            .clone();
        let new_handle = {
            let mut images = self.app.world_mut().resource_mut::<Assets<Image>>();
            images.remove(&old_handle);
            gpu::create_frame_target_sized(&mut images, width, height)
        };
        self.app.world_mut().insert_resource(gpu::GpuFrameTarget {
            handle: new_handle.clone(),
        });
        self.app
            .world_mut()
            .insert_resource(gpu::GpuViewportSize { width, height });
        self.app
            .world_mut()
            .remove_resource::<gpu::GpuFrameStaging>();
        {
            let mut cameras = self
                .app
                .world_mut()
                .query_filtered::<&mut RenderTarget, With<ViewportCamera>>();
            let world = self.app.world_mut();
            for mut target in cameras.iter_mut(world) {
                *target = RenderTarget::from(new_handle.clone());
            }
        }
        Ok(())
    }

    /// Render the current scene state into a frame at the live viewport
    /// extents.
    ///
    /// The frame-publish seam: Swift presents whatever this returns without
    /// interpreting scene content. Pixels come off the GPU
    /// (`gpu::readback_frame`); the scene is static unless a nav op moves
    /// the camera or a recook swaps the cooked meshes, so identical ECS
    /// state publishes identical frames and every nav op is pixel-observable.
    ///
    /// # Errors
    ///
    /// Returns [`SceneError::NoGpuImage`] when the render target has no
    /// GPU image yet, or the staging, poll, and map [`SceneError`] variants
    /// when the copy-back fails.
    pub fn render_frame(&mut self) -> Result<RenderFrame, SceneError> {
        gpu::readback_frame(&mut self.app)
    }

    /// Number of live scene entities (those tagged [`SceneTag`]): the
    /// viewport camera, the light, and the currently cooked graph meshes.
    ///
    /// Takes `&mut self` because some Bevy versions require mutable world
    /// access to construct a query, even though nothing is mutated.
    #[must_use]
    pub fn entity_count(&mut self) -> usize {
        let mut state = self
            .app
            .world_mut()
            .query_filtered::<Entity, With<SceneTag>>();
        state.iter(self.app.world()).count()
    }

    /// Spawn an empty mesh entity tagged with [`MeshId`].
    pub fn spawn_mesh(&mut self, id: MeshId) -> Entity {
        self.app
            .world_mut()
            .spawn((MeshTag(id), MorphWeightComponent::default()))
            .id()
    }

    /// Overwrite morph weights for `entity`. Returns [`SceneError`] if dead.
    ///
    /// # Errors
    ///
    /// Returns [`SceneError::UnknownEntity`] when `entity` is not alive in
    /// the world.
    pub fn set_morph_weights(
        &mut self,
        entity: Entity,
        weights: &MorphWeights,
    ) -> Result<(), SceneError> {
        let mut entity_mut = self
            .app
            .world_mut()
            .get_entity_mut(entity)
            .map_err(|_| SceneError::UnknownEntity)?;
        if let Some(mut component) = entity_mut.get_mut::<MorphWeightComponent>() {
            component.weights.clone_from(&weights.weights);
        } else {
            entity_mut.insert(MorphWeightComponent {
                weights: weights.weights.clone(),
            });
        }
        Ok(())
    }

    /// Run one schedule tick. Call from a background thread, never `MainActor`.
    pub fn update(&mut self) {
        self.app.update();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_entity_is_an_error() {
        let mut world = SceneWorld::new_headless();
        let entity = world.spawn_mesh(MeshId(1));
        world.app.world_mut().despawn(entity);
        assert_eq!(
            world.set_morph_weights(entity, &MorphWeights { weights: vec![0.5] },),
            Err(SceneError::UnknownEntity)
        );
    }

    #[test]
    fn missing_component_is_inserted() {
        let mut world = SceneWorld::new_headless();
        let entity = world.app.world_mut().spawn_empty().id();
        world
            .set_morph_weights(entity, &MorphWeights { weights: vec![0.5] })
            .unwrap();
        let stored = world
            .app
            .world()
            .get::<MorphWeightComponent>(entity)
            .unwrap();
        assert_eq!(stored.weights, vec![0.5]);
    }

    #[test]
    fn morph_weights_round_trip() {
        let mut world = SceneWorld::new_headless();
        let entity = world.spawn_mesh(MeshId(7));
        world
            .set_morph_weights(
                entity,
                &MorphWeights {
                    weights: vec![0.25, 0.75],
                },
            )
            .unwrap();
        world.update();
        let stored = world
            .app
            .world()
            .get::<MorphWeightComponent>(entity)
            .unwrap();
        assert_eq!(stored.weights, vec![0.25, 0.75]);
    }

    #[test]
    fn base_scene_spawns_camera_and_light_only() {
        let mut world = SceneWorld::new_headless();
        let ids = world.spawn_base_scene();
        assert_eq!(world.entity_count(), 2);
        assert!(
            world
                .app
                .world()
                .get::<ViewportCamera>(ids.camera)
                .is_some()
        );
        assert!(world.app.world().get::<SceneLight>(ids.light).is_some());
    }

    #[test]
    fn ticks_advance_count_on_a_static_scene() {
        let mut world = SceneWorld::new_headless();
        let _ = world.spawn_base_scene();
        assert_eq!(world.tick_count(), 0);
        world.update();
        assert_eq!(world.tick_count(), 1);
    }

    #[test]
    fn viewport_size_validation_and_idempotency() {
        let mut world = SceneWorld::new_headless();
        assert_eq!(
            world.viewport_size(),
            (render::FRAME_WIDTH, render::FRAME_HEIGHT)
        );
        for (width, height) in [
            (0, 200),
            (320, 0),
            (render::MAX_VIEWPORT_EDGE + 1, 100),
            (100, render::MAX_VIEWPORT_EDGE + 1),
        ] {
            assert!(
                matches!(
                    world.set_viewport_size(width, height),
                    Err(SceneError::InvalidViewportSize { .. })
                ),
                "extents {width}x{height} must be rejected"
            );
        }
        // Rejected sizes leave the live extents untouched.
        assert_eq!(
            world.viewport_size(),
            (render::FRAME_WIDTH, render::FRAME_HEIGHT)
        );
        world.set_viewport_size(256, 192).unwrap();
        assert_eq!(world.viewport_size(), (256, 192));
        // Same size again is a no-op, not an error.
        world.set_viewport_size(256, 192).unwrap();
        assert_eq!(world.viewport_size(), (256, 192));
    }
}
