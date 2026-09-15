import ComposableArchitecture
import CoreGraphics
import Foundation
import Testing

@testable import Veronica

/// Records intents received by overridden engine dependencies.
private actor IntentRecorder {
    /// One creation intent.
    struct Created: Equatable, Sendable {
        /// Wire kind sent to the engine.
        var kind: String
        /// Parent id, or `nil` for root.
        var parent: UInt64?
        /// Canvas position.
        var position: GraphPosition
    }

    /// One move intent.
    struct Moved: Equatable, Sendable {
        /// Moved operator id.
        var id: UInt64
        /// New canvas position.
        var position: GraphPosition
    }

    /// One rename intent.
    struct Renamed: Equatable, Sendable {
        /// Renamed operator id.
        var id: UInt64
        /// New name.
        var name: String
    }

    /// Created intents.
    var created: [Created] = []
    /// Moved intents.
    var moved: [Moved] = []
    /// Renamed intents.
    var renamed: [Renamed] = []
    /// Deleted ids.
    var deleted: [UInt64] = []
    /// Restored snapshots.
    var restored: [GraphSnapshot] = []
    /// Autosaved snapshots.
    var saved: [GraphSnapshot] = []

    /// Records a creation.
    func recordCreate(kind: String, parent: UInt64?, position: GraphPosition) {
        created.append(Created(kind: kind, parent: parent, position: position))
    }

    /// Records a move.
    func recordMove(id: UInt64, position: GraphPosition) {
        moved.append(Moved(id: id, position: position))
    }

    /// Records a rename.
    func recordRename(id: UInt64, name: String) {
        renamed.append(Renamed(id: id, name: name))
    }

    /// Records a deletion.
    func recordDelete(id: UInt64) {
        deleted.append(id)
    }

    /// Records a restore.
    func recordRestore(_ snapshot: GraphSnapshot) {
        restored.append(snapshot)
    }

    /// Records an autosave.
    func recordSave(_ snapshot: GraphSnapshot) {
        saved.append(snapshot)
    }
}

/// Reducer contract: Swift-local gestures never touch FFI, committed
/// mutations refresh the mirror epoch-guarded and autosave it.
@MainActor
struct NodeGraphFeatureTests {
    /// Builds a mirrored container operator for fixtures.
    private func container(
        id: UInt64,
        name: String = "Container",
        parent: UInt64? = nil,
        x: Double = 0,
        y: Double = 0
    ) -> OperatorMirror {
        OperatorMirror(
            id: id,
            kind: "container",
            name: name,
            parent: parent,
            position: GraphPosition(x: x, y: y)
        )
    }

    @Test func appearedMirrorsEngineSnapshot() async {
        let snapshot = GraphSnapshot(operators: [container(id: 1, x: 120, y: 80)])
        let store = TestStore(initialState: NodeGraphFeature.State()) {
            NodeGraphFeature()
        } withDependencies: {
            $0.engineClient.requestSnapshot = { snapshot }
            $0.graphPersistence.load = { nil }
        }
        await store.send(.appeared) {
            $0.snapshotEpoch = 1
        }
        await store.receive(.snapshotResponse(.success(snapshot), epoch: 1)) {
            $0.operators = snapshot.operators
        }
    }

    @Test func appearedRestoresAutosavedFile() async {
        let saved = GraphSnapshot(operators: [container(id: 7, name: "Hero")])
        let recorder = IntentRecorder()
        let store = TestStore(initialState: NodeGraphFeature.State()) {
            NodeGraphFeature()
        } withDependencies: {
            $0.engineClient.restoreSnapshot = { snapshot in
                await recorder.recordRestore(snapshot)
            }
            $0.engineClient.requestSnapshot = { saved }
            $0.graphPersistence.load = { () async throws(GraphEngineError) -> Data? in
                do {
                    return try JSONEncoder().encode(saved)
                } catch {
                    throw GraphEngineError.snapshotEncodingFailed(error.localizedDescription)
                }
            }
        }
        await store.send(.appeared) {
            $0.snapshotEpoch = 1
        }
        await store.receive(.snapshotResponse(.success(saved), epoch: 1)) {
            $0.operators = saved.operators
        }
        let restored = await recorder.restored
        #expect(restored == [saved])
    }

    @Test func appearedIgnoresCorruptAutosave() async {
        let snapshot = GraphSnapshot(operators: [])
        let store = TestStore(initialState: NodeGraphFeature.State()) {
            NodeGraphFeature()
        } withDependencies: {
            $0.engineClient.requestSnapshot = { snapshot }
            $0.graphPersistence.load = { Data("not-json".utf8) }
        }
        await store.send(.appeared) {
            $0.snapshotEpoch = 1
        }
        await store.receive(.snapshotResponse(.success(snapshot), epoch: 1))
    }

    @Test func staleSnapshotResponseIsDropped() async {
        var state = NodeGraphFeature.State()
        state.snapshotEpoch = 5
        state.operators = [container(id: 1)]
        let store = TestStore(initialState: state) {
            NodeGraphFeature()
        }
        let newer = GraphSnapshot(operators: [container(id: 2)])
        await store.send(.snapshotResponse(.success(newer), epoch: 3))
    }

