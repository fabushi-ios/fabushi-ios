import Foundation

internal enum MobileCommandPaletteTab: String, CaseIterable, Identifiable {
    case all
    case messages
    case agents
    case groups
    case files
    case links
    case routines
    case actions

    var id: String { rawValue }

    var title: String {
        switch self {
        case .all: "All"
        case .messages: "Messages"
        case .agents: "Bots & Chats"
        case .groups: "Groups"
        case .files: "Files"
        case .links: "Links"
        case .routines: "Routines"
        case .actions: "Actions"
        }
    }
}

internal enum MobileCommandPaletteActionKind: String {
    case createBot
    case openWorkspace
}

internal struct MobileCommandPaletteAction: Identifiable {
    let id: String
    let label: String
    let keywords: [String]
    let detail: String
    let kind: MobileCommandPaletteActionKind
}

internal struct MobileCommandPaletteMessage: Identifiable {
    let conversationId: String
    let messageId: String
    let conversationTitle: String
    let snippet: String
    let isOutgoing: Bool

    var id: String { "message:\(conversationId):\(messageId)" }
}

internal struct MobileCommandPaletteFile: Identifiable {
    let conversationId: String
    let messageId: String
    let conversationTitle: String
    let fileName: String
    let kind: String

    var id: String { "file:\(conversationId):\(messageId)" }
}

internal struct MobileCommandPaletteLink: Identifiable {
    let conversationId: String
    let messageId: String
    let conversationTitle: String
    let url: String

    var id: String { "link:\(conversationId):\(messageId):\(url)" }

    var displayURL: String {
        guard let parsed = URL(string: url), let host = parsed.host else { return url }
        let path = parsed.path == "/" ? "" : parsed.path
        return host + path
    }
}

internal enum MobileCommandPaletteEntry: Identifiable {
    case bot(MobileBotSummary)
    case conversation(ConversationSummary)
    case message(MobileCommandPaletteMessage)
    case file(MobileCommandPaletteFile)
    case link(MobileCommandPaletteLink)
    case action(MobileCommandPaletteAction)

    var id: String {
        switch self {
        case .bot(let bot): "bot:\(bot.id)"
        case .conversation(let conversation): "conversation:\(conversation.id)"
        case .message(let message): message.id
        case .file(let file): file.id
        case .link(let link): link.id
        case .action(let action): "action:\(action.id)"
        }
    }

    var accessibilityKey: String {
        String(id.map { character in
            character.isASCII && (character.isLetter || character.isNumber || "-._".contains(character)) ? character : "-"
        })
    }

    var label: String {
        switch self {
        case .bot(let bot): bot.name
        case .conversation(let conversation): conversation.title
        case .message(let message): message.snippet
        case .file(let file): file.fileName
        case .link(let link): link.displayURL
        case .action(let action): action.label
        }
    }

    var detail: String {
        switch self {
        case .bot(let bot):
            return bot.description.isEmpty ? "Bot" : bot.description
        case .conversation(let conversation):
            return conversation.kind == .channel ? "Channel" : conversation.kind == .group ? "Group" : "Chat"
        case .message(let message):
            return "\(message.isOutgoing ? "You in" : "In") \(message.conversationTitle)"
        case .file(let file):
            return "\(file.conversationTitle) · \(file.kind.capitalized)"
        case .link(let link):
            return link.conversationTitle
        case .action(let action):
            return action.detail
        }
    }

    var systemImage: String {
        switch self {
        case .bot: return "sparkles"
        case .conversation(let conversation):
            return conversation.kind == .channel ? "megaphone" : conversation.kind == .group ? "person.3" : "bubble.left.and.bubble.right"
        case .message: return "quote.bubble"
        case .file: return "doc"
        case .link: return "link"
        case .action(let action):
            return action.kind == .createBot ? "plus.circle" : "rectangle.grid.1x2"
        }
    }
}

internal enum GrokMobileCommandPaletteModel {
    private static let maximumFuzzySpanMultiplier = 3

    static func normalizeSearch(_ value: String) -> String {
        let folded = value.folding(
            options: [.diacriticInsensitive, .caseInsensitive, .widthInsensitive],
            locale: Locale.current
        ).lowercased()
        var result = ""
        var pendingSeparator = false
        for character in folded {
            if character.isLetter || character.isNumber {
                if pendingSeparator && !result.isEmpty { result.append(" ") }
                result.append(character)
                pendingSeparator = false
            } else {
                pendingSeparator = true
            }
        }
        return result
    }

    static func searchTokens(_ value: String) -> [String] {
        let normalized = normalizeSearch(value)
        return normalized.isEmpty ? [] : normalized.split(separator: " ").map(String.init)
    }

