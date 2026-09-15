import CoreGraphics
import Foundation

/// Canvas position of an operator, in canvas points.
///
/// Always `Double`/`f64` — never `Float`/`f32` (ADR-0002). The `f64`
/// round-trip is pinned by test, not by hope.
///
/// Explicitly `nonisolated`: positions cross from background engine effects
/// to the main actor, which the target's MainActor-default isolation would
/// otherwise forbid.
nonisolated struct GraphPosition: Codable, Equatable, Sendable {
    /// Horizontal canvas offset in points. Unbounded; no clamping.
    var x: Double
    /// Vertical canvas offset in points. Unbounded; no clamping.
    var y: Double
}

/// Read-only mirror of one Rust-owned operator (ADR-0002 snapshot JSON v1).
///
/// Swift never constructs graph content; it mirrors what `vrn_graph_snapshot`
/// reports and sends mutation intents back across the FFI boundary.
///
/// Explicitly `nonisolated`: mirrors cross from background engine effects
/// to the main actor (see `GraphPosition`).
nonisolated struct OperatorMirror: Codable, Equatable, Sendable, Identifiable {
    /// Rust-issued id, starting at 1. `0` is never issued.
    var id: UInt64
    /// Operator kind. Slice 1 accepts only `"container"`.
    var kind: String
    /// Display name. Defaults to `"Container"` at creation.
    var name: String
    /// Containing container's id, or `nil` for the root network.
    var parent: UInt64?
    /// Canvas position owned by Rust.
    var position: GraphPosition
    /// Per-operator parameter map (slice-1 parameter editor; string map).
    /// Additive: absent on the wire decodes to empty.
    var parameters: [String: String] = [:]

    /// Creates a mirror. `parameters` defaults to empty for slice-1 call sites.
    init(
        id: UInt64,
        kind: String,
        name: String,
        parent: UInt64?,
        position: GraphPosition,
        parameters: [String: String] = [:]
    ) {
        self.id = id
        self.kind = kind
        self.name = name
        self.parent = parent
        self.position = position
        self.parameters = parameters
    }

    /// Decodes a mirror, tolerating a missing `parameters` key on additive reads.
    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        id = try container.decode(UInt64.self, forKey: .id)
        kind = try container.decode(String.self, forKey: .kind)
        name = try container.decode(String.self, forKey: .name)
        parent = try container.decodeIfPresent(UInt64.self, forKey: .parent)
        position = try container.decode(GraphPosition.self, forKey: .position)
        parameters = try container.decodeIfPresent([String: String].self, forKey: .parameters) ?? [:]
    }
}

/// Whole-graph mirror decoded from `vrn_graph_snapshot` JSON (schema v1).
///
/// One format serves persistence and (future) undo: `GraphSnapshot` is what
/// autosave writes and what launch-time `restore` reads back.
///
/// Explicitly `nonisolated`: snapshots cross from background engine effects
/// to the main actor (see `GraphPosition`).
nonisolated struct GraphSnapshot: Codable, Equatable, Sendable {
    /// Snapshot schema version. Always `1`; restore rejects anything else.
    static let currentVersion = 1

    /// Schema version of this snapshot.
    var version: Int
    /// Mirrored operators, in engine order.
    var operators: [OperatorMirror]
    /// Dependency edges as `[dependsOn, dependent]` pairs. Empty in slice 1
    /// (no ports, no wires) but present for forward compatibility.
    var edges: [[UInt64]]

    /// Creates a snapshot. `edges` defaults to empty for slice-1 call sites.
    init(version: Int = currentVersion, operators: [OperatorMirror], edges: [[UInt64]] = []) {
        self.version = version
        self.operators = operators
        self.edges = edges
    }

    /// Decodes a snapshot, tolerating a missing `edges` key on additive reads.
    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        version = try container.decode(Int.self, forKey: .version)
        operators = try container.decode([OperatorMirror].self, forKey: .operators)
        edges = try container.decodeIfPresent([[UInt64]].self, forKey: .edges) ?? []
    }

    /// Returns the mirrored operator with `id`, or `nil` for dead ids.
    ///
    /// Selection and dive paths tolerate dead ids (post-undo future): callers
    /// must handle `nil` without crashing or clearing user state.
    func mirroredOperator(id: UInt64) -> OperatorMirror? {
        operators.first { $0.id == id }
    }

    /// Returns the operators directly inside `parent` (`nil` for root).
    func children(of parent: UInt64?) -> [OperatorMirror] {
        operators.filter { $0.parent == parent }
    }

    /// Returns the display name for `id`, or `nil` when the id is dead.
    func displayName(for id: UInt64) -> String? {
        mirroredOperator(id: id)?.name
    }
}

