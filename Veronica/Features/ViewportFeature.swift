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
    }

    /// `Equatable` so `TestStore` can assert received actions by value.
    enum Action: Equatable {
        /// One display refresh elapsed: request a Rust tick.
        case frame
        /// Engine answered with fresh stats.
        case statsResponse(SceneStats)
    }

    @Dependency(\.engineClient) var engine

    var body: some Reducer<ViewportFeature.State, ViewportFeature.Action> {
        Reduce { state, action in
            switch action {
            case .frame:
                return .run { send in
                    let stats = await engine.tick()
                    await send(.statsResponse(stats))
                }
            case let .statsResponse(stats):
                state.tickCount = stats.tickCount
                state.entityCount = stats.entityCount
                state.frameCount += 1
                return .none
            }
        }
    }
}
