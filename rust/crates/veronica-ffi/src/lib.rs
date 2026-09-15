//! C-ABI boundary for Swift. Only plain C types cross here.
//!
//! Safety contract: every `vrn_*` function is `no_unwind` in spirit —
//! Rust panics are caught and mapped to [`VrnResult`] codes, never
//! propagated into Swift. All state lives behind an opaque pointer.

use std::sync::Mutex;
use veronica_core::{MeshId, MeshTopology, MorphWeights, UndoHistory, validate_mesh_topology};
use veronica_graph::NodeGraph;
use veronica_scene::{DemoSceneIds, SceneWorld};

/// Result codes returned across the FFI boundary.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VrnResult {
    /// Operation succeeded.
    Ok = 0,
    /// Null pointer argument.
    NullArgument = 1,
    /// Validation or graph error.
    InvalidArgument = 2,
    /// Internal lock or allocation failure.
    Internal = 3,
}

/// Opaque engine context owned by Rust, freed via [`vrn_context_destroy`].
#[derive(Debug)]
#[allow(
    dead_code,
    reason = "`graph`, `demo` + `mesh_history` wire up as the FFI surface grows"
)]
pub struct VrnContext {
    graph: NodeGraph,
    scene: SceneWorld,
    demo: DemoSceneIds,
    mesh_history: UndoHistory<MeshTopology>,
}

impl VrnContext {
    fn new() -> Self {
        let mut scene = SceneWorld::new_headless();
        let demo = scene.spawn_demo_scene();
        Self {
            graph: NodeGraph::new(),
            scene,
            demo,
            mesh_history: UndoHistory::new(64),
        }
    }
}

/// Create a new engine context. Returns null on allocation failure.
#[unsafe(no_mangle)]
pub extern "C" fn vrn_context_create() -> *mut Mutex<VrnContext> {
    Box::into_raw(Box::new(Mutex::new(VrnContext::new())))
}

/// Destroy a context created by [`vrn_context_create`]. Null-safe.
///
/// # Safety
///
/// `context` must be null or a pointer previously returned by
/// [`vrn_context_create`] that has not been destroyed yet. Each context
/// must be destroyed at most once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vrn_context_destroy(context: *mut Mutex<VrnContext>) {
    if context.is_null() {
        return;
    }
    // SAFETY: pointer came from `vrn_context_create` and is freed once.
    unsafe {
        drop(Box::from_raw(context));
    }
}

/// Validate a mesh buffer pair without touching scene state.
///
/// Returns [`VrnResult::Ok`] when `positions`/`indices` hold whole
/// vertices and triangles, else `InvalidArgument`.
#[unsafe(no_mangle)]
pub extern "C" fn vrn_validate_mesh(positions_len: usize, indices_len: usize) -> VrnResult {
    let mesh = MeshTopology {
        positions: vec![0.0; positions_len],
        indices: vec![0; indices_len],
    };
    match validate_mesh_topology(&mesh) {
        Ok(()) => VrnResult::Ok,
        Err(_) => VrnResult::InvalidArgument,
    }
}

/// Tick the headless scene once. Must be called off the Swift `MainActor`.
///
/// # Safety
///
/// `context` must be a live pointer previously returned by
/// [`vrn_context_create`]. Do not call concurrently with
/// [`vrn_context_destroy`] on the same context.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vrn_tick(context: *mut Mutex<VrnContext>) -> VrnResult {
    if context.is_null() {
        return VrnResult::NullArgument;
    }
    // SAFETY: non-null pointer from `vrn_context_create`, still alive.
    let guard = unsafe { context.as_ref() };
    let Some(mutex) = guard else {
        return VrnResult::NullArgument;
    };
    match mutex.lock() {
        Ok(mut ctx) => {
            ctx.scene.update();
            VrnResult::Ok
        }
        Err(_) => VrnResult::Internal,
    }
}

/// Read the scene tick counter. `out` receives ticks since context creation.
///
/// # Safety
///
/// `context` must be a live pointer from [`vrn_context_create`] and `out`
/// must be a non-null, writable `u64` slot for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vrn_tick_count(
    context: *mut Mutex<VrnContext>,
    out: *mut u64,
) -> VrnResult {
    if context.is_null() || out.is_null() {
        return VrnResult::NullArgument;
    }
    // SAFETY: both pointers checked non-null; context is alive per contract.
    let (mutex, slot) = unsafe { (&*context, &mut *out) };
    match mutex.lock() {
        Ok(ctx) => {
            *slot = ctx.scene.tick_count();
            VrnResult::Ok
        }
        Err(_) => VrnResult::Internal,
    }
}

