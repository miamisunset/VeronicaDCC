import Foundation

/// In-memory stand-in for the `vrn_graph_*` FFI surface.
///
/// Active only when the app launches with `--vrn-mock-engine` (see
/// `GraphLaunchOptions`). Validation mirrors ADR-0002 so the double
/// cross-checks the contract: strict `"container"` kind, known-id
/// moves/renames/deletes, non-blank names, existing parents, Rust-issued
/// ids from 1, `version == 1` restores, cascading deletes.
///
/// Retained as the hermetic UI-test double (mock mode is the default test
/// path) alongside the real-engine mode: both run the same flow, and the
/// mock visibly badges the UI so the two are never confused (see
/// `mockEngineBadge`). Validation mirrors ADR-0002 so the double
/// cross-checks the contract.
actor MockGraphEngine {

    /// Mirrored operators by id.
    private var operators: [UInt64: OperatorMirror] = [:]
    /// Next Rust-issued id. `0` is never issued (FFI root-parent sentinel).
    private var nextId: UInt64 = 1

    /// Creates an operator, returning its id.
    func create(
        kind: String,
        parent: UInt64?,
        position: GraphPosition
    ) throws(GraphEngineError) -> UInt64 {
        guard kind == "container" else {
            throw .ffiFailed(operation: "vrn_graph_create_operator", code: 2)
        }
        if let parent, operators[parent] == nil {
            throw .ffiFailed(operation: "vrn_graph_create_operator", code: 2)
        }
        let id = nextId
        nextId += 1
        operators[id] = OperatorMirror(
            id: id,
            kind: kind,
            name: "Container",
            parent: parent,
            position: position
        )
        return id
    }

    /// Moves a known operator id.
    func move(id: UInt64, position: GraphPosition) throws(GraphEngineError) {
        guard var mirrored = operators[id] else {
            throw .ffiFailed(operation: "vrn_graph_move_operator", code: 2)
        }
        mirrored.position = position
        operators[id] = mirrored
    }

    /// Renames a known operator id. Blank names are rejected.
    func rename(id: UInt64, name: String) throws(GraphEngineError) {
        guard var mirrored = operators[id] else {
            throw .ffiFailed(operation: "vrn_graph_rename_operator", code: 2)
        }
        guard !name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            throw .ffiFailed(operation: "vrn_graph_rename_operator", code: 2)
        }
        mirrored.name = name
        operators[id] = mirrored
    }

    /// Deletes an operator id, cascading its subtree.
    func delete(id: UInt64) throws(GraphEngineError) {
        guard operators[id] != nil else {
            throw .ffiFailed(operation: "vrn_graph_delete_operator", code: 2)
        }
        var condemned: [UInt64] = [id]
        var index = 0
        while index < condemned.count {
            let current = condemned[index]
            index += 1
            condemned += operators.values
                .filter { $0.parent == current }
                .map(\.id)
        }
        for victim in condemned {
            operators.removeValue(forKey: victim)
        }
    }

    /// Returns the whole-graph mirror in stable id order.
    func snapshot() -> GraphSnapshot {
        GraphSnapshot(operators: operators.values.sorted { $0.id < $1.id })
    }

    /// Replaces the whole DAG. Rejects `version != 1`.
    func restore(_ snapshot: GraphSnapshot) throws(GraphEngineError) {
        guard snapshot.version == GraphSnapshot.currentVersion else {
            throw .ffiFailed(operation: "vrn_graph_restore", code: 2)
        }
        operators = Dictionary(uniqueKeysWithValues: snapshot.operators.map { ($0.id, $0) })
        nextId = (snapshot.operators.map(\.id).max() ?? 0) + 1
    }
}
