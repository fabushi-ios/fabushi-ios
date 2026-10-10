import CryptoKit
import Foundation

internal struct MobileComposerDraftSnapshot: Equatable {
    let text: String
    let attachments: [MobileComposerAttachment]
    let recovery: MobileComposerRecovery?

    static let empty = MobileComposerDraftSnapshot(text: "", attachments: [], recovery: nil)

    var hasActivePayload: Bool {
        mobileComposerHasPayload(text: text, attachments: attachments)
    }
}

private struct MobileComposerAttachmentWire: Codable {
    let id: String
    let name: String
    let path: String
    let mimeType: String?
    let sizeBytes: Int

    init(_ value: MobileComposerAttachment) {
        id = value.id
        name = value.name
        path = value.path
        mimeType = value.mimeType
        sizeBytes = value.sizeBytes
    }

    func project() -> MobileComposerAttachment? {
        guard !id.isEmpty, !name.isEmpty, !path.isEmpty, sizeBytes > 0 else { return nil }
        return .init(id: id, name: name, path: path, mimeType: mimeType, sizeBytes: sizeBytes)
    }
}

private struct MobileComposerRecoveryWire: Codable {
    let requestId: String
    let text: String
    let attachments: [MobileComposerAttachmentWire]

    init(_ value: MobileComposerRecovery) {
        requestId = value.requestId
        text = value.text
        attachments = value.attachments.map(MobileComposerAttachmentWire.init)
    }

    func project() -> MobileComposerRecovery? {
        guard !requestId.isEmpty else { return nil }
        let projected = attachments.compactMap { $0.project() }
        guard projected.count == attachments.count,
              mobileComposerHasPayload(text: text, attachments: projected)
        else { return nil }
        return .init(requestId: requestId, text: text, attachments: projected)
    }
}

private struct MobileComposerDraftEnvelope: Codable {
    let schemaVersion: Int
    let text: String
    let attachments: [MobileComposerAttachmentWire]
    let recovery: MobileComposerRecoveryWire?
}

internal enum MobileComposerDraftPersistence {
    static let schemaVersion = 1
    private static let prefix = "fabushi.mobile.composer-draft"

    static func key(accountScopeKey: String, agentID: String) -> String {
        "\(prefix).v\(schemaVersion).\(stableComponent(accountScopeKey)).\(stableComponent(agentID))"
    }

    static func load(
        accountScopeKey: String,
        agentID: String,
        defaults: UserDefaults = .standard
    ) -> MobileComposerDraftSnapshot {
        let storageKey = key(accountScopeKey: accountScopeKey, agentID: agentID)
        guard let data = defaults.data(forKey: storageKey) else { return .empty }
        do {
            let envelope = try JSONDecoder().decode(MobileComposerDraftEnvelope.self, from: data)
            guard envelope.schemaVersion == schemaVersion else {
                defaults.removeObject(forKey: storageKey)
                return .empty
            }
            let attachments = envelope.attachments.compactMap { $0.project() }
            guard attachments.count == envelope.attachments.count else {
                defaults.removeObject(forKey: storageKey)
                return .empty
            }
            let recovery: MobileComposerRecovery?
            if let rawRecovery = envelope.recovery {
                guard let projected = rawRecovery.project() else {
                    defaults.removeObject(forKey: storageKey)
                    return .empty
                }
                recovery = projected
            } else {
                recovery = nil
            }
            return .init(text: envelope.text, attachments: attachments, recovery: recovery)
        } catch {
            defaults.removeObject(forKey: storageKey)
            return .empty
        }
    }

    static func save(
        accountScopeKey: String,
        agentID: String,
        snapshot: MobileComposerDraftSnapshot,
        defaults: UserDefaults = .standard
    ) {
        let storageKey = key(accountScopeKey: accountScopeKey, agentID: agentID)
        guard snapshot.hasActivePayload || snapshot.recovery != nil else {
            defaults.removeObject(forKey: storageKey)
            return
        }
        let envelope = MobileComposerDraftEnvelope(
            schemaVersion: schemaVersion,
            text: snapshot.text,
            attachments: snapshot.attachments.map(MobileComposerAttachmentWire.init),
            recovery: snapshot.recovery.map(MobileComposerRecoveryWire.init)
        )
        guard let data = try? JSONEncoder().encode(envelope) else { return }
        defaults.set(data, forKey: storageKey)
    }

    static func clear(
        accountScopeKey: String,
        agentID: String,
        defaults: UserDefaults = .standard
    ) {
        defaults.removeObject(forKey: key(accountScopeKey: accountScopeKey, agentID: agentID))
    }

    private static func stableComponent(_ value: String) -> String {
        SHA256.hash(data: Data(value.utf8))
            .prefix(12)
            .map { String(format: "%02x", $0) }
            .joined()
    }
}
