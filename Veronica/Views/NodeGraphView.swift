import ComposableArchitecture
import CoreGraphics
import SwiftUI

/// Right pane: the procedural node graph editor (slice 1).
///
/// The dotted grid draws in an event-driven `Canvas` (never frame-driven);
/// operator boxes are lightweight views for precise gestures and VoiceOver.
/// Dive, breadcrumb, and pan are Swift-local; mutations commit through FFI.
struct NodeGraphView: View {
    @Bindable var store: StoreOf<NodeGraphFeature>

    @FocusState private var canvasFocused: Bool
    @State private var didAppear = false
    @State private var backgroundTranslation: CGSize?

    var body: some View {
        VStack(spacing: 0) {
            breadcrumbBar
            Divider()
            canvasStack
            if let message = store.lastError {
                Text(message)
                    .font(.caption)
                    .foregroundStyle(.red)
                    .padding(6)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .accessibilityIdentifier("nodeGraphError")
            }
        }
        .onAppear {
            guard !didAppear else {
                return
            }
            didAppear = true
            canvasFocused = true
            store.send(.appeared)
        }
    }

    /// Path from the root to the shown network. Tolerates dead ids.
    ///
    /// Every jump reclaims canvas focus: key handling (Enter/Delete/arrows)
    /// lives on the canvas, and a focused breadcrumb button would otherwise
    /// swallow Enter for activation.
    private var breadcrumbBar: some View {
        HStack(spacing: 6) {
            Button("Root") {
                canvasFocused = true
                store.send(.breadcrumbSelected(depth: 0))
            }
            .buttonStyle(.link)
            .focusable(false)
            .accessibilityIdentifier("breadcrumbRoot")
            ForEach(Array(store.path.enumerated()), id: \.offset) { depth, id in
                Image(systemName: "chevron.right")
                    .foregroundStyle(.secondary)
                    .accessibilityHidden(true)
                Button(name(for: id)) {
                    canvasFocused = true
                    store.send(.breadcrumbSelected(depth: depth + 1))
                }
                .buttonStyle(.link)
                .focusable(false)
                .accessibilityIdentifier("breadcrumbSegment-\(depth)")
            }
            Spacer()
            if GraphLaunchOptions.isMockEngineEnabled {
                Text("Mock engine")
                    .font(.caption)
                    .foregroundStyle(.orange)
                    .accessibilityIdentifier("mockEngineBadge")
            }
            if !store.path.isEmpty {
                Button("Back") {
                    canvasFocused = true
                    store.send(.backToParent)
                }
                .focusable(false)
                .accessibilityIdentifier("breadcrumbBack")
            }
        }
        .padding(8)
    }

    /// Display name for a breadcrumb segment, tolerating dead ids.
    private func name(for id: UInt64) -> String {
        store.operators.first { $0.id == id }?.name ?? "Unknown"
    }

