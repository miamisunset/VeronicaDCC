import ComposableArchitecture
import CoreGraphics
import SwiftUI

/// One operator box on the node graph canvas.
///
/// Boxes are lightweight views over the TCA mirror (not `Canvas` drawings)
/// so each box owns precise tap, double-tap, drag, menu, and accessibility
/// hooks. Rendering stays event-driven: boxes re-render only when mirror,
/// selection, pan, or preview state changes — never on a frame loop.
struct OperatorBoxView: View {
    /// Shared feature store.
    @Bindable var store: StoreOf<NodeGraphFeature>
    /// Mirrored operator id rendered by this box. Dead ids render nothing.
    let operatorId: UInt64
    /// Canvas focus binding, claimed on tap so Enter/Delete keys route.
    var canvasFocus: FocusState<Bool>.Binding

    @FocusState private var fieldFocused: Bool

    var body: some View {
        if let mirrored = store.operators.first(where: { $0.id == operatorId }) {
            if store.renaming == mirrored.id {
                // Renaming: no combining, so the overlay field keeps its own
                // identifier for tests and VoiceOver.
                boxContent(mirrored)
                    .position(center(for: mirrored))
            } else {
                boxContent(mirrored)
                    .accessibilityElement(children: .combine)
                    .accessibilityIdentifier("operatorBox-\(mirrored.id)")
                    .accessibilityLabel(mirrored.name)
                    .accessibilityValue(positionDescription(for: mirrored))
                    .accessibilityAddTraits(.isButton)
                    .accessibilityAddTraits(store.selected == mirrored.id ? .isSelected : [])
                    .position(center(for: mirrored))
            }
        }
    }

    /// Center of the box in canvas space, following the drag preview.
    private func center(for mirrored: OperatorMirror) -> CGPoint {
        var placed = mirrored
        if let preview = store.dragPreview, preview.id == mirrored.id {
            placed.position = preview.position
        }
        let frame = GraphCanvasLayout.boxFrame(for: placed, pan: store.panOffset)
        return CGPoint(x: frame.midX, y: frame.midY)
    }

    private func boxContent(_ mirrored: OperatorMirror) -> some View {
        RoundedRectangle(cornerRadius: 10)
            .fill(Color(nsColor: .controlBackgroundColor))
            .frame(
                width: GraphCanvasLayout.boxSize.width,
                height: GraphCanvasLayout.boxSize.height
            )
            .overlay {
                if store.selected == mirrored.id {
                    RoundedRectangle(cornerRadius: 10)
                        .strokeBorder(Color.accentColor, lineWidth: 3)
                }
            }
            .overlay {
                if store.renaming == mirrored.id {
                    renameField
                } else {
                    VStack(spacing: 2) {
                        Text(mirrored.name)
                            .font(.headline)
                            .lineLimit(1)
                        Text(kindLabel(for: mirrored))
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                    }
                    .padding(.horizontal, 12)
                }
            }
            .gesture(
                TapGesture(count: 2).exclusively(before: TapGesture(count: 1))
                    .onEnded { outcome in
                        switch outcome {
                        case .first:
                            canvasFocus.wrappedValue = true
                            store.send(.operatorSelected(mirrored.id))
                            store.send(.diveRequested(mirrored.id))
                        case .second:
                            canvasFocus.wrappedValue = true
                            store.send(.operatorSelected(mirrored.id))
                        }
                    }
            )
            .simultaneousGesture(
                // Measured in the stable canvas space, never `.local`: this
                // gesture moves its own box, so its own space would shift
                // under the finger and the translation would feed back.
                DragGesture(
                    minimumDistance: 1,
                    coordinateSpace: .named(GraphCanvasLayout.canvasSpaceName)
                )
                    .onChanged { value in
                        // Touch-down reports a zero translation; only real
                        // movement previews, so taps never stain the preview.
                        guard GraphCanvasLayout.isDrag(value.translation) else {
                            return
                        }
                        let next = GraphCanvasLayout.draggedPosition(
                            from: mirrored.position,
                            translation: value.translation
                        )
                        store.send(.dragPreviewChanged(id: mirrored.id, position: next))
                    }
                    .onEnded { value in
                        if GraphCanvasLayout.isDrag(value.translation) {
                            let next = GraphCanvasLayout.draggedPosition(
                                from: mirrored.position,
                                translation: value.translation
                            )
                            store.send(.dragCommitted(id: mirrored.id, position: next))
                        } else {
                            canvasFocus.wrappedValue = true
                            store.send(.operatorSelected(mirrored.id))
                            store.send(.dragCancelled)
                        }
                    }
            )
            .contextMenu {
                Button("Dive In") {
                    store.send(.diveRequested(mirrored.id))
                }
                Button("Rename") {
                    store.send(.renameStarted(mirrored.id))
                }
                Divider()
                Button("Delete", role: .destructive) {
                    store.send(.deleteRequested(mirrored.id))
                }
            }
    }

    /// Inline rename overlay, committed with Enter or cancelled with Escape.
    private var renameField: some View {
        TextField(
            "Name",
            text: Binding(
                get: { store.renameDraft },
                set: { store.send(.renameDraftChanged($0)) }
            )
        )
        .textFieldStyle(.roundedBorder)
        .font(.headline)
        .focused($fieldFocused)
        .onSubmit {
            store.send(.renameCommitted)
        }
        .onExitCommand {
            store.send(.renameCancelled)
        }
        .onAppear {
            fieldFocused = true
        }
        .frame(width: GraphCanvasLayout.boxSize.width - 24)
        .accessibilityIdentifier("operatorRenameField")
    }

    /// Registry display name for the box subtitle, falling back to the kind.
    private func kindLabel(for mirrored: OperatorMirror) -> String {
        OperatorTypeRegistry.all.first { $0.kind == mirrored.kind }?.displayName
            ?? mirrored.kind
    }

    /// Committed canvas position, rounded for VoiceOver and UI tests.
    private func positionDescription(for mirrored: OperatorMirror) -> String {
        "at \(Int(mirrored.position.x)), \(Int(mirrored.position.y))"
    }
}

#Preview {
    @Previewable @FocusState var canvasFocused: Bool
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
    return OperatorBoxView(
        store: Store(initialState: state) {
            NodeGraphFeature()
        },
        operatorId: 1,
        canvasFocus: $canvasFocused
    )
}
