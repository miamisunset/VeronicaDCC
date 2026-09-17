//! C-ABI boundary for Swift. Only plain C types cross here.
//!
//! Safety contract: every `vrn_*` function is `no_unwind` in spirit —
//! Rust panics are caught and mapped to [`VrnResult`] codes, never
//! propagated into Swift. All state lives behind an opaque pointer.

use std::ffi::{CStr, CString, c_void};
use std::os::raw::c_char;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use veronica_core::{
    MeshId, MeshTopology, MorphWeights, NodeId, UndoHistory, validate_mesh_topology,
};
use veronica_graph::{
    GRAPH_SNAPSHOT_VERSION, GraphSnapshot, NodeGraph, OperatorGraph, OperatorKind, Position,
};
use veronica_scene::{MAX_VIEWPORT_EDGE, SceneIds, SceneWorld};

mod surface;

use surface::FrameSurface;

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
    reason = "`graph` + `base` wire up as the FFI surface grows; `mesh_history` awaits its intents"
)]
pub struct VrnContext {
    graph: NodeGraph,
    operator_graph: OperatorGraph,
    graph_history: UndoHistory<GraphSnapshot>,
    scene: SceneWorld,
    base: SceneIds,
    /// Graph epoch last cooked into the scene. [`vrn_tick`] recooks via
    /// [`SceneWorld::recook_graph`] whenever the live graph epoch differs,
    /// then records it here — so a quiet tick never respawns, and a failed
    /// cook stays dirty and retries on the next tick.
    last_cooked_epoch: u64,
    mesh_history: UndoHistory<MeshTopology>,
    /// Ping-pong `IOSurface` pair. Each tick uploads into the back slot and
    /// publishes the front slot, so Swift's `draw` blit never reads a
    /// surface Rust is concurrently writing (no tear line / partial frame).
    /// Created lazily on the first tick; both slots are recreated eagerly
    /// whenever the frame extents drift, so no transient size mismatch is
    /// ever published. Swift re-adopts on every address change, so
    /// alternating handles are transparent to it.
    surfaces: [Option<FrameSurface>; 2],
    /// Index into [`VrnContext::surfaces`] of the published front buffer.
    front: usize,
    /// Viewport size intent from [`vrn_viewport_set_size`], applied ahead of
    /// the next [`vrn_tick`]'s scene update (before the render, so the tick's
    /// own frame already matches the new surface).
    pending_size: Option<(u32, u32)>,
    /// Stage splits of the most recent fully published tick
    /// (see [`TickTimings`]). Zeros until the first tick publishes; a
    /// failed tick leaves the previous values, never partial ones.
    last_timings: TickTimings,
}

/// Per-tick stage splits for the frame-publish pipeline (issue #32).
///
/// Attribution for the display-scaling slowdown: `update` is the Bevy
/// schedule (ECS + GPU render submission), `readback` is the
/// texture-to-buffer copy plus the synchronous map (`poll`
/// stall included), and `upload` is the row-stride memcpy into the back
/// `IOSurface`. Surface recreation on extent drift is excluded — it fires
/// only on re-target ticks, so steady-state cells of the repro matrix are
/// unaffected. Read via [`vrn_tick_timings`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TickTimings {
    /// Whole microseconds. The unit lives here, once, instead of on
    /// every field name (`clippy::struct_field_names`).
    update: u64,
    /// Whole microseconds.
    readback: u64,
    /// Whole microseconds.
    upload: u64,
}

impl TickTimings {
    /// Bevy schedule time (`SceneWorld::update`) in whole microseconds.
    #[must_use]
    pub fn update_us(&self) -> u64 {
        self.update
    }

    /// GPU readback time (`SceneWorld::render_frame`) in whole microseconds.
    #[must_use]
    pub fn readback_us(&self) -> u64 {
        self.readback
    }

    /// `IOSurface` upload time (`FrameSurface::upload`) in whole microseconds.
    #[must_use]
    pub fn upload_us(&self) -> u64 {
        self.upload
    }
}

/// Whole microseconds in `elapsed`, saturating on absurd magnitudes
/// (a tick stage can never realistically approach `u64::MAX` µs).
fn micros_saturating(elapsed: Duration) -> u64 {
    u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX)
}

impl VrnContext {
    fn new() -> Self {
        let mut scene = SceneWorld::new_headless();
        let base = scene.spawn_base_scene();
        let operator_graph = OperatorGraph::new();
        let last_cooked_epoch = operator_graph.epoch();
        Self {
            graph: NodeGraph::new(),
            operator_graph,
            graph_history: UndoHistory::new(64),
            scene,
            base,
            last_cooked_epoch,
            mesh_history: UndoHistory::new(64),
            surfaces: [None, None],
            front: 0,
            pending_size: None,
            last_timings: TickTimings::default(),
        }
    }

    /// Apply the pending viewport intent, if any, ahead of a tick.
    ///
    /// Runs before [`SceneWorld::update`] so the tick itself uploads the new
    /// target to the GPU: the same tick's [`VrnContext::publish_frame`] then
    /// reads back the new size instead of hitting [`SceneError::NoGpuImage`].
    /// The intent is pre-validated by [`vrn_viewport_set_size`], so a failure
    /// here is an internal desync, never a caller error.
    fn apply_pending_size(&mut self) -> VrnResult {
        if let Some((width, height)) = self.pending_size.take()
            && self.scene.set_viewport_size(width, height).is_err()
        {
            return VrnResult::Internal;
        }
        VrnResult::Ok
    }

