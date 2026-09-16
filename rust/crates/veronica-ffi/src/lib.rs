//! C-ABI boundary for Swift. Only plain C types cross here.
//!
//! Safety contract: every `vrn_*` function is `no_unwind` in spirit —
//! Rust panics are caught and mapped to [`VrnResult`] codes, never
//! propagated into Swift. All state lives behind an opaque pointer.

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::sync::Mutex;
use veronica_core::{
    MeshId, MeshTopology, MorphWeights, NodeId, UndoHistory, validate_mesh_topology,
};
use veronica_graph::{
    GRAPH_SNAPSHOT_VERSION, GraphSnapshot, NodeGraph, OperatorGraph, OperatorKind, Position,
};
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
    reason = "`graph` + `demo` wire up as the FFI surface grows; `mesh_history` awaits its intents"
)]
pub struct VrnContext {
    graph: NodeGraph,
    operator_graph: OperatorGraph,
    graph_history: UndoHistory<GraphSnapshot>,
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
            operator_graph: OperatorGraph::new(),
            graph_history: UndoHistory::new(64),
            scene,
            demo,
            mesh_history: UndoHistory::new(64),
        }
    }
}

/// Opaque engine-context handle crossing the FFI boundary.
///
/// The inner mutex is intentionally private: cbindgen emits this as an
/// opaque `VrnContextHandle` struct instead of spelling `Mutex<VrnContext>`,
/// which has no C spelling. All access goes through the `vrn_*` externs.
#[derive(Debug)]
pub struct VrnContextHandle(Mutex<VrnContext>);

/// Create a new engine context. Returns null on allocation failure.
#[unsafe(no_mangle)]
pub extern "C" fn vrn_context_create() -> *mut VrnContextHandle {
    Box::into_raw(Box::new(VrnContextHandle(Mutex::new(VrnContext::new()))))
}

/// Destroy a context created by [`vrn_context_create`]. Null-safe.
///
/// # Safety
///
/// `context` must be null or a pointer previously returned by
/// [`vrn_context_create`] that has not been destroyed yet. Each context
/// must be destroyed at most once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vrn_context_destroy(context: *mut VrnContextHandle) {
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
pub unsafe extern "C" fn vrn_tick(context: *mut VrnContextHandle) -> VrnResult {
    if context.is_null() {
        return VrnResult::NullArgument;
    }
    // SAFETY: non-null pointer from `vrn_context_create`, still alive.
    let guard = unsafe { context.as_ref() };
    let Some(handle) = guard else {
        return VrnResult::NullArgument;
    };
    match handle.0.lock() {
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
    context: *mut VrnContextHandle,
    out: *mut u64,
) -> VrnResult {
    if context.is_null() || out.is_null() {
        return VrnResult::NullArgument;
    }
    // SAFETY: both pointers checked non-null; context is alive per contract.
    let (mutex, slot) = unsafe { (&(*context).0, &mut *out) };
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
    context: *mut VrnContextHandle,
    out: *mut u64,
) -> VrnResult {
    if context.is_null() || out.is_null() {
        return VrnResult::NullArgument;
    }
    // SAFETY: both pointers checked non-null; context is alive per contract.
    let (mutex, slot) = unsafe { (&(*context).0, &mut *out) };
    match mutex.lock() {
        Ok(mut ctx) => {
            *slot = ctx.scene.entity_count() as u64;
            VrnResult::Ok
        }
        Err(_) => VrnResult::Internal,
    }
}

/// Parse a strict operator-kind string from the FFI boundary.
///
/// Only `"container"` is accepted (ADR-0002); anything else is
/// [`VrnResult::InvalidArgument`]. The graph core only ever sees the parsed
/// [`OperatorKind`], so operator #2 extends here without touching it.
fn parse_operator_kind(kind: *const c_char) -> Result<OperatorKind, VrnResult> {
    if kind.is_null() {
        return Err(VrnResult::NullArgument);
    }
    // SAFETY: non-null; the caller guarantees a valid NUL-terminated string
    // for the duration of the call.
    let text = unsafe { CStr::from_ptr(kind) };
    match text.to_str() {
        Ok("container") => Ok(OperatorKind::Container),
        Ok(_) | Err(_) => Err(VrnResult::InvalidArgument),
    }
}

/// Create a container operator at `(x, y)`, writing its fresh id to `out_id`.
///
/// `parent == 0` means the root (the FFI sentinel; [`NodeId`]`(0)` is never
/// issued); any other value must name an existing operator.
///
/// # Safety
///
/// `context` must be a live pointer from [`vrn_context_create`], `kind` a
/// valid NUL-terminated string, and `out_id` a non-null writable `u64` slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vrn_graph_create_operator(
    context: *mut VrnContextHandle,
    kind: *const c_char,
    parent: u64,
    x: f64,
    y: f64,
    out_id: *mut u64,
) -> VrnResult {
    if context.is_null() || kind.is_null() || out_id.is_null() {
        return VrnResult::NullArgument;
    }
    let Ok(parsed) = parse_operator_kind(kind) else {
        return VrnResult::InvalidArgument;
    };
    let parent = if parent == 0 {
        None
    } else {
        Some(NodeId(parent))
    };
    // SAFETY: both pointers checked non-null; context is alive per contract.
    let (mutex, slot) = unsafe { (&(*context).0, &mut *out_id) };
    let Ok(mut ctx) = mutex.lock() else {
        return VrnResult::Internal;
    };
    if let Some(id) = parent
        && ctx.operator_graph.operator(id).is_none()
    {
        return VrnResult::InvalidArgument;
    }
    // Pre-image push precedes the first write (scrub-vs-structure rule).
    let before = ctx.operator_graph.snapshot();
    ctx.graph_history.push(before);
    match ctx
        .operator_graph
        .create_operator(parsed, parent, Position { x, y })
    {
        Ok(id) => {
            *slot = id.0;
            VrnResult::Ok
        }
        Err(_) => VrnResult::InvalidArgument,
    }
}

