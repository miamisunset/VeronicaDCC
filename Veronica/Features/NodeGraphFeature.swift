import ComposableArchitecture
import CoreGraphics
import Foundation

/// Right-pane feature: the procedural node graph editor (slice 1).
///
/// Rust owns operators and the DAG; this feature mirrors the v1 snapshot
/// read-only and sends mutation intents. Dive, breadcrumb, pan, selection,
/// and drag previews are Swift-local and never touch FFI. Every committed
/// mutation refreshes the mirror from a post-mutation snapshot and autosaves
/// it; stale responses are dropped by epoch, like `ViewportFeature`'s
/// tick guard.
@Reducer
struct NodeGraphFeature {
    /// Transient box-drag preview: the dragged id and its uncommitted position.
    struct DragPreview: Equatable, Sendable {
        /// Dragged operator id.
        var id: UInt64
        /// Preview position (committed only on release).
        var position: GraphPosition
    }

    @ObservableState
    struct State: Equatable, Sendable {
        /// Mirrored operators, in engine order.
        var operators: [OperatorMirror] = []
        /// Dive stack of container ids from the root to the shown network.
        var path: [UInt64] = []
        /// Single selection. Tolerates dead ids (post-undo future).
        var selected: UInt64?
        /// Swift-local pan offset in points. Never sent to Rust.
        var panOffset: CGSize = .zero
        /// Last canvas point for registry-menu creation (hover-tracked).
        var pendingCreatePosition = GraphPosition(x: 0, y: 0)
        /// Active box-drag preview, cleared on commit.
        var dragPreview: DragPreview?
        /// Operator id under the inline rename overlay, if any.
        var renaming: UInt64?
        /// Rename overlay draft text.
        var renameDraft = ""
        /// Monotonic epoch guarding snapshot responses against reordering.
        var snapshotEpoch: UInt64 = 0
        /// Last intent or persistence failure, shown in the status line.
        var lastError: String?

        /// Operators directly inside the shown network.
        var visibleOperators: [OperatorMirror] {
            let current = path.last
            return operators.filter { $0.parent == current }
        }
    }

    /// `Equatable` so `TestStore` can assert received actions by value.
    enum Action: Equatable {
        /// Pane appeared: load autosave (unless reset), restore, mirror.
        case appeared
        /// Snapshot fetch or post-mutation refresh answered.
        case snapshotResponse(Result<GraphSnapshot, GraphEngineError>, epoch: UInt64)
        /// Click selected a box, or background cleared the selection.
        case operatorSelected(UInt64?)
        /// Double-click dove into a container. Never touches FFI.
        case diveRequested(UInt64)
        /// Breadcrumb jumped to `depth` (0 is root). Never touches FFI.
        case breadcrumbSelected(depth: Int)
        /// Back stepped one level up. Never touches FFI.
        case backToParent
        /// Background drag panned by a delta. Never touches FFI.
        case panChanged(delta: CGSize)
        /// Hover tracked a new canvas point for menu creation.
        case hoverPositionChanged(GraphPosition)
        /// Box drag moved: local preview only, no FFI.
        case dragPreviewChanged(id: UInt64, position: GraphPosition)
        /// Box drag released away from its anchor: cancel a stale preview.
        case dragCancelled
        /// Box drag released past the tap slop: commit the move through FFI.
        case dragCommitted(id: UInt64, position: GraphPosition)
        /// Registry menu requested creation at a canvas point.
        case createRequested(kind: String, position: GraphPosition)
        /// Inline rename overlay opened for an operator.
        case renameStarted(UInt64)
        /// Rename overlay draft changed.
        case renameDraftChanged(String)
        /// Rename overlay committed (Enter).
        case renameCommitted
        /// Rename overlay cancelled (Escape).
        case renameCancelled
        /// Delete key or menu requested deletion (cascades in Rust).
        case deleteRequested(UInt64)
    }

    @Dependency(\.engineClient) var engine
    @Dependency(\.graphPersistence) var persistence