    /// Render the current scene state into the back buffer, then publish it
    /// as the new front. Creates both surfaces on the first call and
    /// recreates both eagerly when the frame extents drift, so the
    /// published handle always matches the current viewport size. Buffers
    /// are reused in place across ticks — never reallocated per tick.
    /// Returns `Internal` when the framework refuses a surface or its
    /// lock, or when the GPU frame readback fails.
    fn publish_frame(&mut self) -> VrnResult {
        let readback_start = Instant::now();
        let frame_result = self.scene.render_frame();
        let readback_us = micros_saturating(readback_start.elapsed());
        let Ok(frame) = frame_result else {
            return VrnResult::Internal;
        };
        let extents_drifted = match self.surfaces.iter().flatten().next() {
            Some(front) => front.width() != frame.width() || front.height() != frame.height(),
            None => true,
        };
        if extents_drifted {
            // Both old surfaces drop here (`CFRelease` in
            // `FrameSurface::drop`) and Swift re-adopts a fresh handle via
            // its address-change path in `adopt(frame:)` — no Swift change
            // needed beyond the re-wrap.
            match (
                FrameSurface::new(frame.width(), frame.height()),
                FrameSurface::new(frame.width(), frame.height()),
            ) {
                (Ok(even), Ok(odd)) => self.surfaces = [Some(even), Some(odd)],
                _ => return VrnResult::Internal,
            }
        }
        let back = 1 - self.front;
        let upload_start = Instant::now();
        let uploaded = match self.surfaces[back].as_ref() {
            Some(surface) => surface.upload(frame.pixels()).is_ok(),
            None => false,
        };
        let upload_us = micros_saturating(upload_start.elapsed());
        if uploaded {
            self.front = back;
            // Only fully published ticks update the splits: a failed tick
            // leaves the previous values rather than partial ones.
            self.last_timings.readback = readback_us;
            self.last_timings.upload = upload_us;
            VrnResult::Ok
        } else {
            VrnResult::Internal
        }
    }

    /// Borrowed handle of the published front buffer, plus its extents.
    ///
    /// Returns `None` until the first tick publishes a frame.
    #[must_use]
    fn front_surface(&self) -> Option<(*mut c_void, u32, u32)> {
        self.surfaces[self.front]
            .as_ref()
            .map(|surface| (surface.handle(), surface.width(), surface.height()))
    }

    /// Stage splits of the most recent fully published tick.
    ///
    /// Zeros until the first tick publishes; see [`TickTimings`].
    #[must_use]
    fn last_tick_timings(&self) -> TickTimings {
        self.last_timings
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

/// Tick the headless scene once, publishing the new frame. Must be called
/// off the Swift `MainActor`.
///
/// Recooks first when the operator graph moved since the last cook
/// (clear-and-respawn through [`SceneWorld::recook_graph`]); a failed cook
/// reports [`VrnResult::Internal`] without publishing, and the epoch stays
/// dirty so the next tick retries.
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
            let size_result = ctx.apply_pending_size();
            if size_result != VrnResult::Ok {
                return size_result;
            }
            if ctx.operator_graph.epoch() != ctx.last_cooked_epoch {
                // Disjoint field borrows: the recook reads the graph while
                // mutating the scene, never the whole context at once (the
                // guard deref forbids mixing those borrows in one call).
                let recook_failed = {
                    let VrnContext {
                        scene,
                        operator_graph,
                        ..
                    } = &mut *ctx;
                    scene.recook_graph(operator_graph).is_err()
                };
                if recook_failed {
                    return VrnResult::Internal;
                }
            }
            ctx.last_cooked_epoch = ctx.operator_graph.epoch();
            let update_start = Instant::now();
            ctx.scene.update();
            let update_us = micros_saturating(update_start.elapsed());
            let publish_result = ctx.publish_frame();
            if publish_result == VrnResult::Ok {
                ctx.last_timings.update = update_us;
            }
            publish_result
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

/// Read the latest published frame: borrowed `IOSurface` handle plus extents.
///
/// The handle is owned by the context (valid until [`vrn_context_destroy`])
/// and must not be released by Swift. Swift wraps it in an `MTLTexture`
/// with no copies on the present path. The context ping-pongs two buffers,
/// so the handle may alternate on every tick; Swift's address-change path
/// re-wraps transparently, and the previously published handle stays valid
/// (it is the next tick's write target, never freed mid-present).
///
/// Returns [`VrnResult::InvalidArgument`] when no tick has published yet.
///
/// # Safety
///
/// `context` must be a live pointer from [`vrn_context_create`]; `out_surface`,
/// `out_width`, and `out_height` must be non-null writable slots for the
/// duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vrn_frame_surface(
    context: *mut VrnContextHandle,
    out_surface: *mut *mut c_void,
    out_width: *mut u32,
    out_height: *mut u32,
) -> VrnResult {
    if context.is_null() || out_surface.is_null() || out_width.is_null() || out_height.is_null() {
        return VrnResult::NullArgument;
    }
    // SAFETY: all pointers checked non-null; context is alive per contract.
    let (mutex, surface_slot, width_slot, height_slot) = unsafe {
        (
            &(*context).0,
            &mut *out_surface,
            &mut *out_width,
            &mut *out_height,
        )
    };
    match mutex.lock() {
        Ok(ctx) => match ctx.front_surface() {
            Some((handle, width, height)) => {
                *surface_slot = handle;
                *width_slot = width;
                *height_slot = height;
                VrnResult::Ok
            }
            None => VrnResult::InvalidArgument,
        },
        Err(_) => VrnResult::Internal,
    }
}

/// Read the stage splits of the most recent fully published tick
/// (see [`TickTimings`]): Bevy-schedule, GPU-readback, and surface-upload
/// microseconds since context creation's tick loop began.
///
/// Debug/attribution getter for issue #32 (which stage owns the tick under
/// display scaling). All three slots read zero until the first tick
/// publishes; a failed tick leaves the previous values.
///
/// # Safety
///
/// `context` must be a live pointer from [`vrn_context_create`];
/// `out_update_us`, `out_readback_us`, and `out_upload_us` must be non-null
/// writable slots for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vrn_tick_timings(
    context: *mut VrnContextHandle,
    out_update_us: *mut u64,
    out_readback_us: *mut u64,
    out_upload_us: *mut u64,
) -> VrnResult {
    if context.is_null()
        || out_update_us.is_null()
        || out_readback_us.is_null()
        || out_upload_us.is_null()
    {
        return VrnResult::NullArgument;
    }
    // SAFETY: all pointers checked non-null; context is alive per contract.
    let (mutex, update_slot, readback_slot, upload_slot) = unsafe {
        (
            &(*context).0,
            &mut *out_update_us,
            &mut *out_readback_us,
            &mut *out_upload_us,
        )
    };
    match mutex.lock() {
        Ok(ctx) => {
            let timings = ctx.last_tick_timings();
            *update_slot = timings.update_us();
            *readback_slot = timings.readback_us();
            *upload_slot = timings.upload_us();
            VrnResult::Ok
        }
        Err(_) => VrnResult::Internal,
    }
}

