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
}

extension EngineClient: DependencyKey {
    static let liveValue = EngineClient(
        tick: { await EngineBridge.tickWithStats() }
    )

    static let testValue = EngineClient(
        tick: { SceneStats(tickCount: 7, entityCount: 3) }
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
