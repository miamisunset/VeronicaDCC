import ComposableArchitecture
import Foundation

/// Launch flags governing graph startup behavior.
///
/// Explicitly `nonisolated`: read from background reducer effects.
nonisolated enum GraphLaunchOptions {
    /// Deletes any autosaved graph before loading (UI-test isolation).
    static let resetArgument = "--vrn-reset-graph"
    /// Enables the in-memory graph engine double (UI tests).
    static let mockEngineArgument = "--vrn-mock-engine"

    /// Returns true when the app launched with `--vrn-reset-graph`.
    static var resetGraphOnLaunch: Bool {
        CommandLine.arguments.contains(resetArgument)
    }

    /// Returns true when the app launched with `--vrn-mock-engine`.
    static var isMockEngineEnabled: Bool {
        CommandLine.arguments.contains(mockEngineArgument)
    }
}

/// TCA dependency for graph autosave (ADR-0002).
///
/// Every committed mutation writes the fresh snapshot JSON to
/// `Application Support/<bundle-id>/graph-v1.json`; launch loads it back via
/// `restore`. A missing or corrupt file starts empty — never a crash.
nonisolated struct GraphPersistence: Sendable {
    /// Snapshot filename inside the bundle's Application Support directory.
    static let fileName = "graph-v1.json"
    /// Bundle id fallback when `Bundle.main` reports none (never in the app).
    static let bundleIdFallback = "com.github.miamisunset.Veronica"

    /// Reads the autosaved snapshot data, or `nil` when no file exists.
    ///
    /// Async so file I/O never blocks a caller and test doubles can
    /// serialize through actors.
    var load: @Sendable () async throws(GraphEngineError) -> Data?
    /// Encodes and atomically writes `snapshot` to the autosave file.
    ///
    /// Async (see `load`).
    var save: @Sendable (GraphSnapshot) async throws(GraphEngineError) -> Void
}

extension GraphPersistence: DependencyKey {
    /// Returns the autosave file URL, creating the directory on demand.
    nonisolated static func fileURL() throws(GraphEngineError) -> URL {
        let bundleId = Bundle.main.bundleIdentifier ?? bundleIdFallback
        let manager = FileManager.default
        do {
            let directory = try manager.url(
                for: .applicationSupportDirectory,
                in: .userDomainMask,
                appropriateFor: nil,
                create: true
            )
            .appendingPathComponent(bundleId, isDirectory: true)
            return directory.appendingPathComponent(fileName, isDirectory: false)
        } catch {
            throw .persistenceFailed(error.localizedDescription)
        }
    }

    /// Encodes and atomically writes `snapshot` to `url`.
    nonisolated static func save(_ snapshot: GraphSnapshot, to url: URL) throws(GraphEngineError) {
        let data: Data
        do {
            let encoder = JSONEncoder()
            encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
            data = try encoder.encode(snapshot)
        } catch {
            throw GraphEngineError.snapshotEncodingFailed(error.localizedDescription)
        }
        do {
            try FileManager.default.createDirectory(
                at: url.deletingLastPathComponent(),
                withIntermediateDirectories: true
            )
            try data.write(to: url, options: .atomic)
        } catch {
            throw GraphEngineError.persistenceFailed(error.localizedDescription)
        }
    }

    /// Reads snapshot data from `url`, or `nil` when no file exists.
    nonisolated static func load(from url: URL) throws(GraphEngineError) -> Data? {
        guard FileManager.default.fileExists(atPath: url.path) else {
            return nil
        }
        do {
            return try Data(contentsOf: url)
        } catch {
            throw GraphEngineError.persistenceFailed(error.localizedDescription)
        }
    }

    static let liveValue = GraphPersistence(
        load: { () async throws(GraphEngineError) -> Data? in
            if GraphLaunchOptions.resetGraphOnLaunch {
                return nil
            }
            return try load(from: fileURL())
        },
        save: { (snapshot: GraphSnapshot) async throws(GraphEngineError) in
            try save(snapshot, to: fileURL())
        }
    )

    static let testValue = GraphPersistence(
        load: { nil },
        save: { _ in }
    )
}

extension DependencyValues {
    /// Access graph autosave from any reducer via `@Dependency(\.graphPersistence)`.
    ///
    /// Explicitly `nonisolated`: the key path must be `Sendable` for
    /// `@Dependency`, which a MainActor-isolated getter would forbid.
    nonisolated var graphPersistence: GraphPersistence {
        get { self[GraphPersistence.self] }
        set { self[GraphPersistence.self] = newValue }
    }
}
