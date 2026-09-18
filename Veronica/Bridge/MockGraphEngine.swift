import Foundation

/// In-memory stand-in for the `vrn_graph_*` FFI surface.
///
/// Active only when the app launches with `--vrn-mock-engine` (see
/// `GraphLaunchOptions`). Validation mirrors ADR-0002 so the double
/// cross-checks the contract: strict `"container"`/`"cube"`/`"sphere"` kinds, known-id
/// moves/renames/deletes, non-blank names, existing parents, Rust-issued
/// ids from 1, `version == currentVersion` restores, cascading deletes.
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

    /// Creates an operator, returning its id. Only `"container"`, `"cube"`,
    /// and `"sphere"` are accepted, mirroring `parse_operator_kind`
    /// strictness.
    func create(
        kind: String,
        parent: UInt64?,
        position: GraphPosition
    ) throws(GraphEngineError) -> UInt64 {
        guard kind == "container" || kind == "cube" || kind == "sphere" else {
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

    /// Sets one text parameter, storing `.text(value)` verbatim. Unknown ids
    /// and blank keys are rejected; the `"name"` key mirrors `rename`
    /// (blank rejected, the old value preserved on failure). Keys are
    /// trimmed and values stored verbatim, mirroring Rust so the double
    /// never accepts what the engine rejects or normalizes what it stores.
    func setParameter(id: UInt64, key: String, value: String) throws(GraphEngineError) {
        guard var mirrored = operators[id] else {
            throw .ffiFailed(operation: "setParameter", code: 2)
        }
        let trimmedKey = key.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmedKey.isEmpty else {
            throw .ffiFailed(operation: "setParameter", code: 2)
        }
        if trimmedKey == "name" {
            guard !value.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
                throw .ffiFailed(operation: "setParameter", code: 2)
            }
            mirrored.name = value
        } else {
            mirrored.parameters[trimmedKey] = .text(value)
        }
        operators[id] = mirrored
    }

    /// Sets one typed parameter value, storing it verbatim. Unknown ids,
    /// blank keys, and the reserved `"name"` key are rejected — mirroring
    /// `vrn_graph_set_parameter_typed` so the double never accepts what the
    /// engine rejects. Keys are trimmed, mirroring Rust.
    func setParameterTyped(id: UInt64, key: String, value: ParameterValue) throws(GraphEngineError) {
        guard var mirrored = operators[id] else {
            throw .ffiFailed(operation: "setParameterTyped", code: 2)
        }
        let trimmedKey = key.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmedKey.isEmpty, trimmedKey != "name" else {
            throw .ffiFailed(operation: "setParameterTyped", code: 2)
        }
        mirrored.parameters[trimmedKey] = value
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

    /// Replaces the whole DAG. Rejects non-current `version`, zero ids, blank
    /// names, blank parameter keys, and dangling parent/edge references —
    /// mirroring Rust so the double never accepts what the engine rejects.
    func restore(_ snapshot: GraphSnapshot) throws(GraphEngineError) {
        guard snapshot.version == GraphSnapshot.currentVersion else {
            throw .ffiFailed(operation: "vrn_graph_restore", code: 2)
        }
        let ids = Set(snapshot.operators.map(\.id))
        guard !ids.contains(0), ids.count == snapshot.operators.count else {
            throw .ffiFailed(operation: "vrn_graph_restore", code: 2)
        }
        for mirrored in snapshot.operators {
            guard !mirrored.name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
                throw .ffiFailed(operation: "vrn_graph_restore", code: 2)
            }
            for key in mirrored.parameters.keys {
                guard !key.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
                    throw .ffiFailed(operation: "vrn_graph_restore", code: 2)
                }
            }
            if let parent = mirrored.parent, !ids.contains(parent) {
                throw .ffiFailed(operation: "vrn_graph_restore", code: 2)
            }
        }
        for edge in snapshot.edges {
            guard edge.count == 2, ids.contains(edge[0]), ids.contains(edge[1]) else {
                throw .ffiFailed(operation: "vrn_graph_restore", code: 2)
            }
        }
        operators = Dictionary(uniqueKeysWithValues: snapshot.operators.map { ($0.id, $0) })
        nextId = (snapshot.operators.map(\.id).max() ?? 0) + 1
    }
}