/// Request a viewport re-target to `width` x `height` backing pixels.
///
/// Stores the size intent; the next [`vrn_tick`] applies it before rendering
/// and recreates the `IOSurface` when the extents change. Swift re-adopts the
/// new handle via its existing address-change path. Coalescing is natural:
/// repeated calls before a tick keep only the latest intent.
///
/// Returns [`VrnResult::InvalidArgument`] when either extent is zero or the
/// longest edge exceeds `MAX_VIEWPORT_EDGE` (2048).
///
/// # Safety
///
/// `context` must be a live pointer from [`vrn_context_create`]. Do not call
/// concurrently with [`vrn_context_destroy`] on the same context.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vrn_viewport_set_size(
    context: *mut VrnContextHandle,
    width: u32,
    height: u32,
) -> VrnResult {
    if context.is_null() {
        return VrnResult::NullArgument;
    }
    if width == 0 || height == 0 || width.max(height) > MAX_VIEWPORT_EDGE {
        return VrnResult::InvalidArgument;
    }
    // SAFETY: non-null pointer from `vrn_context_create`, still alive.
    let mutex = unsafe { &(*context).0 };
    match mutex.lock() {
        Ok(mut ctx) => {
            ctx.pending_size = Some((width, height));
            VrnResult::Ok
        }
        Err(_) => VrnResult::Internal,
    }
}

/// Orbit the viewport camera around its pivot by a drag delta in pixels.
///
/// Turntable semantics (locked Y-up, ±89.9° elevation clamp) live in
/// `veronica-scene`; this only forwards the coarse op. Swift maps the
/// gesture (LMB-drag, Option+two-finger-drag) to these deltas.
///
/// # Safety
///
/// `context` must be a live pointer from [`vrn_context_create`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vrn_viewport_orbit(
    context: *mut VrnContextHandle,
    horizontal_px: f32,
    vertical_px: f32,
) -> VrnResult {
    if context.is_null() {
        return VrnResult::NullArgument;
    }
    // SAFETY: non-null pointer from `vrn_context_create`, still alive.
    let mutex = unsafe { &(*context).0 };
    let Ok(mut ctx) = mutex.lock() else {
        return VrnResult::Internal;
    };
    match ctx.scene.orbit_camera(horizontal_px, vertical_px) {
        Ok(()) => VrnResult::Ok,
        Err(_) => VrnResult::Internal,
    }
}

/// Pan the viewport camera and its pivot rigidly by a drag delta in pixels.
///
/// Swift maps the gesture (MMB-drag, Command+LMB, two-finger-drag) to these
/// deltas; the step scales with pivot distance inside `veronica-scene`.
///
/// # Safety
///
/// `context` must be a live pointer from [`vrn_context_create`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vrn_viewport_pan(
    context: *mut VrnContextHandle,
    horizontal_px: f32,
    vertical_px: f32,
) -> VrnResult {
    if context.is_null() {
        return VrnResult::NullArgument;
    }
    // SAFETY: non-null pointer from `vrn_context_create`, still alive.
    let mutex = unsafe { &(*context).0 };
    let Ok(mut ctx) = mutex.lock() else {
        return VrnResult::Internal;
    };
    match ctx.scene.pan_camera(horizontal_px, vertical_px) {
        Ok(()) => VrnResult::Ok,
        Err(_) => VrnResult::Internal,
    }
}

/// Dolly toward the cursor by `log_factor` (positive zooms in) at
/// `cursor_ndc` (normalized device coordinates, x/y in [-1, 1]).
///
/// Swift maps the gesture (wheel, pinch, RMB-drag, Option+LMB) to the factor
/// and passes the cursor position through; distance scaling, clamping, and
/// the pivot pull live in `veronica-scene`.
///
/// # Safety
///
/// `context` must be a live pointer from [`vrn_context_create`].
#[allow(
    clippy::similar_names,
    reason = "the x/y NDC pair is conventional; renaming would hurt the C signature"
)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vrn_viewport_dolly(
    context: *mut VrnContextHandle,
    log_factor: f32,
    cursor_x_ndc: f32,
    cursor_y_ndc: f32,
) -> VrnResult {
    if context.is_null() {
        return VrnResult::NullArgument;
    }
    // SAFETY: non-null pointer from `vrn_context_create`, still alive.
    let mutex = unsafe { &(*context).0 };
    let Ok(mut ctx) = mutex.lock() else {
        return VrnResult::Internal;
    };
    match ctx
        .scene
        .dolly_camera(log_factor, (cursor_x_ndc, cursor_y_ndc))
    {
        Ok(()) => VrnResult::Ok,
        Err(_) => VrnResult::Internal,
    }
}

