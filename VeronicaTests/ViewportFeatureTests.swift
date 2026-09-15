import ComposableArchitecture
import Testing

@testable import Veronica

/// Reducer contract: each `.frame` ticks the engine off-MainActor and the
/// answered stats land in state with an incremented frame count.
@MainActor
struct ViewportFeatureTests {
    @Test func framePublishesEngineStats() async {
        let store = TestStore(initialState: ViewportFeature.State()) {
            ViewportFeature()
        } withDependencies: {
            $0.engineClient.tick = {
                SceneStats(tickCount: 7, entityCount: 3)
            }
        }
        await store.send(.frame)
        await store.receive(.statsResponse(SceneStats(tickCount: 7, entityCount: 3))) {
            $0.tickCount = 7
            $0.entityCount = 3
            $0.frameCount = 1
        }
    }

    /// Delayed effects resolve out of order at 60–144 Hz: a stale response
    /// must not regress published stats.
    @Test func staleStatsResponseIsDropped() async {
        let store = TestStore(initialState: ViewportFeature.State()) {
            ViewportFeature()
        } withDependencies: {
            $0.engineClient.tick = {
                SceneStats(tickCount: 7, entityCount: 3)
            }
        }
        await store.send(.frame)
        await store.receive(.statsResponse(SceneStats(tickCount: 7, entityCount: 3))) {
            $0.tickCount = 7
            $0.entityCount = 3
            $0.frameCount = 1
        }
        store.dependencies.engineClient.tick = {
            SceneStats(tickCount: 2, entityCount: 3)
        }
        await store.send(.frame)
        await store.receive(.statsResponse(SceneStats(tickCount: 2, entityCount: 3)))
    }
}
