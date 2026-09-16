import Testing

@testable import Veronica

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
