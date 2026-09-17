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

/// Typed value of one operator parameter (wire v2, ADR-0002 snapshot JSON v2).
///
/// Closed set mirroring Rust's `ParamValue`: exactly one lowercase-tagged
/// payload travels on the wire (`{"text": "…"}`, `{"float": 1.5}`,
/// `{"integer": 7}`, `{"flag": true}`, `{"vec3": [x, y, z]}`). `vec3`
/// triples stay `Double`/`f64` end to end (ADR-0002); engine math converts
/// only at its own boundaries.
///
/// Explicitly `nonisolated`: values cross from background engine effects
/// to the main actor (see `GraphPosition`).
nonisolated enum ParameterValue: Codable, Equatable, Sendable {
    /// UTF-8 text. The only variant the editor writes (see `MockGraphEngine`).
    case text(String)
    /// Double-precision scalar.
    case float(Double)
    /// Signed integer.
    case integer(Int)
    /// Boolean flag.
    case flag(Bool)
    /// Triple of doubles, traveling as a 3-element array on the wire.
    case vec3(Double, Double, Double)

    /// Wire tags: exactly the lowercase Rust variant names.
    private enum Tag: String, CodingKey {
        case text
        case float
        case integer
        case flag
        case vec3
    }

    /// Decodes exactly one tagged payload; zero or multiple tags fail.
    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: Tag.self)
        var candidates: [ParameterValue] = []
        if let value = try container.decodeIfPresent(String.self, forKey: .text) {
            candidates.append(.text(value))
        }
        if let value = try container.decodeIfPresent(Double.self, forKey: .float) {
            candidates.append(.float(value))
        }
        if let value = try container.decodeIfPresent(Int.self, forKey: .integer) {
            candidates.append(.integer(value))
        }
        if let value = try container.decodeIfPresent(Bool.self, forKey: .flag) {
            candidates.append(.flag(value))
        }
        if let triple = try container.decodeIfPresent([Double].self, forKey: .vec3) {
            guard triple.count == 3 else {
                throw DecodingError.dataCorruptedError(
                    forKey: .vec3,
                    in: container,
                    debugDescription: "vec3 expects exactly 3 numbers, found \(triple.count)."
                )
            }
            candidates.append(.vec3(triple[0], triple[1], triple[2]))
        }
        guard candidates.count == 1, let value = candidates.first else {
            throw DecodingError.dataCorruptedError(
                forKey: .text,
                in: container,
                debugDescription: "Parameter value must carry exactly one of text/float/integer/flag/vec3."
            )
        }
        self = value
    }

    /// Encodes this value as its single tagged payload.
    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: Tag.self)
        switch self {
        case let .text(value):
            try container.encode(value, forKey: .text)
        case let .float(value):
            try container.encode(value, forKey: .float)
        case let .integer(value):
            try container.encode(value, forKey: .integer)
        case let .flag(value):
            try container.encode(value, forKey: .flag)
        case let .vec3(x, y, z):
            try container.encode([x, y, z], forKey: .vec3)
        }
    }

    /// Lowercase wire tag, reused as the read-only type label in the editor.
    var typeLabel: String {
        switch self {
        case .text:
            "text"
        case .float:
            "float"
        case .integer:
            "integer"
        case .flag:
            "flag"
        case .vec3:
            "vec3"
        }
    }

    /// Read-only rendering of the payload for the parameter editor.
    var displayText: String {
        switch self {
        case let .text(value):
            value
        case let .float(value):
            String(value)
        case let .integer(value):
            String(value)
        case let .flag(value):
            String(value)
        case let .vec3(x, y, z):
            "(\(x), \(y), \(z))"
        }
    }
}

/// Read-only mirror of one Rust-owned operator (ADR-0002 snapshot JSON v2).
///
/// Swift never constructs graph content; it mirrors what `vrn_graph_snapshot`
/// reports and sends mutation intents back across the FFI boundary.
///
/// Explicitly `nonisolated`: mirrors cross from background engine effects
/// to the main actor (see `GraphPosition`).
nonisolated struct OperatorMirror: Codable, Equatable, Sendable, Identifiable {
    /// Rust-issued id, starting at 1. `0` is never issued.
    var id: UInt64
    /// Operator kind: `"container"` or `"cube"` (strict set validated at the
    /// engine boundary, mirrored by `OperatorTypeRegistry` for creation).
    var kind: String
    /// Display name. Defaults to `"Container"` at creation.
    var name: String
    /// Containing container's id, or `nil` for the root network.
    var parent: UInt64?
    /// Canvas position owned by Rust.
    var position: GraphPosition
    /// Per-operator parameter map (typed values, wire v2; keys sorted).
    /// Additive: absent on the wire decodes to empty.
    var parameters: [String: ParameterValue] = [:]

    /// Creates a mirror. `parameters` defaults to empty for call sites
    /// without parameters.
    init(
        id: UInt64,
        kind: String,
        name: String,
        parent: UInt64?,
        position: GraphPosition,
        parameters: [String: ParameterValue] = [:]
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
        parameters = try container.decodeIfPresent([String: ParameterValue].self, forKey: .parameters) ?? [:]
    }
}