    @Test func diveBreadcrumbAndBack() async {
        let store = TestStore(initialState: NodeGraphFeature.State()) {
            NodeGraphFeature()
        }
        await store.send(.diveRequested(7)) {
            $0.path = [7]
        }
        await store.send(.diveRequested(9)) {
            $0.path = [7, 9]
        }
        await store.send(.breadcrumbSelected(depth: 1)) {
            $0.path = [7]
        }
        await store.send(.backToParent) {
            $0.path = []
        }
        await store.send(.backToParent)
        await store.send(.breadcrumbSelected(depth: 0))
    }

    @Test func selectionToleratesDeadIds() async {
        var state = NodeGraphFeature.State()
        state.operators = [container(id: 1)]
        let store = TestStore(initialState: state) {
            NodeGraphFeature()
        }
        await store.send(.operatorSelected(999)) {
            $0.selected = 999
        }
        // A refresh that drops the selected id (post-undo future) keeps the
        // selection instead of crashing or clearing user state.
        await store.send(.snapshotResponse(.success(GraphSnapshot(operators: [])), epoch: 0)) {
            $0.operators = []
        }
        #expect(store.state.selected == 999)
    }

    @Test func createCommitsAutosavesAndUsesDiveParent() async {
        let recorder = IntentRecorder()
        let created = container(id: 42, x: 120, y: 100)
        let snapshot = GraphSnapshot(operators: [created])
        var state = NodeGraphFeature.State()
        state.path = [7]
        let store = TestStore(initialState: state) {
            NodeGraphFeature()
        } withDependencies: {
            $0.engineClient.createOperator = { kind, parent, position in
                await recorder.recordCreate(kind: kind, parent: parent, position: position)
                return 42
            }
            $0.engineClient.requestSnapshot = { snapshot }
            $0.graphPersistence.save = { saved in
                await recorder.recordSave(saved)
            }
        }
        let position = GraphPosition(x: 120, y: 100)
        await store.send(.createRequested(kind: "container", position: position)) {
            $0.snapshotEpoch = 1
        }
        await store.receive(.snapshotResponse(.success(snapshot), epoch: 1)) {
            $0.operators = snapshot.operators
        }
        let recorded = await recorder.created
        #expect(recorded == [IntentRecorder.Created(kind: "container", parent: 7, position: position)])
        #expect(await recorder.saved == [snapshot])
    }

    @Test func dragPreviewsLocallyThenCommits() async {
        let recorder = IntentRecorder()
        let start = container(id: 1, x: 10, y: 10)
        let moved = container(id: 1, x: 60, y: 50)
        var state = NodeGraphFeature.State()
        state.operators = [start]
        let store = TestStore(initialState: state) {
            NodeGraphFeature()
        } withDependencies: {
            $0.engineClient.moveOperator = { id, position in
                await recorder.recordMove(id: id, position: position)
            }
            $0.engineClient.requestSnapshot = { GraphSnapshot(operators: [moved]) }
            $0.graphPersistence.save = { _ in }
        }
        let preview = GraphPosition(x: 40, y: 30)
        await store.send(.dragPreviewChanged(id: 1, position: preview)) {
            $0.dragPreview = NodeGraphFeature.DragPreview(id: 1, position: preview)
        }
        // Preview never moves the mirror and never touches FFI.
        #expect(await recorder.moved.isEmpty)
        let commit = GraphPosition(x: 60, y: 50)
        await store.send(.dragCommitted(id: 1, position: commit)) {
            $0.dragPreview = nil
            $0.snapshotEpoch = 1
        }
        await store.receive(
            .snapshotResponse(.success(GraphSnapshot(operators: [moved])), epoch: 1)
        ) {
            $0.operators = [moved]
        }
        let recorded = await recorder.moved
        #expect(recorded == [IntentRecorder.Moved(id: 1, position: commit)])
    }

    @Test func dragCancelledClearsPreview() async {
        var state = NodeGraphFeature.State()
        state.dragPreview = NodeGraphFeature.DragPreview(
            id: 1,
            position: GraphPosition(x: 40, y: 30)
        )
        let store = TestStore(initialState: state) {
            NodeGraphFeature()
        }
        // A tap-release after hovering movement clears the stale preview
        // without touching FFI (no effect to receive).
        await store.send(.dragCancelled) {
            $0.dragPreview = nil
        }
    }

