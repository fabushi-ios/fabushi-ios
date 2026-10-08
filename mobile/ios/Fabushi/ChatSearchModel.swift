import Foundation

internal struct ChatSearchEntry: Equatable, Sendable {
    let id: String
    let text: String
}

internal struct ChatSearchMatch: Equatable, Sendable {
    let entryId: String
    let occurrence: Int
}

internal func chatSearchText(for message: ChatMessage) -> String {
    [message.text, message.mediaFileName, message.contactName, message.pollQuestion, message.pollOptions.isEmpty ? nil : message.pollOptions.map(\.text).joined(separator: "\n"), message.forwardOrigin]
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
