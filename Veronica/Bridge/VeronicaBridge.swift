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

    /// Serialized placeholder tick counter (engine queue only).
    private static let tickCounter = Mutex<UInt64>(0)

    /// Validate mesh buffer sizes via Rust without touching scene state.
    static func validateMesh(positionsCount: Int, indicesCount: Int) async -> Bool {
        await withCheckedContinuation { continuation in
            engineQueue.async {
                // Placeholder until libveronica.a is linked: mirror the Rust
                // stride rule (whole vertices + whole triangles).
                let valid = positionsCount % 3 == 0 && indicesCount % 3 == 0
                continuation.resume(returning: valid)
            }
        }
    }

    /// Tick once and return mirrored scene stats, off the main actor.
    ///
    /// Placeholder until `libveronica.a` is linked: advances a local counter
    /// and reports the Rust-owned demo scene size (camera, light, cube).
    /// Becomes `vrn_tick` + `vrn_tick_count` + `vrn_entity_count`.
    static func tickWithStats() async -> SceneStats {
        await withCheckedContinuation { continuation in
            engineQueue.async {
                let tick = tickCounter.withLock { counter in
                    counter += 1
                    return counter
                }
                continuation.resume(
                    returning: SceneStats(tickCount: tick, entityCount: 3)
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
    // were verified against the header and ADR-0002 (the `char *` inputs are
    // read-only on both sides despite the non-const pointer type).
    @_silgen_name("vrn_context_create")
    nonisolated private static func vrnContextCreate() -> UnsafeMutableRawPointer?
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

    /// Replaces the whole DAG from a snapshot. Rejects `version != 1` in Rust.
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