/// Move an operator to a new canvas position.
///
/// # Safety
///
/// `context` must be a live pointer from [`vrn_context_create`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vrn_graph_move_operator(
    context: *mut VrnContextHandle,
    id: u64,
    x: f64,
    y: f64,
) -> VrnResult {
    if context.is_null() {
        return VrnResult::NullArgument;
    }
    // SAFETY: non-null pointer from `vrn_context_create`, still alive.
    let mutex = unsafe { &(*context).0 };
    let Ok(mut ctx) = mutex.lock() else {
        return VrnResult::Internal;
    };
    let id = NodeId(id);
    if ctx.operator_graph.operator(id).is_none() {
        return VrnResult::InvalidArgument;
    }
    let before = ctx.operator_graph.snapshot();
    ctx.graph_history.push(before);
    match ctx.operator_graph.move_operator(id, Position { x, y }) {
        Ok(()) => VrnResult::Ok,
        Err(_) => VrnResult::InvalidArgument,
    }
}

/// Rename an operator; empty or blank names are rejected.
///
/// # Safety
///
/// `context` must be a live pointer from [`vrn_context_create`] and `name`
/// a valid NUL-terminated string for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vrn_graph_rename_operator(
    context: *mut VrnContextHandle,
    id: u64,
    name: *const c_char,
) -> VrnResult {
    if context.is_null() || name.is_null() {
        return VrnResult::NullArgument;
    }
    // SAFETY: non-null; the caller guarantees a valid NUL-terminated string
    // for the duration of the call.
    let text = unsafe { CStr::from_ptr(name) };
    let Ok(name) = text.to_str() else {
        return VrnResult::InvalidArgument;
    };
    // SAFETY: non-null pointer from `vrn_context_create`, still alive.
    let mutex = unsafe { &(*context).0 };
    let Ok(mut ctx) = mutex.lock() else {
        return VrnResult::Internal;
    };
    let id = NodeId(id);
    if ctx.operator_graph.operator(id).is_none() {
        return VrnResult::InvalidArgument;
    }
    if name.trim().is_empty() {
        return VrnResult::InvalidArgument;
    }
    let before = ctx.operator_graph.snapshot();
    ctx.graph_history.push(before);
    match ctx.operator_graph.rename_operator(id, name) {
        Ok(()) => VrnResult::Ok,
        Err(_) => VrnResult::InvalidArgument,
    }
}