/// Frame the whole scene: pivot to the bounds center, distance to fit.
///
/// Snap, not animated; preserves the current view direction. A scene with no
/// meshed entities is a successful no-op.
///
/// # Safety
///
/// `context` must be a live pointer from [`vrn_context_create`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vrn_viewport_frame_all(context: *mut VrnContextHandle) -> VrnResult {
    if context.is_null() {
        return VrnResult::NullArgument;
    }
    // SAFETY: non-null pointer from `vrn_context_create`, still alive.
    let mutex = unsafe { &(*context).0 };
    let Ok(mut ctx) = mutex.lock() else {
        return VrnResult::Internal;
    };
    match ctx.scene.frame_all() {
        Ok(()) => VrnResult::Ok,
        Err(_) => VrnResult::Internal,
    }
}

/// Parse a strict operator-kind string from the FFI boundary.
///
/// Only `"container"` and `"cube"` are accepted; anything else is
/// [`VrnResult::InvalidArgument`]. The graph core only ever sees the parsed
/// [`OperatorKind`], so operator #3 extends here without touching it.
fn parse_operator_kind(kind: *const c_char) -> Result<OperatorKind, VrnResult> {
    if kind.is_null() {
        return Err(VrnResult::NullArgument);
    }
    // SAFETY: non-null; the caller guarantees a valid NUL-terminated string
    // for the duration of the call.
    let text = unsafe { CStr::from_ptr(kind) };
    match text.to_str() {
        Ok("container") => Ok(OperatorKind::Container),
        Ok("cube") => Ok(OperatorKind::Cube),
        Ok(_) | Err(_) => Err(VrnResult::InvalidArgument),
    }
}

/// Create an operator of `kind` at `(x, y)`, writing its fresh id to `out_id`.
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

