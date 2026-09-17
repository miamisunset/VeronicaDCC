import ComposableArchitecture
import SwiftUI

/// Third pane: the selection-driven parameter editor.
///
/// Reads the shared `NodeGraphFeature` store. Empty state when nothing (or a
/// dead id) is selected; otherwise the selected operator's kind header, a
/// Name field bound to the editor draft, generic numeric editors for every
/// `.float`/`.vec3`/`.integer` parameter (schema defaults cover absent keys,
/// so a fresh cube's `size`/`center` and a fresh sphere's
/// `segments`/`rings`/`radius`/`center` stay editable), and read-only rows
/// for the rest.
/// Name commits travel as `setParameter(id, "name", draft)`; numeric commits
/// travel as `setParameterTyped` carrying the `ParamValue` wire value (the
/// text setter stores `Text` verbatim, which the cook would reject).
/// Failures surface in-pane via `lastError` so a failed commit is never silent.
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
                let editable = OperatorParameterSchema.editableNumericParams(for: mirrored)
                let readOnly = mirrored.parameters.filter { editable[$0.key] == nil }
                if !editable.isEmpty {
                    Divider()
                    ForEach(editable.keys.sorted(), id: \.self) { key in
                        switch editable[key] {
                        case .float:
                            FloatParamField(
                                key: key,
                                text: store.editorFloatDrafts[key] ?? "",
                                onChanged: { store.send(.editorFloatChanged(key: key, draft: $0)) },
                                onCommitted: { store.send(.editorFloatCommitted(key: key)) },
                                onReverted: { store.send(.editorParamReverted(key: key)) }
                            )
                        case .vec3:
                            Vec3ParamField(
                                key: key,
                                components: store.editorVec3Drafts[key] ?? ["", "", ""],
                                onChanged: { axis, draft in
                                    store.send(.editorVec3Changed(key: key, axis: axis, draft: draft))
                                },
                                onCommitted: { store.send(.editorVec3Committed(key: key)) },
                                onReverted: { store.send(.editorParamReverted(key: key)) }
                            )
                        case .integer:
                            IntParamField(
                                key: key,
                                text: store.editorIntDrafts[key] ?? "",
                                validRange: OperatorParameterSchema.integerRange(
                                    for: mirrored.kind,
                                    key: key
                                ),
                                onChanged: { store.send(.editorIntChanged(key: key, draft: $0)) },
                                onCommitted: { store.send(.editorIntCommitted(key: key)) },
                                onReverted: { store.send(.editorParamReverted(key: key)) }
                            )
                        case .text, .flag, nil:
                            EmptyView()
                        }
                    }
                }
                if !readOnly.isEmpty {
                    Divider()
                    ForEach(readOnly.keys.sorted(), id: \.self) { key in
                        if let value = readOnly[key] {
                            parameterRow(key: key, value: value)
                        }
                    }
                }
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

    /// One read-only parameter row: `key`, payload, and lowercase type tag.
    private func parameterRow(key: String, value: ParameterValue) -> some View {
        HStack {
            Text(key)
                .font(.callout)
            Spacer()
            Text(value.displayText)
                .font(.callout)
                .foregroundStyle(.secondary)
            Text(value.typeLabel)
                .font(.caption)
                .foregroundStyle(.secondary)
        }
        .accessibilityIdentifier("parameterRow-\(key)")
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

#Preview("Cube parameters") {
    var state = NodeGraphFeature.State()
    state.operators = [
        OperatorMirror(
            id: 2,
            kind: "cube",
            name: "Box",
            parent: nil,
            position: GraphPosition(x: 40, y: 60),
            parameters: ["size": .vec3(1, 1, 1), "center": .vec3(0, 0, 0)]
        )
    ]
    state.selected = 2
    state.editorNameDraft = "Box"
    state.editorFloatDrafts = [:]
    state.editorVec3Drafts = ["size": ["1", "1", "1"], "center": ["0", "0", "0"]]
    return ParameterEditorView(
        store: Store(initialState: state) {
            NodeGraphFeature()
        }
    )
}

#Preview("Sphere parameters") {
    var state = NodeGraphFeature.State()
    state.operators = [
        OperatorMirror(
            id: 3,
            kind: "sphere",
            name: "Ball",
            parent: nil,
            position: GraphPosition(x: 40, y: 60),
            parameters: [
                "segments": .integer(32), "rings": .integer(16),
                "radius": .float(0.5), "center": .vec3(0, 0, 0)
            ]
        )
    ]
    state.selected = 3
    state.editorNameDraft = "Ball"
    state.editorIntDrafts = ["segments": "32", "rings": "16"]
    state.editorFloatDrafts = ["radius": "0.5"]
    state.editorVec3Drafts = ["center": ["0", "0", "0"]]
    return ParameterEditorView(
        store: Store(initialState: state) {
            NodeGraphFeature()
        }
    )
}
