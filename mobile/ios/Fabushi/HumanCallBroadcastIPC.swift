import Foundation

internal enum HumanCallBroadcastIPC {
    static let appGroupIdentifier = "group.com.ombhrum.fabushi.call"
    static let extensionBundleIdentifier = "com.ombhrum.fabushi.broadcast"
    static let desiredSessionKey = "fabushi.human-call.broadcast.desired-session"
    static let metadataFilename = "human-call-broadcast-metadata.json"

    static func sharedDefaults() -> UserDefaults? {
        UserDefaults(suiteName: appGroupIdentifier)
    }

    static func containerURL(fileManager: FileManager = .default) -> URL? {
        fileManager.containerURL(
            forSecurityApplicationGroupIdentifier: appGroupIdentifier
        )
    }

    static func beginSession(_ sessionID: String) {
        sharedDefaults()?.set(sessionID, forKey: desiredSessionKey)
    }

    static func endSession(_ sessionID: String) {
        guard sharedDefaults()?.string(forKey: desiredSessionKey) == sessionID else {
            return
        }
        sharedDefaults()?.removeObject(forKey: desiredSessionKey)
    }

    static func desiredSessionID() -> String? {
        guard let raw = sharedDefaults()?.string(forKey: desiredSessionKey) else {
            return nil
        }
        let normalized = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        return normalized.isEmpty ? nil : normalized
    }
}

internal struct HumanCallBroadcastFrameMetadata: Codable, Equatable, Sendable {
    enum State: String, Codable, Sendable {
        case started
        case frame
        case finished
    }

    let sessionID: String
    let sequence: UInt64
    let state: State
    let frameFilename: String?
    let timestampNanoseconds: Int64
    let orientation: UInt32
}
