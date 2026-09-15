import ComposableArchitecture
import SwiftUI

/// Right pane: scaffold for the future node graph editor.
///
/// Empty by agreement (grill Q3). Keeps the accessibility hook
/// (`nodeGraphPane`) that the UI test and future graph UI will target.
struct NodeGraphView: View {
    @Bindable var store: StoreOf<NodeGraphFeature>

    var body: some View {
        VStack(spacing: 8) {
            Text("Node Graph")
                .font(.headline)
            Text("Procedural graph editor lands here.")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .accessibilityIdentifier("nodeGraphPane")
    }
}