    var body: some Reducer<NodeGraphFeature.State, NodeGraphFeature.Action> {
        Reduce { state, action in
            switch action {
            case .appeared:
                state.snapshotEpoch += 1
                let epoch = state.snapshotEpoch
                let client = engine
                let store = persistence
                return .run { send in
                    do {
                        if !GraphLaunchOptions.resetGraphOnLaunch,
                           let data = try await store.load(),
                           let saved = try? JSONDecoder().decode(GraphSnapshot.self, from: data) {
                            // A corrupt file starts empty: the restore is
                            // best-effort and the mirror below still loads.
                            try? await client.restoreSnapshot(saved)
                        }
                        let snapshot = try await client.requestSnapshot()
                        await send(.snapshotResponse(.success(snapshot), epoch: epoch))
                    } catch let error as GraphEngineError {
                        await send(.snapshotResponse(.failure(error), epoch: epoch))
                    }
                }

            case let .snapshotResponse(result, epoch):
                // Snapshot effects resolve out of order under rapid commits:
                // a delayed response must never overwrite a newer mirror.
                guard epoch == state.snapshotEpoch else {
                    return .none
                }
                switch result {
                case let .success(snapshot):
                    state.operators = snapshot.operators
                    state.lastError = nil
                case let .failure(error):
                    state.lastError = error.message
                }
                return .none

            case let .operatorSelected(id):
                state.selected = id
                return .none

            case let .diveRequested(id):
                state.path.append(id)
                return .none

            case let .breadcrumbSelected(depth):
                state.path = Array(state.path.prefix(Swift.max(0, depth)))
                return .none

            case .backToParent:
                if !state.path.isEmpty {
                    state.path.removeLast()
                }
                return .none

            case let .panChanged(delta):
                state.panOffset.width += delta.width
                state.panOffset.height += delta.height
                return .none

            case let .hoverPositionChanged(position):
                state.pendingCreatePosition = position
                return .none

            case let .dragPreviewChanged(id, position):
                state.dragPreview = DragPreview(id: id, position: position)
                return .none

            case .dragCancelled:
                state.dragPreview = nil
                return .none

            case let .dragCommitted(id, position):
                state.dragPreview = nil
                state.snapshotEpoch += 1
                return commit(
                    engine: engine,
                    persistence: persistence,
                    epoch: state.snapshotEpoch
                ) { (client: EngineClient) async throws(GraphEngineError) in
                    try await client.moveOperator(id, position)
                }

            case let .createRequested(kind, position):
                let parent = state.path.last
                state.snapshotEpoch += 1
                return commit(
                    engine: engine,
                    persistence: persistence,
                    epoch: state.snapshotEpoch
                ) { (client: EngineClient) async throws(GraphEngineError) in
                    _ = try await client.createOperator(kind, parent, position)
                }

            case let .renameStarted(id):
                state.renaming = id
                state.renameDraft = state.operators.first { $0.id == id }?.name ?? ""
                return .none

            case let .renameDraftChanged(draft):
                state.renameDraft = draft
                return .none

            case .renameCommitted:
                guard let id = state.renaming else {
                    return .none
                }
                let name = state.renameDraft.trimmingCharacters(in: .whitespacesAndNewlines)
                state.renaming = nil
                state.renameDraft = ""
                // Blank names are rejected at the FFI boundary; cancel
                // locally instead of sending a doomed intent.
                guard !name.isEmpty else {
                    return .none
                }
                state.snapshotEpoch += 1
                return commit(
                    engine: engine,
                    persistence: persistence,
                    epoch: state.snapshotEpoch
                ) { (client: EngineClient) async throws(GraphEngineError) in
                    try await client.renameOperator(id, name)
                }

            case .renameCancelled:
                state.renaming = nil
                state.renameDraft = ""
                return .none

            case let .deleteRequested(id):
                if state.selected == id {
                    state.selected = nil
                }
                state.snapshotEpoch += 1
                return commit(
                    engine: engine,
                    persistence: persistence,
                    epoch: state.snapshotEpoch
                ) { (client: EngineClient) async throws(GraphEngineError) in
                    try await client.deleteOperator(id)
                }
            }
        }
    }

    /// Runs one mutating intent, then refreshes the mirror, autosaves it,
    /// and reports the outcome epoch-guarded.
    private func commit(
        engine: EngineClient,
        persistence: GraphPersistence,
        epoch: UInt64,
        operation: @Sendable @escaping (EngineClient) async throws(GraphEngineError) -> Void
    ) -> Effect<Action> {
        .run { send in
            do {
                try await operation(engine)
                let snapshot = try await engine.requestSnapshot()
                try await persistence.save(snapshot)
                await send(.snapshotResponse(.success(snapshot), epoch: epoch))
            } catch let error as GraphEngineError {
                await send(.snapshotResponse(.failure(error), epoch: epoch))
            }
        }
    }
}