/// Whole-graph mirror decoded from `vrn_graph_snapshot` JSON (schema v2).
///
/// One format serves persistence and (future) undo: `GraphSnapshot` is what
/// autosave writes and what launch-time `restore` reads back.
///
/// Explicitly `nonisolated`: snapshots cross from background engine effects
/// to the main actor (see `GraphPosition`).
nonisolated struct GraphSnapshot: Codable, Equatable, Sendable {
    /// Snapshot schema version. Always `2`; restore rejects anything else.
    static let currentVersion = 2

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

/// Canvas-menu group for one creatable operator kind.
///
/// `nil` on the definition means the menu root (structural operators like
/// `Container`); a group means a named submenu (operator families like
/// `Geometry`). Menu labels never carry an "Add" prefix.
nonisolated enum OperatorMenuGroup: String, Equatable, Sendable {
    /// Geometry family submenu.
    case geometry

    /// Submenu title shown on the canvas menu.
    var title: String {
        switch self {
        case .geometry:
            "Geometry"
        }
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
    /// Menu placement: `nil` for the menu root, a group for a submenu.
    var menuGroup: OperatorMenuGroup?
}

/// One named canvas-menu submenu and the operator kinds it holds.
nonisolated struct OperatorSubmenu: Equatable, Sendable {
    /// Submenu title (for example `"Geometry"`).
    var title: String
    /// Operator kinds in this submenu, in registry order.
    var items: [OperatorTypeDef]
}

/// Static registry backing the canvas menu (ADR-0002: Swift-static).
///
/// `Container` stays at the menu root (structural); `Cube` lives in the
/// `Geometry` submenu, establishing the family pattern for future operators.
///
/// Explicitly `nonisolated`: read from background engine effects and views.
nonisolated enum OperatorTypeRegistry {
    /// All creatable operator kinds.
    static let all: [OperatorTypeDef] = [
        OperatorTypeDef(kind: "container", displayName: "Container", menuGroup: nil),
        OperatorTypeDef(kind: "cube", displayName: "Cube", menuGroup: .geometry)
    ]
}

/// Pure canvas-menu taxonomy derived from the registry (unit-tested; the
/// view renders this model verbatim so menu structure never drifts).
nonisolated enum OperatorMenuModel {
    /// Operator kinds shown at the menu root, in registry order.
    static var rootItems: [OperatorTypeDef] {
        OperatorTypeRegistry.all.filter { $0.menuGroup == nil }
    }

    /// Named submenus and their kinds, in first-appearance registry order.
    static var submenus: [OperatorSubmenu] {
        var ordered: [OperatorMenuGroup] = []
        for definition in OperatorTypeRegistry.all {
            guard let group = definition.menuGroup, !ordered.contains(group) else {
                continue
            }
            ordered.append(group)
        }
        return ordered.map { group in
            OperatorSubmenu(
                title: group.title,
                items: OperatorTypeRegistry.all.filter { $0.menuGroup == group }
            )
        }
    }
}

/// Schema defaults for user-editable numeric parameters, by operator kind.
///
/// A fresh cube carries no `size`/`center` keys (creation seeds only the
/// name); the cook falls back to Rust's `DEFAULT_CUBE_SIZE`/`DEFAULT_CUBE_CENTER`
/// (`[1, 1, 1]`/`[0, 0, 0]`). The editor seeds its triple-fields from these
/// same values so absent keys stay editable. Mirrors `CubeParams` — any
/// drift breaks the cook contract (see the schema tests).
nonisolated enum OperatorParameterSchema {
    /// Editable numeric defaults for `kind`: cube's `size`/`center`, empty
    /// for kinds with no numeric schema (for example `container`).
    static func editableNumericDefaults(for kind: String) -> [String: ParameterValue] {
        guard kind == "cube" else {
            return [:]
        }
        return [
            "size": .vec3(1, 1, 1),
            "center": .vec3(0, 0, 0)
        ]
    }

    /// Editable numeric parameters for one mirror: schema defaults for
    /// absent keys, mirror values where present, restricted to the
    /// `.float`/`.vec3` cases the generic editor components commit.
    /// Present-but-mistyped values (for example a text `size`) stay out:
    /// they render as read-only rows, never as editors.
    static func editableNumericParams(for mirrored: OperatorMirror?) -> [String: ParameterValue] {
        guard let mirrored else {
            return [:]
        }
        let merged = editableNumericDefaults(for: mirrored.kind)
            .merging(mirrored.parameters) { _, mirror in mirror }
        return merged.filter {
            switch $1 {
            case .float, .vec3:
                true
            case .text, .integer, .flag:
                false
            }
        }
    }
}

/// Parsed triple-field result: one double per axis.
nonisolated struct Vec3Components: Equatable, Sendable {
    /// X component.
    var x: Double
    /// Y component.
    var y: Double
    /// Z component.
    var z: Double
}

/// Pure field-level validation for the generic numeric editor components.
///
/// Both helpers trim surrounding whitespace (like the name commit path) and
/// accept finite doubles only: `Double` also parses `nan`/`inf`, but those
/// are never meaningful parameter values, so they are rejected at the field
/// and never committed. Anything else is rejected the same way, keeping
/// bad values away from the cook's `InvalidParameter` backstop.
nonisolated enum NumericDraftParsing {
    /// Parses one float-field draft, or `nil` when it must not commit.
    static func parseFloatDraft(_ draft: String) -> Double? {
        let trimmed = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, let value = Double(trimmed), value.isFinite else {
            return nil
        }
        return value
    }

    /// Parses one triple-field draft (exactly 3 components), or `nil` when
    /// any component is non-numeric and the edit must not commit.
    static func parseVec3Draft(_ drafts: [String]) -> Vec3Components? {
        guard drafts.count == 3,
              let x = parseFloatDraft(drafts[0]),
              let y = parseFloatDraft(drafts[1]),
              let z = parseFloatDraft(drafts[2])
        else {
            return nil
        }
        return Vec3Components(x: x, y: y, z: z)
    }

    /// Seed text for one float component. Round-trips through the parser.
    static func seedText(for value: Double) -> String {
        String(value)
    }
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
