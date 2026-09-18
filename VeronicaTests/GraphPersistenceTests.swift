import Foundation
import Testing

@testable import Veronica

/// Autosave round-trips through an isolated temp directory (never the real
/// Application Support folder).
struct GraphPersistenceTests {
    /// Creates a unique scratch directory for one test.
    private func scratchDirectory() throws -> URL {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        return url
    }

    @Test func saveThenLoadRoundTrips() throws {
        let directory = try scratchDirectory()
        defer { try? FileManager.default.removeItem(at: directory) }
        let url = directory.appendingPathComponent(GraphPersistence.fileName)
        let snapshot = GraphSnapshot(operators: [
            OperatorMirror(
                id: 7, kind: "container", name: "Hero", parent: nil,
                position: GraphPosition(x: 120, y: 80)
            )
        ])
        try GraphPersistence.save(snapshot, to: url)
        let data = try #require(try GraphPersistence.load(from: url))
        #expect(try JSONDecoder().decode(GraphSnapshot.self, from: data) == snapshot)
    }

    @Test func loadMissingFileReturnsNil() throws {
        let directory = try scratchDirectory()
        defer { try? FileManager.default.removeItem(at: directory) }
        let url = directory.appendingPathComponent(GraphPersistence.fileName)
        #expect(try GraphPersistence.load(from: url) == nil)
    }

    @Test func saveCreatesMissingDirectories() throws {
        let directory = try scratchDirectory()
        defer { try? FileManager.default.removeItem(at: directory) }
        let url = directory
            .appendingPathComponent("nested", isDirectory: true)
            .appendingPathComponent(GraphPersistence.fileName)
        try GraphPersistence.save(GraphSnapshot(operators: []), to: url)
        let data = try #require(try GraphPersistence.load(from: url))
        #expect(try JSONDecoder().decode(GraphSnapshot.self, from: data) == GraphSnapshot(operators: []))
    }

    @Test func fileNameAndBundleFallbackAreContract() {
        #expect(GraphPersistence.fileName == "graph-v2.json")
        #expect(GraphPersistence.bundleIdFallback == "com.github.miamisunset.Veronica")
    }
}

/// The in-memory double mirrors ADR-0002 validation: strict kinds, known
/// ids, non-blank names, existing parents, cascading deletes, versioned
/// restores, ids from 1.
struct MockGraphEngineTests {
    @Test func createIssuesIdsFromOneWithDefaultName() async throws {
        let engine = MockGraphEngine()
        let first = try await engine.create(
            kind: "container",
            parent: nil,
            position: GraphPosition(x: 1, y: 2)
        )
        let second = try await engine.create(
            kind: "container",
            parent: nil,
            position: GraphPosition(x: 3, y: 4)
        )
        #expect(first == 1)
        #expect(second == 2)
        let snapshot = await engine.snapshot()
        #expect(snapshot.operators.map(\.name) == ["Container", "Container"])
    }

    @Test func createAcceptsCubeKind() async throws {
        let engine = MockGraphEngine()
        let id = try await engine.create(
            kind: "cube",
            parent: nil,
            position: GraphPosition(x: 0, y: 0)
        )
        #expect(id == 1)
        let snapshot = await engine.snapshot()
        #expect(snapshot.operators.map(\.kind) == ["cube"])
    }

    @Test func createAcceptsSphereKind() async throws {
        let engine = MockGraphEngine()
        let id = try await engine.create(
            kind: "sphere",
            parent: nil,
            position: GraphPosition(x: 0, y: 0)
        )
        #expect(id == 1)
        let snapshot = await engine.snapshot()
        #expect(snapshot.operators.map(\.kind) == ["sphere"])
    }