    static func fuzzyScore(value: String, query: String) -> Double? {
        guard !query.isEmpty else { return 0 }
        let valueCharacters = Array(value.lowercased())
        let queryCharacters = Array(query.lowercased())
        var score = 0
        var queryIndex = 0
        var previousMatch = -2
        var firstMatch = -1
        var lastMatch = -1

        for index in valueCharacters.indices where queryIndex < queryCharacters.count {
            guard valueCharacters[index] == queryCharacters[queryIndex] else { continue }
            if firstMatch < 0 { firstMatch = index }
            let previousCharacter = index > 0 ? valueCharacters[index - 1] : nil
            let boundary = index == 0 || previousCharacter == " " || previousCharacter == "-" || previousCharacter == "_" || previousCharacter == "/" || previousCharacter == "."
            var characterScore = 1
            if boundary { characterScore += 4 }
            if previousMatch == index - 1 { characterScore += 3 }
            score += characterScore
            previousMatch = index
            lastMatch = index
            queryIndex += 1
        }

        guard queryIndex == queryCharacters.count else { return nil }
        guard lastMatch - firstMatch + 1 <= queryCharacters.count * maximumFuzzySpanMultiplier else { return nil }
        return Double(score) - Double(firstMatch) * 0.1 - Double(valueCharacters.count) * 0.02
    }

    static func messages(
        conversations: [ConversationSummary],
        messagesByConversation: [String: [ChatMessage]]
    ) -> [MobileCommandPaletteMessage] {
        var rows: [MobileCommandPaletteMessage] = []
        for conversation in conversations where !conversation.isArchived {
            for message in (messagesByConversation[conversation.id] ?? []).reversed() {
                let snippet = message.text.trimmingCharacters(in: .whitespacesAndNewlines)
                guard !snippet.isEmpty else { continue }
                rows.append(.init(
                    conversationId: conversation.id,
                    messageId: message.id,
                    conversationTitle: conversation.title,
                    snippet: snippet,
                    isOutgoing: message.isOutgoing
                ))
            }
        }
        return rows
    }

    static func files(
        conversations: [ConversationSummary],
        messagesByConversation: [String: [ChatMessage]]
    ) -> [MobileCommandPaletteFile] {
        var rows: [MobileCommandPaletteFile] = []
        for conversation in conversations where !conversation.isArchived {
            for message in (messagesByConversation[conversation.id] ?? []).reversed() {
                guard let fileName = message.mediaFileName, !fileName.isEmpty else { continue }
                rows.append(.init(
                    conversationId: conversation.id,
                    messageId: message.id,
                    conversationTitle: conversation.title,
                    fileName: fileName,
                    kind: fileKind(message: message)
                ))
            }
        }
        return rows
    }

    static func links(
        conversations: [ConversationSummary],
        messagesByConversation: [String: [ChatMessage]]
    ) -> [MobileCommandPaletteLink] {
        var rows: [MobileCommandPaletteLink] = []
        var seen = Set<String>()
        for conversation in conversations where !conversation.isArchived {
            for message in (messagesByConversation[conversation.id] ?? []).reversed() {
                for url in extractHTTPLinks(message.text) {
                    guard seen.insert(url).inserted else { continue }
                    rows.append(.init(
                        conversationId: conversation.id,
                        messageId: message.id,
                        conversationTitle: conversation.title,
                        url: url
                    ))
                }
            }
        }
        return rows
    }

    static func entries(
        bots: [MobileBotSummary],
        conversations: [ConversationSummary],
        messagesByConversation: [String: [ChatMessage]],
        actions: [MobileCommandPaletteAction],
        query: String,
        tab: MobileCommandPaletteTab
    ) -> [MobileCommandPaletteEntry] {
        let uniqueBots = deduplicatedBots(bots)
        let conversationEntries = conversations
            .filter { !$0.isArchived }
            .map(MobileCommandPaletteEntry.conversation)
        let botEntries = uniqueBots.map(MobileCommandPaletteEntry.bot)
        let actionEntries = actions.map(MobileCommandPaletteEntry.action)
        let messageEntries = messages(conversations: conversations, messagesByConversation: messagesByConversation)
            .map(MobileCommandPaletteEntry.message)
        let fileEntries = files(conversations: conversations, messagesByConversation: messagesByConversation)
            .map(MobileCommandPaletteEntry.file)
        let linkEntries = links(conversations: conversations, messagesByConversation: messagesByConversation)
            .map(MobileCommandPaletteEntry.link)

        let base = (botEntries + conversationEntries + fileEntries + linkEntries + messageEntries + actionEntries)
            .filter { matches(tab: tab, entry: $0) }
        let tokens = searchTokens(query)
        if tokens.isEmpty {
            if tab == .all {
                return (botEntries + conversationEntries + actionEntries).prefix(100).map { $0 }
            }
            return base.prefix(100).map { $0 }
        }

        let normalizedQuery = tokens.joined(separator: " ")
        let scored = base.enumerated().compactMap { index, entry -> (Int, Double, MobileCommandPaletteEntry)? in
            guard let score = score(entry: entry, tokens: tokens, normalizedQuery: normalizedQuery) else { return nil }
            return (index, score, entry)
        }
        return scored.sorted {
            if $0.1 == $1.1 { return $0.0 < $1.0 }
            return $0.1 > $1.1
        }.prefix(100).map { $0.2 }
    }