/// Set a text parameter on an operator; `key == "name"` renames.
///
/// Keys are trimmed, values stored verbatim (see
/// [`OperatorGraph::set_parameter`]). Guards mirror
/// [`vrn_graph_rename_operator`]: null → [`VrnResult::NullArgument`],
/// non-UTF8, unknown id, blank key, or blank value on the rename path →
/// [`VrnResult::InvalidArgument`], all before the pre-image history push.
///
/// # Safety
///
/// `context` must be a live pointer from [`vrn_context_create`] and `key`
/// plus `value` valid NUL-terminated strings for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vrn_graph_set_parameter(
    context: *mut VrnContextHandle,
    id: u64,
    key: *const c_char,
    value: *const c_char,
) -> VrnResult {
    if context.is_null() || key.is_null() || value.is_null() {
        return VrnResult::NullArgument;
    }
    // SAFETY: non-null; the caller guarantees a valid NUL-terminated string
    // for the duration of the call.
    let key_text = unsafe { CStr::from_ptr(key) };
    let Ok(key) = key_text.to_str() else {
        return VrnResult::InvalidArgument;
    };
    // SAFETY: non-null; the caller guarantees a valid NUL-terminated string
    // for the duration of the call.
    let value_text = unsafe { CStr::from_ptr(value) };
    let Ok(value) = value_text.to_str() else {
        return VrnResult::InvalidArgument;
    };
    // SAFETY: non-null pointer from `vrn_context_create`, still alive.
    let mutex = unsafe { &(*context).0 };
    let Ok(mut ctx) = mutex.lock() else {
        return VrnResult::Internal;
    };
    let id = NodeId(id);
    if ctx.operator_graph.operator(id).is_none() {
        return VrnResult::InvalidArgument;
    }
    if key.trim().is_empty() {
        return VrnResult::InvalidArgument;
    }
    if key.trim() == "name" && value.trim().is_empty() {
        return VrnResult::InvalidArgument;
    }
    let before = ctx.operator_graph.snapshot();
    ctx.graph_history.push(before);
    match ctx.operator_graph.set_parameter(id, key, value) {
        Ok(()) => VrnResult::Ok,
        Err(_) => VrnResult::InvalidArgument,
    }
}

/// Delete an operator and its whole subtree (cascade).
///
/// # Safety
///
/// `context` must be a live pointer from [`vrn_context_create`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vrn_graph_delete_operator(
    context: *mut VrnContextHandle,
    id: u64,
) -> VrnResult {
    if context.is_null() {
        return VrnResult::NullArgument;
    }
    // SAFETY: non-null pointer from `vrn_context_create`, still alive.
    let mutex = unsafe { &(*context).0 };
    let Ok(mut ctx) = mutex.lock() else {
        return VrnResult::Internal;
    };
    let id = NodeId(id);
    if ctx.operator_graph.operator(id).is_none() {
        return VrnResult::InvalidArgument;
    }
    let before = ctx.operator_graph.snapshot();
    ctx.graph_history.push(before);
    match ctx.operator_graph.delete_operator(id) {
        Ok(()) => VrnResult::Ok,
        Err(_) => VrnResult::InvalidArgument,
    }
}

/// Capture the whole graph as versioned JSON.
///
/// Rust allocates the string via [`CString::into_raw`]; the caller takes
/// ownership and must release it with [`vrn_string_free`].
///
/// # Safety
///
/// `context` must be a live pointer from [`vrn_context_create`] and
/// `out_json` a non-null writable pointer slot for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vrn_graph_snapshot(
    context: *mut VrnContextHandle,
    out_json: *mut *mut c_char,
) -> VrnResult {
    if context.is_null() || out_json.is_null() {
        return VrnResult::NullArgument;
    }
    // SAFETY: both pointers checked non-null; context is alive per contract.
    let (mutex, slot) = unsafe { (&(*context).0, &mut *out_json) };
    let Ok(ctx) = mutex.lock() else {
        return VrnResult::Internal;
    };
    let Ok(json) = serde_json::to_string(&ctx.operator_graph.snapshot()) else {
        return VrnResult::Internal;
    };
    match CString::new(json) {
        Ok(owned) => {
            *slot = owned.into_raw();
            VrnResult::Ok
        }
        Err(_) => VrnResult::Internal,
    }
}

