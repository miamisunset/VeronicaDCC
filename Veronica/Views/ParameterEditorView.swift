import ComposableArchitecture
import SwiftUI

/// Third pane: the selection-driven parameter editor (slice 1: name only).
///
/// Reads the shared `NodeGraphFeature` store. Empty state when nothing (or a
/// dead id) is selected; otherwise the selected operator's kind header plus
/// a Name field bound to the editor draft. Commits travel as
/// `setParameter(id, "name", draft)`; failures surface in-pane via
/// `lastError` so a failed commit is never silent.
struct ParameterEditorView: View {
    /// Shared feature store.
    @Bindable var store: StoreOf<NodeGraphFeature>

    @FocusState private var nameFocused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Parameters")
                .font(.headline)
                .accessibilityIdentifier("parameterEditorPane")
            Divider()
            if let mirrored = store.operators.first(where: { $0.id == store.selected }) {
                Text(kindLabel(for: mirrored))
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                TextField(
                    "Name",
                    text: Binding(
                        get: { store.editorNameDraft },
                        set: { store.send(.editorNameChanged($0)) }
                    )
                )
                .textFieldStyle(.roundedBorder)
                .focused($nameFocused)
                .onSubmit {
                    store.send(.editorNameCommitted)
                }
                .onExitCommand {
                    store.send(.editorNameReverted)
                }
                .accessibilityIdentifier("parameterNameField")
                .accessibilityLabel("Operator name")
                if let message = store.lastError {
                    Text(message)
                        .font(.caption)
                        .foregroundStyle(.red)
                        .accessibilityIdentifier("parameterEditorError")
                }
            } else {
                Text("Select an operator to edit its parameters.")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .accessibilityIdentifier("parameterEmptyState")
            }
            Spacer()
        }
        .padding(12)
        .onChange(of: nameFocused) {
            // Focus loss commits unconfirmed typing (submit already cleared
            // the dirty flag, so Enter never commits twice).
            if !nameFocused, store.editorDirty {
                store.send(.editorNameCommitted)
            }
        }
    }

    /// Registry display name for the kind header, falling back to the kind.
    private func kindLabel(for mirrored: OperatorMirror) -> String {
        OperatorTypeRegistry.all.first { $0.kind == mirrored.kind }?.displayName
            ?? mirrored.kind
    }
}

#Preview {
    var state = NodeGraphFeature.State()
    state.operators = [
        OperatorMirror(
            id: 1,
            kind: "container",
            name: "Hero",
            parent: nil,
            position: GraphPosition(x: 120, y: 80)
        )
    ]
    state.selected = 1
    state.editorNameDraft = "Hero"
    return ParameterEditorView(
        store: Store(initialState: state) {
            NodeGraphFeature()
        }
    )
}
