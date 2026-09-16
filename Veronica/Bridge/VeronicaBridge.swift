import Foundation
import Synchronization

/// Swift side of the Rust bridge.
///
/// Ownership: Rust owns domain & scene state (mesh topology, morph weights,
/// bone transforms, DAG topology, undo/redo). Swift owns presentation & UI
/// view state (panels, filters, tabs, dialogs) and mirrors Rust read-only.
///
/// Threading: the Bevy scene must never tick on MainActor. All `vrn_*`
/// calls go through `engineQueue`. Graph externs link directly against the
/// prebuilt `libveronica.a` via `@_silgen_name` (a missing symbol fails the
/// link, never silently at runtime); declarations mirror the generated
/// `rust/crates/veronica-ffi/include/veronica.h`.
nonisolated enum EngineBridge {
    private static let engineQueue = DispatchQueue(
        label: "com.github.miamisunset.Veronica.engine",
        qos: .userInitiated
    )

    /// Validate mesh buffer sizes via Rust without touching scene state.
    static func validateMesh(positionsCount: Int, indicesCount: Int) async -> Bool {
        await withCheckedContinuation { continuation in
            engineQueue.async {
                let code = vrnValidateMesh(
                    UInt(bitPattern: positionsCount),
                    UInt(bitPattern: indicesCount)
                )
                continuation.resume(returning: code == VrnResultCode.ok.rawValue)
            }
        }
    }

    /// Coalescing tick state (engine queue only, via `tickState`).
    ///
    /// The display link fires every refresh, but a tick now renders and
    /// uploads a frame: when a tick is already running, late refreshes reuse
    /// the latest stats instead of queueing behind it. Without this, tick
    /// work piles up on the serial `engineQueue` and starves graph intents.
    private struct TickState: Sendable {
        /// A tick is currently executing on `engineQueue`.
        var inFlight = false
        /// Last completed stats, served to coalesced refreshes.
        var latest = SceneStats(tickCount: 0, entityCount: 0, frame: nil)
    }

    /// Guards `TickState`. Short critical sections only; never held across
    /// engine calls.
    private static let tickState = Mutex<TickState>(TickState())

    /// Tick once and return mirrored scene stats, off the main actor.
    ///
    /// Calls the real `libveronica.a` surface on the shared context: one
    /// `vrn_tick` (advances the Rust turntable and publishes the frame),
    /// then the stats getters plus the borrowed `IOSurface` handle.
    /// Refreshes arriving while a tick runs share its result (see
    /// `TickState`).
    static func tickWithStats() async -> SceneStats {
        let shouldRun = tickState.withLock { state in
            if state.inFlight {
                return false
            }
            state.inFlight = true
            return true
        }
        guard shouldRun else {
            return tickState.withLock { $0.latest }
        }
        let stats = await runTick()
        return tickState.withLock { state in
            state.inFlight = false
            // A zero tick count marks the failure path in `runTick`; serving
            // it would move coalesced callers backward in time. Keep the
            // last good stats instead (the reducer drops stale ones anyway).
            if stats.tickCount > 0 {
                state.latest = stats
                return stats
            }
            return state.latest
        }
    }

    /// The uncoalesced tick body: one `vrn_tick` plus stats on `engineQueue`.
    private static func runTick() async -> SceneStats {
        await withCheckedContinuation { continuation in
            engineQueue.async {
                guard let context = graphContext.pointer else {
                    continuation.resume(
                        returning: SceneStats(tickCount: 0, entityCount: 0, frame: nil)
                    )
                    return
                }
                guard vrnTick(context) == VrnResultCode.ok.rawValue else {
                    continuation.resume(
                        returning: SceneStats(tickCount: 0, entityCount: 0, frame: nil)
                    )
                    return
                }
                var tick: UInt64 = 0
                var entities: UInt64 = 0
                var surface: UnsafeMutableRawPointer?
                var width: UInt32 = 0
                var height: UInt32 = 0
                _ = vrnTickCount(context, &tick)
                _ = vrnEntityCount(context, &entities)
                let frameCode = vrnFrameSurface(context, &surface, &width, &height)
                let frame: VideoFrame?
                if frameCode == VrnResultCode.ok.rawValue, let surface {
                    frame = VideoFrame(
                        surfaceAddress: UInt64(UInt(bitPattern: surface)),
                        width: UInt64(width),
                        height: UInt64(height)
                    )
                } else {
                    frame = nil
                }
                continuation.resume(
                    returning: SceneStats(tickCount: tick, entityCount: entities, frame: frame)
                )
            }
        }
    }

    // MARK: - Operator graph (ADR-0002)

    /// `VrnResult` codes mirrored from `veronica-ffi`.
    ///
    /// Must match the Rust enum discriminant-for-discriminant; drift fails
    /// loudly as misclassified intent errors.
    private enum VrnResultCode: Int32 {
        /// Operation succeeded.
        case ok = 0
        /// Null pointer argument.
        case nullArgument = 1
        /// Validation or graph error.
        case invalidArgument = 2
        /// Internal lock or allocation failure.
        case internalError = 3
    }

    /// Checked continuations typed with the graph domain error.
    private typealias GraphContinuation<T> = CheckedContinuation<T, GraphEngineError>

    // MARK: - Direct FFI declarations (`rust/crates/veronica-ffi/include/veronica.h`)
    //
    // The prebuilt `libveronica.a` (OTHER_LDFLAGS `-lveronica`) backs these;
    // a missing symbol fails the LINK, never silently at runtime. Signatures
    // were verified against the generated header and ADR-0002: the string
    // inputs are `const char *` (read-only on both sides).
    @_silgen_name("vrn_context_create")
    nonisolated private static func vrnContextCreate() -> UnsafeMutableRawPointer?
    @_silgen_name("vrn_validate_mesh")
    nonisolated private static func vrnValidateMesh(_ positionsLen: UInt, _ indicesLen: UInt) -> Int32
    @_silgen_name("vrn_tick")
    nonisolated private static func vrnTick(_ context: UnsafeMutableRawPointer?) -> Int32
    @_silgen_name("vrn_tick_count")
    nonisolated private static func vrnTickCount(
        _ context: UnsafeMutableRawPointer?,
        _ out: UnsafeMutablePointer<UInt64>?
    ) -> Int32
    @_silgen_name("vrn_entity_count")
    nonisolated private static func vrnEntityCount(
        _ context: UnsafeMutableRawPointer?,
        _ out: UnsafeMutablePointer<UInt64>?
    ) -> Int32
    @_silgen_name("vrn_frame_surface")
    nonisolated private static func vrnFrameSurface(
        _ context: UnsafeMutableRawPointer?,
        _ outSurface: UnsafeMutablePointer<UnsafeMutableRawPointer?>?,
        _ outWidth: UnsafeMutablePointer<UInt32>?,
        _ outHeight: UnsafeMutablePointer<UInt32>?
    ) -> Int32
    @_silgen_name("vrn_graph_create_operator")
    nonisolated private static func vrnGraphCreateOperator(
        _ context: UnsafeMutableRawPointer?,
        _ kind: UnsafePointer<CChar>?,
        _ parent: UInt64,
        _ x: Double,
        _ y: Double,
        _ outID: UnsafeMutablePointer<UInt64>?
    ) -> Int32
    @_silgen_name("vrn_graph_move_operator")
    nonisolated private static func vrnGraphMoveOperator(
        _ context: UnsafeMutableRawPointer?,
        _ id: UInt64,
        _ x: Double,
        _ y: Double
    ) -> Int32
    @_silgen_name("vrn_graph_rename_operator")
    nonisolated private static func vrnGraphRenameOperator(
        _ context: UnsafeMutableRawPointer?,
        _ id: UInt64,
        _ name: UnsafePointer<CChar>?
    ) -> Int32
    @_silgen_name("vrn_graph_delete_operator")
    nonisolated private static func vrnGraphDeleteOperator(
        _ context: UnsafeMutableRawPointer?,
        _ id: UInt64
    ) -> Int32
    @_silgen_name("vrn_graph_set_parameter")
    nonisolated private static func vrnGraphSetParameter(
        _ context: UnsafeMutableRawPointer?,
        _ id: UInt64,
        _ key: UnsafePointer<CChar>?,
        _ value: UnsafePointer<CChar>?
    ) -> Int32
    @_silgen_name("vrn_graph_snapshot")
    nonisolated private static func vrnGraphSnapshot(
        _ context: UnsafeMutableRawPointer?,
        _ outJSON: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?
    ) -> Int32
    @_silgen_name("vrn_graph_restore")
    nonisolated private static func vrnGraphRestore(
        _ context: UnsafeMutableRawPointer?,
        _ json: UnsafePointer<CChar>?
    ) -> Int32
    @_silgen_name("vrn_string_free")
    nonisolated private static func vrnStringFree(_ string: UnsafeMutablePointer<CChar>?)

    // SAFETY: never mutated or freed; all Rust-side access is mutex-guarded.
    /// Process-lifetime Rust context for graph intents.
    ///
    /// Created on first graph call; never destroyed (matches app lifetime).
    ///
    /// The pointer itself is immutable after creation and Rust serializes
    /// every context access behind its internal `Mutex`, so sharing it
    /// across Swift concurrency domains is sound.
    private struct SharedGraphContext: @unchecked Sendable {
        /// Opaque `VrnContext` pointer, or `nil` on allocation failure.
        let pointer: UnsafeMutableRawPointer?
    }

    /// Shared Rust context, created once on first graph call.
    private static let graphContext = SharedGraphContext(pointer: vrnContextCreate())

    /// Returns the shared Rust context, resuming `continuation` with
    /// `.engineUnavailable` (and returning `nil`) when allocation failed.
    ///
    /// Every graph intent enters through here so the nil-context path
    /// cannot drift between call sites.
    private static func requireGraphContext<T>(
        _ continuation: GraphContinuation<T>,
        operation: String
    ) -> UnsafeMutableRawPointer? {
        guard let context = graphContext.pointer else {
            continuation.resume(throwing: .engineUnavailable(operation: operation))
            return nil
        }
        return context
    }

    /// Creates an operator of `kind` under `parent` (`nil` maps to the `0`
    /// root sentinel), returning the Rust-issued id.
    ///
    /// Explicitly `nonisolated`: called from background TCA effects, and the
    /// typed error must not cross actor isolation (which would erase it).
    nonisolated static func createOperator(
        kind: String,
        parent: UInt64?,
        position: GraphPosition
    ) async throws(GraphEngineError) -> UInt64 {
        try await withCheckedThrowingContinuation { (continuation: GraphContinuation<UInt64>) in
            engineQueue.async {
                guard let context = requireGraphContext(
                    continuation,
                    operation: "vrn_graph_create_operator"
                ) else {
                    return
                }
                let result: Result<UInt64, GraphEngineError> = kind.withCString { kindPtr in
                    var outId: UInt64 = 0
                    let code = vrnGraphCreateOperator(
                        context,
                        kindPtr,
                        parent ?? 0,
                        position.x,
                        position.y,
                        &outId
                    )
                    guard code == VrnResultCode.ok.rawValue else {
                        return .failure(.ffiFailed(operation: "vrn_graph_create_operator", code: code))
                    }
                    return .success(outId)
                }
                continuation.resume(with: result)
            }
        }
    }

    /// Moves a known operator id to a new canvas position.
    ///
    /// Explicitly `nonisolated` (see `createOperator`).
    nonisolated static func moveOperator(
        id: UInt64,
        position: GraphPosition
    ) async throws(GraphEngineError) {
        try await withCheckedThrowingContinuation { (continuation: GraphContinuation<Void>) in
            engineQueue.async {
                guard let context = requireGraphContext(
                    continuation,
                    operation: "vrn_graph_move_operator"
                ) else {
                    return
                }
                let code = vrnGraphMoveOperator(context, id, position.x, position.y)
                guard code == VrnResultCode.ok.rawValue else {
                    continuation.resume(
                        throwing: .ffiFailed(operation: "vrn_graph_move_operator", code: code)
                    )
                    return
                }
                continuation.resume()
            }
        }
    }

    /// Renames a known operator id.
    ///
    /// Explicitly `nonisolated` (see `createOperator`).
    nonisolated static func renameOperator(id: UInt64, name: String) async throws(GraphEngineError) {
        try await withCheckedThrowingContinuation { (continuation: GraphContinuation<Void>) in
            engineQueue.async {
                guard let context = requireGraphContext(
                    continuation,
                    operation: "vrn_graph_rename_operator"
                ) else {
                    return
                }
                let result: Result<Void, GraphEngineError> = name.withCString { namePtr in
                    let code = vrnGraphRenameOperator(context, id, namePtr)
                    guard code == VrnResultCode.ok.rawValue else {
                        return .failure(
                            .ffiFailed(operation: "vrn_graph_rename_operator", code: code)
                        )
                    }
                    return .success(())
                }
                continuation.resume(with: result)
            }
        }
    }

    /// Deletes an operator id, cascading its subtree.
    ///
    /// Explicitly `nonisolated` (see `createOperator`).
    nonisolated static func deleteOperator(id: UInt64) async throws(GraphEngineError) {
        try await withCheckedThrowingContinuation { (continuation: GraphContinuation<Void>) in
            engineQueue.async {
                guard let context = requireGraphContext(
                    continuation,
                    operation: "vrn_graph_delete_operator"
                ) else {
                    return
                }
                let code = vrnGraphDeleteOperator(context, id)
                guard code == VrnResultCode.ok.rawValue else {
                    continuation.resume(
                        throwing: .ffiFailed(operation: "vrn_graph_delete_operator", code: code)
                    )
                    return
                }
                continuation.resume()
            }
        }
    }

    /// Sets one string parameter on a known operator id. The `"name"` key
    /// carries rename semantics in Rust (trimmed, blank rejected).
    ///
    /// Explicitly `nonisolated` (see `createOperator`).
    nonisolated static func setParameter(
        id: UInt64,
        key: String,
        value: String
    ) async throws(GraphEngineError) {
        try await withCheckedThrowingContinuation { (continuation: GraphContinuation<Void>) in
            engineQueue.async {
                guard let context = requireGraphContext(
                    continuation,
                    operation: "setParameter"
                ) else {
                    return
                }
                let result: Result<Void, GraphEngineError> = key.withCString { keyPtr in
                    value.withCString { valuePtr in
                        let code = vrnGraphSetParameter(context, id, keyPtr, valuePtr)
                        guard code == VrnResultCode.ok.rawValue else {
                            return .failure(
                                .ffiFailed(operation: "setParameter", code: code)
                            )
                        }
                        return .success(())
                    }
                }
                continuation.resume(with: result)
            }
        }
    }

    /// Fetches the whole-graph mirror, owning the allocate/free boundary.
    ///
    /// The `CString` Rust hands out is copied and freed inside the engine
    /// block; the raw pointer never escapes this function.
    ///
    /// Explicitly `nonisolated` (see `createOperator`).
    nonisolated static func requestGraphSnapshot() async throws(GraphEngineError) -> GraphSnapshot {
        let json = try await withCheckedThrowingContinuation { (continuation: GraphContinuation<String>) in
            engineQueue.async {
                guard let context = requireGraphContext(
                    continuation,
                    operation: "vrn_graph_snapshot"
                ) else {
                    return
                }
                var out: UnsafeMutablePointer<CChar>?
                let code = vrnGraphSnapshot(context, &out)
                guard code == VrnResultCode.ok.rawValue else {
                    continuation.resume(
                        throwing: .ffiFailed(operation: "vrn_graph_snapshot", code: code)
                    )
                    return
                }
                guard let raw = out else {
                    continuation.resume(
                        throwing: .ffiFailed(
                            operation: "vrn_graph_snapshot",
                            code: VrnResultCode.internalError.rawValue
                        )
                    )
                    return
                }
                defer { vrnStringFree(raw) }
                guard let string = String(validatingCString: raw) else {
                    continuation.resume(
                        throwing: .snapshotDecodingFailed("snapshot is not valid UTF-8")
                    )
                    return
                }
                continuation.resume(returning: string)
            }
        }
        do {
            return try JSONDecoder().decode(GraphSnapshot.self, from: Data(json.utf8))
        } catch {
            throw GraphEngineError.snapshotDecodingFailed(error.localizedDescription)
        }
    }

    /// Replaces the whole DAG from a snapshot. Rejects `version != 2` in Rust.
    ///
    /// Explicitly `nonisolated` (see `createOperator`).
    nonisolated static func restoreGraphSnapshot(_ snapshot: GraphSnapshot) async throws(GraphEngineError) {
        let encoded: Data
        do {
            encoded = try JSONEncoder().encode(snapshot)
        } catch {
            throw GraphEngineError.snapshotEncodingFailed(error.localizedDescription)
        }
        guard let json = String(bytes: encoded, encoding: .utf8) else {
            throw GraphEngineError.snapshotEncodingFailed("snapshot is not valid UTF-8")
        }
        try await withCheckedThrowingContinuation { (continuation: GraphContinuation<Void>) in
            engineQueue.async {
                guard let context = requireGraphContext(
                    continuation,
                    operation: "vrn_graph_restore"
                ) else {
                    return
                }
                let result: Result<Void, GraphEngineError> = json.withCString { jsonPtr in
                    let code = vrnGraphRestore(context, jsonPtr)
                    guard code == VrnResultCode.ok.rawValue else {
                        return .failure(
                            .ffiFailed(operation: "vrn_graph_restore", code: code)
                        )
                    }
                    return .success(())
                }
                continuation.resume(with: result)
            }
        }
    }
}
