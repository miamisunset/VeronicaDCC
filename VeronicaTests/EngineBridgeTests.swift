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
}
