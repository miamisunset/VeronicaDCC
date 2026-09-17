import ComposableArchitecture
import Foundation

/// Left-pane feature: the Bevy-driven viewport.
///
/// State holds only mirrored observations (`tickCount`, `entityCount`);
/// scene content itself lives in Rust ECS. Each display refresh sends
/// `.frame`; the effect ticks the engine off-MainActor and publishes stats.
/// Cancellation scope for the viewport pick effect: taps serialize,
/// latest wins.
///
/// File-scope and explicitly `nonisolated`: nested in the MainActor-
/// defaulted reducer, the `Hashable` conformance would be actor-isolated
/// and fail TCA's `Sendable` requirement on cancellation ids.
nonisolated private enum ViewportCancelID: Hashable, Sendable {
    /// In-flight pick, superseded by the next tap.
    case tap
}

@Reducer
struct ViewportFeature {
    @ObservableState
    struct State: Equatable, Sendable {
        /// Last observed Rust tick counter.
        var tickCount: UInt64 = 0
        /// Last observed demo-scene entity count.
        var entityCount: UInt64 = 0
        /// Frames presented by Swift (ticks requested).
        var frameCount: UInt64 = 0
        /// Latest published frame handle for the Metal host.
        var frame: VideoFrame?
        /// Wall-clock cost of the last published tick, in microseconds.
        var tickMicroseconds: UInt64 = 0
        /// Per-stage splits of the last published tick.
        var timings = TickTimings()
        /// Smoothed display-link rate (achieved presentation fps).
        var frameRate: Double = 0
        /// Last completed present cost, in microseconds.
        var presentMicroseconds: UInt64 = 0
        /// Read-only mirror of the Rust Selection (issue #64): the picked
        /// `(node, face)`, or `nil` when nothing is selected. Rust owns the
        /// truth; a background miss clears both sides, an engine error
        /// leaves the mirror untouched.
        var selection: ViewportPick?
    }

    /// `Equatable` so `TestStore` can assert received actions by value.
    enum Action: Equatable {
        /// One display refresh elapsed: request a Rust tick, carrying the
        /// Metal host's pacing observations (fps + last present cost).
        case frame(pacing: FramePacing)
        /// Engine answered with fresh stats.
        case statsResponse(SceneStats)
        /// Orbit the camera by a drag delta in pixels (LMB-drag).
        case orbitDelta(dx: Double, dy: Double)
        /// Pan camera and pivot rigidly by a drag delta in pixels
        /// (MMB-drag, Command+LMB).
        case panDelta(dx: Double, dy: Double)
        /// Dolly toward `cursor` (NDC) by `logFactor` (wheel, RMB-drag,
        /// Option+LMB).
        case dolly(logFactor: Double, cursor: CursorNDC)
        /// Tap the viewport at `cursor` (NDC): Pick input (issue #59), not
        /// navigation. The reducer forwards it to the engine seam; the tap
        /// never moves the camera.
        case tapAt(cursor: CursorNDC)
        /// Engine answered a tap with the painted selection (`nil` is the
        /// background-miss mirror: Rust cleared, Swift clears with it).
        case pickResponse(TaskResult<ViewportPick?>)
        /// Frame the whole scene (snap to fit, `F` key).
        case frameAll
    }

    @Dependency(\.engineClient) var engine

    var body: some Reducer<ViewportFeature.State, ViewportFeature.Action> {
        Reduce { state, action in
            switch action {
            case let .frame(pacing):
                state.frameRate = pacing.fps
                state.presentMicroseconds = pacing.presentMicroseconds
                return .run { send in
                    let stats = await engine.tick()
                    await send(.statsResponse(stats))
                }
            case let .statsResponse(stats):
                // Frames are independent effects with no ordering guarantee:
                // a delayed response must never overwrite newer stats.
                guard stats.tickCount > state.tickCount else { return .none }
                state.tickCount = stats.tickCount
                state.entityCount = stats.entityCount
                state.frame = stats.frame
                state.tickMicroseconds = stats.tickMicroseconds
                state.timings = stats.timings
                state.frameCount += 1
                return .none
            case let .orbitDelta(dx, dy):
                // Fire-and-forget nav intents: no state change, no answer.
                // The closure is snapshotted here (MainActor) because the
                // `.run` body is nonisolated; the client hops to the engine
                // queue itself.
                let orbit = engine.viewportOrbit
                return .run { _ in orbit(dx, dy) }
            case let .panDelta(dx, dy):
                let pan = engine.viewportPan
                return .run { _ in pan(dx, dy) }
            case let .dolly(logFactor, cursor):
                let dolly = engine.viewportDolly
                return .run { _ in dolly(logFactor, cursor) }
            case let .tapAt(cursor):
                // Pick intent, not navigation: the camera never moves, but
                // the Selection mirror now answers. The closure is
                // snapshotted here (MainActor) because the `.run` body is
                // nonisolated; the client hops to the engine queue itself.
                //
                // Latest wins: FFI executes taps FIFO on the serial engine
                // queue, but responses race back on concurrent tasks — a
                // stale hit-then-miss double-tap could otherwise leave the
                // mirror disagreeing with Rust. Cancelling in flight keeps
                // the newest tap's answer.
                let tap = engine.sendTapNDC
                return .run { send in
                    await send(.pickResponse(TaskResult { try await tap(cursor) }))
                }
                .cancellable(id: ViewportCancelID.tap, cancelInFlight: true)
            case let .pickResponse(.success(pick)):
                // Hit paints, miss clears — both sides already agree, the
                // mirror just catches up.
                state.selection = pick
                return .none
            case .pickResponse(.failure):
                // The engine state is untouched on error paths, so the
                // mirror keeps its stale value rather than inventing one.
                return .none
            case .frameAll:
                let frameAll = engine.viewportFrameAll
                return .run { _ in frameAll() }
            }
        }
    }
}
