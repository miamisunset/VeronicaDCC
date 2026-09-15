//! Headless Bevy scene: ECS world mirroring the procedural DAG output.
//!
//! Runs without a window or renderer (`ScheduleRunnerPlugin`-only setup).
//! Never call `App::run()` on the Swift `MainActor` thread — drive via
//! [`SceneWorld::update`] from a background thread or a Swift-driven tick.
//!
//! The world owns a small Rust-built demo scene (camera marker, light
//! marker, spinning cube) so the frame loop has observable ECS state before
//! the render slice (ADR 0001) attaches visual meaning to it.

use bevy_app::{App, ScheduleRunnerPlugin, Update};
use bevy_ecs::prelude::*;
use bevy_transform::prelude::Transform;
use std::time::Duration;
use thiserror::Error;
use veronica_core::{BoneTransform, MeshId, MorphWeights};

/// Errors for scene operations.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum SceneError {
    /// Requested entity does not exist.
    #[error("unknown scene entity")]
    UnknownEntity,
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

/// Tags every entity belonging to the Rust-owned demo scene.
///
/// Bevy itself owns internal entities (schedules and friends), so raw world
/// counts are meaningless to Swift. [`SceneWorld::entity_count`] counts only
/// entities carrying this marker: the demo camera, light, and cube.
#[derive(Debug, Clone, Copy, Default, Component)]
pub struct DemoScene;

/// Marker for the demo camera. Render meaning attaches in the render slice
/// (ADR 0001); until then this only proves ECS ownership of scene content.
#[derive(Debug, Clone, Copy, Default, Component)]
pub struct DemoCamera;

/// Marker for the demo light. See [`DemoCamera`].
#[derive(Debug, Clone, Copy, Default, Component)]
pub struct DemoLight;

/// Marker for the demo cube. Its [`Transform`] is rotated a fixed step every
/// [`SceneWorld::update`] by [`spin_demo_cubes`], so successive ticks are
/// observably different even before pixels exist.
#[derive(Debug, Clone, Copy, Default, Component)]
pub struct DemoCube;

/// Entity ids of the Rust-owned demo scene, for future render wiring.
#[derive(Debug, Clone, Copy)]
pub struct DemoSceneIds {
    /// Camera entity.
    pub camera: Entity,
    /// Light entity.
    pub light: Entity,
    /// Spinning cube entity.
    pub cube: Entity,
}

/// Ticks elapsed since creation. Incremented by [`count_ticks`] each update.
#[derive(Debug, Default, Resource)]
struct TickCount(u64);

/// Fixed per-tick rotation step (radians) for [`spin_demo_cubes`].
/// Deterministic on purpose: tests assert rotation advances without a clock.
const DEMO_SPIN_STEP: f32 = 0.02;

/// Advance [`TickCount`] once per schedule run.
fn count_ticks(mut count: ResMut<TickCount>) {
    count.0 = count.0.saturating_add(1);
}

/// Rotate every [`DemoCube`] one fixed step around Y.
fn spin_demo_cubes(mut cubes: Query<&mut Transform, With<DemoCube>>) {
    for mut transform in &mut cubes {
        transform.rotate_y(DEMO_SPIN_STEP);
    }
}

/// Headless scene world. Owns the Bevy [`App`] plus id lookup tables.
#[derive(Debug, Default)]
pub struct SceneWorld {
    app: App,
}

impl SceneWorld {
    /// Create a headless app: no window, no renderer, fixed 60 Hz schedule.
    #[must_use]
    pub fn new_headless() -> Self {
        let mut app = App::new();
        app.add_plugins(bevy_app::ScheduleRunnerPlugin::run_loop(
            Duration::from_secs_f64(1.0 / 60.0),
        ));
        app.init_resource::<TickCount>()
            .add_systems(Update, (count_ticks, spin_demo_cubes));
        Self { app }
    }

    /// Spawn the Rust-owned demo scene: camera marker, light marker, and a
    /// spinning cube with an identity [`Transform`]. All three carry
    /// [`DemoScene`] so [`SceneWorld::entity_count`] sees exactly them.
    #[must_use]
    pub fn spawn_demo_scene(&mut self) -> DemoSceneIds {
        let world = self.app.world_mut();
        let camera = world.spawn((DemoScene, DemoCamera)).id();
        let light = world.spawn((DemoScene, DemoLight)).id();
        let cube = world
            .spawn((
                DemoScene,
                DemoCube,
                MeshTag(MeshId(0)),
                Transform::default(),
            ))
            .id();
        DemoSceneIds {
            camera,
            light,
            cube,
        }
    }

    /// Ticks elapsed since creation.
    #[must_use]
    pub fn tick_count(&self) -> u64 {
        self.app.world().resource::<TickCount>().0
    }

    /// Number of live demo-scene entities (those tagged [`DemoScene`]).
    ///
    /// Takes `&mut self` because some Bevy versions require mutable world
    /// access to construct a query, even though nothing is mutated.
    #[must_use]
    pub fn entity_count(&mut self) -> usize {
        let mut state = self
            .app
            .world_mut()
            .query_filtered::<Entity, With<DemoScene>>();
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

// Keep `ScheduleRunnerPlugin` import used across Bevy versions.
#[allow(dead_code, reason = "compile-time assertion shim, not runtime code")]
fn _assert_runner_plugin_is_linkable(p: ScheduleRunnerPlugin) -> ScheduleRunnerPlugin {
    p
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
    fn demo_scene_spawns_three_entities() {
        let mut world = SceneWorld::new_headless();
        let ids = world.spawn_demo_scene();
        assert_eq!(world.entity_count(), 3);
        assert!(world.app.world().get::<DemoCamera>(ids.camera).is_some());
        assert!(world.app.world().get::<DemoLight>(ids.light).is_some());
        assert!(world.app.world().get::<DemoCube>(ids.cube).is_some());
    }

    #[test]
    fn ticks_advance_count_and_cube_rotation() {
        let mut world = SceneWorld::new_headless();
        let ids = world.spawn_demo_scene();
        assert_eq!(world.tick_count(), 0);
        let before = *world.app.world().get::<Transform>(ids.cube).unwrap();
        world.update();
        assert_eq!(world.tick_count(), 1);
        let after = *world.app.world().get::<Transform>(ids.cube).unwrap();
        assert_ne!(before.rotation, after.rotation);
    }
}
