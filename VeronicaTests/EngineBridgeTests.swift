import Testing

@testable import Veronica

struct EngineBridgeTests {
    @Test func validateMeshAcceptsWholeElements() async {
        await #expect(EngineBridge.validateMesh(positionsCount: 6, indicesCount: 3))
        await #expect(!EngineBridge.validateMesh(positionsCount: 4, indicesCount: 3))
        await #expect(!EngineBridge.validateMesh(positionsCount: 3, indicesCount: 4))
    }

    @Test func tickCompletesOffMainActor() async {
        await EngineBridge.tick()
    }
}