    @Test func createRejectsUnknownKindAndParent() async {
        let engine = MockGraphEngine()
        await #expect(throws: GraphEngineError.self) {
            try await engine.create(
                kind: "blur",
                parent: nil,
                position: GraphPosition(x: 0, y: 0)
            )
        }
        await #expect(throws: GraphEngineError.self) {
            try await engine.create(
                kind: "container",
                parent: 999,
                position: GraphPosition(x: 0, y: 0)
            )
        }
    }

    @Test func restoreRejectsZeroDuplicateBlankAndDangling() async {
        let engine = MockGraphEngine()
        func snapshot(operators: [OperatorMirror]) -> GraphSnapshot {
            GraphSnapshot(operators: operators)
        }
        let good = OperatorMirror(
            id: 1,
            kind: "container",
            name: "Root",
            parent: nil,
            position: GraphPosition(x: 0, y: 0)
        )
        await #expect(throws: GraphEngineError.self) {
            try await engine.restore(snapshot(operators: [
                OperatorMirror(
                    id: 0,
                    kind: "container",
                    name: "Zero",
                    parent: nil,
                    position: GraphPosition(x: 0, y: 0)
                )
            ]))
        }
        await #expect(throws: GraphEngineError.self) {
            try await engine.restore(snapshot(operators: [good, good]))
        }
        await #expect(throws: GraphEngineError.self) {
            try await engine.restore(snapshot(operators: [
                OperatorMirror(
                    id: 2,
                    kind: "container",
                    name: "   ",
                    parent: nil,
                    position: GraphPosition(x: 0, y: 0)
                )
            ]))
        }
        await #expect(throws: GraphEngineError.self) {
            try await engine.restore(snapshot(operators: [
                OperatorMirror(
                    id: 3,
                    kind: "container",
                    name: "Orphan",
                    parent: 999,
                    position: GraphPosition(x: 0, y: 0)
                )
            ]))
        }
        #expect(await engine.snapshot().operators.isEmpty)
    }

    @Test func moveRenameDeleteFlow() async throws {
        let engine = MockGraphEngine()
        let id = try await engine.create(
            kind: "container",
            parent: nil,
            position: GraphPosition(x: 0, y: 0)
        )
        try await engine.move(id: id, position: GraphPosition(x: 9, y: 8))
        try await engine.rename(id: id, name: "Hero")
        let snapshot = await engine.snapshot()
        #expect(snapshot.operators.first?.position == GraphPosition(x: 9, y: 8))
        #expect(snapshot.operators.first?.name == "Hero")
        try await engine.delete(id: id)
        #expect(await engine.snapshot().operators.isEmpty)
    }

    @Test func unknownIdsAndBlankNamesAreRejected() async throws {
        let engine = MockGraphEngine()
        await #expect(throws: GraphEngineError.self) {
            try await engine.move(id: 404, position: GraphPosition(x: 0, y: 0))
        }
        await #expect(throws: GraphEngineError.self) {
            try await engine.rename(id: 404, name: "Ghost")
        }
        await #expect(throws: GraphEngineError.self) {
            try await engine.delete(id: 404)
        }
        let id = try await engine.create(
            kind: "container",
            parent: nil,
            position: GraphPosition(x: 0, y: 0)
        )
        await #expect(throws: GraphEngineError.self) {
            try await engine.rename(id: id, name: "   ")
        }
    }

    @Test func deleteCascadesSubtree() async throws {
        let engine = MockGraphEngine()
        let root = try await engine.create(
            kind: "container",
            parent: nil,
            position: GraphPosition(x: 0, y: 0)
        )
        let child = try await engine.create(
            kind: "container",
            parent: root,
            position: GraphPosition(x: 1, y: 1)
        )
        _ = try await engine.create(
            kind: "container",
            parent: child,
            position: GraphPosition(x: 2, y: 2)
        )
        try await engine.delete(id: root)
        #expect(await engine.snapshot().operators.isEmpty)
    }

    @Test func restoreRejectsWrongVersionAndContinuesIds() async throws {
        let engine = MockGraphEngine()
        await #expect(throws: GraphEngineError.self) {
            // The pre-retype v1 schema is rejected loudly, never misread.
            try await engine.restore(GraphSnapshot(version: 1, operators: []))
        }
        // The previous wire version restores like current (Rust accepts
        // both); only older schemas are rejected.
        try await engine.restore(GraphSnapshot(version: 2, operators: [
            OperatorMirror(
                id: 7, kind: "container", name: "Hero", parent: nil,
                position: GraphPosition(x: 120, y: 80)
            )
        ]))
        #expect(await engine.snapshot().operators.first?.name == "Hero")
        try await engine.restore(GraphSnapshot(operators: [
            OperatorMirror(
                id: 7, kind: "container", name: "Hero", parent: nil,
                position: GraphPosition(x: 120, y: 80)
            )
        ]))
        let next = try await engine.create(
            kind: "container",
            parent: nil,
            position: GraphPosition(x: 0, y: 0)
        )
        #expect(next == 8)
    }
}
