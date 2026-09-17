import ComposableArchitecture
import CoreGraphics
import Foundation

/// Right-pane feature: the procedural node graph editor (slice 1).
///
/// Rust owns operators and the DAG; this feature mirrors the v2 snapshot
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

    /// Which of the Graph/Parameters panes comes first in the arrangement.
    enum PaneOrder: Equatable, Sendable {
        /// Graph left (row) or above (column): the slice-1 default.
        case graphFirst
        /// Parameters left (row) or above (column).
        case parametersFirst

        /// The order the swap controls switch to.
        var toggled: PaneOrder {
            self == .graphFirst ? .parametersFirst : .graphFirst
        }
    }

    /// Row (side by side) or column (stacked) arrangement of the
    /// Graph/Parameters panes. The viewport stays put either way.
    enum PaneOrientation: Equatable, Sendable {
        /// Side by side: the slice-1 default.
        case row
        /// Stacked vertically.
        case column

        /// The orientation the switch controls switch to.
        var toggled: PaneOrientation {
            self == .row ? .column : .row
        }
    }

    @ObservableState
    struct State: Equatable, Sendable {
        /// Mirrored operators, in engine order.
        var operators: [OperatorMirror] = []
        /// Dive stack of container ids from the root to the shown network.
        var path: [UInt64] = []
        /// Browser-model past: every path the chevrons can go back to, most
        /// recent last. Dives — including breadcrumb jumps — push the
        /// departed path here and clear `forwardStack`.
        var backStack: [[UInt64]] = []
        /// Browser-model future: paths a new dive orphaned, next first from
        /// the back. Back pushes the departed path here; forward pops it.
        var forwardStack: [[UInt64]] = []
        /// Single selection. Tolerates dead ids (post-undo future).
        var selected: UInt64?
        /// Swift-local pan offset in points. Never sent to Rust.
        var panOffset: CGSize = .zero
        /// Last canvas point for registry-menu creation (hover-tracked).
        var pendingCreatePosition = GraphPosition(x: 0, y: 0)
        /// Active box-drag preview, cleared on commit.
        var dragPreview: DragPreview?
        /// Committed-but-unconfirmed position, shown until the post-commit
        /// snapshot arrives. Without it the box snaps back to its stale
        /// mirror slot for the round-trip and flickers as a ghost.
        var pendingCommit: DragPreview?
        /// Operator id under the inline rename overlay, if any.
        var renaming: UInt64?
        /// Rename overlay draft text.
        var renameDraft = ""
        /// Parameter-editor draft of the selected operator's name, seeded
        /// from the mirror on selection.
        var editorNameDraft = ""
        /// True after the editor draft diverges from the mirror. A confirmed
        /// round-trip re-seeds only while clean, so typing is never clobbered.
        var editorDirty = false
        /// Parameter-editor drafts of numeric (float) parameters, by key,
        /// seeded from the mirror (or the kind schema for absent keys) on
        /// selection and re-seeded from confirmed mirrors while clean.
        var editorFloatDrafts: [String: String] = [:]
        /// Parameter-editor drafts of triple (vec3) parameters, by key:
        /// exactly 3 component strings each. Same seeding lifetime as
        /// `editorFloatDrafts`.
        var editorVec3Drafts: [String: [String]] = [:]
        /// Parameter-editor drafts of integer parameters, by key. Same
        /// seeding lifetime as `editorFloatDrafts`.
        var editorIntDrafts: [String: String] = [:]
        /// Numeric parameter keys whose drafts diverge from the mirror. A
        /// confirmed round-trip re-seeds only clean keys, so typing in one
        /// field survives commits from another.
        var editorParamDirtyKeys: Set<String> = []
        /// Epoch of the in-flight numeric-parameter commit, if any. Only
        /// the matching response may restore the optimistically cleared
        /// dirty key (same scoping as `editorCommitEpoch`).
        var editorParamCommitEpoch: UInt64?
        /// Numeric key of the in-flight commit, if any.
        var editorParamCommitKey: String?
        /// Epoch of the in-flight editor commit, if any. Only the matching
        /// response may restore the optimistically cleared dirty flag, so
        /// unrelated intent failures never dirty a clean editor.
        var editorCommitEpoch: UInt64?
        /// Monotonic epoch guarding snapshot responses against reordering.
        var snapshotEpoch: UInt64 = 0
        /// Last intent or persistence failure, shown in the status line.
        var lastError: String?
        /// Which of the Graph/Parameters panes comes first. Session-local:
        /// reset on every launch, never persisted or sent to Rust.
        var paneOrder = PaneOrder.graphFirst
        /// Row or column arrangement of Graph/Parameters. Same
        /// session-local lifetime as `paneOrder`.
        var paneOrientation = PaneOrientation.row

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
        /// Post-mutation refresh answered but autosave failed: the mirror
        /// still advances (the engine already committed) and the error shows.
        case snapshotSaveFailed(GraphSnapshot, GraphEngineError, epoch: UInt64)
        /// Click selected a box, or background cleared the selection.
        case operatorSelected(UInt64?)
        /// Double-click dove into a container. Never touches FFI.
        case diveRequested(UInt64)
        /// Breadcrumb jump to `depth` (0 is root). A jump is a new dive:
        /// the departed path is pushed onto the back stack and the forward
        /// stack is cleared. Never touches FFI.
        case breadcrumbSelected(depth: Int)
        /// Back chevron (or Command-[): the departed path goes onto the
        /// forward stack. No-op at the history start. Never touches FFI.
        case historyBack
        /// Forward chevron (or Command-]): the departed path goes back onto
        /// the back stack. No-op at the history end. Never touches FFI.
        case historyForward
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
        /// Parameter-editor name draft changed.
        case editorNameChanged(String)
        /// Parameter-editor name committed (Enter or focus loss).
        case editorNameCommitted
        /// Parameter-editor edit reverted (Escape).
        case editorNameReverted
        /// Parameter-editor float draft changed for `key`.
        case editorFloatChanged(key: String, draft: String)
        /// Parameter-editor float committed for `key` (Enter or focus loss).
        case editorFloatCommitted(key: String)
        /// Parameter-editor triple component changed for `key` (`axis` 0-2).
        case editorVec3Changed(key: String, axis: Int, draft: String)
        /// Parameter-editor triple committed for `key` (Enter or focus loss).
        case editorVec3Committed(key: String)
        /// Parameter-editor integer draft changed for `key`.
        case editorIntChanged(key: String, draft: String)
        /// Parameter-editor integer committed for `key` (Enter or focus
        /// loss).
        case editorIntCommitted(key: String)
        /// Parameter-editor numeric edit reverted for `key` (Escape).
        case editorParamReverted(key: String)
        /// Delete key or menu requested deletion (cascades in Rust).
        case deleteRequested(UInt64)
        /// Explicit order swap of the Graph/Parameters panes (menu or
        /// toolbar). Pure layout: never touches FFI, selection, drafts, or
        /// dive state.
        case paneOrderChanged(PaneOrder)
        /// Order flip for the swap controls: the toggle lives here (not in
        /// the views) so both call sites share one tested transition.
        case paneOrderToggled
        /// Explicit row/column switch of the Graph/Parameters panes (menu
        /// or toolbar). Same pure-layout lifetime as `paneOrderChanged`.
        case paneOrientationChanged(PaneOrientation)
        /// Orientation flip for the switch controls. Same rationale as
        /// `paneOrderToggled`.
        case paneOrientationToggled
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
                // Any confirmed round-trip retires the pending position: the
                // mirror below is authoritative again from here on.
                state.pendingCommit = nil
                // The editor commit this response answers (if any) is no
                // longer in flight, whatever any newer commit did after it.
                let editorLanded = state.editorCommitEpoch == epoch
                state.editorCommitEpoch = nil
                let landedParamKey: String? =
                    state.editorParamCommitEpoch == epoch ? state.editorParamCommitKey : nil
                state.editorParamCommitEpoch = nil
                state.editorParamCommitKey = nil
                switch result {
                case let .success(snapshot):
                    state.operators = snapshot.operators
                    state.lastError = nil
                    // A clean editor follows the mirror; a dirty draft is the
                    // user's unconfirmed typing and must survive the refresh.
                    if !state.editorDirty {
                        state.editorNameDraft = state.operators.first { $0.id == state.selected }?.name ?? ""
                    }
                    reseedCleanNumericParams(&state)
                case let .failure(error):
                    state.lastError = error.message
                    // Only the failed editor commit restores the
                    // optimistically cleared flag (see
                    // `editorNameCommitted`): the typed value survives, and
                    // unrelated failures leave a clean editor clean.
                    if editorLanded {
                        state.editorDirty = true
                    }
                    if let key = landedParamKey {
                        state.editorParamDirtyKeys.insert(key)
                    }
                }
                return .none

            case let .snapshotSaveFailed(snapshot, error, epoch):
                // Same ordering guard: a delayed save-failure must not
                // overwrite a newer mirror either.
                guard epoch == state.snapshotEpoch else {
                    return .none
                }
                state.pendingCommit = nil
                // Same landed-key capture as `snapshotResponse`: the engine
                // committed, so the typed value is in the mirror — but the
                // user's draft must still survive the reseed below.
                let editorLanded = state.editorCommitEpoch == epoch
                state.editorCommitEpoch = nil
                let landedParamKey: String? =
                    state.editorParamCommitEpoch == epoch ? state.editorParamCommitKey : nil
                state.editorParamCommitEpoch = nil
                state.editorParamCommitKey = nil
                state.operators = snapshot.operators
                state.lastError = error.message
                if editorLanded {
                    state.editorDirty = true
                }
                if let key = landedParamKey {
                    state.editorParamDirtyKeys.insert(key)
                }
                // Same protection as the success path: only a clean editor
                // follows the advanced mirror.
                if !state.editorDirty {
                    state.editorNameDraft = state.operators.first { $0.id == state.selected }?.name ?? ""
                }
                reseedCleanNumericParams(&state)
                return .none

            case let .operatorSelected(id):
                state.selected = id
                seedEditor(&state)
                return .none

            case let .diveRequested(id):
                // A dive orphans the forward future, like a browser: the
                // departed path stays reachable through the back stack.
                state.backStack.append(state.path)
                state.forwardStack = []
                state.path.append(id)
                seedEditor(&state)
                return .none

            case let .breadcrumbSelected(depth):
                let next = Array(state.path.prefix(Swift.max(0, depth)))
                // Re-clicking the current segment changes nothing: recording
                // it would pollute the back stack with a self-loop.
                guard next != state.path else {
                    return .none
                }
                state.backStack.append(state.path)
                state.forwardStack = []
                state.path = next
                seedEditor(&state)
                return .none

            case .historyBack:
                guard let previous = state.backStack.popLast() else {
                    return .none
                }
                state.forwardStack.append(state.path)
                state.path = previous
                seedEditor(&state)
                return .none

            case .historyForward:
                guard let next = state.forwardStack.popLast() else {
                    return .none
                }
                state.backStack.append(state.path)
                state.path = next
                seedEditor(&state)
                return .none

            case let .panChanged(delta):
                // A box drag fires the background pan gesture from the same
                // touch (simultaneous gestures). While a preview is live the
                // preview owns all movement; applying the pan too would move
                // the dragged box twice and drift every other box.
                guard state.dragPreview == nil else {
                    return .none
                }
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
                // Hold the committed spot on screen until the confirming
                // snapshot arrives; the mirror still shows the stale slot.
                state.pendingCommit = DragPreview(id: id, position: position)
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
                    seedEditor(&state)
                }
                state.snapshotEpoch += 1
                return commit(
                    engine: engine,
                    persistence: persistence,
                    epoch: state.snapshotEpoch
                ) { (client: EngineClient) async throws(GraphEngineError) in
                    try await client.deleteOperator(id)
                }

            case let .editorNameChanged(draft):
                state.editorNameDraft = draft
                state.editorDirty = true
                return .none

            case .editorNameCommitted:
                // No-op unless the draft diverged and a selection exists.
                guard state.editorDirty, let id = state.selected else {
                    return .none
                }
                // Trim client-side like the rename overlay: the engines store
                // values verbatim, so padding must never leave the client.
                // Blank names are rejected at the FFI boundary; cancel
                // locally instead of sending a doomed intent.
                let draft = state.editorNameDraft.trimmingCharacters(in: .whitespacesAndNewlines)
                guard !draft.isEmpty else {
                    seedEditor(&state)
                    return .none
                }
                state.editorNameDraft = draft
                // Cleared optimistically; only the matching failed response
                // restores it (see `snapshotResponse`) so the typed value
                // survives.
                state.editorDirty = false
                state.snapshotEpoch += 1
                state.editorCommitEpoch = state.snapshotEpoch
                return commit(
                    engine: engine,
                    persistence: persistence,
                    epoch: state.snapshotEpoch
                ) { (client: EngineClient) async throws(GraphEngineError) in
                    try await client.setParameter(id, "name", draft)
                }

            case .editorNameReverted:
                seedEditor(&state)
                return .none

            case let .editorFloatChanged(key, draft):
                state.editorFloatDrafts[key] = draft
                state.editorParamDirtyKeys.insert(key)
                return .none

            case let .editorFloatCommitted(key):
                // No-op unless the draft diverged, a selection exists, and
                // the text parses: non-numeric input is rejected at the
                // field and never commits (the cook's `InvalidParameter`
                // stays a backend backstop, never a user-visible path).
                guard let id = state.selected,
                      state.editorParamDirtyKeys.contains(key),
                      let draft = state.editorFloatDrafts[key],
                      let value = NumericDraftParsing.parseFloatDraft(draft)
                else {
                    return .none
                }
                return commitNumericParam(
                    state: &state,
                    id: id,
                    key: key,
                    value: .float(value),
                    engine: engine,
                    persistence: persistence
                )

            case let .editorVec3Changed(key, axis, draft):
                guard axis >= 0, axis < 3 else {
                    return .none
                }
                var triple = state.editorVec3Drafts[key] ?? ["", "", ""]
                while triple.count < 3 {
                    triple.append("")
                }
                triple[axis] = draft
                state.editorVec3Drafts[key] = triple
                state.editorParamDirtyKeys.insert(key)
                return .none

            case let .editorVec3Committed(key):
                // Same triple-or-nothing rule as the field: one bad
                // component vetoes the whole commit.
                guard let id = state.selected,
                      state.editorParamDirtyKeys.contains(key),
                      let drafts = state.editorVec3Drafts[key],
                      let triple = NumericDraftParsing.parseVec3Draft(drafts)
                else {
                    return .none
                }
                return commitNumericParam(
                    state: &state,
                    id: id,
                    key: key,
                    value: .vec3(triple.x, triple.y, triple.z),
                    engine: engine,
                    persistence: persistence
                )

            case let .editorIntChanged(key, draft):
                state.editorIntDrafts[key] = draft
                state.editorParamDirtyKeys.insert(key)
                return .none

            case let .editorIntCommitted(key):
                // Same shape as the float commit, plus the schema range:
                // non-numeric or out-of-range input is rejected at the field
                // and never commits (the cook's `InvalidParameter` stays a
                // backend backstop, never a user-visible path). Keys with no
                // schema range accept any parsed integer.
                guard let id = state.selected,
                      state.editorParamDirtyKeys.contains(key),
                      let draft = state.editorIntDrafts[key],
                      let value = NumericDraftParsing.parseIntegerDraft(draft),
                      let kind = state.operators.first(where: { $0.id == id })?.kind,
                      OperatorParameterSchema.integerRange(for: kind, key: key)?.contains(value) ?? true
                else {
                    return .none
                }
                return commitNumericParam(
                    state: &state,
                    id: id,
                    key: key,
                    value: .integer(value),
                    engine: engine,
                    persistence: persistence
                )

            case let .editorParamReverted(key):
                state.editorParamDirtyKeys.remove(key)
                if state.editorParamCommitKey == key {
                    state.editorParamCommitEpoch = nil
                    state.editorParamCommitKey = nil
                }
                reseedCleanNumericParams(&state)
                return .none

            case let .paneOrderChanged(order):
                state.paneOrder = order
                return .none

            case .paneOrderToggled:
                state.paneOrder = state.paneOrder.toggled
                return .none

            case let .paneOrientationChanged(orientation):
                state.paneOrientation = orientation
                return .none

            case .paneOrientationToggled:
                state.paneOrientation = state.paneOrientation.toggled
                return .none
            }
        }
    }

    /// Seeds the parameter-editor draft from the selected mirror (`""` when
    /// nothing or a dead id is selected) and marks it clean, discarding any
    /// unconfirmed typing. Selection, dive, and revert paths enter here.
    /// Seeding establishes draft == mirror, so any in-flight editor commit
    /// is moot from here on: late responses follow the normal clean rules.
    private func seedEditor(_ state: inout State) {
        state.editorNameDraft = state.operators.first { $0.id == state.selected }?.name ?? ""
        state.editorDirty = false
        state.editorCommitEpoch = nil
        state.editorFloatDrafts = [:]
        state.editorVec3Drafts = [:]
        state.editorIntDrafts = [:]
        state.editorParamDirtyKeys = []
        state.editorParamCommitEpoch = nil
        state.editorParamCommitKey = nil
        reseedCleanNumericParams(&state)
    }

    /// Re-seeds numeric drafts for clean keys from the current mirror
    /// (schema defaults for absent keys); dirty keys keep the user's
    /// typing, and drafts for keys that stopped being editable are dropped
    /// unless dirty.
    private func reseedCleanNumericParams(_ state: inout State) {
        let mirrored = state.operators.first { $0.id == state.selected }
        let editable = OperatorParameterSchema.editableNumericParams(for: mirrored)
        let known = Set(state.editorFloatDrafts.keys)
            .union(state.editorVec3Drafts.keys)
            .union(state.editorIntDrafts.keys)
            .union(editable.keys)
        for key in known {
            guard !state.editorParamDirtyKeys.contains(key) else {
                continue
            }
            state.editorFloatDrafts.removeValue(forKey: key)
            state.editorVec3Drafts.removeValue(forKey: key)
            state.editorIntDrafts.removeValue(forKey: key)
            switch editable[key] {
            case let .float(value):
                state.editorFloatDrafts[key] = NumericDraftParsing.seedText(for: value)
            case let .vec3(x, y, z):
                state.editorVec3Drafts[key] = [
                    NumericDraftParsing.seedText(for: x),
                    NumericDraftParsing.seedText(for: y),
                    NumericDraftParsing.seedText(for: z)
                ]
            case let .integer(value):
                state.editorIntDrafts[key] = NumericDraftParsing.seedText(for: value)
            case .text, .flag, nil:
                break
            }
        }
    }

    /// Commits one numeric parameter through the typed intent (issue #54).
    ///
    /// The value crosses as its `ParamValue` wire JSON, so the cook sees
    /// exactly what a restore would have written — the same `commit()`
    /// refresh-and-autosave path as every other intent, with no
    /// read-modify-write and no residual interleave window for two
    /// in-flight numeric commits.
    private func commitNumericParam(
        state: inout State,
        id: UInt64,
        key: String,
        value: ParameterValue,
        engine: EngineClient,
        persistence: GraphPersistence
    ) -> Effect<Action> {
        // Cleared optimistically; only the matching failed response
        // restores the key (see `snapshotResponse`).
        state.editorParamDirtyKeys.remove(key)
        state.snapshotEpoch += 1
        state.editorParamCommitEpoch = state.snapshotEpoch
        state.editorParamCommitKey = key
        return commit(
            engine: engine,
            persistence: persistence,
            epoch: state.snapshotEpoch
        ) { (client: EngineClient) async throws(GraphEngineError) in
            try await client.setParameterTyped(id, key, value)
        }
    }

    /// Runs one mutating intent, then refreshes the mirror, autosaves it,
    /// and reports the outcome epoch-guarded.
    ///
    /// A failed autosave still delivers the fresh mirror: the engine already
    /// committed, so the UI must not show stale boxes. The persistence error
    /// travels alongside instead of replacing the snapshot.
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
                do {
                    try await persistence.save(snapshot)
                } catch let error as GraphEngineError {
                    await send(.snapshotSaveFailed(snapshot, error, epoch: epoch))
                    return
                }
                await send(.snapshotResponse(.success(snapshot), epoch: epoch))
            } catch let error as GraphEngineError {
                await send(.snapshotResponse(.failure(error), epoch: epoch))
            }
        }
    }
}
