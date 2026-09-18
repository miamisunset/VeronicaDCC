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
        let pick = ViewportPick(node: 3, polygon: 5)
        let store = TestStore(initialState: ViewportFeature.State()) {
            ViewportFeature()
        } withDependencies: {
            $0.engineClient.sendTapNDC = { cursor in
                recorded.withLock { $0.append(cursor) }
                return pick
            }
        }
        let cursor = CursorNDC(x: 0.25, y: -0.5)
        await store.send(.tapAt(cursor: cursor))
        let taps = recorded.withLock { $0 }
        #expect(taps.count == 1)
        #expect(taps.first == cursor)
        await store.receive(.pickResponse(.success(pick))) {
            $0.selection = pick
        }
    }

    /// A hit paints the mirror with the returned identity.
    @Test func pickHitMirrorsReturnedIdentity() async {
        let pick = ViewportPick(node: 1, polygon: 0)
        let store = TestStore(initialState: ViewportFeature.State()) {
            ViewportFeature()
        } withDependencies: {
            $0.engineClient.sendTapNDC = { _ in pick }
        }
        await store.send(.tapAt(cursor: CursorNDC(x: 0, y: 0)))
        await store.receive(.pickResponse(.success(pick))) {
            $0.selection = pick
        }
    }

    /// A background miss clears the mirror (Rust cleared with it).
    @Test func pickMissClearsMirror() async {
        let store = TestStore(
            initialState: ViewportFeature.State(selection: ViewportPick(node: 1, polygon: 0))
        ) {
            ViewportFeature()
        } withDependencies: {
            $0.engineClient.sendTapNDC = { _ in nil }
        }
        await store.send(.tapAt(cursor: CursorNDC(x: 0.9, y: 0.9)))
        await store.receive(.pickResponse(.success(nil))) {
            $0.selection = nil
        }
    }

    /// An engine error leaves the mirror untouched (Rust state is
    /// untouched on error paths, so the mirror keeps its value).
    @Test func pickFailureKeepsMirror() async {
        let held = ViewportPick(node: 2, polygon: 7)
        let store = TestStore(initialState: ViewportFeature.State(selection: held)) {
            ViewportFeature()
        } withDependencies: {
            $0.engineClient.sendTapNDC = { (_: CursorNDC) async throws(GraphEngineError) -> ViewportPick? in
                throw GraphEngineError.ffiFailed(operation: "vrn_viewport_pick", code: 3)
            }
        }
        await store.send(.tapAt(cursor: CursorNDC(x: 0, y: 0)))
        await store.receive(
            .pickResponse(
                .failure(GraphEngineError.ffiFailed(operation: "vrn_viewport_pick", code: 3))
            )
        )
    }

    /// Nav intents carry no observations: state (including stats) is
    /// untouched, so rapid drags never disturb the tick mirror. The tap
    /// still answers through the pick seam (a miss here — the test client
    /// selects nothing), but the answer changes no state either.
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
        await store.receive(.pickResponse(.success(nil)))
        await store.send(.frameAll)
    }

    /// Rapid taps serialize latest-wins: the superseded pick's answer is
    /// dropped, so a stale hit-then-miss double-tap cannot leave the
    /// mirror disagreeing with Rust.
    @Test func rapidSecondTapWins() async throws {
        struct Parked: Sendable {
            var cursor: CursorNDC
            var continuation: CheckedContinuation<ViewportPick?, Error>
        }
        let parked = Mutex<[Parked]>([])
        // Explicitly typed: the unannotated form infers `throws(any
        // Error)`, which does not convert to the client's typed throw.
        let gatedPick: @Sendable (CursorNDC) async throws(GraphEngineError) -> ViewportPick? = { cursor in
            do {
                return try await withCheckedThrowingContinuation { continuation in
                    parked.withLock { $0.append(Parked(cursor: cursor, continuation: continuation)) }
                }
            } catch {
                throw GraphEngineError.ffiFailed(operation: "rapidSecondTapWins", code: -1)
            }
        }
        let store = TestStore(initialState: ViewportFeature.State()) {
            ViewportFeature()
        } withDependencies: {
            $0.engineClient.sendTapNDC = gatedPick
        }
        let first = CursorNDC(x: -0.5, y: 0)
        let second = CursorNDC(x: 0.5, y: 0)
        await store.send(.tapAt(cursor: first))
        await store.send(.tapAt(cursor: second))
        // Both effects parked (arrival order is irrelevant — each parks
        // under its own cursor); the poll is bounded so a stuck effect
        // fails here instead of deadlocking the suite.
        for _ in 0..<100 {
            if parked.withLock({ $0.count }) >= 2 { break }
            try await Task.sleep(for: .milliseconds(10))
        }
        let stale = try #require(parked.withLock { $0.first(where: { $0.cursor == first }) })
        let latest = try #require(parked.withLock { $0.first(where: { $0.cursor == second }) })
        // The superseded answer is dropped (its effect was cancelled); only
        // the newest tap paints the mirror.
        stale.continuation.resume(returning: ViewportPick(node: 1, polygon: 0))
        let pick = ViewportPick(node: 2, polygon: 3)
        latest.continuation.resume(returning: pick)
        await store.receive(.pickResponse(.success(pick))) {
            $0.selection = pick
        }
    }
}
