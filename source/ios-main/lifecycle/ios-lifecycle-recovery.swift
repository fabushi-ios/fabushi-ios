import Foundation

enum IOSLifecyclePhase: String, Codable, Equatable, Sendable {
    case starting
    case active
    case inactive
    case background
    case shuttingDown
}

struct IOSLifecycleCheckpoint: Codable, Equatable, Sendable {
    let sessionID: String
    var phase: IOSLifecyclePhase
    var updatedAt: Date
    var cleanShutdown: Bool
    var needsResync: Bool
}

@MainActor
final class IOSLifecycleRecoveryStore {
    private let checkpointURL: URL
    private let now: () -> Date
    private(set) var previousCheckpoint: IOSLifecycleCheckpoint?
    private(set) var currentCheckpoint: IOSLifecycleCheckpoint

    init(
        appDataDirectory: URL,
        now: @escaping () -> Date = Date.init
    ) throws {
        self.now = now
        checkpointURL = appDataDirectory.appendingPathComponent(
            "ios-lifecycle-checkpoint.json",
            isDirectory: false
        )
        try FileManager.default.createDirectory(
            at: appDataDirectory,
            withIntermediateDirectories: true
        )

        previousCheckpoint = try Self.readCheckpoint(at: checkpointURL)
        currentCheckpoint = IOSLifecycleCheckpoint(
            sessionID: UUID().uuidString.lowercased(),
            phase: .starting,
            updatedAt: now(),
            cleanShutdown: false,
            needsResync: previousCheckpoint.map { !$0.cleanShutdown || $0.needsResync } ?? false
        )
        try persist()
    }

    var requiresColdStartResync: Bool { currentCheckpoint.needsResync }

    func transition(to phase: IOSLifecyclePhase) {
        currentCheckpoint.phase = phase
        currentCheckpoint.updatedAt = now()
        if phase == .background || phase == .inactive {
            currentCheckpoint.needsResync = true
        }
        if phase == .shuttingDown {
            currentCheckpoint.cleanShutdown = true
        }
        try? persist()
    }

    func markResyncRequired() {
        currentCheckpoint.needsResync = true
        currentCheckpoint.updatedAt = now()
        try? persist()
    }

    func markResyncCompleted() {
        currentCheckpoint.needsResync = false
        currentCheckpoint.updatedAt = now()
        try? persist()
    }

    private func persist() throws {
        let data = try JSONEncoder().encode(currentCheckpoint)
        try data.write(to: checkpointURL, options: [.atomic])
    }

    private static func readCheckpoint(at url: URL) throws -> IOSLifecycleCheckpoint? {
        guard FileManager.default.fileExists(atPath: url.path) else { return nil }
        let data = try Data(contentsOf: url)
        return try JSONDecoder().decode(IOSLifecycleCheckpoint.self, from: data)
    }
}
