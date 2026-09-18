import ComposableArchitecture
import SwiftUI

/// Main window: Bevy viewport plus the rearrangeable Graph/Parameters pair.
///
/// Slice 2: Graph and Parameters swap order and stack in a row or column via
/// the toolbar buttons and the Panes menu — never border gestures or divider
/// drags beyond resizing. The arrangement is session-local feature state
/// (never persisted, never sent to Rust); the viewport stays first either
/// way. Dividers remain sizing-only with sensible minimums on every pane.
struct ContentView: View {
    @Bindable var store: StoreOf<AppFeature>

    var body: some View {
        Group {
            if store.nodeGraph.paneOrientation == .column {
                HSplitView {
                    viewportPane
                    VSplitView {
                        arrangedPanes
                    }
                }
            } else {
                HSplitView {
                    viewportPane
                    arrangedPanes
                }
            }
        }
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                Button("Swap Panes") {
                    store.send(.nodeGraph(.paneOrderToggled))
                }
                .accessibilityIdentifier("swapPanesButton")
            }
            ToolbarItem(placement: .primaryAction) {
                Button(store.nodeGraph.paneOrientation == .row ? "Stack Panes" : "Side by Side") {
                    store.send(.nodeGraph(.paneOrientationToggled))
                }
                .accessibilityIdentifier("paneOrientationButton")
            }
        }
        .navigationTitle("Veronica")
        .onAppear {
            // UI-test geometry pin (issue #94): no-op without the flag.
            if WindowLaunchOptions.hasFlag {
                WindowLaunchOptions.applyPinnedSize()
            }
        }
    }

    /// Bevy viewport: fixed first in every arrangement.
    private var viewportPane: some View {
        ViewportView(
            store: store.scope(state: \.viewport, action: \.viewport)
        )
        .frame(minWidth: 300, minHeight: 300)
    }

    /// Graph/Parameters in the stored order. Distinct view types keep
    /// SwiftUI identity stable across swaps, so selection, drafts, and dive
    /// state ride along untouched.
    @ViewBuilder
    private var arrangedPanes: some View {
        if store.nodeGraph.paneOrder == .graphFirst {
            graphPane
            parametersPane
        } else {
            parametersPane
            graphPane
        }
    }

    /// Node graph canvas.
    private var graphPane: some View {
        NodeGraphView(
            store: store.scope(state: \.nodeGraph, action: \.nodeGraph)
        )
        .frame(minWidth: 300, minHeight: 200)
    }

    /// Selection-driven parameter editor.
    private var parametersPane: some View {
        ParameterEditorView(
            store: store.scope(state: \.nodeGraph, action: \.nodeGraph)
        )
        .frame(minWidth: 240, minHeight: 160)
    }
}

#Preview {
    ContentView(
        store: Store(initialState: AppFeature.State()) {
            AppFeature()
        }
    )
}