    private static func deduplicatedBots(_ bots: [MobileBotSummary]) -> [MobileBotSummary] {
        var seen = Set<String>()
        return bots.filter { seen.insert($0.id).inserted }
    }

    private static func matches(tab: MobileCommandPaletteTab, entry: MobileCommandPaletteEntry) -> Bool {
        switch tab {
        case .all: return true
        case .messages:
            if case .message = entry { return true }
            return false
        case .agents:
            if case .bot = entry { return true }
            if case .conversation(let conversation) = entry {
                return conversation.kind == .direct || conversation.kind == .savedMessages || conversation.kind == .secret
            }
            return false
        case .groups:
            if case .conversation(let conversation) = entry {
                return conversation.kind == .group || conversation.kind == .channel
            }
            return false
        case .files:
            if case .file = entry { return true }
            return false
        case .links:
            if case .link = entry { return true }
            return false
        case .routines:
            return false
        case .actions:
            if case .action = entry { return true }
            return false
        }
    }

    private static func score(
        entry: MobileCommandPaletteEntry,
        tokens: [String],
        normalizedQuery: String
    ) -> Double? {
        let label = normalizeSearch(entry.label)
        let candidates = [label] + entryKeywords(entry).map(normalizeSearch)
        var total = 0.0
        for token in tokens {
            var best: Double?
            for candidate in candidates {
                guard let value = fuzzyScore(value: candidate, query: token) else { continue }
                if best == nil || value > best! { best = value }
            }
            guard let best else { return nil }
            total += best
        }
        return total + (fuzzyScore(value: label, query: normalizedQuery) ?? 0)
    }

    private static func entryKeywords(_ entry: MobileCommandPaletteEntry) -> [String] {
        switch entry {
        case .bot(let bot):
            return [bot.description, "bot", "agent"]
        case .conversation(let conversation):
            return [conversation.description, conversation.preview, conversation.kind.rawValue]
        case .message(let message):
            // Desktop message search receives focused backend snippets. The iOS
            // adapter projects the canonical local transcript directly, so add
            // lexical candidates to prevent an earlier URL character from
            // consuming the bounded fuzzy match for a later exact word.
            return [message.conversationTitle, message.snippet] + searchTokens(message.snippet)
        case .file(let file):
            return [file.conversationTitle, file.kind, (file.fileName as NSString).pathExtension]
        case .link(let link):
            return [link.conversationTitle, link.url]
        case .action(let action):
            return action.keywords
        }
    }

    private static func fileKind(message: ChatMessage) -> String {
        switch message.contentType {
        case "photo": return "image"
        case "video": return "video"
        case "audio": return "audio"
        case "voice": return "audio"
        case "document":
            let ext = ((message.mediaFileName ?? "") as NSString).pathExtension.lowercased()
            if ext == "pdf" { return "pdf" }
            if ["md", "markdown"].contains(ext) { return "markdown" }
            if ["csv", "tsv", "xls", "xlsx"].contains(ext) { return "table" }
            if ext == "json" { return "json" }
            if ["txt", "log"].contains(ext) { return "text" }
            if ["zip", "tar", "gz", "7z", "rar"].contains(ext) { return "archive" }
            return "document"
        default:
            if message.mediaMimeType?.hasPrefix("image/") == true { return "image" }
            if message.mediaMimeType?.hasPrefix("video/") == true { return "video" }
            if message.mediaMimeType?.hasPrefix("audio/") == true { return "audio" }
            return "file"
        }
    }

    private static func extractHTTPLinks(_ text: String) -> [String] {
        guard let expression = try? NSRegularExpression(pattern: #"https?://[^\s<>"']+"#, options: [.caseInsensitive]) else {
            return []
        }
        let range = NSRange(text.startIndex..<text.endIndex, in: text)
        let trailing = CharacterSet(charactersIn: ".,;:!?)]}>'\"")
        return expression.matches(in: text, range: range).compactMap { match in
            guard let swiftRange = Range(match.range, in: text) else { return nil }
            let raw = String(text[swiftRange]).trimmingCharacters(in: trailing)
            guard let url = URL(string: raw),
                  let scheme = url.scheme?.lowercased(),
                  (scheme == "http" || scheme == "https"),
                  url.host?.isEmpty == false
            else { return nil }
            return url.absoluteString
        }
    }
}
