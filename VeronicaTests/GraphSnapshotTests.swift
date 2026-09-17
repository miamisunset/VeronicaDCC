import CoreGraphics
import Foundation
import SwiftUI
import Testing

@testable import Veronica

/// Contract pins for the v2 snapshot schema: golden-fixture
/// drift, `f64` round-trips, registry shape, and canvas geometry.
struct GraphSnapshotTests {
    /// Repo-relative URL of the golden fixture owned by `veronica-graph`.
    private var fixtureURL: URL {
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .appendingPathComponent("rust/crates/veronica-graph/tests/fixtures/graph-v2.json")
    }

    @Test func goldenFixtureDecodesAndRoundTrips() throws {
        let url = fixtureURL
        guard FileManager.default.fileExists(atPath: url.path) else {
            withKnownIssue("Rust track has not delivered graph-v2.json yet") {
                Issue.record("missing golden fixture: \(url.path)")
            }
            return
        }
        let data = try Data(contentsOf: url)
        let decoded = try JSONDecoder().decode(GraphSnapshot.self, from: data)
        #expect(decoded.version == GraphSnapshot.currentVersion)
        #expect(decoded.edges.isEmpty)
        #expect(decoded.operators == [
            OperatorMirror(
                id: 7,
                kind: "container",
                name: "Hero",
                parent: nil,
                position: GraphPosition(x: 120, y: 80),
                parameters: [
                    "count": .integer(3),
                    "label": .text("Hero"),
                    "opacity": .float(0.5),
                    "size": .vec3(1.0, 2.0, 3.0),
                    "visible": .flag(true)
                ]
            ),
            OperatorMirror(
                id: 8,
                kind: "container",
                name: "Sidekick",
                parent: 7,
                position: GraphPosition(x: 200, y: 80)
            )
        ])
        // The file triple survives bit-identical, not just `==`.
        if case let .vec3(x, y, z) = decoded.operators[0].parameters["size"] {
            #expect(x.bitPattern == (1.0).bitPattern)
            #expect(y.bitPattern == (2.0).bitPattern)
            #expect(z.bitPattern == (3.0).bitPattern)
        } else {
            Issue.record("expected a vec3 for size")
        }
        // Decode -> encode -> decode equality: the mirror survives the wire.
        let recoded = try JSONEncoder().encode(decoded)
        let roundTripped = try JSONDecoder().decode(GraphSnapshot.self, from: recoded)
        #expect(roundTripped == decoded)
    }

    /// Decoding is structurally tolerant (a v1 payload still parses) while
    /// restore rejects it — see `restoreRejectsWrongVersionAndContinuesIds`.
    /// Decode and restore are different gates; this pins the first.
    @Test func v1PayloadDecodesStructurallyButIsNotCurrent() throws {
        let json = """
        {"version": 1, "operators": [
            {"id": 7, "kind": "container", "name": "Hero",
             "parent": null, "position": {"x": 120.0, "y": 80.0}}
        ], "edges": []}
        """
        let decoded = try JSONDecoder().decode(GraphSnapshot.self, from: Data(json.utf8))
        #expect(decoded.version == 1)
        #expect(decoded.version != GraphSnapshot.currentVersion)
        #expect(decoded.operators.first?.name == "Hero")
    }

    @Test func f64PositionsSurviveRoundTripBitIdentical() throws {
        let positions = [
            GraphPosition(x: 0.1 + 0.2, y: Double.pi),
            GraphPosition(x: 1e300, y: -273.15),
            GraphPosition(x: 120, y: 80)
        ]
        let snapshot = GraphSnapshot(
            operators: positions.enumerated().map { index, position in
                OperatorMirror(
                    id: UInt64(index + 1),
                    kind: "container",
                    name: "Op\(index)",
                    parent: nil,
                    position: position
                )
            }
        )
        let data = try JSONEncoder().encode(snapshot)
        let roundTripped = try JSONDecoder().decode(GraphSnapshot.self, from: data)
        #expect(roundTripped == snapshot)
        for (original, restored) in zip(snapshot.operators, roundTripped.operators) {
            #expect(original.position.x.bitPattern == restored.position.x.bitPattern)
            #expect(original.position.y.bitPattern == restored.position.y.bitPattern)
        }
    }

    @Test func missingEdgesDefaultsToEmpty() throws {
        let json = """
        {"version": 2, "operators": []}
        """
        let decoded = try JSONDecoder().decode(GraphSnapshot.self, from: Data(json.utf8))
        #expect(decoded.version == GraphSnapshot.currentVersion)
        #expect(decoded.edges.isEmpty)
    }

    @Test func missingParametersDefaultsToEmpty() throws {
        let json = """
        {"version": 2, "operators": [
            {"id": 1, "kind": "container", "name": "P",
             "parent": null, "position": {"x": 1.0, "y": 2.0}}
        ], "edges": []}
        """
        let decoded = try JSONDecoder().decode(GraphSnapshot.self, from: Data(json.utf8))
        #expect(decoded.operators.first?.parameters == [:])
    }

    @Test func parametersSurviveRoundTrip() throws {
        let mirrored = OperatorMirror(
            id: 1, kind: "container", name: "P", parent: nil,
            position: GraphPosition(x: 1, y: 2),
            parameters: ["seed": .text("7")]
        )
        let snapshot = GraphSnapshot(operators: [mirrored])
        let data = try JSONEncoder().encode(snapshot)
        let roundTripped = try JSONDecoder().decode(GraphSnapshot.self, from: data)
        #expect(roundTripped == snapshot)
        #expect(roundTripped.operators.first?.parameters == ["seed": .text("7")])
    }

    @Test func typedParametersDecodeAllFiveVariantsWithF64Parity() throws {
        let json = """
        {"version": 2, "operators": [
            {"id": 1, "kind": "container", "name": "P",
             "parent": null, "position": {"x": 1.0, "y": 2.0},
             "parameters": {
                 "label": {"text": "hello"},
                 "gain": {"float": 0.12345678901234568},
                 "seed": {"integer": 7},
                 "enabled": {"flag": true},
                 "offset": {"vec3": [0.1, 0.2, 0.30000000000000004]}
             }}
        ], "edges": []}
        """
        let decoded = try JSONDecoder().decode(GraphSnapshot.self, from: Data(json.utf8))
        #expect(decoded.version == GraphSnapshot.currentVersion)
        #expect(GraphSnapshot.currentVersion == 2)
        let parameters = try #require(decoded.operators.first?.parameters)
        #expect(parameters["label"] == .text("hello"))
        #expect(parameters["gain"] == .float(0.12345678901234568))
        #expect(parameters["seed"] == .integer(7))
        #expect(parameters["enabled"] == .flag(true))
        #expect(parameters["offset"] == .vec3(0.1, 0.2, 0.30000000000000004))
        // f64 triple parity: the wire triple survives bit-identical.
        if case let .vec3(x, y, z) = parameters["offset"] {
            #expect(x.bitPattern == (0.1).bitPattern)
            #expect(y.bitPattern == (0.2).bitPattern)
            #expect(z.bitPattern == (0.30000000000000004).bitPattern)
        } else {
            Issue.record("expected a vec3 for offset")
        }
        // Round-trip preserves the typed map.
        let recoded = try JSONEncoder().encode(decoded)
        let roundTripped = try JSONDecoder().decode(GraphSnapshot.self, from: recoded)
        #expect(roundTripped == decoded)
    }

    @Test(
        "malformed parameter values are rejected",
        arguments: [
            "{}",
            #"{"text": "a", "flag": true}"#,
            #"{"vec3": [1.0, 2.0]}"#
        ]
    )
    func malformedParameterValuesAreRejected(payload: String) throws {
        let json = """
        {"version": 2, "operators": [
            {"id": 1, "kind": "container", "name": "P",
             "parent": null, "position": {"x": 0.0, "y": 0.0},
             "parameters": {"bad": \(payload)}}
        ], "edges": []}
        """
        #expect(throws: DecodingError.self) {
            try JSONDecoder().decode(GraphSnapshot.self, from: Data(json.utf8))
        }
    }

    @Test func codableUsesDoubleNeverFloat() throws {
        // `position` must decode full `f64` precision, not `Float` mush.
        let json = """
        {"version": 2, "operators": [
            {"id": 1, "kind": "container", "name": "P",
             "parent": null, "position": {"x": 0.12345678901234568, "y": 0.0}}
        ], "edges": []}
        """
        let decoded = try JSONDecoder().decode(GraphSnapshot.self, from: Data(json.utf8))
        let x = try #require(decoded.operators.first?.position.x)
        #expect(x == 0.12345678901234568)
        #expect(x != Double(Float(0.12345678901234568)))
    }

    @Test func registryHoldsContainerAndCube() {
        #expect(OperatorTypeRegistry.all == [
            OperatorTypeDef(kind: "container", displayName: "Container", menuGroup: nil),
            OperatorTypeDef(kind: "cube", displayName: "Cube", menuGroup: .geometry)
        ])
    }

    @Test func menuTaxonomyRootsContainerNestsCubeUnderGeometry() {
        #expect(OperatorMenuModel.rootItems == [
            OperatorTypeDef(kind: "container", displayName: "Container", menuGroup: nil)
        ])
        #expect(OperatorMenuModel.submenus == [
            OperatorSubmenu(
                title: "Geometry",
                items: [OperatorTypeDef(kind: "cube", displayName: "Cube", menuGroup: .geometry)]
            )
        ])
    }

    @Test func cubeSchemaDefaultsMirrorRustCubeParams() {
        let defaults = OperatorParameterSchema.editableNumericDefaults(for: "cube")
        #expect(defaults == [
            "size": .vec3(1, 1, 1),
            "center": .vec3(0, 0, 0)
        ])
        // Bit-exact: the editor seeds what the cook defaults to.
        if case let .vec3(x, y, z) = defaults["size"] {
            #expect(x.bitPattern == (1.0).bitPattern)
            #expect(y.bitPattern == (1.0).bitPattern)
            #expect(z.bitPattern == (1.0).bitPattern)
        } else {
            Issue.record("expected a vec3 for size")
        }
        #expect(OperatorParameterSchema.editableNumericDefaults(for: "container") == [:])
        #expect(OperatorParameterSchema.editableNumericDefaults(for: "unknown") == [:])
    }

    @Test(
        "float drafts commit numerics and reject the rest",
        arguments: [
            ("2", 2.0), ("-0.5", -0.5), ("  3.25  ", 3.25), ("1e3", 1_000.0)
        ]
    )
    func floatDraftAcceptsNumerics(draft: String, expected: Double) {
        #expect(NumericDraftParsing.parseFloatDraft(draft) == expected)
    }

    @Test(
        "float drafts reject non-numerics",
        arguments: ["", "   ", "abc", "1,2", "12px", "--3", "1.2.3", "nan", "NaN", "inf", "-inf", "infinity"]
    )
    func floatDraftRejectsNonNumerics(draft: String) {
        #expect(NumericDraftParsing.parseFloatDraft(draft) == nil)
    }

    @Test func vec3DraftCommitsCompleteTriplesOnly() {
        let parsed = NumericDraftParsing.parseVec3Draft(["1", "2", "3"])
        #expect(parsed == Vec3Components(x: 1, y: 2, z: 3))
        // One bad component vetoes the whole triple: never partially commit.
        #expect(NumericDraftParsing.parseVec3Draft(["1", "oops", "3"]) == nil)
        #expect(NumericDraftParsing.parseVec3Draft(["1", "", "3"]) == nil)
        #expect(NumericDraftParsing.parseVec3Draft(["1", "2"]) == nil)
        #expect(NumericDraftParsing.parseVec3Draft(["1", "2", "3", "4"]) == nil)
        #expect(NumericDraftParsing.parseVec3Draft([]) == nil)
    }

    @Test func editableNumericsPreferMirrorAndSkipMistyped() {
        let mirrored = OperatorMirror(
            id: 1, kind: "cube", name: "Box", parent: nil,
            position: GraphPosition(x: 0, y: 0),
            parameters: ["size": .text("big"), "center": .vec3(0, 0, 0), "seed": .integer(7)]
        )
        let editable = OperatorParameterSchema.editableNumericParams(for: mirrored)
        // Present-but-mistyped stays out (read-only row, never an editor);
        // stored triples win over the schema defaults.
        #expect(editable["size"] == nil)
        #expect(editable["center"] == .vec3(0, 0, 0))
        #expect(editable["seed"] == nil)
        #expect(OperatorParameterSchema.editableNumericParams(for: nil) == [:])
    }

    @Test func seedTextRoundTripsThroughParser() {
        for value in [1.0, -2.5, 0.1 + 0.2, 1_000.0] {
            let parsed = NumericDraftParsing.parseFloatDraft(NumericDraftParsing.seedText(for: value))
            #expect(parsed == value)
        }
    }

    @Test func childrenFilterByParent() {
        let snapshot = GraphSnapshot(operators: [
            OperatorMirror(
                id: 1, kind: "container", name: "Root", parent: nil,
                position: GraphPosition(x: 0, y: 0)
            ),
            OperatorMirror(
                id: 2, kind: "container", name: "Child", parent: 1,
                position: GraphPosition(x: 10, y: 10)
            )
        ])
        #expect(snapshot.children(of: nil).map(\.id) == [1])
        #expect(snapshot.children(of: 1).map(\.id) == [2])
        #expect(snapshot.displayName(for: 2) == "Child")
        #expect(snapshot.displayName(for: 999) == nil)
        #expect(snapshot.mirroredOperator(id: 999) == nil)
    }

    @Test func layoutFramesShiftWithPan() {
        let mirrored = OperatorMirror(
            id: 1, kind: "container", name: "Box", parent: nil,
            position: GraphPosition(x: 100, y: 50)
        )
        let frame = GraphCanvasLayout.boxFrame(for: mirrored, pan: CGSize(width: 10, height: -5))
        #expect(frame.origin.x == 110)
        #expect(frame.origin.y == 45)
        #expect(frame.size == GraphCanvasLayout.boxSize)
    }

    @Test func draggedPositionAddsTranslation() {
        let next = GraphCanvasLayout.draggedPosition(
            from: GraphPosition(x: 10, y: 10),
            translation: CGSize(width: 5, height: -7)
        )
        #expect(next == GraphPosition(x: 15, y: 3))
    }

    @Test func tapSlopSeparatesTapsFromDrags() {
        #expect(!GraphCanvasLayout.isDrag(CGSize(width: 1, height: 1)))
        #expect(GraphCanvasLayout.isDrag(CGSize(width: 60, height: 40)))
    }

    @Test func backspaceKeyMatchesDel() {
        #expect(NodeGraphView.backspaceKey.character.unicodeScalars.first?.value == 0x7F)
    }
}