    private var canvasStack: some View {
        GeometryReader { _ in
            ZStack(alignment: .topLeading) {
                Canvas { context, size in
                    drawGrid(context: &context, size: size, pan: store.panOffset)
                }
                .accessibilityElement()
                .accessibilityIdentifier("nodeGraphPane")
                .accessibilityLabel("Node graph canvas")
                ForEach(store.visibleOperators) { mirrored in
                    OperatorBoxView(
                        store: store,
                        operatorId: mirrored.id,
                        canvasFocus: $canvasFocused
                    )
                }
                if store.visibleOperators.isEmpty {
                    Text("Right-click the canvas to add a container. Double-click a box to dive in.")
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .multilineTextAlignment(.center)
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                        .accessibilityIdentifier("nodeGraphEmptyHint")
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .contentShape(Rectangle())
            .onContinuousHover { phase in
                guard case let .active(point) = phase else {
                    return
                }
                store.send(
                    .hoverPositionChanged(GraphPosition(x: point.x, y: point.y))
                )
            }
            .gesture(
                DragGesture(minimumDistance: 1, coordinateSpace: .local)
                    .onChanged { value in
                        let last = backgroundTranslation ?? .zero
                        let delta = CGSize(
                            width: value.translation.width - last.width,
                            height: value.translation.height - last.height
                        )
                        backgroundTranslation = value.translation
                        store.send(.panChanged(delta: delta))
                    }
                    .onEnded { _ in
                        backgroundTranslation = nil
                    }
            )
            .contextMenu {
                ForEach(OperatorTypeRegistry.all, id: \.kind) { definition in
                    Button("Add \(definition.displayName)") {
                        store.send(
                            .createRequested(
                                kind: definition.kind,
                                position: store.pendingCreatePosition
                            )
                        )
                    }
                }
            }
        }
        .focusable()
        .focused($canvasFocused)
        .onChange(of: store.renaming) {
            // The rename field claims focus while open so typing lands in
            // it; the canvas reclaims focus on commit/cancel for shortcuts.
            canvasFocused = store.renaming == nil
        }
        .onKeyPress(phases: .down) { press in
            // Backspace (U+007F) never matches a specific-key handler — not
            // even an explicitly constructed equivalent — so it is matched
            // by character here. Evidence: a phases(.all) probe observes
            // .down:7f for the key while specific handlers stay silent.
            guard press.key.character == Self.backspaceKey.character else {
                return .ignored
            }
            guard store.renaming == nil, let selected = store.selected else {
                return .ignored
            }
            store.send(.deleteRequested(selected))
            return .handled
        }
        .onKeyPress(.deleteForward) {
            guard store.renaming == nil, let selected = store.selected else {
                return .ignored
            }
            store.send(.deleteRequested(selected))
            return .handled
        }
        .onKeyPress(.leftArrow) {
            nudgeSelection(by: CGSize(width: -GraphCanvasLayout.nudgeStep, height: 0))
        }
        .onKeyPress(.rightArrow) {
            nudgeSelection(by: CGSize(width: GraphCanvasLayout.nudgeStep, height: 0))
        }
        .onKeyPress(.upArrow) {
            nudgeSelection(by: CGSize(width: 0, height: -GraphCanvasLayout.nudgeStep))
        }
        .onKeyPress(.downArrow) {
            nudgeSelection(by: CGSize(width: 0, height: GraphCanvasLayout.nudgeStep))
        }
        .onKeyPress(KeyEquivalent(Character("\r"))) {
            guard store.renaming == nil, let selected = store.selected else {
                return .ignored
            }
            store.send(.renameStarted(selected))
            return .handled
        }
    }

    /// Backspace key equivalent.
    ///
    /// `KeyEquivalent.delete` is U+0008 (Ctrl+H); physical and synthetic
    /// backspace keys deliver U+007F, which no stock equivalent matches,
    /// so it is spelled out explicitly.
    static let backspaceKey = KeyEquivalent(Character("\u{7F}"))

    /// Moves the selection one keyboard step, committing through the same
    /// drag-commit path as a box drag.
    ///
    /// Keyboard/VoiceOver users cannot perform drags; arrows are their move
    /// affordance (and the UI-test move path where synthetic drags are
    /// unavailable). Not a gesture-table change: intents and persistence
    /// behave exactly like a drag release.
    private func nudgeSelection(by delta: CGSize) -> KeyPress.Result {
        guard store.renaming == nil,
              let selected = store.selected,
              let mirrored = store.operators.first(where: { $0.id == selected })
        else {
            return .ignored
        }
        store.send(
            .dragCommitted(
                id: selected,
                position: GraphPosition(
                    x: mirrored.position.x + delta.width,
                    y: mirrored.position.y + delta.height
                )
            )
        )
        return .handled
    }

    /// Dotted background grid, shifted by the Swift-local pan offset.
    private func drawGrid(context: inout GraphicsContext, size: CGSize, pan: CGSize) {
        let spacing = GraphCanvasLayout.gridSpacing
        func origin(_ pan: Double, _ length: Double) -> Double {
            let remainder = pan.truncatingRemainder(dividingBy: spacing)
            let start = remainder <= 0 ? remainder : remainder - spacing
            return Swift.min(start, length)
        }
        var dots = Path()
        let startX = origin(pan.width, size.width)
        let startY = origin(pan.height, size.height)
        for x in stride(from: startX, to: size.width, by: spacing) {
            for y in stride(from: startY, to: size.height, by: spacing) {
                dots.addRect(CGRect(x: x, y: y, width: 1.5, height: 1.5))
            }
        }
        context.fill(dots, with: .color(.secondary.opacity(0.35)))
    }
}

#Preview {
    ContentView(
        store: Store(initialState: AppFeature.State()) {
            AppFeature()
        }
    )
}
