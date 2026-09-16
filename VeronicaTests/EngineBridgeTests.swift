import Testing

@testable import Veronica

/// Serialized: every test hits the shared process-lifetime engine context,
/// so parallel runs would interleave ticks and flake the oracles.
@Suite(.serialized)
struct EngineBridgeTests {
    @Test func validateMeshAcceptsWholeElements() async {
        await #expect(EngineBridge.validateMesh(positionsCount: 6, indicesCount: 3))
        await #expect(!EngineBridge.validateMesh(positionsCount: 4, indicesCount: 3))
        await #expect(!EngineBridge.validateMesh(positionsCount: 3, indicesCount: 4))
    }

    @Test func tickWithStatsAdvancesMonotonicallyOffMainActor() async {
        let first = await EngineBridge.tickWithStats()
        let second = await EngineBridge.tickWithStats()
        #expect(second.tickCount > first.tickCount)
        #expect(first.entityCount == 3)
    }

    /// Slice-2 oracle: every tick publishes a valid frame — a borrowed
    /// `IOSurface` handle with nonzero extents. Extents are dynamic since
    /// #30 (these tests are app-hosted, so the live pane re-targets the
    /// shared context to its backing size); this pins validity, not size.
    /// Exact propagation is covered in Rust
    /// (`viewport_resize_propagates_mid_life`).
    @Test func tickPublishesValidFrame() async throws {
        let stats = await EngineBridge.tickWithStats()
        let frame = try #require(stats.frame)
        #expect(frame.surfaceAddress != 0)
        #expect(frame.width > 0)
        #expect(frame.height > 0)
    }

    /// The graph externs are linked from `libveronica.a` and resolved
    /// at runtime. This smoke pin proves the allocate/free boundary end to
    /// end: the snapshot decodes as the current schema version. Mutating intents stay covered
    /// by the mock-backed tests and the UI flow (mock and real-engine modes)
    /// to avoid shared-context races between parallel unit tests.
    @Test func graphSnapshotDecodesAsCurrentVersion() async throws {
        let snapshot = try await EngineBridge.requestGraphSnapshot()
        #expect(snapshot.version == GraphSnapshot.currentVersion)
    }
}
