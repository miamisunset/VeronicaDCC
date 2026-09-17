import ComposableArchitecture
import Synchronization
import Testing

@testable import Veronica

/// Reducer contract for nav intents (issue #41): each gesture action runs a
/// fire-and-forget client call with the mapped units and changes no state.
@MainActor
struct ViewportNavEffectTests {
    @Test func orbitDeltaInvokesClient() async {
        let recorded = Mutex<[(Double, Double)]>([])
        let store = TestStore(initialState: ViewportFeature.State()) {
            ViewportFeature()
        } withDependencies: {
            $0.engineClient.viewportOrbit = { dx, dy in
                recorded.withLock { $0.append((dx, dy)) }
            }
        }
        await store.send(.orbitDelta(dx: 12, dy: -7))
        let orbits = recorded.withLock { $0 }
        #expect(orbits.count == 1)
        #expect(orbits.first?.0 == 12)
        #expect(orbits.first?.1 == -7)
    }

    @Test func panDeltaInvokesClient() async {
        let recorded = Mutex<[(Double, Double)]>([])
        let store = TestStore(initialState: ViewportFeature.State()) {
            ViewportFeature()
        } withDependencies: {
            $0.engineClient.viewportPan = { dx, dy in
                recorded.withLock { $0.append((dx, dy)) }
            }
        }
        await store.send(.panDelta(dx: -3, dy: 9))
        let pans = recorded.withLock { $0 }
        #expect(pans.count == 1)
        #expect(pans.first?.0 == -3)
        #expect(pans.first?.1 == 9)
    }

    @Test func dollyInvokesClient() async {
        let logFactors = Mutex<[Double]>([])
        let cursors = Mutex<[CursorNDC]>([])
        let store = TestStore(initialState: ViewportFeature.State()) {
            ViewportFeature()
        } withDependencies: {
            $0.engineClient.viewportDolly = { logFactor, cursor in
                logFactors.withLock { $0.append(logFactor) }
                cursors.withLock { $0.append(cursor) }
            }
        }
        let cursor = CursorNDC(x: 0.25, y: -0.5)
        await store.send(.dolly(logFactor: 0.1, cursor: cursor))
        let factors = logFactors.withLock { $0 }
        #expect(factors.count == 1)
        #expect(factors.first == 0.1)
        let gotCursors = cursors.withLock { $0 }
        #expect(gotCursors.count == 1)
        #expect(gotCursors.first == cursor)
    }

    @Test func frameAllInvokesClient() async {
        let recorded = Mutex(0)
        let store = TestStore(initialState: ViewportFeature.State()) {
            ViewportFeature()
        } withDependencies: {
            $0.engineClient.viewportFrameAll = {
                recorded.withLock { $0 += 1 }
            }
        }
        await store.send(.frameAll)
        let count = recorded.withLock { $0 }
        #expect(count == 1)
    }

    @Test func tapAtInvokesClientWithNDCIntact() async {
        let recorded = Mutex<[CursorNDC]>([])
        let store = TestStore(initialState: ViewportFeature.State()) {
            ViewportFeature()
        } withDependencies: {
            $0.engineClient.sendTapNDC = { cursor in
                recorded.withLock { $0.append(cursor) }
            }
        }
        let cursor = CursorNDC(x: 0.25, y: -0.5)
        await store.send(.tapAt(cursor: cursor))
        let taps = recorded.withLock { $0 }
        #expect(taps.count == 1)
        #expect(taps.first == cursor)
    }

    /// Nav intents carry no observations: state (including stats) is
    /// untouched, so rapid drags never disturb the tick mirror.
    @Test func navIntentsLeaveStateUntouched() async {
        let store = TestStore(
            initialState: ViewportFeature.State(tickCount: 5, entityCount: 3, frameCount: 2)
        ) {
            ViewportFeature()
        }
        await store.send(.orbitDelta(dx: 1, dy: 1))
        await store.send(.panDelta(dx: 1, dy: 1))
        await store.send(.dolly(logFactor: 0.01, cursor: CursorNDC(x: 0, y: 0)))
        await store.send(.tapAt(cursor: CursorNDC(x: 0.25, y: -0.5)))
        await store.send(.frameAll)
    }
}
