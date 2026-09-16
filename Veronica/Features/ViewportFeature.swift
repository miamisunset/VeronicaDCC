import ComposableArchitecture
import Foundation

/// Left-pane feature: the Bevy-driven viewport.
///
/// State holds only mirrored observations (`tickCount`, `entityCount`);
/// scene content itself lives in Rust ECS. Each display refresh sends
/// `.frame`; the effect ticks the engine off-MainActor and publishes stats.
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
    }

    /// `Equatable` so `TestStore` can assert received actions by value.
    enum Action: Equatable {
        /// One display refresh elapsed: request a Rust tick, carrying the
        /// Metal host's pacing observations (fps + last present cost).
        case frame(pacing: FramePacing)
        /// Engine answered with fresh stats.
        case statsResponse(SceneStats)
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
            }
        }
    }
}
