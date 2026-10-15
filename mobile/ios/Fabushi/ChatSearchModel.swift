import Foundation

internal struct ChatSearchEntry: Equatable, Sendable {
    let id: String
    let text: String
}

internal struct ChatSearchMatch: Equatable, Sendable {
    let entryId: String
    let occurrence: Int
}

internal func chatSearchText(for message: ChatMessage, author: String? = nil) -> String {
    [message.text, author, message.mediaFileName, message.contactName, message.pollQuestion, message.pollOptions.isEmpty ? nil : message.pollOptions.map(\.text).joined(separator: "\n"), message.forwardOrigin]
        .compactMap { $0?.trimmingCharacters(in: .whitespacesAndNewlines) }
        .filter { !$0.isEmpty }
        .joined(separator: "\n")
}

internal func chatSearchMatches(_ entries: [ChatSearchEntry], query: String) -> [ChatSearchMatch] {
    guard !query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return [] }
    var matches: [ChatSearchMatch] = []
    for entry in entries {
        var searchRange = entry.text.startIndex..<entry.text.endIndex
        var occurrence = 0
        while let range = entry.text.range(of: query, options: [.caseInsensitive, .diacriticInsensitive], range: searchRange, locale: .current) {
            matches.append(ChatSearchMatch(entryId: entry.id, occurrence: occurrence))
            occurrence += 1
            guard range.upperBound < entry.text.endIndex else { break }
            searchRange = range.upperBound..<entry.text.endIndex
        }
    }
    return matches
}

internal func nextChatSearchIndex(current: Int?, count: Int, delta: Int) -> Int? {
    guard count > 0, delta == 1 || delta == -1 else { return nil }
    if let current {
        let normalized = min(max(current, 0), count - 1)
        return (normalized + delta + count) % count
    }
    return delta > 0 ? 0 : count - 1
}


internal struct DurableContentSearchResult: Equatable, Sendable {
    let entryId: String
    let conversationId: String
    let snippet: String
    let timestampMs: Int64?
    let score: Int
}

@MainActor
internal struct DurableContentSearchClient {
    let bridge: IOSPreloadBridge

    private func normalizedScope(accountKey: String, conversationId: String) -> (String, String)? {
        let account = accountKey.trimmingCharacters(in: .whitespacesAndNewlines)
        let conversation = conversationId.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !account.isEmpty, !conversation.isEmpty else { return nil }
        return (account, conversation)
    }

    func replaceScope(
        accountKey: String,
        conversationId: String,
        entries: [ChatSearchEntry]
    ) async throws {
        guard let (account, conversation) = normalizedScope(
            accountKey: accountKey,
            conversationId: conversationId
        ) else { return }
        _ = try await bridge.request(
            method: "feature.contentSearch.replaceScope",
            params: [
                "accountKey": account,
                "conversationId": conversation,
                "entries": Self.indexPayload(entries),
            ]
        )
    }

    func upsertEntries(
        accountKey: String,
        conversationId: String,
        entries: [ChatSearchEntry]
    ) async throws {
        guard !entries.isEmpty,
              let (account, conversation) = normalizedScope(
                accountKey: accountKey,
                conversationId: conversationId
              )
        else { return }
        _ = try await bridge.request(
            method: "feature.contentSearch.upsertEntries",
            params: [
                "accountKey": account,
                "conversationId": conversation,
                "entries": Self.indexPayload(entries),
            ]
        )
    }

    func deleteEntry(
        accountKey: String,
        conversationId: String,
        entryId: String
    ) async throws {
        guard let (account, conversation) = normalizedScope(
            accountKey: accountKey,
            conversationId: conversationId
        ) else { return }
        let id = entryId.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !id.isEmpty else { return }
        _ = try await bridge.request(
            method: "feature.contentSearch.deleteEntry",
            params: [
                "accountKey": account,
                "conversationId": conversation,
                "entryId": id,
            ]
        )
    }

    func searchMessages(
        accountKey: String,
        conversationId: String,
        query: String,
        limit: Int = 200
    ) async throws -> [DurableContentSearchResult] {
        guard let (account, conversation) = normalizedScope(
            accountKey: accountKey,
            conversationId: conversationId
        ) else { return [] }
        let query = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !query.isEmpty else { return [] }
        let response = try await bridge.request(
            method: "feature.contentSearch.searchMessages",
            params: [
                "accountKey": account,
                "conversationId": conversation,
                "query": query,
                "limit": max(1, min(limit, 200)),
            ]
        )
        return Self.results(from: response.value, conversationId: conversation)
    }

    nonisolated static func indexPayload(_ entries: [ChatSearchEntry]) -> [[String: Any]] {
        entries.map {
            [
                "id": $0.id,
                "kind": "message",
                "timestampMs": NSNumber(value: 0),
                "role": "assistant",
                "body": $0.text,
            ]
        }
    }

    nonisolated static func results(
        from value: Any,
        conversationId: String
    ) -> [DurableContentSearchResult] {
        guard let object = value as? [String: Any],
              let rows = object["results"] as? [[String: Any]]
        else { return [] }
        return rows.compactMap { row in
            guard row["kind"] as? String == "message",
                  let entryId = (row["entryId"] as? String) ?? (row["id"] as? String),
                  !entryId.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
                  let resultConversationId = row["conversationId"] as? String,
                  resultConversationId == conversationId
            else { return nil }
            return DurableContentSearchResult(
                entryId: entryId,
                conversationId: resultConversationId,
                snippet: row["snippet"] as? String ?? "",
                timestampMs: (row["timestampMs"] as? NSNumber)?.int64Value,
                score: (row["score"] as? NSNumber)?.intValue ?? 0
            )
        }
    }
}