/// Create a default cube operator at the root, returning its fresh id.
///
/// Test-only helper: several tick/frame tests need real cooked geometry now
/// that the scene ships without built-in content.
///
/// # Panics
///
/// Panics when the create call fails; that means the FFI graph path is
/// broken, not the test input.
#[cfg(test)]
fn test_create_cube(context: *mut VrnContextHandle) -> u64 {
    let kind = CString::new("cube").unwrap();
    let mut id = 0u64;
    // SAFETY: live context, single-threaded test; string outlives the call.
    unsafe {
        assert_eq!(
            vrn_graph_create_operator(context, kind.as_ptr().cast_mut(), 0, 0.0, 0.0, &raw mut id),
            VrnResult::Ok
        );
    }
    id
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
    fn base_stats_advance_with_ticks() {
        let context = vrn_context_create();
        assert!(!context.is_null());
        let mut ticks = 0u64;
        let mut entities = 0u64;
        // SAFETY: just created, alive, single-threaded test.
        unsafe {
            assert_eq!(vrn_entity_count(context, &raw mut entities), VrnResult::Ok);
            assert_eq!(entities, 2);
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
mod viewport_tests {
    use super::*;
    use std::ptr;
    use veronica_scene::{FRAME_HEIGHT, FRAME_WIDTH};

    /// Camera translation read through the context lock.
    ///
    /// # Panics
    ///
    /// Panics when the lock is poisoned or the camera is gone; both indicate
    /// a broken test setup, not fallible production input.
    fn camera_translation(context: *mut VrnContextHandle) -> [f32; 3] {
        // SAFETY: live context, single-threaded test; lock is unpoisoned.
        let mut ctx = unsafe { (*context).0.lock().unwrap() };
        ctx.scene.camera_translation().unwrap()
    }

    /// Euclidean distance of a translation from the viewport pivot (origin).
    fn distance_from_origin(translation: [f32; 3]) -> f32 {
        (translation[0].powi(2) + translation[1].powi(2) + translation[2].powi(2)).sqrt()
    }

    #[test]
    fn navigate_entry_points_reject_null_context() {
        // SAFETY: null is the input under test; no context is touched.
        unsafe {
            assert_eq!(
                vrn_viewport_orbit(ptr::null_mut(), 10.0, 0.0),
                VrnResult::NullArgument
            );
            assert_eq!(
                vrn_viewport_pan(ptr::null_mut(), 10.0, 0.0),
                VrnResult::NullArgument
            );
            assert_eq!(
                vrn_viewport_dolly(ptr::null_mut(), 1.0, 0.0, 0.0),
                VrnResult::NullArgument
            );
            assert_eq!(
                vrn_viewport_frame_all(ptr::null_mut()),
                VrnResult::NullArgument
            );
        }
    }

    #[test]
    fn orbit_round_trip_moves_the_viewport_camera() {
        let context = vrn_context_create();
        let before = camera_translation(context);
        // SAFETY: just created, alive, single-threaded test.
        unsafe {
            assert_eq!(vrn_viewport_orbit(context, 60.0, 20.0), VrnResult::Ok);
        }
        let after = camera_translation(context);
        // SAFETY: alive until this destroy; single-threaded test.
        unsafe {
            vrn_context_destroy(context);
        }
        let moved = before
            .iter()
            .zip(after.iter())
            .any(|(a, b)| (a - b).abs() > 1e-6);
        assert!(moved, "a 60x20px orbit must move the camera");
    }

    #[test]
    fn dolly_round_trip_shortens_the_distance() {
        let context = vrn_context_create();
        let before = distance_from_origin(camera_translation(context));
        // SAFETY: just created, alive, single-threaded test.
        unsafe {
            assert_eq!(vrn_viewport_dolly(context, 1.0, 0.0, 0.0), VrnResult::Ok);
        }
        let after = distance_from_origin(camera_translation(context));
        // SAFETY: alive until this destroy; single-threaded test.
        unsafe {
            vrn_context_destroy(context);
        }
        assert!(
            after < before,
            "a centered dolly-in must shorten the distance, went {before} -> {after}"
        );
    }

    #[test]
    fn pan_round_trip_moves_the_viewport_camera() {
        let context = vrn_context_create();
        let before = camera_translation(context);
        // SAFETY: just created, alive, single-threaded test.
        unsafe {
            assert_eq!(vrn_viewport_pan(context, 60.0, 20.0), VrnResult::Ok);
        }
        let after = camera_translation(context);
        // SAFETY: alive until this destroy; single-threaded test.
        unsafe {
            vrn_context_destroy(context);
        }
        let moved = before
            .iter()
            .zip(after.iter())
            .any(|(a, b)| (a - b).abs() > 1e-6);
        assert!(moved, "a 60x20px pan must move the camera");
    }

    #[test]
    fn frame_all_round_trip_refits_cooked_geometry() {
        let context = vrn_context_create();
        // Arrange: a cooked cube to fit — the empty base scene is a no-op
        // by design (see `empty_frame_all_preserves_camera_and_pivot`).
        let _ = test_create_cube(context);
        // SAFETY: just created, alive, single-threaded test.
        unsafe {
            assert_eq!(vrn_tick(context), VrnResult::Ok);
        }
        let before = distance_from_origin(camera_translation(context));
        // SAFETY: just created, alive, single-threaded test.
        unsafe {
            assert_eq!(vrn_viewport_frame_all(context), VrnResult::Ok);
        }
        let after = distance_from_origin(camera_translation(context));
        // SAFETY: alive until this destroy; single-threaded test.
        unsafe {
            vrn_context_destroy(context);
        }
        assert!(
            (after - before).abs() > 0.1,
            "frame-all must refit the cooked distance, went {before} -> {after}"
        );
    }

    #[test]
    fn empty_frame_all_preserves_camera_and_pivot() {
        let context = vrn_context_create();
        let before = camera_translation(context);
        // SAFETY: live context, single-threaded test; lock is unpoisoned.
        let pivot = unsafe { (*context).0.lock().unwrap().scene.pivot() };
        // SAFETY: just created, alive, single-threaded test.
        unsafe {
            assert_eq!(vrn_viewport_frame_all(context), VrnResult::Ok);
        }
        let after = camera_translation(context);
        // SAFETY: live context, single-threaded test; lock is unpoisoned.
        let pivot_after = unsafe { (*context).0.lock().unwrap().scene.pivot() };
        // SAFETY: alive until this destroy; single-threaded test.
        unsafe {
            vrn_context_destroy(context);
        }
        assert_eq!(
            before.map(f32::to_bits),
            after.map(f32::to_bits),
            "empty frame-all must not move the camera"
        );
        assert_eq!(
            pivot.to_array().map(f32::to_bits),
            pivot_after.to_array().map(f32::to_bits),
            "empty frame-all must not move the pivot"
        );
    }
    #[test]
    fn set_size_guards_map_to_result_codes() {
        let context = vrn_context_create();
        assert!(!context.is_null());
        // SAFETY: just created, alive, single-threaded test.
        unsafe {
            assert_eq!(
                vrn_viewport_set_size(ptr::null_mut(), 320, 200),
                VrnResult::NullArgument
            );
            for (width, height) in [(0, 200), (320, 0), (0, 0), (2049, 100), (100, 4096)] {
                assert_eq!(
                    vrn_viewport_set_size(context, width, height),
                    VrnResult::InvalidArgument,
                    "extents {width}x{height} must be rejected"
                );
            }
            // Long-edge cap: exactly 2048 is accepted, 2049 is not.
            assert_eq!(vrn_viewport_set_size(context, 2048, 2048), VrnResult::Ok);
            assert_eq!(
                vrn_viewport_set_size(context, 2049, 2048),
                VrnResult::InvalidArgument
            );
            vrn_context_destroy(context);
        }
    }

    #[test]
    fn pending_size_applies_on_tick_and_recreates_surface() {
        let context = vrn_context_create();
        assert!(!context.is_null());
        let mut surface: *mut c_void = ptr::null_mut();
        let mut width = 0u32;
        let mut height = 0u32;
        // SAFETY: just created, alive, single-threaded test; slots are live.
        unsafe {
            assert_eq!(vrn_tick(context), VrnResult::Ok);
            assert_eq!(
                vrn_frame_surface(context, &raw mut surface, &raw mut width, &raw mut height),
                VrnResult::Ok
            );
            let first = surface;
            assert_eq!((width, height), (FRAME_WIDTH, FRAME_HEIGHT));
            // Re-target mid-life: the next tick publishes the new extents on
            // a fresh surface (new address); following ticks ping-pong
            // between the two fresh buffers, never reusing the old one.
            assert_eq!(vrn_viewport_set_size(context, 320, 200), VrnResult::Ok);
            assert_eq!(vrn_tick(context), VrnResult::Ok);
            assert_eq!(
                vrn_frame_surface(context, &raw mut surface, &raw mut width, &raw mut height),
                VrnResult::Ok
            );
            assert_eq!((width, height), (320, 200));
            assert_ne!(surface, first);
            let second = surface;
            assert_eq!(vrn_tick(context), VrnResult::Ok);
            assert_eq!(
                vrn_frame_surface(context, &raw mut surface, &raw mut width, &raw mut height),
                VrnResult::Ok
            );
            assert_eq!((width, height), (320, 200));
            assert_ne!(surface, second);
            assert_ne!(surface, first);
            let third = surface;
            assert_eq!(vrn_tick(context), VrnResult::Ok);
            assert_eq!(
                vrn_frame_surface(context, &raw mut surface, &raw mut width, &raw mut height),
                VrnResult::Ok
            );
            assert_eq!((width, height), (320, 200));
            assert_eq!(surface, second);
            assert_ne!(third, second);
            vrn_context_destroy(context);
        }
    }
}

#[cfg(test)]
mod frame_tests {
    use super::*;
    use std::ptr;
    use veronica_scene::{FRAME_HEIGHT, FRAME_WIDTH};

    /// Ticks before the test gives up waiting for the lit cube to appear.
    ///
    /// Mirrors the scene crate's pipeline warm-up: early frames may be
    /// clear-only while shaders compile on first use.
    const MAX_WARMUP_TICKS: u32 = 240;

    /// True when at least one pixel differs from the first: a clear-only
    /// frame is perfectly uniform, so this proves scene content reached
    /// the pixels.
    fn is_non_uniform(pixels: &[u8]) -> bool {
        let Some(first) = pixels.chunks_exact(4).next() else {
            return false;
        };
        pixels.chunks_exact(4).any(|pixel| pixel != first)
    }

    /// Read the published front buffer's packed bytes via the context lock.
    ///
    /// # Panics
    ///
    /// Panics when the lock is poisoned or no frame is published yet;
    /// both indicate a broken test setup, not fallible production input.
    fn front_bytes(context: *mut VrnContextHandle) -> Vec<u8> {
        // SAFETY: live context, single-threaded test; lock is unpoisoned.
        let ctx = unsafe { (*context).0.lock().unwrap() };
        let front = ctx.front;
        ctx.surfaces[front].as_ref().unwrap().snapshot_bytes()
    }

    /// Tick until the published front carries scene content, then return
    /// its bytes.
    ///
    /// # Panics
    ///
    /// Panics when no non-uniform frame appears within
    /// [`MAX_WARMUP_TICKS`] ticks; that means the GPU path never produced
    /// scene content.
    ///
    /// # Safety
    ///
    /// `context` must be a live context that is not accessed concurrently.
    unsafe fn front_bytes_when_ready(context: *mut VrnContextHandle) -> Vec<u8> {
        for _ in 0..MAX_WARMUP_TICKS {
            // SAFETY: live context, single-threaded test.
            unsafe {
                assert_eq!(vrn_tick(context), VrnResult::Ok);
            }
            let bytes = front_bytes(context);
            if is_non_uniform(&bytes) {
                return bytes;
            }
        }
        panic!("GPU frame never showed the lit cube after {MAX_WARMUP_TICKS} ticks");
    }

    /// Tick once and return the newly published surface handle plus extents.
    ///
    /// The repeating two lines of every ping-pong assertion, so the tests
    /// stay under the line-count lint.
    ///
    /// # Panics
    ///
    /// Panics when the tick or the surface read fails; either means the
    /// frame-publish seam is broken, not the test input.
    ///
    /// # Safety
    ///
    /// `context` must be a live context that is not accessed concurrently.
    unsafe fn tick_and_read(context: *mut VrnContextHandle) -> (*mut c_void, u32, u32) {
        let mut surface: *mut c_void = ptr::null_mut();
        let mut width = 0u32;
        let mut height = 0u32;
        // SAFETY: live context with live slots, single-threaded test.
        unsafe {
            assert_eq!(vrn_tick(context), VrnResult::Ok);
            assert_eq!(
                vrn_frame_surface(context, &raw mut surface, &raw mut width, &raw mut height),
                VrnResult::Ok
            );
        }
        (surface, width, height)
    }

    #[test]
    fn published_frame_is_valid_after_tick() {
        let context = vrn_context_create();
        assert!(!context.is_null());
        let mut surface: *mut c_void = ptr::null_mut();
        let mut width = 0u32;
        let mut height = 0u32;
        // SAFETY: just created, alive, single-threaded test; slots are live.
        unsafe {
            // Nothing published before the first tick.
            assert_eq!(
                vrn_frame_surface(context, &raw mut surface, &raw mut width, &raw mut height),
                VrnResult::InvalidArgument
            );
            // Ping-pong contract: consecutive ticks strictly alternate
            // handles (h0, h1, h0, ...). Both handles stay valid and extents
            // follow the viewport size from the very first tick.
            let (first, w, h) = tick_and_read(context);
            assert!(!first.is_null());
            assert_eq!((w, h), (FRAME_WIDTH, FRAME_HEIGHT));
            let (second, w, h) = tick_and_read(context);
            assert!(!second.is_null());
            assert_ne!(second, first);
            assert_eq!((w, h), (FRAME_WIDTH, FRAME_HEIGHT));
            let (third, _, _) = tick_and_read(context);
            assert_eq!(third, first);
            let (fourth, _, _) = tick_and_read(context);
            assert_eq!(fourth, second);
            // The empty base scene publishes identical bytes on alternating
            // fronts: handles flip every tick while the clear color holds.
            let bytes_before = front_bytes(context);
            assert!(!bytes_before.is_empty());
            let (flipped, _, _) = tick_and_read(context);
            assert_ne!(flipped, fourth);
            assert_eq!(front_bytes(context), bytes_before);
            let (back, _, _) = tick_and_read(context);
            assert_eq!(back, fourth);
            assert_eq!(front_bytes(context), bytes_before);
            // Null slots and null context are safe.
            assert_eq!(
                vrn_frame_surface(context, ptr::null_mut(), &raw mut width, &raw mut height),
                VrnResult::NullArgument
            );
            assert_eq!(
                vrn_frame_surface(context, &raw mut surface, ptr::null_mut(), &raw mut height),
                VrnResult::NullArgument
            );
            assert_eq!(
                vrn_frame_surface(context, &raw mut surface, &raw mut width, ptr::null_mut()),
                VrnResult::NullArgument
            );
            assert_eq!(
                vrn_frame_surface(
                    ptr::null_mut(),
                    &raw mut surface,
                    &raw mut width,
                    &raw mut height
                ),
                VrnResult::NullArgument
            );
            vrn_context_destroy(context);
        }
    }

    /// Count of BGRA8 pixels differing between two same-length frames.
    fn differing_pixels(first: &[u8], second: &[u8]) -> usize {
        assert_eq!(first.len(), second.len());
        first
            .chunks_exact(4)
            .zip(second.chunks_exact(4))
            .filter(|(a, b)| a != b)
            .count()
    }

    /// Count of BGRA8 pixels with relative luminance above 0.5.
    ///
    /// Integer math only: the threshold scales by `10_000` so the Rec. 709
    /// weights stay exact without float casts.
    fn bright_pixels(pixels: &[u8]) -> usize {
        pixels
            .chunks_exact(4)
            .filter(|pixel| {
                2126 * u32::from(pixel[2]) + 7152 * u32::from(pixel[1]) + 722 * u32::from(pixel[0])
                    > 1_275_000
            })
            .count()
    }

    #[test]
    fn frame_all_moves_published_gpu_pixels() {
        let context = vrn_context_create();
        assert!(!context.is_null());
        // Arrange: one cooked cube to fit — the empty base scene is a
        // frame-all no-op by design.
        let _ = test_create_cube(context);
        // SAFETY: just created, alive, single-threaded test.
        unsafe {
            // Arrange: warmed-up front carrying the lit cube, plus one
            // static tick as the no-motion baseline. The scene holds still
            // (no auto-spin since #46), so consecutive ticks publish
            // near-identical bytes.
            let settled = front_bytes_when_ready(context);
            assert_eq!(vrn_tick(context), VrnResult::Ok);
            let still = front_bytes(context);
            let baseline_pixels = differing_pixels(&settled, &still);
            // Act: frame-all, then the tick that renders it.
            assert_eq!(vrn_viewport_frame_all(context), VrnResult::Ok);
            assert_eq!(vrn_tick(context), VrnResult::Ok);
            let framed = front_bytes(context);
            let frame_pixels = differing_pixels(&still, &framed);
            let still_bright = bright_pixels(&still);
            let frame_bright = bright_pixels(&framed);
            vrn_context_destroy(context);
            // Assert: the refit (spawn ~4.74 to fit ~2.6) must dwarf static
            // tick-to-tick noise in both raw churn and lit area — otherwise
            // F is a visual no-op (issue #46).
            assert!(
                frame_pixels > 100 * baseline_pixels.max(1),
                "frame-all moved {frame_pixels}px vs static baseline {baseline_pixels}px"
            );
            assert!(
                frame_bright > 2 * still_bright,
                "frame-all lit {frame_bright}px vs static {still_bright}px"
            );
        }
    }
}

#[cfg(test)]
mod timings_tests {
    use super::*;
    use std::ptr;

    #[test]
    fn null_slots_are_null_argument() {
        let context = vrn_context_create();
        assert!(!context.is_null());
        let mut slot = 0u64;
        // SAFETY: just created, alive, single-threaded test; slot is live.
        unsafe {
            assert_eq!(
                vrn_tick_timings(ptr::null_mut(), &raw mut slot, &raw mut slot, &raw mut slot),
                VrnResult::NullArgument
            );
            assert_eq!(
                vrn_tick_timings(context, ptr::null_mut(), &raw mut slot, &raw mut slot),
                VrnResult::NullArgument
            );
            assert_eq!(
                vrn_tick_timings(context, &raw mut slot, ptr::null_mut(), &raw mut slot),
                VrnResult::NullArgument
            );
            assert_eq!(
                vrn_tick_timings(context, &raw mut slot, &raw mut slot, ptr::null_mut()),
                VrnResult::NullArgument
            );
            vrn_context_destroy(context);
        }
    }

    #[test]
    fn splits_start_at_zero_and_capture_after_tick() {
        let context = vrn_context_create();
        assert!(!context.is_null());
        let mut update = 0u64;
        let mut readback = 0u64;
        let mut upload = 0u64;
        // SAFETY: just created, alive, single-threaded test; slots are live.
        unsafe {
            assert_eq!(
                vrn_tick_timings(context, &raw mut update, &raw mut readback, &raw mut upload),
                VrnResult::Ok
            );
            assert_eq!((update, readback, upload), (0, 0, 0));
            assert_eq!(vrn_tick(context), VrnResult::Ok);
            assert_eq!(
                vrn_tick_timings(context, &raw mut update, &raw mut readback, &raw mut upload),
                VrnResult::Ok
            );
            // No wall-time threshold: any real Bevy render plus readback
            // plus upload takes far longer than 1µs, so a nonzero sum
            // proves every stage captured without a flaky bound.
            assert!(update + readback + upload > 0);
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
    fn strict_kind_validation_rejects_anything_but_known_kinds() {
        let context = vrn_context_create();
        assert!(!context.is_null());
        let mut id = 0u64;
        // SAFETY: just created, alive, single-threaded test.
        unsafe {
            for rejected in [
                "box",
                "Container",
                "CONTAINER",
                "",
                "container ",
                "Cube",
                "cube ",
            ] {
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
    fn cube_kind_is_accepted_and_visible_in_snapshots() {
        let context = vrn_context_create();
        assert!(!context.is_null());
        let kind = cstring("cube");
        let mut id = 0u64;
        // SAFETY: just created, alive, single-threaded test.
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
            assert_eq!(id, 1);
            let json = snapshot_json(context);
            assert!(
                json.contains(r#""kind":"cube""#),
                "snapshot must carry the cube kind, got: {json}"
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

#[cfg(test)]
mod recook_tests {
    use super::*;
    use veronica_core::NodeId;

    /// Live `(operator, entity-bits)` cooked pairs through the context lock.
    ///
    /// Entities cross as `to_bits` so this module stays Bevy-free: identity
    /// is all the churn assertions need.
    ///
    /// # Panics
    ///
    /// Panics when the lock is poisoned; that indicates a broken test setup,
    /// not fallible production input.
    fn cooked_pairs(context: *mut VrnContextHandle) -> Vec<(NodeId, u64)> {
        // SAFETY: live context, single-threaded test; lock is unpoisoned.
        let mut ctx = unsafe { (*context).0.lock().unwrap() };
        ctx.scene
            .cooked_entities()
            .into_iter()
            .map(|(id, entity)| (id, entity.to_bits()))
            .collect()
    }

    /// `(live graph epoch, last-cooked epoch)` through the context lock.
    ///
    /// # Panics
    ///
    /// Panics when the lock is poisoned; that indicates a broken test setup.
    fn epochs(context: *mut VrnContextHandle) -> (u64, u64) {
        // SAFETY: live context, single-threaded test; lock is unpoisoned.
        let ctx = unsafe { (*context).0.lock().unwrap() };
        (ctx.operator_graph.epoch(), ctx.last_cooked_epoch)
    }

    /// Live scene entity count through the FFI boundary.
    ///
    /// # Panics
    ///
    /// Panics when the count call fails; that means the FFI boundary is
    /// broken, not the test input.
    ///
    /// # Safety
    ///
    /// `context` must be a live context that is not accessed concurrently.
    unsafe fn entity_count(context: *mut VrnContextHandle) -> u64 {
        let mut count = 0u64;
        // SAFETY: live context with a live slot, single-threaded test.
        unsafe {
            assert_eq!(vrn_entity_count(context, &raw mut count), VrnResult::Ok);
        }
        count
    }

    #[test]
    fn empty_graph_tick_cooks_nothing_and_stays_stable() {
        let context = vrn_context_create();
        assert!(!context.is_null());
        // SAFETY: just created, alive, single-threaded test.
        unsafe {
            assert_eq!(vrn_tick(context), VrnResult::Ok);
            assert_eq!(entity_count(context), 2);
            assert!(cooked_pairs(context).is_empty());
            // Second tick: epoch clean, nothing to reconcile.
            assert_eq!(vrn_tick(context), VrnResult::Ok);
            assert_eq!(entity_count(context), 2);
            assert!(cooked_pairs(context).is_empty());
            assert_eq!(epochs(context).0, epochs(context).1);
            vrn_context_destroy(context);
        }
    }

    #[test]
    fn mutate_tick_cooks_and_second_tick_is_churn_free() {
        let context = vrn_context_create();
        assert!(!context.is_null());
        let id = test_create_cube(context);
        // SAFETY: just created, alive, single-threaded test.
        unsafe {
            // Mutate, then tick: the cooked cube appears and the epoch tracks.
            assert_eq!(vrn_tick(context), VrnResult::Ok);
            let first = cooked_pairs(context);
            assert_eq!(first.len(), 1);
            assert_eq!(first[0].0, NodeId(id));
            assert_eq!(entity_count(context), 3);
            assert_eq!(epochs(context).0, epochs(context).1);
            // Second tick without mutation: the same live entity, no respawn.
            assert_eq!(vrn_tick(context), VrnResult::Ok);
            assert_eq!(cooked_pairs(context), first);
            assert_eq!(entity_count(context), 3);
            // A move bumps the epoch too, so the next tick respawns fresh.
            assert_eq!(
                vrn_graph_move_operator(context, id, 5.0, 5.0),
                VrnResult::Ok
            );
            assert_eq!(vrn_tick(context), VrnResult::Ok);
            assert_ne!(cooked_pairs(context), first);
            assert_eq!(cooked_pairs(context).len(), 1);
            vrn_context_destroy(context);
        }
    }

    #[test]
    fn delete_then_tick_clears_cooked() {
        let context = vrn_context_create();
        assert!(!context.is_null());
        let id = test_create_cube(context);
        // SAFETY: just created, alive, single-threaded test.
        unsafe {
            assert_eq!(vrn_tick(context), VrnResult::Ok);
            assert_eq!(cooked_pairs(context).len(), 1);
            assert_eq!(vrn_graph_delete_operator(context, id), VrnResult::Ok);
            assert_eq!(vrn_tick(context), VrnResult::Ok);
            assert!(cooked_pairs(context).is_empty());
            assert_eq!(entity_count(context), 2);
            vrn_context_destroy(context);
        }
    }
}
