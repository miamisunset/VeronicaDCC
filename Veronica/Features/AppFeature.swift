import ComposableArchitecture
import Foundation

/// Root feature: owns the split-window layout state.
///
/// Children: `viewport` (Bevy-driven left pane) and `nodeGraph` (right-pane
/// scaffold). Future destinations (inspector, dialogs) attach here via
/// `@Presents`.
@Reducer
struct AppFeature {
    @ObservableState
    struct State: Equatable, Sendable {
        /// Left-pane viewport state.
        var viewport = ViewportFeature.State()
        /// Right-pane node graph scaffold state.
        var nodeGraph = NodeGraphFeature.State()
    }

    enum Action {
        /// Forwarded viewport actions.
        case viewport(ViewportFeature.Action)
        /// Forwarded node graph actions.
        case nodeGraph(NodeGraphFeature.Action)
    }

    var body: some Reducer<AppFeature.State, AppFeature.Action> {
        Scope(state: \.viewport, action: \.viewport) {
            ViewportFeature()
        }
        Scope(state: \.nodeGraph, action: \.nodeGraph) {
            NodeGraphFeature()
        }
        Reduce { _, _ in .none }
    }
}