    @Test func renameFlowCommitsTrimmedName() async {
        let recorder = IntentRecorder()
        let renamed = container(id: 1, name: "Hero")
        var state = NodeGraphFeature.State()
        state.operators = [container(id: 1, name: "Old")]
        let store = TestStore(initialState: state) {
            NodeGraphFeature()
        } withDependencies: {
            $0.engineClient.renameOperator = { id, name in
                await recorder.recordRename(id: id, name: name)
            }
            $0.engineClient.requestSnapshot = { GraphSnapshot(operators: [renamed]) }
            $0.graphPersistence.save = { _ in }
        }
        await store.send(.renameStarted(1)) {
            $0.renaming = 1
            $0.renameDraft = "Old"
        }
        await store.send(.renameDraftChanged("  Hero  ")) {
            $0.renameDraft = "  Hero  "
        }
        await store.send(.renameCommitted) {
            $0.renaming = nil
            $0.renameDraft = ""
            $0.snapshotEpoch = 1
        }
        await store.receive(
            .snapshotResponse(.success(GraphSnapshot(operators: [renamed])), epoch: 1)
        ) {
            $0.operators = [renamed]
        }
        let recorded = await recorder.renamed
        #expect(recorded == [IntentRecorder.Renamed(id: 1, name: "Hero")])
    }

    @Test func renameBlankCancelsWithoutIntent() async {
        var state = NodeGraphFeature.State()
        state.operators = [container(id: 1, name: "Old")]
        let store = TestStore(initialState: state) {
            NodeGraphFeature()
        }
        await store.send(.renameStarted(1)) {
            $0.renaming = 1
            $0.renameDraft = "Old"
        }
        await store.send(.renameDraftChanged("   ")) {
            $0.renameDraft = "   "
        }
        // Blank names are rejected at the FFI boundary; the reducer cancels
        // locally and sends no intent (no effect to receive).
        await store.send(.renameCommitted) {
            $0.renaming = nil
            $0.renameDraft = ""
        }
    }

    @Test func renameStartedWithDeadIdDraftsEmpty() async {
        let store = TestStore(initialState: NodeGraphFeature.State()) {
            NodeGraphFeature()
        }
        await store.send(.renameStarted(999)) {
            $0.renaming = 999
            $0.renameDraft = ""
        }
        await store.send(.renameCancelled) {
            $0.renaming = nil
        }
    }

    @Test func deleteClearsDirectSelectionKeepsCascadeOrphans() async {
        let recorder = IntentRecorder()
        var state = NodeGraphFeature.State()
        state.operators = [container(id: 5), container(id: 6, parent: 5)]
        state.selected = 5
        let store = TestStore(initialState: state) {
            NodeGraphFeature()
        } withDependencies: {
            $0.engineClient.deleteOperator = { id in
                await recorder.recordDelete(id: id)
            }
            $0.engineClient.requestSnapshot = { GraphSnapshot(operators: []) }
            $0.graphPersistence.save = { _ in }
        }
        await store.send(.deleteRequested(5)) {
            $0.selected = nil
            $0.snapshotEpoch = 1
        }
        await store.receive(.snapshotResponse(.success(GraphSnapshot(operators: [])), epoch: 1)) {
            $0.operators = []
        }
        #expect(await recorder.deleted == [5])
    }

    @Test func deleteKeepsUnrelatedSelection() async {
        let survivor = container(id: 6)
        var state = NodeGraphFeature.State()
        state.operators = [container(id: 5), survivor]
        state.selected = 6
        let store = TestStore(initialState: state) {
            NodeGraphFeature()
        } withDependencies: {
            $0.engineClient.deleteOperator = { _ in }
            $0.engineClient.requestSnapshot = { GraphSnapshot(operators: [survivor]) }
            $0.graphPersistence.save = { _ in }
        }
        await store.send(.deleteRequested(5)) {
            $0.snapshotEpoch = 1
        }
        await store.receive(
            .snapshotResponse(.success(GraphSnapshot(operators: [survivor])), epoch: 1)
        ) {
            $0.operators = [survivor]
        }
        #expect(store.state.selected == 6)
    }

    @Test func commitFailureSurfacesError() async {
        let failure = GraphEngineError.ffiFailed(operation: "vrn_graph_move_operator", code: 2)
        var state = NodeGraphFeature.State()
        state.operators = [container(id: 1)]
        let store = TestStore(initialState: state) {
            NodeGraphFeature()
        } withDependencies: {
            $0.engineClient.moveOperator = { (_: UInt64, _: GraphPosition) async throws(GraphEngineError) in
                throw failure
            }
            $0.graphPersistence.save = { _ in }
        }
        await store.send(.dragCommitted(id: 1, position: GraphPosition(x: 5, y: 5))) {
            $0.snapshotEpoch = 1
        }
        await store.receive(.snapshotResponse(.failure(failure), epoch: 1)) {
            $0.lastError = failure.message
        }
    }

    @Test func panAccumulatesAndHoverTracks() async {
        let store = TestStore(initialState: NodeGraphFeature.State()) {
            NodeGraphFeature()
        }
        await store.send(.panChanged(delta: CGSize(width: 10, height: -4))) {
            $0.panOffset = CGSize(width: 10, height: -4)
        }
        await store.send(.panChanged(delta: CGSize(width: 10, height: -4))) {
            $0.panOffset = CGSize(width: 20, height: -8)
        }
        await store.send(.hoverPositionChanged(GraphPosition(x: 3, y: 4))) {
            $0.pendingCreatePosition = GraphPosition(x: 3, y: 4)
        }
    }
}