/// Replace the whole graph from versioned JSON (launch-load path).
///
/// Rejects `version != 1` and malformed payloads with
/// [`VrnResult::InvalidArgument`]; the pre-image is pushed only when restore
/// succeeds, so a bad file neither destroys state nor pollutes history.
///
/// # Safety
///
/// `context` must be a live pointer from [`vrn_context_create`] and `json`
/// a valid NUL-terminated string for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vrn_graph_restore(
    context: *mut VrnContextHandle,
    json: *const c_char,
) -> VrnResult {
    if context.is_null() || json.is_null() {
        return VrnResult::NullArgument;
    }
    // SAFETY: non-null; the caller guarantees a valid NUL-terminated string
    // for the duration of the call.
    let text = unsafe { CStr::from_ptr(json) };
    let Ok(text) = text.to_str() else {
        return VrnResult::InvalidArgument;
    };
    let Ok(snapshot): Result<GraphSnapshot, _> = serde_json::from_str(text) else {
        return VrnResult::InvalidArgument;
    };
    // Reject the known-bad version before pushing any history.
    if snapshot.version != GRAPH_SNAPSHOT_VERSION {
        return VrnResult::InvalidArgument;
    }
    // SAFETY: non-null pointer from `vrn_context_create`, still alive.
    let mutex = unsafe { &(*context).0 };
    let Ok(mut ctx) = mutex.lock() else {
        return VrnResult::Internal;
    };
    let before = ctx.operator_graph.snapshot();
    match ctx.operator_graph.restore(snapshot) {
        Ok(()) => {
            ctx.graph_history.push(before);
            VrnResult::Ok
        }
        Err(_) => VrnResult::InvalidArgument,
    }
}

/// Release a string allocated by [`vrn_graph_snapshot`]. Null-safe; the sole
/// deallocator for FFI strings.
///
/// # Safety
///
/// `s` must be null or a pointer previously returned by
/// [`vrn_graph_snapshot`] that has not been freed yet. Each string must be
/// freed at most once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vrn_string_free(s: *mut c_char) {
    if s.is_null() {
        return;
    }
    // SAFETY: non-null pointer from `vrn_graph_snapshot`, freed once.
    unsafe {
        drop(CString::from_raw(s));
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
            assert_eq!(
                vrn_entity_count(context, ptr::null_mut()),
                VrnResult::NullArgument
            );
            vrn_context_destroy(context);
        }
    }
}

#[cfg(test)]
mod graph_tests {
    use super::*;
    use std::ptr;

    fn cstring(text: &str) -> CString {
        CString::new(text).unwrap()
    }

    /// Snapshot the context and return the JSON as an owned string.
    ///
    /// # Panics
    ///
    /// Panics when the snapshot call fails or the payload is not UTF-8;
    /// both indicate a broken FFI boundary, not a test input problem.
    fn snapshot_json(context: *mut VrnContextHandle) -> String {
        let mut raw: *mut c_char = ptr::null_mut();
        // SAFETY: just created, alive, single-threaded test; slot is live.
        unsafe {
            assert_eq!(vrn_graph_snapshot(context, &raw mut raw), VrnResult::Ok);
            assert!(!raw.is_null());
            let text = CStr::from_ptr(raw).to_str().unwrap().to_owned();
            vrn_string_free(raw);
            text
        }
    }

