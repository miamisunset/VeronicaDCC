//! Headless Bevy scene: ECS world mirroring the procedural DAG output.
//!
//! Runs without a window or renderer (`MinimalPlugins`-style setup only).
//! Never call `App::run()` on the Swift `MainActor` thread — drive via
//! [`SceneWorld::update`] from a background thread or a Swift-driven tick.

use bevy_app::{App, ScheduleRunnerPlugin};
use bevy_ecs::prelude::*;
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
        Self { app }
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
}