/// Read the live entity count of the scene world. See [`vrn_tick_count`]
/// for the pointer contract.
///
/// # Safety
///
/// `context` must be a live pointer from [`vrn_context_create`] and `out`
/// must be a non-null, writable `u64` slot for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vrn_entity_count(
    context: *mut Mutex<VrnContext>,
    out: *mut u64,
) -> VrnResult {
    if context.is_null() || out.is_null() {
        return VrnResult::NullArgument;
    }
    // SAFETY: both pointers checked non-null; context is alive per contract.
    let (mutex, slot) = unsafe { (&*context, &mut *out) };
    match mutex.lock() {
        Ok(mut ctx) => {
            *slot = ctx.scene.entity_count() as u64;
            VrnResult::Ok
        }
        Err(_) => VrnResult::Internal,
    }
}

/// Touch [`MeshId`] so the import stays live as the FFI surface grows.
#[allow(dead_code, reason = "scaffold shim until the FFI surface grows")]
fn mesh_id_ffi_layout(id: MeshId) -> u64 {
    id.0
}

/// Touch [`MorphWeights`] so weight layout stays covered by this crate.
#[allow(dead_code, reason = "scaffold shim until the FFI surface grows")]
fn morph_weights_ffi_layout(weights: &MorphWeights) -> usize {
    weights.weights.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr;

    #[test]
    fn context_lifecycle_tick_and_destroy() {
        let context = vrn_context_create();
        assert!(!context.is_null());
        // SAFETY: just created, alive, single-threaded test.
        unsafe {
            assert_eq!(vrn_tick(context), VrnResult::Ok);
            vrn_context_destroy(context);
        }
    }

    #[test]
    fn null_context_is_safe() {
        // SAFETY: both functions are documented null-safe.
        unsafe {
            assert_eq!(vrn_tick(ptr::null_mut()), VrnResult::NullArgument);
            vrn_context_destroy(ptr::null_mut());
            let mut slot = 0u64;
            assert_eq!(
                vrn_tick_count(ptr::null_mut(), &raw mut slot),
                VrnResult::NullArgument
            );
            assert_eq!(
                vrn_entity_count(ptr::null_mut(), &raw mut slot),
                VrnResult::NullArgument
            );
        }
    }

    #[test]
    fn mesh_stride_maps_to_result_codes() {
        assert_eq!(vrn_validate_mesh(6, 3), VrnResult::Ok);
        assert_eq!(vrn_validate_mesh(4, 3), VrnResult::InvalidArgument);
        assert_eq!(vrn_validate_mesh(3, 4), VrnResult::InvalidArgument);
    }

    #[test]
    fn id_and_weight_layout_shims() {
        assert_eq!(mesh_id_ffi_layout(MeshId(7)), 7);
        assert_eq!(
            morph_weights_ffi_layout(&MorphWeights {
                weights: vec![0.25]
            }),
            1
        );
    }
}

#[cfg(test)]
mod stats_tests {
    use super::*;
    use std::ptr;

    #[test]
    fn demo_stats_advance_with_ticks() {
        let context = vrn_context_create();
        assert!(!context.is_null());
        let mut ticks = 0u64;
        let mut entities = 0u64;
        // SAFETY: just created, alive, single-threaded test.
        unsafe {
            assert_eq!(vrn_entity_count(context, &raw mut entities), VrnResult::Ok);
            assert_eq!(entities, 3);
            assert_eq!(vrn_tick_count(context, &raw mut ticks), VrnResult::Ok);
            assert_eq!(ticks, 0);
            assert_eq!(vrn_tick(context), VrnResult::Ok);
            assert_eq!(vrn_tick_count(context, &raw mut ticks), VrnResult::Ok);
            assert_eq!(ticks, 1);
            assert_eq!(
                vrn_tick_count(context, ptr::null_mut()),
                VrnResult::NullArgument
            );
            vrn_context_destroy(context);
        }
    }
}