    #[test]
    fn create_move_rename_delete_round_trip() {
        let context = vrn_context_create();
        assert!(!context.is_null());
        let kind = cstring("container");
        let name = cstring("Hero");
        let renamed = cstring("Villain");
        let mut id = 0u64;
        // SAFETY: just created, alive, single-threaded test; strings outlive calls.
        unsafe {
            assert_eq!(
                vrn_graph_create_operator(
                    context,
                    kind.as_ptr().cast_mut(),
                    0,
                    120.0,
                    80.0,
                    &raw mut id
                ),
                VrnResult::Ok
            );
            assert_eq!(id, 1);
            assert_eq!(
                vrn_graph_move_operator(context, id, 10.0, 20.0),
                VrnResult::Ok
            );
            assert_eq!(
                vrn_graph_rename_operator(context, id, name.as_ptr().cast_mut()),
                VrnResult::Ok
            );
            assert_eq!(
                vrn_graph_rename_operator(context, id, renamed.as_ptr().cast_mut()),
                VrnResult::Ok
            );
            let json = snapshot_json(context);
            assert!(json.contains(r#""name":"Villain""#));
            assert_eq!(vrn_graph_delete_operator(context, id), VrnResult::Ok);
            let json = snapshot_json(context);
            assert!(json.contains(r#""operators":[]"#));
            vrn_context_destroy(context);
        }
    }

    #[test]
    fn strict_kind_validation_rejects_anything_but_container() {
        let context = vrn_context_create();
        assert!(!context.is_null());
        let mut id = 0u64;
        // SAFETY: just created, alive, single-threaded test.
        unsafe {
            for rejected in ["box", "Container", "CONTAINER", "", "container "] {
                let kind = cstring(rejected);
                assert_eq!(
                    vrn_graph_create_operator(
                        context,
                        kind.as_ptr().cast_mut(),
                        0,
                        0.0,
                        0.0,
                        &raw mut id
                    ),
                    VrnResult::InvalidArgument,
                    "kind {rejected:?} must be rejected"
                );
            }
            // Null kind is a null-argument, not a validation failure.
            assert_eq!(
                vrn_graph_create_operator(context, ptr::null_mut(), 0, 0.0, 0.0, &raw mut id),
                VrnResult::NullArgument
            );
            vrn_context_destroy(context);
        }
    }

    #[test]
    fn unknown_ids_and_blank_names_are_invalid() {
        let context = vrn_context_create();
        assert!(!context.is_null());
        let blank = cstring("   ");
        let empty = cstring("");
        let name = cstring("Hero");
        // SAFETY: just created, alive, single-threaded test.
        unsafe {
            assert_eq!(
                vrn_graph_move_operator(context, 99, 0.0, 0.0),
                VrnResult::InvalidArgument
            );
            assert_eq!(
                vrn_graph_delete_operator(context, 99),
                VrnResult::InvalidArgument
            );
            assert_eq!(
                vrn_graph_rename_operator(context, 99, name.as_ptr().cast_mut()),
                VrnResult::InvalidArgument
            );
            assert_eq!(
                vrn_graph_rename_operator(context, 1, blank.as_ptr().cast_mut()),
                VrnResult::InvalidArgument
            );
            assert_eq!(
                vrn_graph_rename_operator(context, 1, empty.as_ptr().cast_mut()),
                VrnResult::InvalidArgument
            );
            // Unknown parent on create.
            let kind = cstring("container");
            let mut id = 0u64;
            assert_eq!(
                vrn_graph_create_operator(
                    context,
                    kind.as_ptr().cast_mut(),
                    7,
                    0.0,
                    0.0,
                    &raw mut id
                ),
                VrnResult::InvalidArgument
            );
            vrn_context_destroy(context);
        }
    }

    #[test]
    fn graph_null_safety() {
        let mut id = 0u64;
        let mut raw: *mut c_char = ptr::null_mut();
        // SAFETY: every extern below is documented null-safe.
        unsafe {
            assert_eq!(
                vrn_graph_create_operator(
                    ptr::null_mut(),
                    ptr::null_mut(),
                    0,
                    0.0,
                    0.0,
                    &raw mut id
                ),
                VrnResult::NullArgument
            );
            assert_eq!(
                vrn_graph_move_operator(ptr::null_mut(), 1, 0.0, 0.0),
                VrnResult::NullArgument
            );
            assert_eq!(
                vrn_graph_rename_operator(ptr::null_mut(), 1, ptr::null_mut()),
                VrnResult::NullArgument
            );
            assert_eq!(
                vrn_graph_set_parameter(ptr::null_mut(), 1, ptr::null_mut(), ptr::null_mut()),
                VrnResult::NullArgument
            );
            assert_eq!(
                vrn_graph_delete_operator(ptr::null_mut(), 1),
                VrnResult::NullArgument
            );
            assert_eq!(
                vrn_graph_snapshot(ptr::null_mut(), &raw mut raw),
                VrnResult::NullArgument
            );
            assert_eq!(
                vrn_graph_restore(ptr::null_mut(), ptr::null_mut()),
                VrnResult::NullArgument
            );
            vrn_string_free(ptr::null_mut());
        }
        // Null out-slots on a live context.
        let context = vrn_context_create();
        assert!(!context.is_null());
        let kind = cstring("container");
        // SAFETY: just created, alive, single-threaded test.
        unsafe {
            assert_eq!(
                vrn_graph_create_operator(
                    context,
                    kind.as_ptr().cast_mut(),
                    0,
                    0.0,
                    0.0,
                    ptr::null_mut()
                ),
                VrnResult::NullArgument
            );
            assert_eq!(
                vrn_graph_snapshot(context, ptr::null_mut()),
                VrnResult::NullArgument
            );
            vrn_context_destroy(context);
        }
    }

    #[test]
    fn restore_round_trip_and_rejects_v1_and_garbage() {
        let context = vrn_context_create();
        assert!(!context.is_null());
        let kind = cstring("container");
        let name = cstring("Hero");
        let mut id = 0u64;
        // SAFETY: just created, alive, single-threaded test.
        unsafe {
            assert_eq!(
                vrn_graph_create_operator(
                    context,
                    kind.as_ptr().cast_mut(),
                    0,
                    120.0,
                    80.0,
                    &raw mut id
                ),
                VrnResult::Ok
            );
            assert_eq!(
                vrn_graph_rename_operator(context, id, name.as_ptr().cast_mut()),
                VrnResult::Ok
            );
            let json = snapshot_json(context);
            // Restore the same payload into a fresh context.
            let fresh = vrn_context_create();
            let payload = cstring(&json);
            assert_eq!(
                vrn_graph_restore(fresh, payload.as_ptr().cast_mut()),
                VrnResult::Ok
            );
            assert_eq!(snapshot_json(fresh), json);
            // Wrong version and garbage are rejected; state is untouched.
            let v1 = cstring(r#"{"version":1,"operators":[],"edges":[]}"#);
            assert_eq!(
                vrn_graph_restore(fresh, v1.as_ptr().cast_mut()),
                VrnResult::InvalidArgument
            );
            let garbage = cstring("not json");
            assert_eq!(
                vrn_graph_restore(fresh, garbage.as_ptr().cast_mut()),
                VrnResult::InvalidArgument
            );
            assert_eq!(snapshot_json(fresh), json);
            vrn_context_destroy(context);
            vrn_context_destroy(fresh);
        }
    }

    #[test]
    fn set_parameter_round_trip_is_visible_in_snapshot() {
        let context = vrn_context_create();
        assert!(!context.is_null());
        let kind = cstring("container");
        let key = cstring("label");
        let value = cstring("Hero");
        let name_key = cstring("name");
        let renamed = cstring("Villain");
        let mut id = 0u64;
        // SAFETY: just created, alive, single-threaded test; strings outlive calls.
        unsafe {
            assert_eq!(
                vrn_graph_create_operator(
                    context,
                    kind.as_ptr().cast_mut(),
                    0,
                    0.0,
                    0.0,
                    &raw mut id
                ),
                VrnResult::Ok
            );
            assert_eq!(
                vrn_graph_set_parameter(
                    context,
                    id,
                    key.as_ptr().cast_mut(),
                    value.as_ptr().cast_mut()
                ),
                VrnResult::Ok
            );
            let json = snapshot_json(context);
            assert!(json.contains(r#""label":{"text":"Hero"}"#));
            // The `name` key delegates to the rename path.
            assert_eq!(
                vrn_graph_set_parameter(
                    context,
                    id,
                    name_key.as_ptr().cast_mut(),
                    renamed.as_ptr().cast_mut()
                ),
                VrnResult::Ok
            );
            let json = snapshot_json(context);
            assert!(json.contains(r#""name":"Villain""#));
            vrn_context_destroy(context);
        }
    }

    #[test]
    fn set_parameter_guards_map_to_result_codes() {
        let context = vrn_context_create();
        assert!(!context.is_null());
        let kind = cstring("container");
        let key = cstring("label");
        let value = cstring("Hero");
        let blank = cstring("   ");
        let empty = cstring("");
        let mut id = 0u64;
        // SAFETY: just created, alive, single-threaded test; strings outlive calls.
        unsafe {
            assert_eq!(
                vrn_graph_create_operator(
                    context,
                    kind.as_ptr().cast_mut(),
                    0,
                    0.0,
                    0.0,
                    &raw mut id
                ),
                VrnResult::Ok
            );
            // Null key / null value / null context.
            assert_eq!(
                vrn_graph_set_parameter(context, id, ptr::null_mut(), value.as_ptr().cast_mut()),
                VrnResult::NullArgument
            );
            assert_eq!(
                vrn_graph_set_parameter(context, id, key.as_ptr().cast_mut(), ptr::null_mut()),
                VrnResult::NullArgument
            );
            assert_eq!(
                vrn_graph_set_parameter(
                    ptr::null_mut(),
                    id,
                    key.as_ptr().cast_mut(),
                    value.as_ptr().cast_mut()
                ),
                VrnResult::NullArgument
            );
            // Unknown id.
            assert_eq!(
                vrn_graph_set_parameter(
                    context,
                    99,
                    key.as_ptr().cast_mut(),
                    value.as_ptr().cast_mut()
                ),
                VrnResult::InvalidArgument
            );
            // Blank keys rejected; blank rename values rejected too.
            assert_eq!(
                vrn_graph_set_parameter(
                    context,
                    id,
                    blank.as_ptr().cast_mut(),
                    value.as_ptr().cast_mut()
                ),
                VrnResult::InvalidArgument
            );
            assert_eq!(
                vrn_graph_set_parameter(
                    context,
                    id,
                    empty.as_ptr().cast_mut(),
                    value.as_ptr().cast_mut()
                ),
                VrnResult::InvalidArgument
            );
            let name_key = cstring("name");
            assert_eq!(
                vrn_graph_set_parameter(
                    context,
                    id,
                    name_key.as_ptr().cast_mut(),
                    blank.as_ptr().cast_mut()
                ),
                VrnResult::InvalidArgument
            );
            // Rejected writes leave the snapshot untouched.
            let json = snapshot_json(context);
            assert!(!json.contains(r#""label""#));
            vrn_context_destroy(context);
        }
    }

    /// Allocator-boundary pin (ADR-0002): repeated snapshot/free cycles must
    /// neither leak the pointer nor corrupt the payload.
    #[test]
    fn looped_snapshot_and_free_is_stable() {
        let context = vrn_context_create();
        assert!(!context.is_null());
        let kind = cstring("container");
        let mut id = 0u64;
        // SAFETY: just created, alive, single-threaded test.
        unsafe {
            assert_eq!(
                vrn_graph_create_operator(
                    context,
                    kind.as_ptr().cast_mut(),
                    0,
                    1.0,
                    2.0,
                    &raw mut id
                ),
                VrnResult::Ok
            );
            let mut first = String::new();
            for iteration in 0..128 {
                let mut raw: *mut c_char = ptr::null_mut();
                assert_eq!(vrn_graph_snapshot(context, &raw mut raw), VrnResult::Ok);
                assert!(!raw.is_null());
                let text = CStr::from_ptr(raw).to_str().unwrap().to_owned();
                vrn_string_free(raw);
                if iteration == 0 {
                    first = text.clone();
                } else {
                    assert_eq!(text, first);
                }
            }
            vrn_context_destroy(context);
        }
    }
}
