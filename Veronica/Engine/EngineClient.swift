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
    /// Delete an operator id, cascading its subtree.
    var deleteOperator: @Sendable (UInt64) async throws(GraphEngineError) -> Void
    /// Fetch the whole-graph mirror. Owns the allocate/free boundary — the
    /// raw FFI pointer never escapes `EngineBridge`.
    var requestSnapshot: @Sendable () async throws(GraphEngineError) -> GraphSnapshot
    /// Replace the whole DAG from a snapshot. Rejects `version != 2`.
    var restoreSnapshot: @Sendable (GraphSnapshot) async throws(GraphEngineError) -> Void
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
            }
        )
    }()

    static let testValue = EngineClient(
        tick: { SceneStats(tickCount: 7, entityCount: 3) },
        createOperator: { _, _, _ in 1 },
        moveOperator: { _, _ in },
        renameOperator: { _, _ in },
        setParameter: { _, _, _ in },
        deleteOperator: { _ in },
        requestSnapshot: { GraphSnapshot(operators: []) },
        restoreSnapshot: { _ in }
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
