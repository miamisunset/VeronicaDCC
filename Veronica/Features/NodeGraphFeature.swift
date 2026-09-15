import ComposableArchitecture
import Foundation

/// Right-pane feature: scaffold for the future node graph editor.
///
/// Holds only placeholder content today. It will mirror the Rust DAG
/// read-only once the graph FFI surface exists; the TCA boundary (state,
/// action, scope in `AppFeature`) already reserves its shape.
@Reducer
struct NodeGraphFeature {
    @ObservableState
    struct State: Equatable, Sendable {
        /// Pane title, owned by Swift as pure view state.
        var title = "Node Graph"
    }

    enum Action {
        /// Pane appeared. No-op until graph content exists.
        case appeared
    }

    var body: some Reducer<NodeGraphFeature.State, NodeGraphFeature.Action> {
        Reduce { _, action in
            switch action {
            case .appeared:
                return .none
            }
        }
    }
}
