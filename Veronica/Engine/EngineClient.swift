import ComposableArchitecture
import Foundation

/// Read-only snapshot of Rust scene state for one tick.
///
/// Mirrors `vrn_tick_count` / `vrn_entity_count` across the FFI boundary.
/// Swift never constructs scene content — this is observation only.
///
/// Explicitly `nonisolated`: snapshots cross from the engine queue to the
/// main actor every frame, which the target's MainActor-default isolation
/// would otherwise forbid.
nonisolated struct SceneStats: Equatable, Sendable {
    /// Ticks elapsed since engine context creation.
    var tickCount: UInt64
    /// Live demo-scene entities (camera, light, cube).
    var entityCount: UInt64
    /// Latest published frame, if a tick has published one yet.
    var frame: VideoFrame?
    /// Wall-clock cost of the tick that produced these stats
    /// (Bevy schedule + readback + upload), in whole microseconds.
    /// Measured on the engine queue around `vrn_tick`.
    var tickMicroseconds: UInt64 = 0
    /// Per-stage splits of that tick (see `vrn_tick_timings`).
    var timings = TickTimings()
}

/// Per-stage splits of one Rust tick, in whole microseconds.
///
/// Mirrors `vrn_tick_timings`: `update` is the Bevy schedule (ECS + GPU
/// render submission), `readback` the texture-to-buffer copy plus the
/// synchronous map, `upload` the row-stride memcpy into the back `IOSurface`.
/// Debug/attribution only (issue #32) — never drives behavior.
nonisolated struct TickTimings: Equatable, Sendable {
    /// Bevy-schedule microseconds.
    var update: UInt64 = 0
    /// GPU-readback microseconds.
    var readback: UInt64 = 0
    /// Surface-upload microseconds.
    var upload: UInt64 = 0
}

/// Display-pacing observations from the Metal host, carried on `.frame`.
///
/// `fps` is the display-link EMA (achieved presentation rate — drops below
/// the display rate only when the main thread, i.e. the present path, is
/// the bottleneck); `presentMicroseconds` is the last completed `draw`
/// body. Stale by one frame by construction; a meter, not a signal.
nonisolated struct FramePacing: Equatable, Sendable {
    /// Smoothed frames per second.
    var fps: Double = 0
    /// Last completed present cost in whole microseconds.
    var presentMicroseconds: UInt64 = 0
}

/// Borrowed handle to the Rust-owned `IOSurface` for one published frame.
///
/// The address is the `IOSurfaceRef` pointer owned by the engine context
/// (valid for the context lifetime; Swift never releases it). Compared by
/// value so the view can skip re-wrapping an unchanged surface.
nonisolated struct VideoFrame: Equatable, Sendable {
    /// `IOSurfaceRef` pointer address.
    var surfaceAddress: UInt64
    /// Frame width in pixels.
    var width: UInt64
    /// Frame height in pixels.
    var height: UInt64
}

/// Read-only mirror of one Rust Selection: the picked face identity.
///
/// `(node, face)` is the face-ordinal contract from ADR-0007 — the same
/// pair `vrn_viewport_pick` returns. Swift never constructs scene content;
/// this is observation only, cleared on a background miss (`nil`).
nonisolated struct ViewportPick: Equatable, Sendable {
    /// Rust-issued node id of the picked mesh.
    var node: UInt64
    /// Face ordinal (realized triangle index) within that mesh.
    var face: UInt32
}

