import ComposableArchitecture
import SwiftUI

/// Main window: Houdini-style split — Bevy viewport left, node graph right.
///
/// The divider is freely resizable with sensible minimums on both panes.
struct ContentView: View {
    @Bindable var store: StoreOf<AppFeature>

    var body: some View {
        HSplitView {
            ViewportView(
                store: store.scope(state: \.viewport, action: \.viewport)
            )
            .frame(minWidth: 300, minHeight: 300)
            NodeGraphView(
                store: store.scope(state: \.nodeGraph, action: \.nodeGraph)
            )
            .frame(minWidth: 300, minHeight: 300)
        }
        .navigationTitle("Veronica")
    }
}

#Preview {
    ContentView(
        store: Store(initialState: AppFeature.State()) {
            AppFeature()
        }
    )
}
