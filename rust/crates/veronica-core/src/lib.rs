//! Domain types owned by Rust: the single source of truth for scene state.
//!
//! Covers mesh topology, morph-target weights, bone transforms,
//! node DAG topology, and undo/redo history. Swift mirrors these
//! read-only; it never writes them directly (see `veronica-ffi`).

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Stable identifier for a node in the procedural DAG.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeId(pub u64);

/// Stable identifier for a mesh in the scene.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MeshId(pub u64);

/// Per-morph-target blend weights, indexed by target position.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MorphWeights {
    /// One weight per morph target, typically in `0.0..=1.0`.
    pub weights: Vec<f32>,
}

/// A bone transform in model space (translation + rotation + scale).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BoneTransform {
    /// Translation in model units.
    pub translation: [f32; 3],
    /// Unit quaternion `(x, y, z, w)`.
    pub rotation: [f32; 4],
    /// Non-zero scale per axis.
    pub scale: [f32; 3],
}

impl Default for BoneTransform {
    fn default() -> Self {
        Self {
            translation: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        }
    }
}

/// Minimal mesh topology: indexed triangle list.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MeshTopology {
    /// Flat vertex positions (`len % 3 == 0`).
    pub positions: Vec<f32>,
    /// Triangle indices into the vertex list (`len % 3 == 0`).
    pub indices: Vec<u32>,
}

/// Errors for domain validation.
#[derive(Debug, Error, PartialEq)]
pub enum CoreError {
    /// A morph weight was outside the valid range.
    #[error("morph weight out of range: {0}")]
    MorphWeightOutOfRange(f32),
    /// Mesh buffers were not a multiple of the element stride.
    #[error("mesh topology has invalid stride")]
    InvalidMeshStride,
}

/// Validate a single morph weight is within `0.0..=1.0`.
///
/// # Errors
///
/// Returns [`CoreError::MorphWeightOutOfRange`] when `weight` is outside
/// `0.0..=1.0`.
pub fn validate_morph_weight(weight: f32) -> Result<f32, CoreError> {
    if (0.0..=1.0).contains(&weight) {
        Ok(weight)
    } else {
        Err(CoreError::MorphWeightOutOfRange(weight))
    }
}

/// Validate mesh buffers hold whole vertices and triangles.
///
/// # Errors
///
/// Returns [`CoreError::InvalidMeshStride`] when either buffer length is
/// not a multiple of 3.
pub fn validate_mesh_topology(mesh: &MeshTopology) -> Result<(), CoreError> {
    if mesh.positions.len().is_multiple_of(3) && mesh.indices.len().is_multiple_of(3) {
        Ok(())
    } else {
        Err(CoreError::InvalidMeshStride)
    }
}

/// Bounded undo/redo history over snapshots of `T`.
#[derive(Debug, Clone, Default)]
pub struct UndoHistory<T> {
    past: Vec<T>,
    future: Vec<T>,
    capacity: usize,
}

impl<T: Clone> UndoHistory<T> {
    /// Create a history holding at most `capacity` past snapshots.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            past: Vec::new(),
            future: Vec::new(),
            capacity: capacity.max(1),
        }
    }

    /// Push a new snapshot, clearing the redo stack.
    pub fn push(&mut self, snapshot: T) {
        if self.past.len() >= self.capacity {
            self.past.remove(0);
        }
        self.past.push(snapshot);
        self.future.clear();
    }

    /// Undo one step, returning the snapshot to restore, if any.
    pub fn undo(&mut self, current: T) -> Option<T> {
        let previous = self.past.pop()?;
        self.future.push(current);
        Some(previous)
    }

    /// Redo one step, returning the snapshot to restore, if any.
    pub fn redo(&mut self, current: T) -> Option<T> {
        let next = self.future.pop()?;
        self.past.push(current);
        Some(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_out_of_range_morph_weight() {
        assert!(validate_morph_weight(0.5).is_ok());
        assert_eq!(
            validate_morph_weight(1.5),
            Err(CoreError::MorphWeightOutOfRange(1.5))
        );
    }

    #[test]
    fn undo_redo_round_trips() {
        let mut history = UndoHistory::new(8);
        history.push(1_u32);
        assert_eq!(history.undo(2), Some(1));
        assert_eq!(history.redo(1), Some(2));
    }

    #[test]
    #[allow(
        clippy::float_cmp,
        reason = "comparing against exact literal constants"
    )]
    fn bone_transform_default_is_identity() {
        let transform = BoneTransform::default();
        assert_eq!(transform.translation, [0.0; 3]);
        assert_eq!(transform.rotation, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(transform.scale, [1.0; 3]);
    }

    #[test]
    fn mesh_stride_accepts_whole_elements_only() {
        let valid = MeshTopology {
            positions: vec![0.0; 6],
            indices: vec![0; 3],
        };
        assert!(validate_mesh_topology(&valid).is_ok());
        let ragged = MeshTopology {
            positions: vec![0.0; 4],
            indices: vec![0; 3],
        };
        assert_eq!(
            validate_mesh_topology(&ragged),
            Err(CoreError::InvalidMeshStride)
        );
    }

    #[test]
    fn empty_history_undo_redo_are_none() {
        let mut history: UndoHistory<u32> = UndoHistory::new(4);
        assert_eq!(history.undo(1), None);
        assert_eq!(history.redo(1), None);
    }

    #[test]
    fn history_evicts_oldest_at_capacity() {
        let mut history = UndoHistory::new(2);
        history.push(1_u32);
        history.push(2);
        history.push(3);
        assert_eq!(history.undo(9), Some(3));
        assert_eq!(history.undo(9), Some(2));
        assert_eq!(history.undo(9), None);
    }
}