/// TCA dependency for the Rust engine. The live value will call `vrn_tick`
/// plus the stats getters once `libveronica.a` is linked; until then it
/// awaits the placeholder bridge on the engine queue.
///
/// Explicitly `nonisolated`: the reducer invokes `tick` from background
/// effect contexts.
nonisolated struct EngineClient: Sendable {
    /// Advance the scene one tick and return the resulting stats.
    var tick: @Sendable () async -> SceneStats
    /// Create an operator of `kind` under `parent` (`nil` for root) and
    /// return the Rust-issued id. Slice 1 accepts only `"container"`.
    var createOperator: @Sendable (String, UInt64?, GraphPosition) async throws(GraphEngineError) -> UInt64
    /// Move a known operator id to a new canvas position.
    var moveOperator: @Sendable (UInt64, GraphPosition) async throws(GraphEngineError) -> Void
    /// Rename a known operator id. Blank names are rejected.
    var renameOperator: @Sendable (UInt64, String) async throws(GraphEngineError) -> Void
    /// Set one string parameter on a known operator id. The `"name"` key
    /// carries rename semantics (trimmed, blank rejected, old value kept).
    var setParameter: @Sendable (UInt64, String, String) async throws(GraphEngineError) -> Void
    /// Set one typed parameter value on a known operator id. Numeric
    /// commits travel here (issue #54): the value crosses as its
    /// `ParamValue` wire JSON, so the cook sees exactly what a restore
    /// would have written — no read-modify-write, no interleave window.
    /// The `"name"` key is reserved; Rust rejects it.
    var setParameterTyped: @Sendable (UInt64, String, ParameterValue) async throws(GraphEngineError) -> Void
    /// Delete an operator id, cascading its subtree.
    var deleteOperator: @Sendable (UInt64) async throws(GraphEngineError) -> Void
    /// Fetch the whole-graph mirror. Owns the allocate/free boundary — the
    /// raw FFI pointer never escapes `EngineBridge`.
    var requestSnapshot: @Sendable () async throws(GraphEngineError) -> GraphSnapshot
    /// Replace the whole DAG from a snapshot. Rejects `version != 2`.
    var restoreSnapshot: @Sendable (GraphSnapshot) async throws(GraphEngineError) -> Void
    /// Orbit the viewport camera by a drag delta in pixels. Fire-and-forget
    /// (issue #41): hops to the engine queue inside `EngineBridge`.
    var viewportOrbit: @Sendable (Double, Double) -> Void
    /// Pan the viewport camera and pivot rigidly by a drag delta in pixels.
    /// Fire-and-forget (see `viewportOrbit`).
    var viewportPan: @Sendable (Double, Double) -> Void
    /// Dolly toward `cursor` (NDC) by `logFactor` (positive zooms in).
    /// Fire-and-forget (see `viewportOrbit`).
    var viewportDolly: @Sendable (Double, CursorNDC) -> Void
    /// Send a viewport tap at `cursor` (NDC) to the engine Pick path
    /// (issue #59, FFI from T5/#63) and return the painted selection.
    ///
    /// `nil` is the background-miss mirror: Rust cleared the Selection and
    /// Swift clears with it. Throws the engine error on stale targets or
    /// GPU faults; the mock engine owns no scene, so it always misses.
    var sendTapNDC: @Sendable (CursorNDC) async throws(GraphEngineError) -> ViewportPick?
    /// Frame the whole scene (snap to fit). Fire-and-forget.
    var viewportFrameAll: @Sendable () -> Void
}