/// Registry entry describing one creatable operator kind.
///
/// Explicitly `nonisolated`: read from background engine effects and views.
nonisolated struct OperatorTypeDef: Equatable, Sendable {
    /// Wire kind sent to `vrn_graph_create_operator` (for example `"container"`).
    var kind: String
    /// Human-readable menu label.
    var displayName: String
}

/// Static registry backing the canvas add menu (ADR-0002: Swift-static).
///
/// Single entry in slice 1; operator #2 is a pure extension.
///
/// Explicitly `nonisolated`: read from background engine effects and views.
nonisolated enum OperatorTypeRegistry {
    /// All creatable operator kinds.
    static let all: [OperatorTypeDef] = [
        OperatorTypeDef(kind: "container", displayName: "Container")
    ]
}

/// Fallible domain boundary for every graph intent (AGENTS.md: typed throws).
///
/// Explicitly `nonisolated`: thrown across background engine effects.
nonisolated enum GraphEngineError: Error, Equatable, Sendable {
    /// An `vrn_graph_*` extern returned a non-`Ok` `VrnResult` code.
    case ffiFailed(operation: String, code: Int32)
    /// The shared Rust context failed to allocate; no intent was attempted.
    case engineUnavailable(operation: String)
    /// Snapshot JSON from Rust failed to decode.
    case snapshotDecodingFailed(String)
    /// A snapshot failed to encode for restore or autosave.
    case snapshotEncodingFailed(String)
    /// Autosave write or launch-load read failed.
    case persistenceFailed(String)

    /// Human-readable description for the status line and diagnostics.
    var message: String {
        switch self {
        case let .ffiFailed(operation, code):
            "Graph intent '\(operation)' failed (code \(code))."
        case let .engineUnavailable(operation):
            "Graph engine unavailable during '\(operation)'."
        case let .snapshotDecodingFailed(detail):
            "Could not decode graph snapshot: \(detail)"
        case let .snapshotEncodingFailed(detail):
            "Could not encode graph snapshot: \(detail)"
        case let .persistenceFailed(detail):
            "Graph persistence failed: \(detail)"
        }
    }
}

/// Pure canvas geometry for the node graph (unit-tested; no view code here).
///
/// Explicitly `nonisolated`: called from views and background effects alike.
nonisolated enum GraphCanvasLayout {
    /// Fixed visual size of an operator box in points.
    static let boxSize = CGSize(width: 150, height: 64)
    /// Spacing of the dotted background grid in points.
    static let gridSpacing: Double = 24
    /// Translation magnitude in points separating a tap from a drag.
    static let tapSlop: Double = 2
    /// Keyboard nudge step in points (arrow keys move the selection).
    static let nudgeStep: Double = 10
    /// Name of the stable canvas coordinate space box drags measure in.
    ///
    /// A drag that moves its own box must not use `.local`: the box's space
    /// moves with every preview, so the cumulative translation feeds back
    /// into itself and the box stops tracking the cursor 1:1.
    static let canvasSpaceName = "NodeGraphCanvas"

    /// Returns the box frame for `mirrored` shifted by the Swift-local pan.
    static func boxFrame(for mirrored: OperatorMirror, pan: CGSize) -> CGRect {
        CGRect(
            x: mirrored.position.x + pan.width,
            y: mirrored.position.y + pan.height,
            width: boxSize.width,
            height: boxSize.height
        )
    }

    /// Returns true when a drag translation counts as a move, not a tap.
    static func isDrag(_ translation: CGSize) -> Bool {
        hypot(translation.width, translation.height) >= tapSlop
    }

    /// Returns the operator position under a box drag.
    ///
    /// `translation` is the cumulative drag translation; the committed
    /// position is the anchor because the mirror only moves on commit.
    static func draggedPosition(
        from anchor: GraphPosition,
        translation: CGSize
    ) -> GraphPosition {
        GraphPosition(x: anchor.x + translation.width, y: anchor.y + translation.height)
    }
}
