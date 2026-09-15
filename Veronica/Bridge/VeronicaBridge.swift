import Foundation
import Synchronization

/// Swift side of the Rust bridge.
///
/// Ownership: Rust owns domain & scene state (mesh topology, morph weights,
/// bone transforms, DAG topology, undo/redo). Swift owns presentation & UI
/// view state (panels, filters, tabs, dialogs) and mirrors Rust read-only.
///
/// Threading: the Bevy scene must never tick on MainActor. All `vrn_*`
/// calls go through `engineQueue`. This file intentionally uses raw C
/// declarations until `rust/target/include/veronica.h` is wired into Xcode
/// via Header Search Paths (see AGENTS.md).
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
}
