import Foundation

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

    /// Tick the headless scene once, off the main actor.
    static func tick() async {
        await withCheckedContinuation { continuation in
            engineQueue.async {
                // Becomes `vrn_tick(context)` once the staticlib is linked.
                continuation.resume()
            }
        }
    }
}
