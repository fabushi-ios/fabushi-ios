import Foundation

@MainActor
internal enum MobileUiAgentRefsPersistence {
    static let schemaVersion = 1
    static let maxMentionRecents = 20
    static let maxEmojiRecents = 50

    private struct Envelope: Codable, Equatable {
        let schemaVersion: Int
        var recentKeys: [String]
    }

    static func normalizedRecentKeys(_ values: [String]) -> [String] {
        var seen = Set<String>()
        var mentionCount = 0
        var emojiCount = 0
        var output: [String] = []

        for raw in values {
            let key = raw.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !key.isEmpty,
                  seen.insert(key).inserted,
                  let separator = key.firstIndex(of: ":"),
                  separator > key.startIndex,
                  separator < key.index(before: key.endIndex)
            else { continue }

            let category = String(key[..<separator])
            switch category {
            case "emoji":
                guard emojiCount < maxEmojiRecents else { continue }
                emojiCount += 1
            case "assistants", "automations", "tools":
                guard mentionCount < maxMentionRecents else { continue }
                mentionCount += 1
            default:
                continue
            }
            output.append(key)
        }
        return output
    }

    static func recordingRecent(
        _ key: String,
        existing: [String]
    ) -> [String] {
        normalizedRecentKeys([key] + existing.filter { $0 != key })
    }

    static func emojiValues(from recentKeys: [String]) -> [String] {
        normalizedRecentKeys(recentKeys).compactMap { key in
            guard key.hasPrefix("emoji:") else { return nil }
            let emoji = String(key.dropFirst("emoji:".count))
                .trimmingCharacters(in: .whitespacesAndNewlines)
            return emoji.isEmpty ? nil : emoji
        }
    }

    static func loadRecentKeys(
        accountScopeKey: String,
        defaults: UserDefaults = .standard
    ) -> [String] {
        let account = accountScopeKey.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !account.isEmpty,
              let key = storageKey(account),
              let data = defaults.data(forKey: key)
        else { return [] }

        let envelope: Envelope
        do {
            envelope = try JSONDecoder().decode(Envelope.self, from: data)
        } catch {
            defaults.removeObject(forKey: key)
            return []
        }
        guard envelope.schemaVersion == schemaVersion else {
            defaults.removeObject(forKey: key)
            return []
        }
        return normalizedRecentKeys(envelope.recentKeys)
    }

    static func persistRecentKeys(
        _ values: [String],
        accountScopeKey: String,
        defaults: UserDefaults = .standard
    ) {
        let account = accountScopeKey.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !account.isEmpty,
              let key = storageKey(account)
        else { return }

        let normalized = normalizedRecentKeys(values)
        if normalized.isEmpty {
            defaults.removeObject(forKey: key)
            return
        }
        let envelope = Envelope(
            schemaVersion: schemaVersion,
            recentKeys: normalized
        )
        guard let data = try? JSONEncoder().encode(envelope) else { return }
        defaults.set(data, forKey: key)
    }

    static func storageKey(_ accountScopeKey: String) -> String? {
        let account = accountScopeKey.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !account.isEmpty else { return nil }
        let encoded = Data(account.utf8).base64EncodedString()
            .replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "+", with: "-")
            .replacingOccurrences(of: "=", with: "")
        return "fabushi.mobile.ui-agent-refs.v1.\(encoded)"
    }
}