extension EngineClient: DependencyKey {
    static let liveValue: EngineClient = {
        let mock = MockGraphEngine()
        return EngineClient(
            tick: { await EngineBridge.tickWithStats() },
            createOperator: { (kind: String, parent: UInt64?, position: GraphPosition) async throws(GraphEngineError) -> UInt64 in
                if GraphLaunchOptions.isMockEngineEnabled {
                    return try await mock.create(kind: kind, parent: parent, position: position)
                }
                return try await EngineBridge.createOperator(
                    kind: kind,
                    parent: parent,
                    position: position
                )
            },
            moveOperator: { (id: UInt64, position: GraphPosition) async throws(GraphEngineError) in
                if GraphLaunchOptions.isMockEngineEnabled {
                    return try await mock.move(id: id, position: position)
                }
                return try await EngineBridge.moveOperator(id: id, position: position)
            },
            renameOperator: { (id: UInt64, name: String) async throws(GraphEngineError) in
                if GraphLaunchOptions.isMockEngineEnabled {
                    return try await mock.rename(id: id, name: name)
                }
                return try await EngineBridge.renameOperator(id: id, name: name)
            },
            setParameter: { (id: UInt64, key: String, value: String) async throws(GraphEngineError) in
                if GraphLaunchOptions.isMockEngineEnabled {
                    return try await mock.setParameter(id: id, key: key, value: value)
                }
                return try await EngineBridge.setParameter(id: id, key: key, value: value)
            },
            setParameterTyped: { (id: UInt64, key: String, value: ParameterValue) async throws(GraphEngineError) in
                if GraphLaunchOptions.isMockEngineEnabled {
                    return try await mock.setParameterTyped(id: id, key: key, value: value)
                }
                return try await EngineBridge.setParameterTyped(id: id, key: key, value: value)
            },
            deleteOperator: { (id: UInt64) async throws(GraphEngineError) in
                if GraphLaunchOptions.isMockEngineEnabled {
                    return try await mock.delete(id: id)
                }
                return try await EngineBridge.deleteOperator(id: id)
            },
            requestSnapshot: { () async throws(GraphEngineError) -> GraphSnapshot in
                if GraphLaunchOptions.isMockEngineEnabled {
                    return await mock.snapshot()
                }
                return try await EngineBridge.requestGraphSnapshot()
            },
            restoreSnapshot: { (snapshot: GraphSnapshot) async throws(GraphEngineError) in
                if GraphLaunchOptions.isMockEngineEnabled {
                    return try await mock.restore(snapshot)
                }
                return try await EngineBridge.restoreGraphSnapshot(snapshot)
            },
            viewportOrbit: { (dx: Double, dy: Double) in
                EngineBridge.viewportOrbit(dxPixels: dx, dyPixels: dy)
            },
            viewportPan: { (dx: Double, dy: Double) in
                EngineBridge.viewportPan(dxPixels: dx, dyPixels: dy)
            },
            viewportDolly: { (logFactor: Double, cursor: CursorNDC) in
                EngineBridge.viewportDolly(
                    logFactor: logFactor,
                    cursorXNDC: cursor.x,
                    cursorYNDC: cursor.y
                )
            },
            // The mock owns no scene, so every tap is a background miss;
            // the real engine paints the Selection through the FFI pick.
            sendTapNDC: { (cursor: CursorNDC) async throws(GraphEngineError) -> ViewportPick? in
                if GraphLaunchOptions.isMockEngineEnabled {
                    return nil
                }
                return try await EngineBridge.viewportPick(at: cursor)
            },
            viewportFrameAll: { EngineBridge.viewportFrameAll() }
        )
    }()

    static let testValue = EngineClient(
        tick: { SceneStats(tickCount: 7, entityCount: 3) },
        createOperator: { _, _, _ in 1 },
        moveOperator: { _, _ in },
        renameOperator: { _, _ in },
        setParameter: { _, _, _ in },
        setParameterTyped: { _, _, _ in },
        deleteOperator: { _ in },
        requestSnapshot: { GraphSnapshot(operators: []) },
        restoreSnapshot: { _ in },
        viewportOrbit: { _, _ in },
        viewportPan: { _, _ in },
        viewportDolly: { _, _ in },
        sendTapNDC: { _ in nil },
        viewportFrameAll: {}
    )
}

extension DependencyValues {
    /// Access the engine client from any reducer via `@Dependency(\.engineClient)`.
    ///
    /// Explicitly `nonisolated`: the key path must be `Sendable` for
    /// `@Dependency`, which a MainActor-isolated getter would forbid.
    nonisolated var engineClient: EngineClient {
        get { self[EngineClient.self] }
        set { self[EngineClient.self] = newValue }
    }
}
