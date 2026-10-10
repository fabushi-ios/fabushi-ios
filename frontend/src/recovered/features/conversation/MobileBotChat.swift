import AVKit
import CryptoKit
import SwiftUI
import UIKit
import UniformTypeIdentifiers

enum MobileTranscriptLoadErrorCopy {
    static let title = "Couldn't load conversation"
    static let detail = "Couldn't load this conversation. Check your connection and try again."
    static let retry = "Retry"
}

internal enum MobileReactionPickerCategory: String, CaseIterable, Identifiable {
    case all = "All"
    case people = "Smileys & People"
    case nature = "Animals & Nature"
    case food = "Food & Drink"
    case activities = "Activities"
    case travel = "Travel & Places"
    case objects = "Objects"
    case symbols = "Symbols"
    case flags = "Flags"

    var id: String { rawValue }
}

internal struct MobileReactionCatalogItem: Identifiable, Equatable {
    let id: String
    let emoji: String
    let name: String
    let category: MobileReactionPickerCategory
    let shortcodes: [String]
    let search: String
    let hexcode: String
    let baseHexcode: String?
    let isSkinVariant: Bool

    init(
        id: String? = nil,
        emoji: String,
        name: String,
        category: MobileReactionPickerCategory,
        shortcodes: [String] = [],
        search: String? = nil,
        hexcode: String = "",
        baseHexcode: String? = nil,
        isSkinVariant: Bool = false
    ) {
        self.id = id ?? "\(category.rawValue):\(emoji)"
        self.emoji = emoji
        self.name = name
        self.category = category
        self.shortcodes = shortcodes
        self.search = search ?? ([name, id ?? "", shortcodes.joined(separator: " ")]
            .joined(separator: " ")
            .lowercased())
        self.hexcode = hexcode
        self.baseHexcode = baseHexcode
        self.isSkinVariant = isSkinVariant
    }
}

internal let mobileReactionCatalog: [MobileReactionCatalogItem] = mobileDesktopEmojiCatalog

internal func mobileReactionPickerResults(
    query: String,
    category: MobileReactionPickerCategory,
    limit: Int = 96,
    recentIds: [String] = []
) -> [MobileReactionCatalogItem] {
    let boundedLimit = max(0, min(limit, 96))
    guard boundedLimit > 0 else { return [] }
    let normalizedQuery = query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
    var exact: [(Int, MobileReactionCatalogItem)] = []
    var secondary: [(Int, MobileReactionCatalogItem)] = []

    for (sourceIndex, item) in mobileReactionCatalog.enumerated() {
        guard category == .all || item.category == category else { continue }
        if normalizedQuery.isEmpty {
            exact.append((sourceIndex, item))
            continue
        }
        guard item.search.contains(normalizedQuery) else { continue }
        let name = item.name.lowercased()
        let isExact = item.id.lowercased().hasPrefix(normalizedQuery)
            || item.shortcodes.contains(where: { $0.lowercased().hasPrefix(normalizedQuery) })
            || name.hasPrefix(normalizedQuery)
            || name.contains(" " + normalizedQuery)
        if isExact {
            exact.append((sourceIndex, item))
        } else {
            secondary.append((sourceIndex, item))
        }
    }

    var recentOrder: [String: Int] = [:]
    for (index, id) in recentIds.enumerated() where recentOrder[id] == nil {
        recentOrder[id] = index
    }
    func ordered(_ values: [(Int, MobileReactionCatalogItem)]) -> [MobileReactionCatalogItem] {
        values.sorted { left, right in
            let leftRecent = recentOrder[left.1.id] ?? Int.max
            let rightRecent = recentOrder[right.1.id] ?? Int.max
            if leftRecent != rightRecent { return leftRecent < rightRecent }
            return left.0 < right.0
        }.map(\.1)
    }
    return Array((ordered(exact) + ordered(secondary)).prefix(boundedLimit))
}

internal func isMobileReactionActionable(_ entry: MobileChatMessage) -> Bool {
    guard entry.kind == .message,
          !entry.streaming,
          entry.optimisticDeliveryPhase == nil,
          let canonicalId = entry.canonicalMessageId?
            .trimmingCharacters(in: .whitespacesAndNewlines),
          !canonicalId.isEmpty
    else { return false }
    return true
}

internal func mobileReactionPickerAccessibilityLabel(
    _ item: MobileReactionCatalogItem,
    reactedByCurrentUser: Bool
) -> String {
    reactedByCurrentUser
        ? "\(item.name), \(item.emoji), reacted by you"
        : "\(item.name), \(item.emoji), not reacted"
}

internal enum MobileReactionPickerMove: Equatable {
    case left
    case right
    case up
    case down
    case first
    case last
}

internal func mobileReactionPickerNextIndex(
    current: Int?,
    count: Int,
    columns: Int,
    move: MobileReactionPickerMove
) -> Int? {
    guard count > 0, columns > 0 else { return nil }
    let index = min(max(current ?? 0, 0), count - 1)
    switch move {
    case .left:
        return max(0, index - 1)
    case .right:
        return min(count - 1, index + 1)
    case .up:
        return max(0, index - columns)
    case .down:
        return min(count - 1, index + columns)
    case .first:
        return 0
    case .last:
        return count - 1
    }
}

internal enum MobileEditorSuggestionCategory: String, Equatable {
    case assistants
    case automations
    case tools
    case pullRequests
    case emoji
}

internal enum MobileEditorSuggestionSourceStatus: Equatable {
    case idle
    case loading
    case ready
    case empty
    case failed
    case unavailable
    case cancelled
}

internal struct MobileEditorSuggestionItem: Identifiable, Equatable {
    let id: String
    let category: MobileEditorSuggestionCategory
    let label: String
    var subtitle: String?
    let insertion: String
    var triggerSchedule: String?
    var triggerEnabled: Bool?
    var keywords: [String] = []
    var iconURL: String?
    var mcpReference: MobileComposerMcpReference?
    var prReference: MobileComposerPrReference?
}

internal struct MobileEditorSuggestionContext: Equatable {
    let trigger: Character
    let query: String
    let replacementRange: NSRange
}

internal enum MobileEditorSuggestionMove: Equatable {
    case previous
    case next
    case first
    case last
}

private func mobileEditorSuggestionNonEmpty(_ value: Any?) -> String? {
    guard let string = value as? String else { return nil }
    let trimmed = string.trimmingCharacters(in: .whitespacesAndNewlines)
    return trimmed.isEmpty ? nil : trimmed
}

internal func projectMobileEditorMentionSuggestions(
    _ bots: [MobileBotSummary],
    allowEveryone: Bool = true
) -> [MobileEditorSuggestionItem] {
    var seen = Set<String>()
    let visible = bots.compactMap { bot -> MobileEditorSuggestionItem? in
        guard !bot.hidden, !bot.id.isEmpty, !bot.name.isEmpty, seen.insert(bot.id).inserted else {
            return nil
        }
        return .init(
            id: bot.id,
            category: .assistants,
            label: bot.name,
            subtitle: bot.isGroup && !bot.memberIds.isEmpty
                ? "\(bot.memberIds.count) agents"
                : bot.title,
            insertion: "@\(bot.name)",
            keywords: [bot.name, bot.title ?? "", bot.description]
        )
    }
    guard allowEveryone, visible.count >= 2 else { return visible }
    return [
        .init(
            id: "__everyone__",
            category: .assistants,
            label: "everyone",
            insertion: "@everyone",
            keywords: ["everyone", "all"]
        ),
    ] + visible
}

internal func projectMobileEditorWorkflowSuggestions(
    _ rows: [[String: Any]]
) -> [MobileEditorSuggestionItem] {
    var seen = Set<String>()
    return rows.compactMap { row in
        guard let id = mobileEditorSuggestionNonEmpty(row["id"]),
              let name = mobileEditorSuggestionNonEmpty(row["name"]),
              seen.insert(id).inserted
        else { return nil }

        var schedule: String?
        var enabled: Bool?
        if let rawTrigger = row["trigger"] {
            guard let trigger = rawTrigger as? [String: Any],
                  let projectedSchedule = mobileEditorSuggestionNonEmpty(trigger["schedule"]),
                  let projectedEnabled = (trigger["isEnabled"] ?? trigger["enabled"]) as? Bool
            else { return nil }
            schedule = projectedSchedule
            enabled = projectedEnabled
        }
        let subtitle = mobileEditorSuggestionNonEmpty(row["scheduleDescription"])
            ?? mobileEditorSuggestionNonEmpty(row["description"])
        return .init(
            id: id,
            category: .automations,
            label: name,
            subtitle: subtitle,
            insertion: "@\(name)",
            triggerSchedule: schedule,
            triggerEnabled: enabled,
            keywords: [
                name,
                subtitle ?? "",
                schedule ?? "",
            ]
        )
    }
}

private func mobileEditorMcpSafeAccountLabel(_ value: String) -> String {
    String(
        value
            .filter { !["\"", "'", "`", "[", "]", "{", "}", "(", ")", "<", ">"].contains(String($0)) }
            .split(whereSeparator: { $0.isWhitespace })
            .joined(separator: " ")
            .prefix(64)
    )
}

private func mobileEditorMcpStatusLabel(_ server: MarketplaceMcpServer) -> String {
    switch server.status {
    case "connected": return "connected"
    case "needsAuth": return "needs auth"
    case "error": return "error"
    case "initializing": return "connecting"
    case "disconnected": return "disconnected"
    case "disabledByTeamAdminPolicy": return "disabled"
    default: return server.accountKey == DEFAULT_MCP_ACCOUNT_KEY
        ? server.statusDetail ?? ""
        : server.accountKey
    }
}

private func mobileEditorMcpCatalogEntry(
    server: MarketplaceMcpServer,
    catalog: [MobileConnectorCatalogEntry]
) -> MobileConnectorCatalogEntry? {
    let candidates = [
        server.serverIdentifier,
        server.name,
        server.serverId,
        server.rowServerIdentifier ?? "",
    ].map(normalizeMobileConnectorName)
    return catalog.first { entry in
        [entry.id, entry.name, entry.displayName]
            .map(normalizeMobileConnectorName)
            .contains { candidates.contains($0) }
    }
}

internal func projectMobileEditorMcpSuggestions(
    servers: [MarketplaceMcpServer],
    catalog: [MobileConnectorCatalogEntry],
    accountKey scopedAccountKey: String
) -> [MobileEditorSuggestionItem] {
    let scope = scopedAccountKey.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !scope.isEmpty else { return [] }
    var seen = Set<String>()
    return servers.compactMap { server in
        let serverIdentifier = server.serverIdentifier.trimmingCharacters(in: .whitespacesAndNewlines)
        let serverId = server.serverId.trimmingCharacters(in: .whitespacesAndNewlines)
        let accountKey = server.accountKey.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !serverIdentifier.isEmpty, !serverId.isEmpty, accountKey == scope,
              seen.insert(serverIdentifier).inserted else { return nil }
        let safeAccount = mobileEditorMcpSafeAccountLabel(accountKey)
        let label = accountKey == DEFAULT_MCP_ACCOUNT_KEY ? server.name : "\(server.name) (\(safeAccount))"
        let subtitle = mobileEditorMcpStatusLabel(server)
        let metadata = mobileEditorMcpCatalogEntry(server: server, catalog: catalog)
        let reference = MobileComposerMcpReference(
            workflowReferenceID: "mcp:\(serverId)", serverId: serverId,
            serverIdentifier: serverIdentifier, accountKey: accountKey,
            label: label, status: server.status, iconURL: metadata?.iconURL
        )
        return .init(
            id: "mcp:\(serverIdentifier)", category: .tools, label: label,
            subtitle: subtitle.isEmpty ? nil : subtitle, insertion: "@\(label)",
            keywords: [label, serverIdentifier, accountKey, subtitle],
            iconURL: metadata?.iconURL, mcpReference: reference
        )
    }
}

internal func projectScopedMobileEditorMcpSuggestions(
    servers: [MarketplaceMcpServer],
    catalog: [MobileConnectorCatalogEntry],
    ownedMcpAccountKey: String,
    currentMcpAccountKey: String,
    ownedAppAccountKey: String,
    currentAppAccountKey: String,
    ownedAgentID: String,
    currentAgentID: String
) -> [MobileEditorSuggestionItem] {
    guard !ownedMcpAccountKey.isEmpty, !ownedAppAccountKey.isEmpty, !ownedAgentID.isEmpty,
          ownedMcpAccountKey == currentMcpAccountKey,
          ownedAppAccountKey == currentAppAccountKey,
          ownedAgentID == currentAgentID else { return [] }
    return projectMobileEditorMcpSuggestions(servers: servers, catalog: catalog, accountKey: ownedMcpAccountKey)
}

internal func pruneMobileComposerMcpReferences(
    draft: String,
    references: [MobileComposerMcpReference]
) -> [MobileComposerMcpReference] {
    var seen = Set<String>()
    return references.filter { reference in
        draft.contains("@\(reference.label)")
            && seen.insert(reference.workflowReferenceID).inserted
    }
}

private func mobileEditorPrCandidateFromURL(_ rawValue: String) -> MobileComposerPrReference? {
    let raw = rawValue.trimmingCharacters(in: CharacterSet(charactersIn: ".,;:!?"))
    guard let url = URL(string: raw),
          ["http", "https"].contains(url.scheme?.lowercased() ?? "")
    else { return nil }
    let host = url.host?.lowercased()
    let parts = url.path.split(separator: "/").map(String.init)
    let number: Int?
    if (host == "github.com" || host == "www.github.com"),
       parts.count >= 4,
       parts[2] == "pull" {
        number = Int(parts[3])
    } else if host == "review.cursor.com",
              parts.count >= 5,
              parts[0] == "github",
              parts[1] == "pr" {
        number = Int(parts[4])
    } else {
        number = nil
    }
    guard let number, number > 0 else { return nil }
    return .init(prNumber: number, title: nil, url: raw, source: "text", state: nil)
}

private func mobileEditorPrCandidatesFromText(_ text: String) -> [MobileComposerPrReference] {
    guard let regex = try? NSRegularExpression(pattern: #"https?://[^\s<>()[\]]+"#) else {
        return []
    }
    let ns = text as NSString
    return regex.matches(in: text, range: NSRange(location: 0, length: ns.length)).compactMap { match in
        mobileEditorPrCandidateFromURL(ns.substring(with: match.range))
    }
}

private func mobileEditorPrPositiveNumber(_ value: Any?) -> Int? {
    if let number = value as? NSNumber {
        let result = number.intValue
        return result > 0 ? result : nil
    }
    if let string = value as? String,
       let result = Int(string),
       result > 0 {
        return result
    }
    return nil
}

private func mobileEditorPrOptionalText(_ value: Any?) -> String? {
    guard let string = value as? String else { return nil }
    let trimmed = string.trimmingCharacters(in: .whitespacesAndNewlines)
    return trimmed.isEmpty ? nil : trimmed
}

private func mobileEditorPrCandidatesFromRichTextNode(
    _ node: Any,
    output: inout [MobileComposerPrReference]
) {
    guard let object = node as? [String: Any] else { return }
    if object["type"] as? String == "prReference",
       let attrs = object["attrs"] as? [String: Any],
       let number = mobileEditorPrPositiveNumber(attrs["prNumber"]) {
        output.append(.init(
            prNumber: number,
            title: mobileEditorPrOptionalText(attrs["title"]),
            url: mobileEditorPrOptionalText(attrs["url"]),
            source: "node",
            state: nil
        ))
    } else if object["type"] as? String == "text" {
        let text = object["text"] as? String ?? ""
        output.append(contentsOf: mobileEditorPrCandidatesFromText(text))
        if let marks = object["marks"] as? [[String: Any]] {
            for mark in marks where mark["type"] as? String == "link" {
                guard let attrs = mark["attrs"] as? [String: Any],
                      let href = attrs["href"] as? String,
                      let candidate = mobileEditorPrCandidateFromURL(href)
                else { continue }
                output.append(candidate)
            }
        }
    }
    if let children = object["content"] as? [Any] {
        for child in children {
            mobileEditorPrCandidatesFromRichTextNode(child, output: &output)
        }
    }
}

private func mobileEditorPrCandidatesFromEntry(_ entry: MobileChatMessage) -> [MobileComposerPrReference] {
    if let richText = entry.richText,
       !richText.isEmpty,
       let data = richText.data(using: .utf8),
       let root = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
       root["type"] as? String == "doc" {
        var output: [MobileComposerPrReference] = []
        mobileEditorPrCandidatesFromRichTextNode(root, output: &output)
        return output
    }
    return mobileEditorPrCandidatesFromText(entry.text)
}

internal func projectMobileEditorPrReferences(
    entries: [MobileChatMessage],
    cloudInfos: [String: MobileCloudAgentInfo],
    ownedAccountKey: String,
    currentAccountKey: String,
    ownedAgentID: String,
    currentAgentID: String
) -> [MobileComposerPrReference] {
    guard !ownedAccountKey.isEmpty,
          !ownedAgentID.isEmpty,
          ownedAccountKey == currentAccountKey,
          ownedAgentID == currentAgentID
    else { return [] }

    let priority = ["text": 0, "cloud": 1, "node": 2]
    var ordered: [Int] = []
    var byNumber: [Int: MobileComposerPrReference] = [:]
    func add(_ candidate: MobileComposerPrReference) {
        guard candidate.prNumber > 0 else { return }
        if let existing = byNumber[candidate.prNumber] {
            if (priority[candidate.source] ?? -1) > (priority[existing.source] ?? -1) {
                byNumber[candidate.prNumber] = candidate
            }
        } else {
            ordered.append(candidate.prNumber)
            byNumber[candidate.prNumber] = candidate
        }
    }

    for entry in entries.reversed() {
        for candidate in mobileEditorPrCandidatesFromEntry(entry) {
            add(candidate)
        }
        if let bcId = entry.cloudAgentBcId,
           let info = cloudInfos[bcId],
           let number = info.prNumber.map(Int.init),
           number > 0 {
            add(.init(
                prNumber: number,
                title: mobileEditorPrOptionalText(info.name) ?? mobileEditorPrOptionalText(entry.actionTitle),
                url: mobileEditorPrOptionalText(info.prURL),
                source: "cloud",
                state: mobileEditorPrOptionalText(info.prState)
            ))
        }
    }
    return ordered.compactMap { byNumber[$0] }
}

internal func projectMobileEditorPrSuggestionItems(
    _ references: [MobileComposerPrReference]
) -> [MobileEditorSuggestionItem] {
    references.map { reference in
        .init(
            id: "pr:\(reference.prNumber)",
            category: .pullRequests,
            label: "#\(reference.prNumber)",
            subtitle: reference.title,
            insertion: "#\(reference.prNumber)",
            keywords: [
                String(reference.prNumber),
                reference.title ?? "",
                reference.url ?? "",
                reference.state ?? "",
            ],
            prReference: reference
        )
    }
}

internal func pruneMobileComposerPrReferences(
    draft: String,
    references: [MobileComposerPrReference]
) -> [MobileComposerPrReference] {
    var seen = Set<Int>()
    return references.filter { reference in
        draft.contains("#\(reference.prNumber)")
            && seen.insert(reference.prNumber).inserted
    }
}

internal func mobileComposerRichText(
    draft: String,
    references: [MobileComposerMcpReference],
    prReferences: [MobileComposerPrReference] = []
) -> String? {
    let activeMcp = pruneMobileComposerMcpReferences(draft: draft, references: references)
    let activePr = pruneMobileComposerPrReferences(draft: draft, references: prReferences)

    struct NodeMatch {
        let range: Range<String.Index>
        let node: [String: Any]
    }
    var matches: [NodeMatch] = []
    for reference in activeMcp {
        guard let range = draft.range(of: "@\(reference.label)") else { continue }
        var attrs: [String: Any] = [
            "id": reference.workflowReferenceID,
            "label": reference.label,
        ]
        if let iconURL = reference.iconURL, !iconURL.isEmpty {
            attrs["iconUrl"] = iconURL
        }
        matches.append(.init(
            range: range,
            node: ["type": "workflowReference", "attrs": attrs]
        ))
    }
    for reference in activePr {
        guard let range = draft.range(of: "#\(reference.prNumber)") else { continue }
        var attrs: [String: Any] = ["prNumber": reference.prNumber]
        if let title = reference.title, !title.isEmpty { attrs["title"] = title }
        if let url = reference.url, !url.isEmpty { attrs["url"] = url }
        matches.append(.init(
            range: range,
            node: ["type": "prReference", "attrs": attrs]
        ))
    }
    guard !matches.isEmpty else { return nil }
    matches.sort { $0.range.lowerBound < $1.range.lowerBound }

    var content: [[String: Any]] = []
    var cursor = draft.startIndex
    for match in matches {
        guard match.range.lowerBound >= cursor else { continue }
        if cursor < match.range.lowerBound {
            content.append([
                "type": "text",
                "text": String(draft[cursor..<match.range.lowerBound]),
            ])
        }
        content.append(match.node)
        cursor = match.range.upperBound
    }
    if cursor < draft.endIndex {
        content.append(["type": "text", "text": String(draft[cursor...])])
    }
    let document: [String: Any] = [
        "type": "doc",
        "content": [[
            "type": "paragraph",
            "content": content,
        ]],
    ]
    guard JSONSerialization.isValidJSONObject(document),
          let data = try? JSONSerialization.data(withJSONObject: document, options: [.sortedKeys])
    else { return nil }
    return String(data: data, encoding: .utf8)
}

internal func mobileEditorSuggestionContext(
    _ draft: String
) -> MobileEditorSuggestionContext? {
    let ns = draft as NSString
    let fullLength = ns.length
    guard fullLength > 0 else { return nil }
    let contextLength = min(200, fullLength)
    let start = fullLength - contextLength
    let tail = ns.substring(with: NSRange(location: start, length: contextLength))

    let patterns: [(Character, String, Int)] = [
        ("@", #"(^|[\s(])@([^@#/:\n]{0,50})$"#, 2),
        ("/", #"(^|[\s(])/([^@#/:\n]{0,50})$"#, 2),
        ("#", #"(^|[\s(])#([^@#/:\n]{0,50})$"#, 2),
        (":", #"(^|[^\p{L}\p{N}_:/]):([A-Za-z0-9_+\-]{2,50})$"#, 2),
    ]
    for (trigger, pattern, queryGroup) in patterns {
        guard let regex = try? NSRegularExpression(pattern: pattern),
              let match = regex.firstMatch(
                in: tail,
                range: NSRange(location: 0, length: (tail as NSString).length)
              )
        else { continue }
        let queryRange = match.range(at: queryGroup)
        guard queryRange.location != NSNotFound else { continue }
        let query = (tail as NSString).substring(with: queryRange)
        let triggerLocation = queryRange.location - 1
        guard triggerLocation >= 0 else { continue }
        return .init(
            trigger: trigger,
            query: query,
            replacementRange: NSRange(
                location: start + triggerLocation,
                length: fullLength - (start + triggerLocation)
            )
        )
    }
    return nil
}

private func mobileEditorSuggestionNormalized(_ value: String) -> String {
    value.folding(options: [.diacriticInsensitive, .caseInsensitive], locale: .current)
        .lowercased()
        .split(whereSeparator: { !$0.isLetter && !$0.isNumber })
        .joined(separator: " ")
}

private func mobileEditorSuggestionScore(
    _ item: MobileEditorSuggestionItem,
    query: String
) -> Int? {
    let query = mobileEditorSuggestionNormalized(query)
    guard !query.isEmpty else { return 0 }
    let candidates = [item.label, item.subtitle ?? ""] + item.keywords
    var best: Int?
    for candidate in candidates {
        let normalized = mobileEditorSuggestionNormalized(candidate)
        if normalized == query {
            best = max(best ?? Int.min, 10_000)
        } else if normalized.hasPrefix(query) {
            best = max(best ?? Int.min, 5_000 - normalized.count)
        } else if normalized.contains(query) {
            best = max(best ?? Int.min, 2_500 - normalized.count)
        } else {
            var search = normalized.startIndex
            var matched = 0
            for character in query {
                guard let index = normalized[search...].firstIndex(of: character) else {
                    matched = -1
                    break
                }
                matched += 1
                search = normalized.index(after: index)
                if search == normalized.endIndex && matched < query.count {
                    matched = -1
                    break
                }
            }
            if matched == query.count {
                best = max(best ?? Int.min, 1_000 - normalized.count)
            }
        }
    }
    return best
}

internal func mobileEditorSuggestionRows(
    context: MobileEditorSuggestionContext?,
    assistants: [MobileEditorSuggestionItem],
    workflows: [MobileEditorSuggestionItem],
    mcpReferences: [MobileEditorSuggestionItem] = [],
    prReferences: [MobileEditorSuggestionItem] = [],
    recentKeys: [String] = []
) -> [MobileEditorSuggestionItem] {
    guard let context else { return [] }
    let source: [MobileEditorSuggestionItem]
    switch context.trigger {
    case "@":
        source = assistants + workflows.filter { $0.triggerSchedule != nil } + mcpReferences
    case "/":
        source = workflows.filter { $0.triggerSchedule == nil }
    case "#":
        source = prReferences
    case ":":
        return mobileReactionPickerResults(
            query: context.query,
            category: .all
        ).map {
            .init(
                id: $0.id,
                category: .emoji,
                label: $0.emoji,
                subtitle: $0.name,
                insertion: $0.emoji,
                keywords: [$0.name, $0.category.rawValue]
            )
        }
    default:
        return []
    }

    let recency = Dictionary(uniqueKeysWithValues: recentKeys.enumerated().map { ($0.element, $0.offset) })
    return source.compactMap { item -> (MobileEditorSuggestionItem, Int)? in
        guard let score = mobileEditorSuggestionScore(item, query: context.query) else { return nil }
        return (item, score)
    }
    .sorted { lhs, rhs in
        if lhs.1 != rhs.1 { return lhs.1 > rhs.1 }
        let leftKey = "\(lhs.0.category.rawValue):\(lhs.0.id)"
        let rightKey = "\(rhs.0.category.rawValue):\(rhs.0.id)"
        let leftRecent = recency[leftKey] ?? Int.max
        let rightRecent = recency[rightKey] ?? Int.max
        if leftRecent != rightRecent { return leftRecent < rightRecent }
        return lhs.0.label.localizedCaseInsensitiveCompare(rhs.0.label) == .orderedAscending
    }
    .prefix(96)
    .map(\.0)
}

internal func applyMobileEditorSuggestion(
    draft: String,
    context: MobileEditorSuggestionContext,
    item: MobileEditorSuggestionItem
) -> String {
    let ns = NSMutableString(string: draft)
    guard NSMaxRange(context.replacementRange) <= ns.length else { return draft }
    ns.replaceCharacters(
        in: context.replacementRange,
        with: item.insertion + " "
    )
    return ns as String
}

internal func mobileEditorSuggestionNextIndex(
    current: Int?,
    count: Int,
    move: MobileEditorSuggestionMove
) -> Int? {
    guard count > 0 else { return nil }
    let index = min(max(current ?? 0, 0), count - 1)
    switch move {
    case .previous:
        return (index - 1 + count) % count
    case .next:
        return (index + 1) % count
    case .first:
        return 0
    case .last:
        return count - 1
    }
}


internal func isMobileBotVisibleAssistantCompletion(
    _ event: [String: Any],
    operationId: String
) -> Bool {
    guard event["type"] as? String == "chat.message" else { return false }
    let eventOperationId = event["operationId"] as? String ?? operationId
    guard eventOperationId == operationId, event["role"] as? String != "user" else { return false }
    let text = event["text"] as? String ?? ""
    let attachment = event["attachment"] as? [String: Any]
    return !text.isEmpty || attachment != nil
}

private let mobileAcknowledgementResendPrefix = "sand-resend-v1:"

internal func mobileAcknowledgementLogicalNonce(_ nonce: String) -> String {
    guard nonce.hasPrefix(mobileAcknowledgementResendPrefix) else { return nonce }
    let encoded = String(nonce.dropFirst(mobileAcknowledgementResendPrefix.count))
    guard let separator = encoded.firstIndex(of: ":") else { return nonce }
    let lengthText = String(encoded[..<separator])
    guard let length = Int(lengthText), length > 0 else { return nonce }
    let start = encoded.index(after: separator)
    guard let end = encoded.index(start, offsetBy: length, limitedBy: encoded.endIndex),
          end < encoded.endIndex,
          encoded[end] == ":",
          encoded.index(after: end) < encoded.endIndex
    else { return nonce }
    return String(encoded[start..<end])
}

internal func mobileAcknowledgementRetryNonce(logicalNonce: String, retryToken: String) -> String {
    let logical = mobileAcknowledgementLogicalNonce(logicalNonce)
    return "\(mobileAcknowledgementResendPrefix)\(logical.count):\(logical):\(retryToken)"
}

private func mobileAcknowledgementRecordMatches(
    _ message: MobileChatMessage,
    nonce: String
) -> Bool {
    let logical = mobileAcknowledgementLogicalNonce(nonce)
    let candidates = [
        message.optimisticNonce,
        message.canonicalMessageId,
        message.id,
    ].compactMap { $0 } + message.optimisticPriorNonces
    return candidates.contains {
        $0 == nonce || mobileAcknowledgementLogicalNonce($0) == logical
    }
}

@discardableResult
internal func applyMobileOptimisticUserEcho(
    _ event: [String: Any],
    accountKey: String,
    agentId: String,
    messages: inout [MobileChatMessage]
) -> Bool {
    guard event["type"] as? String == "chat.message",
          event["role"] as? String == "user",
          let rawMessageId = event["messageId"] as? String
    else { return false }
    let messageId = rawMessageId.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !messageId.isEmpty,
          let index = messages.firstIndex(where: {
              $0.role == .user
                  && $0.optimisticDeliveryPhase != nil
                  && $0.optimisticAccountKey == accountKey
                  && $0.optimisticAgentId == agentId
                  && mobileAcknowledgementRecordMatches($0, nonce: messageId)
          })
    else { return false }

    messages[index].canonicalMessageId = messageId
    messages[index].optimisticDeliveryPhase = nil
    messages[index].optimisticDeliveryError = nil
    if let replyTo = event["replyToMessageId"] as? String,
       !replyTo.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
        messages[index].replyToMessageId = replyTo
    }
    if let branched = event["branched"] as? Bool {
        messages[index].branched = branched
    }
    return true
}

internal func projectMobileTranscriptCardWithFallback(
    event: [String: Any],
    operationId: String?
) -> MobileChatMessage? {
    if let projected = projectMobileTranscriptCard(event: event, operationId: operationId) {
        return projected
    }
    guard
        event["card"] is [String: Any],
        let rawEntryId = event["entryId"] as? String
    else {
        return nil
    }
    let entryId = rawEntryId.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !entryId.isEmpty else { return nil }
    return MobileChatMessage(
        id: "transcript-card-fallback:\(entryId)",
        role: .assistant,
        text: "This message can’t be shown in this version of Fabushi",
        kind: .notice,
        operationId: operationId,
        canonicalMessageId: entryId
    )
}

internal func projectMobileConversationWindowMessage(_ row: [String: Any]) -> MobileChatMessage? {
    guard
        let id = (row["id"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines),
        !id.isEmpty,
        let roleRaw = row["role"] as? String,
        let role = MobileChatRole(rawValue: roleRaw),
        let text = row["text"] as? String,
        let createdAtMs = GrokMobileBotService.int64Value(row["createdAtMs"])
    else {
        return nil
    }
    var replyToMessageId: String?
    if let rawReplyTo = row["replyToMessageId"] {
        guard let value = rawReplyTo as? String else { return nil }
        let normalized = value.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !normalized.isEmpty else { return nil }
        replyToMessageId = normalized
    }
    if row["branched"] != nil, row["branched"] is Bool == false { return nil }
    var message = MobileChatMessage(
        id: "history:\(id)",
        role: role,
        text: text,
        canonicalMessageId: id,
        replyToMessageId: replyToMessageId,
        reactions: projectMobileTranscriptReactions(row["reactions"]),
        branched: row["branched"] as? Bool ?? false
    )
    message.fromUserPresent = row["fromUser"] != nil && !(row["fromUser"] is NSNull)
    message.createdAt = Date(timeIntervalSince1970: TimeInterval(createdAtMs) / 1_000)
    return message
}

internal func projectMobileConversationWindowToolCall(
    _ row: [String: Any]
) -> MobileChatMessage? {
    guard row["kind"] as? String == "tool-call",
          let id = (row["id"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines),
          !id.isEmpty,
          let name = (row["name"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines),
          !name.isEmpty,
          let status = row["status"] as? String,
          ["pending", "running", "done", "failed", "error", "aborted"].contains(status),
          let timestampMs = GrokMobileBotService.int64Value(row["timestampMs"])
    else { return nil }
    if row["summary"] != nil && !(row["summary"] is String) && !(row["summary"] is NSNull) {
        return nil
    }

    let summary = row["summary"] as? String
    var message = MobileChatMessage(
        id: "tool-call:\(id)",
        role: .assistant,
        text: "",
        kind: .toolCall,
        actionTitle: name,
        actionDetail: summary,
        actionStatus: status
    )
    message.toolCallId = id
    message.toolName = name
    message.toolStatus = status
    message.toolSummary = summary
    message.createdAt = Date(timeIntervalSince1970: TimeInterval(timestampMs) / 1_000)
    return message
}

internal func projectMobileConversationWindowEntries(
    _ row: [String: Any]
) -> [MobileChatMessage]? {
    if row["kind"] as? String == "tool-call" {
        guard let toolCall = projectMobileConversationWindowToolCall(row) else { return nil }
        return [toolCall]
    }
    guard var message = projectMobileConversationWindowMessage(row) else { return nil }
    var projected: [MobileChatMessage] = []
    let cards: [[String: Any]]
    if let rawCards = row["cards"] {
        guard let typedCards = rawCards as? [[String: Any]] else { return nil }
        cards = typedCards
    } else {
        cards = []
    }
    if !message.text.isEmpty || cards.isEmpty {
        projected.append(message)
    }
    let sourceMessageId = message.canonicalMessageId ?? message.id
    for (index, card) in cards.enumerated() {
        let entryId = "\(sourceMessageId)-card-\(index)"
        guard var cardMessage = projectMobileTranscriptCardWithFallback(
            event: [
                "type": "transcript.card",
                "entryId": entryId,
                "card": card,
            ],
            operationId: nil
        ) else { return nil }
        cardMessage.createdAt = message.createdAt
        projected.append(cardMessage)
    }
    return projected
}

internal func mobileTranscriptCanonicalId(_ message: MobileChatMessage) -> String {
    message.canonicalMessageId ?? message.id
}

internal func mobileMainTranscriptEntries(_ entries: [MobileChatMessage]) -> [MobileChatMessage] {
    let topology = entries.map {
        TranscriptEntry(
            kind: "message",
            id: mobileTranscriptCanonicalId($0),
            replyTo: $0.replyToMessageId,
            branched: $0.branched
        )
    }
    let mainIds = Set(getMainTranscriptEntries(topology).compactMap(\.id))
    return entries.filter { mainIds.contains(mobileTranscriptCanonicalId($0)) }
}

internal struct MobileTranscriptAdjacency: Equatable {
    let isContinuedFromPrev: Bool
    let isContinuedToNext: Bool
    let isGroupStart: Bool
    let isRunStart: Bool
    let isFollowedByThreadChip: Bool
    let isGroupEnd: Bool

    static let empty = MobileTranscriptAdjacency(
        isContinuedFromPrev: false,
        isContinuedToNext: false,
        isGroupStart: false,
        isRunStart: false,
        isFollowedByThreadChip: false,
        isGroupEnd: false
    )
}

private struct MobileTranscriptAdjacencySemantics {
    let role: MobileChatRole?
    let groupKey: String?
    let isBubble: Bool
    let hasReaction: Bool
}

private func mobileTranscriptStandaloneEmoji(_ text: String) -> Bool {
    let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !trimmed.isEmpty, trimmed.count == 1 else { return false }
    return trimmed.unicodeScalars.contains { scalar in
        scalar.properties.isEmojiPresentation || scalar.properties.isEmoji
    }
}

private func mobileTranscriptImageOnlyMarkdown(_ text: String) -> Bool {
    let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !trimmed.isEmpty,
          let expression = try? NSRegularExpression(pattern: #"!\[[^\]]*\]\([^)]*\)"#)
    else { return false }
    let range = NSRange(trimmed.startIndex..<trimmed.endIndex, in: trimmed)
    let remainder = expression.stringByReplacingMatches(
        in: trimmed,
        options: [],
        range: range,
        withTemplate: ""
    )
    return remainder.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
}

private func mobileTranscriptAdjacencySemantics(
    _ entry: MobileChatMessage
) -> MobileTranscriptAdjacencySemantics {
    guard entry.kind == .message else {
        return .init(role: nil, groupKey: nil, isBubble: false, hasReaction: false)
    }

    let hasAttachment = !entry.optimisticAttachments.isEmpty
        || entry.attachmentProjection != nil
        || !(entry.attachmentURL?
            .trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ?? true)
    let isSendMessageCard = entry.sendMessageTextProjection != nil
    let text = entry.text.trimmingCharacters(in: .whitespacesAndNewlines)
    let isBubble = !hasAttachment
        && !isSendMessageCard
        && !text.isEmpty
        && !mobileTranscriptImageOnlyMarkdown(text)
        && !(entry.role == .user && mobileTranscriptStandaloneEmoji(text))

    return .init(
        role: entry.role,
        groupKey: entry.role.rawValue,
        isBubble: isBubble,
        hasReaction: !entry.reactions.isEmpty
    )
}

internal func projectMobileTranscriptAdjacency(
    _ entries: [MobileChatMessage],
    threadChipEntryIDs: Set<String> = []
) -> [MobileTranscriptAdjacency] {
    entries.enumerated().map { index, entry in
        let current = mobileTranscriptAdjacencySemantics(entry)
        guard current.role != nil, current.groupKey != nil else {
            return .empty
        }

        let previousEntry = index > 0 ? entries[index - 1] : nil
        let nextEntry = index + 1 < entries.count ? entries[index + 1] : nil
        let previous = previousEntry.map(mobileTranscriptAdjacencySemantics)
        let next = nextEntry.map(mobileTranscriptAdjacencySemantics)
        let currentId = mobileTranscriptCanonicalId(entry)
        let hasThreadChip = threadChipEntryIDs.contains(currentId)
        let previousHasThreadChip = previousEntry
            .map { threadChipEntryIDs.contains(mobileTranscriptCanonicalId($0)) }
            ?? false
        let isIndicatorSeaming = current.role == .assistant
            && !hasThreadChip
            && !current.hasReaction

        return .init(
            isContinuedFromPrev: current.isBubble
                && previous?.groupKey == current.groupKey
                && previous?.isBubble == true
                && !previousHasThreadChip,
            isContinuedToNext: current.isBubble
                && (
                    (next?.groupKey == current.groupKey && next?.isBubble == true)
                    || isIndicatorSeaming
                ),
            isGroupStart: previousEntry != nil && previous?.groupKey != current.groupKey,
            isRunStart: previousEntry == nil || previous?.groupKey != current.groupKey,
            isFollowedByThreadChip: current.isBubble && hasThreadChip && !current.hasReaction,
            isGroupEnd: current.role != .assistant
                && (nextEntry == nil || next?.groupKey != current.groupKey)
        )
    }
}

internal func mobileThreadEntries(
    _ entries: [MobileChatMessage],
    rootId: String
) -> [MobileChatMessage] {
    let topology = entries.map {
        TranscriptEntry(
            kind: "message",
            id: mobileTranscriptCanonicalId($0),
            replyTo: $0.replyToMessageId,
            branched: $0.branched
        )
    }
    let threadIds = Set(getThreadTranscriptEntries(topology, rootId: rootId).compactMap(\.id))
    return entries.filter { threadIds.contains(mobileTranscriptCanonicalId($0)) }
}

internal func mobileThreadReplyCounts(_ entries: [MobileChatMessage]) -> [String: Int] {
    let branched = entries.compactMap { message -> BranchedTranscriptEntry? in
        guard message.branched, let replyTo = message.replyToMessageId else { return nil }
        return BranchedTranscriptEntry(id: mobileTranscriptCanonicalId(message), replyTo: replyTo)
    }
    return branchReplyCounts(branched)
}

internal func mobileTranscriptCopyText(_ entry: MobileChatMessage) -> String? {
    if let projection = entry.sendMessageTextProjection {
        guard case .text = projection.presentation else { return nil }
        return projection.content.isEmpty ? nil : projection.content
    }
    guard entry.kind == .message, !entry.text.isEmpty else { return nil }
    return entry.text
}

internal struct MobileMessageCardSeamProjection: Equatable {
    let isSourceTrusted: Bool
    let isFromUser: Bool
    let isStandaloneEmoji: Bool
    let url: String?
    let copyText: String?
}

private func mobileMessageCardStrictHTTPSURL(_ text: String) -> String? {
    let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !trimmed.isEmpty,
          !trimmed.contains(where: { $0.isWhitespace }),
          let components = URLComponents(string: trimmed),
          components.scheme?.lowercased() == "https",
          let host = components.host,
          !host.isEmpty
    else { return nil }
    return components.url?.absoluteString
}

internal func projectMobileMessageCardSeam(
    _ entry: MobileChatMessage
) -> MobileMessageCardSeamProjection {
    guard entry.kind == .message else {
        return .init(
            isSourceTrusted: false,
            isFromUser: false,
            isStandaloneEmoji: false,
            url: nil,
            copyText: nil
        )
    }

    let text = entry.sendMessageTextProjection?.content ?? entry.text
    let hasProjectedImages = !(entry.sendMessageTextProjection?.images.isEmpty ?? true)
    let standaloneEmoji = entry.role == .user
        && entry.optimisticAttachments.isEmpty
        && entry.attachmentProjection == nil
        && entry.attachmentURL == nil
        && !hasProjectedImages
        && mobileTranscriptStandaloneEmoji(text)
    let url: String?
    if entry.role == .user,
       !entry.fromUserPresent,
       entry.optimisticAttachments.isEmpty,
       entry.attachmentProjection == nil,
       entry.attachmentURL == nil
    {
        if let projection = entry.sendMessageTextProjection,
           case let .urlCard(rawURL) = projection.presentation {
            url = mobileMessageCardStrictHTTPSURL(rawURL)
        } else {
            url = mobileMessageCardStrictHTTPSURL(text)
        }
    } else {
        url = nil
    }

    return .init(
        isSourceTrusted: entry.role == .assistant,
        isFromUser: entry.role == .user && entry.fromUserPresent,
        isStandaloneEmoji: standaloneEmoji,
        url: url,
        copyText: mobileTranscriptCopyText(entry)
    )
}

internal func mobileToolResultForEntry(
    _ entry: MobileChatMessage,
    cardsByAgent: [String: [MobileToolResultCard]],
    agentId: String
) -> MobileToolResultCard? {
    guard entry.kind == .toolCall,
          let toolCallId = entry.toolCallId?.trimmingCharacters(in: .whitespacesAndNewlines),
          !toolCallId.isEmpty
    else { return nil }
    return cardsByAgent[agentId]?.first { $0.toolCallId == toolCallId }
}

internal enum MobileReplyReferencePreview: Equatable {
    case userText(String)
    case assistantText(String)
    case image(url: String)
    case file(url: String, name: String?)
    case link(url: String)
    case missing
}

internal struct MobileReplyReferenceResolution: Equatable {
    let targetID: String
    let preview: MobileReplyReferencePreview
    let isResolved: Bool
}

internal func mobileStableReplyTargetID(_ entry: MobileChatMessage) -> String? {
    guard entry.kind == .message,
          !entry.streaming,
          entry.optimisticDeliveryPhase == nil,
          let raw = entry.canonicalMessageId
    else { return nil }
    let targetID = raw.trimmingCharacters(in: .whitespacesAndNewlines)
    return targetID.isEmpty ? nil : targetID
}

private func mobileReplyReferenceNormalizedText(_ value: String) -> String {
    value.split(whereSeparator: { $0.isWhitespace }).joined(separator: " ")
}

private func mobileReplyReferenceTruncatedText(_ value: String, limit: Int) -> String {
    let normalized = mobileReplyReferenceNormalizedText(value)
    guard normalized.count > limit, limit > 1 else { return normalized }
    let end = normalized.index(normalized.startIndex, offsetBy: limit - 1)
    return String(normalized[..<end]).trimmingCharacters(in: .whitespacesAndNewlines) + "…"
}

private func mobileReplyReferenceBasename(_ value: String) -> String {
    if let url = URL(string: value), !url.lastPathComponent.isEmpty {
        return url.lastPathComponent
    }
    let normalized = value.replacingOccurrences(of: "\\", with: "/")
    return normalized.split(separator: "/", omittingEmptySubsequences: true).last.map(String.init)
        ?? "Attachment"
}

private func mobileReplyReferenceLinkHost(_ value: String) -> String {
    guard let url = URL(string: value), let host = url.host, !host.isEmpty else {
        return value
    }
    return host
}

internal func mobileReplyReferencePreview(
    for entry: MobileChatMessage
) -> MobileReplyReferencePreview {
    guard entry.kind == .message, !entry.streaming else { return .missing }

    if let projection = entry.sendMessageTextProjection {
        switch projection.presentation {
        case .urlCard(let rawURL):
            return .link(url: rawURL)
        case .text:
            let text = mobileReplyReferenceNormalizedText(projection.content)
            if !text.isEmpty {
                return entry.role == .user ? .userText(text) : .assistantText(text)
            }
        }
    }

    let text = mobileReplyReferenceNormalizedText(entry.text)
    if !text.isEmpty {
        return entry.role == .user ? .userText(text) : .assistantText(text)
    }

    guard let rawURL = entry.attachmentURL?
        .trimmingCharacters(in: .whitespacesAndNewlines),
        !rawURL.isEmpty
    else { return .missing }

    switch classifyMobileAttachmentURL(rawURL) {
    case .legacyLink:
        return .link(url: rawURL)
    case .media:
        if mobileAttachmentMediaPresentation(rawURL) == .image {
            return .image(url: rawURL)
        }
        return .file(
            url: rawURL,
            name: entry.attachmentFileName ?? entry.attachmentAlt
        )
    case .file:
        return .file(
            url: rawURL,
            name: entry.attachmentFileName ?? entry.attachmentAlt
        )
    case .box:
        return .file(url: rawURL, name: "Computer attachment")
    }
}

internal func mobileResolveReplyReference(
    targetID: String,
    entries: [MobileChatMessage]
) -> MobileReplyReferenceResolution {
    let normalizedTargetID = targetID.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !normalizedTargetID.isEmpty,
          let target = entries.first(where: {
              mobileStableReplyTargetID($0) == normalizedTargetID
          })
    else {
        return .init(
            targetID: normalizedTargetID,
            preview: .missing,
            isResolved: false
        )
    }
    return .init(
        targetID: normalizedTargetID,
        preview: mobileReplyReferencePreview(for: target),
        isResolved: true
    )
}

internal func mobileReplyReferenceComposerLabel(
    _ preview: MobileReplyReferencePreview
) -> String {
    switch preview {
    case .userText(let text), .assistantText(let text):
        let label = mobileReplyReferenceTruncatedText(text, limit: 40)
        return label.isEmpty ? "Thread" : label
    case .image:
        return "Photo"
    case .file(let url, let name):
        let value = name?.trimmingCharacters(in: .whitespacesAndNewlines)
        if let value, !value.isEmpty { return value }
        return mobileReplyReferenceBasename(url)
    case .link(let url):
        return mobileReplyReferenceLinkHost(url)
    case .missing:
        return "Thread"
    }
}

internal func mobileReplyReferenceQuoteLabel(
    _ preview: MobileReplyReferencePreview
) -> String {
    switch preview {
    case .userText(let text), .assistantText(let text):
        let label = mobileReplyReferenceTruncatedText(text, limit: 96)
        return label.isEmpty ? "(empty)" : label
    case .image:
        return "Photo"
    case .file(let url, let name):
        let value = name?.trimmingCharacters(in: .whitespacesAndNewlines)
        return mobileReplyReferenceTruncatedText(
            value.flatMap { $0.isEmpty ? nil : $0 } ?? mobileReplyReferenceBasename(url),
            limit: 96
        )
    case .link(let url):
        return mobileReplyReferenceTruncatedText(
            mobileReplyReferenceLinkHost(url),
            limit: 96
        )
    case .missing:
        return "(deleted)"
    }
}

internal func mobileBotChatSearchEntries(
    _ entries: [MobileChatMessage],
    botName: String
) -> [ChatSearchEntry] {
    let normalizedBotName = botName.trimmingCharacters(in: .whitespacesAndNewlines)
    return mobileMainTranscriptEntries(entries).compactMap { entry in
        guard entry.kind == .message else { return nil }
        var fields: [String] = []
        if let text = mobileTranscriptCopyText(entry)?
            .trimmingCharacters(in: .whitespacesAndNewlines),
           !text.isEmpty
        {
            fields.append(text)
        }
        fields.append(entry.role == .user ? "You" : (normalizedBotName.isEmpty ? "Agent" : normalizedBotName))
        if let fileName = entry.attachmentFileName?
            .trimmingCharacters(in: .whitespacesAndNewlines),
           !fileName.isEmpty
        {
            fields.append(fileName)
        }
        return ChatSearchEntry(id: entry.id, text: fields.joined(separator: "\n"))
    }
}

internal func mobileBotForwardMessageId(
    _ entry: MobileChatMessage,
    sourceConversationId: String?
) -> String? {
    guard entry.kind == .message,
          !entry.streaming,
          entry.optimisticDeliveryPhase == nil,
          let rawMessageId = entry.canonicalMessageId,
          let sourceConversationId,
          !sourceConversationId.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    else { return nil }

    let hasText = !entry.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        || !(entry.sendMessageTextProjection?.content
            .trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ?? true)
    let hasAttachment = entry.attachmentProjection != nil
        || !(entry.attachmentURL?
            .trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ?? true)
        || !(entry.attachmentBatchId?
            .trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ?? true)
    guard hasText || hasAttachment else { return nil }

    let messageId = rawMessageId.trimmingCharacters(in: .whitespacesAndNewlines)
    return messageId.isEmpty ? nil : messageId
}

internal func mergeMobileConversationHistory(
    current: [MobileChatMessage],
    fetched: [MobileChatMessage]
) -> [MobileChatMessage] {
    func historyIdentity(_ message: MobileChatMessage) -> String {
        message.kind == .message ? mobileTranscriptCanonicalId(message) : message.id
    }
    var merged = fetched
    var seen = Set(fetched.map(historyIdentity))
    for message in current {
        let key = historyIdentity(message)
        guard !seen.contains(key) else { continue }
        merged.append(message)
        seen.insert(key)
    }
    return merged
}

internal func reconcileMobileConversationBaseline(
    baseline: [MobileChatMessage],
    current: [MobileChatMessage],
    identitiesAtRequestStart: Set<String>
) -> [MobileChatMessage] {
    func identity(_ message: MobileChatMessage) -> String {
        message.canonicalMessageId ?? message.id
    }

    var merged = baseline
    var seen = Set(baseline.map(identity))
    for message in current {
        let key = identity(message)
        guard !seen.contains(key) else { continue }

        let arrivedAfterRequestStarted = !identitiesAtRequestStart.contains(key)
        let unresolvedOptimistic = message.id.hasPrefix("ios-mobile-bot-chat-")
            || (message.kind == .message && message.canonicalMessageId == message.id)
        let activeEphemeral = message.kind != .message || message.streaming
        guard arrivedAfterRequestStarted || unresolvedOptimistic || activeEphemeral else { continue }

        merged.append(message)
        seen.insert(key)
    }
    return merged.sorted {
        if $0.createdAt != $1.createdAt { return $0.createdAt < $1.createdAt }
        return $0.id < $1.id
    }
}

struct MobileSecretRequestFence: Equatable {
    let accountKey: String
    let agentId: String
    let generation: Int

    func accepts(accountKey: String, agentId: String, generation: Int) -> Bool {
        self.accountKey == accountKey
            && self.agentId == agentId
            && self.generation == generation
    }
}

private struct MobileLinkMetadataCard: View {
    let url: String
    let model: MarketplaceModel
    let healingRevision: Int
    var isGroupStart = false

    @State private var metadata: MobileLinkMetadata?
    @State private var loading = false

    var body: some View {
        Link(destination: URL(string: url)!) {
            VStack(alignment: .leading, spacing: 5) {
                if let imageURL = metadata?.imageURL,
                   let image = URL(string: imageURL)
                {
                    AsyncImage(url: image) { phase in
                        switch phase {
                        case .success(let imageView):
                            imageView
                                .resizable()
                                .scaledToFill()
                                .frame(maxWidth: .infinity)
                                .frame(height: 96)
                                .clipped()
                                .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
                        case .empty:
                            ProgressView().controlSize(.small)
                        case .failure:
                            EmptyView()
                        @unknown default:
                            EmptyView()
                        }
                    }
                }
                Text(metadata?.displayTitle ?? url)
                    .font(.system(size: 14, weight: .semibold))
                    .lineLimit(2)
                    .multilineTextAlignment(.leading)
                if let description = metadata?.description {
                    Text(description)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(2)
                        .multilineTextAlignment(.leading)
                }
                if let hostname = metadata?.hostname {
                    Text(hostname)
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                } else if loading {
                    ProgressView().controlSize(.mini)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(10)
            .background(
                Color.black.opacity(0.045),
                in: RoundedRectangle(cornerRadius: 12, style: .continuous)
            )
        }
        .buttonStyle(.plain)
        .accessibilityLabel(metadata?.displayTitle ?? url)
        .accessibilityIdentifier("mobile-bot-link-card")
        .task(id: "\(url)|\(healingRevision)") {
            metadata = nil
            loading = true
            defer { loading = false }
            metadata = try? await model.linkMetadata(for: url)
        }
    }
}

private struct MobileTranscriptMediaAttachmentView: View {
    let rawURL: String
    let alt: String?

    @State private var player: AVPlayer?
    @State private var isPlaying = false

    private var destination: URL? {
        let trimmed = rawURL.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return nil }
        if let url = URL(string: trimmed), url.scheme != nil { return url }
        if trimmed.hasPrefix("/") { return URL(fileURLWithPath: trimmed) }
        return nil
    }

    var body: some View {
        Group {
            if let destination {
                switch mobileAttachmentMediaPresentation(rawURL) {
                case .image:
                    if destination.isFileURL,
                       let image = UIImage(contentsOfFile: destination.path) {
                        Image(uiImage: image)
                            .resizable()
                            .scaledToFit()
                            .frame(maxHeight: 260)
                            .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
                            .accessibilityLabel(alt ?? "Image attachment")
                    } else {
                        AsyncImage(url: destination) { phase in
                            switch phase {
                            case let .success(image):
                                image
                                    .resizable()
                                    .scaledToFit()
                                    .frame(maxHeight: 260)
                                    .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
                            case .empty:
                                ProgressView().controlSize(.small)
                            default:
                                Link(alt ?? "Open image", destination: destination)
                            }
                        }
                        .accessibilityLabel(alt ?? "Image attachment")
                    }

                case .video:
                    if let player {
                        VideoPlayer(player: player)
                            .frame(minHeight: 180, maxHeight: 280)
                            .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
                            .accessibilityLabel(alt ?? "Video attachment")
                    } else {
                        ProgressView().controlSize(.small)
                    }

                case .audio:
                    HStack(spacing: 10) {
                        Button {
                            guard let player else { return }
                            if isPlaying {
                                player.pause()
                            } else {
                                player.play()
                            }
                            isPlaying.toggle()
                        } label: {
                            Image(systemName: isPlaying ? "pause.fill" : "play.fill")
                        }
                        .buttonStyle(.bordered)
                        Text(
                            alt
                                ?? (destination.lastPathComponent.isEmpty
                                    ? "Audio attachment"
                                    : destination.lastPathComponent)
                        )
                        .font(.caption)
                        .lineLimit(1)
                        Spacer()
                        Link(destination: destination) {
                            Image(systemName: "arrow.up.right.square")
                        }
                    }
                    .accessibilityElement(children: .contain)

                case .file:
                    Link(alt ?? "Open attachment", destination: destination)
                }
            } else {
                Label(alt ?? "Attachment unavailable", systemImage: "paperclip")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        .task(id: destination?.absoluteString) {
            guard let destination,
                  [.video, .audio].contains(mobileAttachmentMediaPresentation(rawURL))
            else {
                player?.pause()
                player = nil
                isPlaying = false
                return
            }
            player?.pause()
            player = AVPlayer(url: destination)
            isPlaying = false
        }
        .onDisappear {
            player?.pause()
            isPlaying = false
        }
    }
}

internal func mobileConversationHeaderStatus(_ bot: MobileBotSummary) -> String? {
    bot.isRunning ? "Working" : nil
}

internal let mobileComposerAttachmentLimit = 6

internal enum MobileComposerStageFailureReason: Equatable {
    case empty
    case tooLarge
    case failed
}

internal struct MobileComposerStageFailure: Equatable {
    let name: String
    let reason: MobileComposerStageFailureReason
}

internal func mobileComposerStageFileName(
    proposedName: String?,
    fallbackLastPathComponent: String,
    mimeType: String?
) -> String {
    if let proposedName = proposedName?.trimmingCharacters(in: .whitespacesAndNewlines),
       !proposedName.isEmpty {
        return proposedName
    }
    let pathName = fallbackLastPathComponent.trimmingCharacters(in: .whitespacesAndNewlines)
    if !pathName.isEmpty { return pathName }
    return mimeType?.lowercased().hasPrefix("image/") == true ? "image.png" : "file"
}

internal func mobileComposerStageFailureNotice(
    _ failures: [MobileComposerStageFailure]
) -> String? {
    guard let first = failures.first else { return nil }
    if failures.count == 1 {
        switch first.reason {
        case .tooLarge:
            return AttachmentLimits.formatTooLargeNotice(filename: first.name)
        case .empty:
            return "\"\(first.name)\" is empty, so it wasn't attached."
        case .failed:
            return "Couldn't attach \"\(first.name)\"."
        }
    }
    if failures.allSatisfy({ $0.reason == .tooLarge }) {
        return "\(failures.count) files are too large to attach (max 25 MB, or 200 MB for video)."
    }
    return "\(failures.count) files couldn't be attached."
}

internal func mobileComposerHasPayload(
    text: String,
    attachments: [MobileComposerAttachment]
) -> Bool {
    !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || !attachments.isEmpty
}

internal func mergeMobileComposerVoiceTranscript(
    existing: String,
    transcript: String
) -> String {
    let inserted = transcript.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !inserted.isEmpty else { return existing }
    guard !existing.isEmpty else { return inserted }
    let separator = existing.last?.isWhitespace == true ? "" : " "
    return existing + separator + inserted
}

internal func mergeMobileConversationOlderPage(
    older: [MobileChatMessage],
    current: [MobileChatMessage]
) -> [MobileChatMessage] {
    var seen = Set(current.map(\.id))
    var prefix: [MobileChatMessage] = []
    prefix.reserveCapacity(older.count)
    for entry in older {
        guard seen.insert(entry.id).inserted else { continue }
        prefix.append(entry)
    }
    return prefix + current
}

internal func mobileComposerAttachmentCommandPayload(
    _ attachment: MobileComposerAttachment
) -> [String: Any] {
    var payload: [String: Any] = [
        "id": attachment.id,
        "name": attachment.name,
        "path": attachment.path,
        "sizeBytes": attachment.sizeBytes,
    ]
    if let mimeType = attachment.mimeType, !mimeType.isEmpty {
        payload["mimeType"] = mimeType
    }
    return payload
}

internal struct MobileBotChat: View {
    @Environment(\.scenePhase) private var scenePhase
    let bot: MobileBotSummary
    var availableBots: [MobileBotSummary] = []
    let bridge: IOSPreloadBridge
    let model: MarketplaceModel
    let messaging: MessagingModel
    let appAgentSurface: FabushiAppAgentSurface
    let reconnectGeneration: Int
    let focusPromptGeneration: Int
    let onClose: () -> Void
    let onOpenSettings: () -> Void
    let onOpenAutomation: (String) -> Void

    @Binding var draft: String
    @Binding var composerAttachments: [MobileComposerAttachment]
    @Binding var composerRecovery: MobileComposerRecovery?
    @Binding var entries: [MobileChatMessage]
    @State private var busy = false
    @State private var activeOperationId: String?
    @State private var errorText: String?
    @State private var openedMiniApp = false
    @State private var asyncTasksPresented = false
    @State private var conversationOutlinePresented = false
    @State private var replyTargetId: String?
    @State private var replyIsFork = false
    @State private var linkMetadataFocusRevision = 0
    @State private var threadRootId: String?
    @State private var threadLoadGeneration = 0
    @State private var threadLoadingRootId: String?
    @State private var threadLoadError: String?
    @State private var voiceRecorder = VoiceRecorder()
    @State private var voiceTranscriber = OfflineSpeechTranscriber()
    @State private var transcribingVoice = false
    @State private var voiceInputGeneration = 0
    @State private var attachmentImporterPresented = false
    @State private var stagingAttachments = false
    @State private var attachmentStageGeneration = 0
    @State private var reactionGeneration = 0
    @State private var cloudAgentInfoByBcId: [String: MobileCloudAgentInfo] = [:]
    @State private var cloudAgentErrorsByBcId: [String: String] = [:]
    @State private var reactionPickerPresented = false
    @State private var reactionPickerTargetId: String?
    @State private var reactionPickerDraft = ""
    @State private var reactionPickerSearch = ""
    @State private var reactionPickerCategory: MobileReactionPickerCategory = .all
    @State private var reactionPickerRecentIds: [String] = []
    @State private var editorSuggestionWorkflows: [MobileEditorSuggestionItem] = []
    @State private var editorSuggestionMcpReferences: [MobileEditorSuggestionItem] = []
    @State private var composerMcpReferences: [MobileComposerMcpReference] = []
    @State private var composerPrReferences: [MobileComposerPrReference] = []
    @State private var editorSuggestionStatus: MobileEditorSuggestionSourceStatus = .idle
    @State private var editorSuggestionGeneration = 0
    @State private var editorSuggestionActiveIndex: Int?
    @State private var editorSuggestionRecents: [String] = []
    @State private var approvalGeneration = 0
    @State private var localToolPermissionGeneration = 0
    @State private var localToolPermissionPendingEntryIds: Set<String> = []
    @State private var localToolPermissionCeilings: [String: SandLocalToolPermission] = [:]
    @State private var localToolPermissionCeilingLoadedEntryIds: Set<String> = []
    @State private var localToolPermissionErrors: [String: String] = [:]
    @State private var transcriptBaselineGeneration = 0
    @State private var transcriptBaselineError: String?
    @State private var transcriptPaginationGeneration = 0
    @State private var transcriptOlderLoading = false
    @State private var transcriptOlderExhausted = false
    @State private var transcriptPaginationError: String?
    @State private var transcriptPrependAnchorId: String?
    @State private var widgetGeneration = 0
    @State private var widgetPendingEntryIds: Set<String> = []
    @State private var widgetCustomAnswers: [String: String] = [:]
    @State private var widgetErrors: [String: String] = [:]
    @State private var transcriptDraftRecipients: [String: String] = [:]
    @State private var transcriptDraftSubjects: [String: String] = [:]
    @State private var transcriptDraftBodies: [String: String] = [:]
    @State private var transcriptDraftPendingEntryIds: Set<String> = []
    @State private var transcriptDraftStatuses: [String: String] = [:]
    @State private var transcriptDraftErrors: [String: String] = [:]
    @State private var secretDrafts: [String: String] = [:]
    @State private var secretPendingEntryIds: Set<String> = []
    @State private var secretRequestGeneration = 0
    @State private var secretProvidedEntryIds: Set<String> = []
    @State private var secretErrors: [String: String] = [:]
    @State private var findPresented = false
    @State private var findQuery = ""
    @State private var findIndex: Int?
    @State private var forwardMessage: MobileChatMessage?
    @State private var composerFocusGeneration = 0
    @FocusState private var findFocused: Bool
    @FocusState private var reactionPickerFocusedId: String?

    var body: some View {
        VStack(spacing: 0) {
            chatHeader
            Divider().opacity(0.35)
            if findPresented {
                findBar
                Divider().opacity(0.25)
            }
            transcriptList
            replyBanner
            voiceStatusBanner
            composer
        }
        .background(Color(red: 0.985, green: 0.985, blue: 0.975))
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("mobile-bot-chat")
        .task(id: semanticFingerprint) { publishAppAgentSurface() }
        .task(id: "\(bot.id):\(bot.conversationId ?? "")") {
            await loadInitialConversationTail()
        }
        .onChange(of: scenePhase) { _, phase in
            if phase == .active {
                linkMetadataFocusRevision = linkMetadataFocusRevision == Int.max
                    ? 1
                    : linkMetadataFocusRevision + 1
            }
        }
        .task(id: listenerScopeFingerprint) {
            await pollVisibleListenerIntegrations()
        }
        .task(id: cloudAgentScopeFingerprint) {
            await pollVisibleCloudAgents()
        }
        .task(id: connectorCardScopeFingerprint) {
            await runConnectorCardLifecycle()
        }
        .task(id: editorSuggestionScopeFingerprint) {
            await refreshEditorSuggestions()
        }
        .onChange(of: model.mcpServers) { _, _ in
            adoptEditorMcpReferencesFromModel()
        }
        .onChange(of: model.connectorCatalog) { _, _ in
            adoptEditorMcpReferencesFromModel()
        }
        .onChange(of: model.mcpBackendAccountKey) { _, _ in
            invalidateEditorSuggestions()
        }
        .onChange(of: bot.id) { _, _ in
            resetAcknowledgementScope()
            cancelVoiceInput()
            invalidateComposerAttachmentStaging()
            invalidateReactionScope()
            resetCloudAgentState()
            approvalGeneration &+= 1
            resetLocalToolPermissionUI()
            transcriptBaselineGeneration &+= 1
            transcriptBaselineError = nil
            widgetGeneration &+= 1
            widgetPendingEntryIds.removeAll()
            widgetCustomAnswers.removeAll()
            widgetErrors.removeAll()
            threadLoadGeneration &+= 1
            transcriptPaginationGeneration &+= 1
            transcriptOlderLoading = false
            transcriptOlderExhausted = false
            transcriptPaginationError = nil
            transcriptPrependAnchorId = nil
            threadLoadingRootId = nil
            threadLoadError = nil
            threadRootId = nil
            resetTranscriptDraftUI()
            resetSecretRequestUI()
            forwardMessage = nil
            closeFind()
            invalidateEditorSuggestions()
        }
        .onChange(of: model.settingsNoticeAccountKey) { _, _ in
            resetAcknowledgementScope()
            transcriptPaginationGeneration &+= 1
            transcriptOlderLoading = false
            transcriptOlderExhausted = false
            transcriptPaginationError = nil
            transcriptPrependAnchorId = nil
            cancelVoiceInput()
            invalidateComposerAttachmentStaging()
            invalidateReactionScope()
            resetCloudAgentState()
            resetLocalToolPermissionUI()
            invalidateEditorSuggestions()
        }
        .onDisappear {
            resetAcknowledgementScope()
            cancelVoiceInput()
            invalidateComposerAttachmentStaging()
            invalidateReactionScope()
            resetCloudAgentState()
            approvalGeneration &+= 1
            resetLocalToolPermissionUI()
            transcriptBaselineGeneration &+= 1
            widgetGeneration &+= 1
            widgetPendingEntryIds.removeAll()
            threadLoadGeneration &+= 1
            threadLoadingRootId = nil
            forwardMessage = nil
            closeFind()
            invalidateEditorSuggestions()
        }
        .fullScreenCover(isPresented: $openedMiniApp) {
            miniAppCover
        }
        .sheet(isPresented: $conversationOutlinePresented) {
            MobileConversationOutlinePanel(
                agentId: bot.id,
                agentName: bot.name,
                accountKey: model.settingsNoticeAccountKey,
                bridge: bridge,
                reconnectGeneration: reconnectGeneration,
                historicalSubagents: bot.subagents,
                onClose: { conversationOutlinePresented = false }
            )
        }
        .sheet(isPresented: $asyncTasksPresented) {
            MobileAsyncTasksPanel(
                agentId: bot.id,
                agentName: bot.name,
                bridge: bridge,
                reconnectGeneration: reconnectGeneration,
                onClose: { asyncTasksPresented = false }
            )
        }
        .sheet(isPresented: $reactionPickerPresented) {
            reactionPickerSheet
        }
        .sheet(item: $forwardMessage) { entry in
            if let sourceConversationId = bot.conversationId,
               let messageId = mobileBotForwardMessageId(
                   entry,
                   sourceConversationId: sourceConversationId
               )
            {
                ForwardMessageSheet(
                    sourceConversationId: sourceConversationId,
                    messageId: messageId,
                    messaging: messaging,
                    appAgentSurface: appAgentSurface
                ) {
                    forwardMessage = nil
                }
            }
        }
        .sheet(
            isPresented: Binding(
                get: { threadRootId != nil },
                set: { presented in if !presented { threadRootId = nil } }
            )
        ) {
            threadSheet
        }
        .fileImporter(
            isPresented: $attachmentImporterPresented,
            allowedContentTypes: [.item],
            allowsMultipleSelection: true
        ) { result in
            Task { await stageImportedComposerAttachments(result) }
        }
    }

    private var visibleListenerPlatforms: [String] {
        Array(Set(entries.compactMap { $0.listenerPlatform })).sorted()
    }

    private var listenerScopeFingerprint: String {
        "\(bot.id)|\(model.accountEmail)|\(visibleListenerPlatforms.joined(separator: ","))"
    }

    @MainActor
    private func pollVisibleListenerIntegrations() async {
        guard !visibleListenerPlatforms.isEmpty else { return }
        while !Task.isCancelled {
            await model.refreshListenerIntegrations()
            do {
                try await Task.sleep(for: .seconds(5))
            } catch {
                return
            }
        }
    }

    private var visibleConnectorNames: [String] {
        normalizeMobileConnectorNames(
            entries.flatMap { $0.connectorNames ?? [] }
        )
    }

    private var connectorCardScopeFingerprint: String {
        [
            model.settingsNoticeAccountKey,
            bot.id,
            visibleConnectorNames.map(normalizeMobileConnectorName).joined(separator: ","),
        ].joined(separator: "|")
    }

    @MainActor
    private func runConnectorCardLifecycle() async {
        guard !visibleConnectorNames.isEmpty else {
            model.closeConnectorCards()
            return
        }
        await model.openConnectorCards()
        defer { model.closeConnectorCards() }
        while !Task.isCancelled {
            do {
                try await Task.sleep(for: .seconds(60))
            } catch {
                return
            }
        }
    }

    private var visibleCloudAgentIds: [String] {
        Array(Set(entries.compactMap { message in
            message.cloudAgentBcId?.trimmingCharacters(in: .whitespacesAndNewlines)
        }.filter { !$0.isEmpty })).sorted()
    }

    private var cloudAgentScopeFingerprint: String {
        [
            model.settingsNoticeAccountKey,
            bot.id,
            visibleCloudAgentIds.joined(separator: ","),
        ].joined(separator: "|")
    }

    @MainActor
    private func resetCloudAgentState() {
        cloudAgentInfoByBcId.removeAll()
        cloudAgentErrorsByBcId.removeAll()
    }

    @MainActor
    private func pollVisibleCloudAgents() async {
        let scope = cloudAgentScopeFingerprint
        let ids = visibleCloudAgentIds
        guard !ids.isEmpty else {
            resetCloudAgentState()
            return
        }
        var nextPollAt: [String: Date] = [:]
        while !Task.isCancelled {
            var hasNonterminal = false
            let now = Date()
            for bcId in ids {
                if Task.isCancelled || cloudAgentScopeFingerprint != scope { return }
                if cloudAgentInfoByBcId[bcId]?.isTerminal == true { continue }
                hasNonterminal = true
                if let next = nextPollAt[bcId], next > now { continue }
                do {
                    let info = try await model.cloudAgentInfo(bcId: bcId)
                    guard !Task.isCancelled, cloudAgentScopeFingerprint == scope else { return }
                    cloudAgentInfoByBcId[bcId] = info
                    cloudAgentErrorsByBcId.removeValue(forKey: bcId)
                    if info.isTerminal {
                        nextPollAt[bcId] = .distantFuture
                    } else {
                        nextPollAt[bcId] = Date().addingTimeInterval(5)
                    }
                } catch is CancellationError {
                    return
                } catch {
                    guard !Task.isCancelled, cloudAgentScopeFingerprint == scope else { return }
                    cloudAgentErrorsByBcId[bcId] = error.localizedDescription
                    nextPollAt[bcId] = Date().addingTimeInterval(60)
                }
            }
            if !hasNonterminal { return }
            do {
                try await Task.sleep(for: .seconds(5))
            } catch {
                return
            }
        }
    }

    private var chatHeader: some View {
        HStack(spacing: 12) {
            Button(action: onClose) {
                Image(systemName: "chevron.left")
                    .font(.system(size: 17, weight: .semibold))
                    .foregroundStyle(.black)
                    .frame(width: 38, height: 38)
                    .background(Color.black.opacity(0.045), in: Circle())
            }
            Spacer()
            Button(action: onOpenSettings) {
                HStack(spacing: 8) {
                    MobileAgentAvatar(bot: bot, size: 28, activeOverride: busy || bot.isRunning)
                    VStack(alignment: .leading, spacing: 1) {
                        Text(bot.name).font(.system(size: 17, weight: .semibold))
                        if let status = mobileConversationHeaderStatus(bot) {
                            Text(status)
                                .font(.caption2.weight(.medium))
                                .foregroundStyle(.secondary)
                        }
                    }
                }
                .padding(.horizontal, 13).padding(.vertical, 7)
                .background(.white, in: Capsule())
                .shadow(color: .black.opacity(0.08), radius: 12, y: 3)
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Bot settings")
            .accessibilityValue(mobileConversationHeaderStatus(bot) ?? "")
            .accessibilityIdentifier("mobile-bot-settings")
            Spacer()
            Button {
                openFind()
            } label: {
                Image(systemName: "magnifyingglass")
                    .font(.system(size: 16, weight: .medium))
                    .frame(width: 38, height: 38)
                    .background(Color.black.opacity(0.045), in: Circle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Find in chat")
            .accessibilityIdentifier("mobile-bot-find")

            Button {
                conversationOutlinePresented = true
            } label: {
                Image(systemName: "list.bullet.rectangle")
                    .font(.system(size: 16, weight: .medium))
                    .frame(width: 38, height: 38)
                    .background(Color.black.opacity(0.045), in: Circle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Full conversation")
            .accessibilityIdentifier("mobile-bot-full-conversation")

            Button {
                asyncTasksPresented = true
            } label: {
                Image(systemName: "clock.arrow.circlepath")
                    .font(.system(size: 16, weight: .medium))
                    .frame(width: 38, height: 38)
                    .background(Color.black.opacity(0.045), in: Circle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Async tasks")
            .accessibilityIdentifier("mobile-bot-async-tasks")
        }
        .padding(.horizontal, 14).padding(.vertical, 8)
        .background(Color.white.opacity(0.97))
    }

    private var transcriptList: some View {
        let mainEntries = mobileMainTranscriptEntries(entries)
        let threadReplyCounts = mobileThreadReplyCounts(entries)
        let threadChipEntryIDs = Set(
            threadReplyCounts.compactMap { key, value in value > 0 ? key : nil }
        )
        let adjacency = projectMobileTranscriptAdjacency(
            mainEntries,
            threadChipEntryIDs: threadChipEntryIDs
        )

        return ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 7) {
                    if !entries.isEmpty, bot.miniAppId == nil, !transcriptOlderExhausted {
                        Button {
                            Task { await loadOlderConversationEntries() }
                        } label: {
                            if transcriptOlderLoading {
                                HStack(spacing: 7) {
                                    ProgressView().controlSize(.small)
                                    Text("Loading earlier messages…")
                                }
                            } else {
                                Label("Load earlier messages", systemImage: "arrow.up")
                            }
                        }
                        .buttonStyle(.bordered)
                        .disabled(transcriptOlderLoading)
                        .frame(maxWidth: .infinity)
                        .accessibilityIdentifier("mobile-bot-load-older")
                    }

                    if let transcriptPaginationError {
                        Text(transcriptPaginationError)
                            .font(.caption)
                            .foregroundStyle(.red)
                            .frame(maxWidth: .infinity)
                            .accessibilityIdentifier("mobile-bot-load-older-error")
                    }

                    if entries.isEmpty {
                        VStack(spacing: 13) {
                            MobileAgentAvatar(bot: bot, size: 82)
                            Text(bot.name).font(.title2.bold())
                            if !bot.description.isEmpty {
                                Text(bot.description)
                                    .font(.subheadline)
                                    .foregroundStyle(.secondary)
                                    .multilineTextAlignment(.center)
                            }
                        }
                        .frame(maxWidth: .infinity)
                        .padding(.top, 96)
                        .padding(.horizontal, 30)
                    }

                    ForEach(Array(mainEntries.enumerated()), id: \.element.id) { index, entry in
                        transcript(
                            entry,
                            adjacency: adjacency.indices.contains(index) ? adjacency[index] : .empty
                        )
                            .padding(
                                .top,
                                adjacency.indices.contains(index) && adjacency[index].isContinuedFromPrev
                                    ? -4
                                    : 0
                            )
                            .background(
                                RoundedRectangle(cornerRadius: 10, style: .continuous)
                                    .fill(
                                        currentFindMatch?.entryId == entry.id
                                            ? Color.yellow.opacity(0.20)
                                            : findMatchEntryIDs.contains(entry.id)
                                                ? Color.yellow.opacity(0.08)
                                                : Color.clear
                                    )
                            )
                            .overlay(
                                RoundedRectangle(cornerRadius: 10, style: .continuous)
                                    .stroke(
                                        currentFindMatch?.entryId == entry.id
                                            ? Color.orange.opacity(0.65)
                                            : Color.clear,
                                        lineWidth: 1
                                    )
                            )
                            .id(entry.id)
                    }
                    if let transcriptBaselineError {
                        VStack(spacing: 8) {
                            Text("Couldn't load conversation")
                                .font(.headline)
                            Text("Couldn't load this conversation. Check your connection and try again.")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                                .multilineTextAlignment(.center)
                            Button(MobileTranscriptLoadErrorCopy.retry) {
                                Task { await loadInitialConversationTail() }
                            }
                            .buttonStyle(.borderedProminent)
                        }
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 16)
                        .accessibilityElement(children: .contain)
                        .accessibilityLabel("Couldn't load conversation. \(transcriptBaselineError)")
                        .accessibilityIdentifier("mobile-bot-transcript-load-error")
                    }
                    if let errorText {
                        Text(errorText)
                            .font(.caption)
                            .foregroundStyle(.red)
                            .padding(.top, 4)
                            .accessibilityIdentifier("mobile-bot-error")
                    }
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 18)
            }
            .background(Color(red: 0.985, green: 0.985, blue: 0.975))
            .onChange(of: entries.count) { _, _ in
                if let prependAnchor = transcriptPrependAnchorId {
                    transcriptPrependAnchorId = nil
                    proxy.scrollTo(prependAnchor, anchor: .top)
                } else if findPresented, let match = currentFindMatch {
                    withAnimation(.easeOut(duration: 0.16)) {
                        proxy.scrollTo(match.entryId, anchor: .center)
                    }
                } else if let last = mainEntries.last {
                    withAnimation(.easeOut(duration: 0.16)) {
                        proxy.scrollTo(last.id, anchor: .bottom)
                    }
                }
            }
            .onChange(of: currentFindMatch?.entryId) { _, entryId in
                guard findPresented, let entryId else { return }
                withAnimation(.easeOut(duration: 0.16)) {
                    proxy.scrollTo(entryId, anchor: .center)
                }
            }
        }
    }

    private var findSearchEntries: [ChatSearchEntry] {
        mobileBotChatSearchEntries(entries, botName: bot.name)
    }

    private var findMatches: [ChatSearchMatch] {
        chatSearchMatches(findSearchEntries, query: findQuery)
    }

    private var currentFindMatch: ChatSearchMatch? {
        guard let findIndex, findMatches.indices.contains(findIndex) else { return nil }
        return findMatches[findIndex]
    }

    private var findMatchEntryIDs: Set<String> {
        Set(findMatches.map(\.entryId))
    }

    private var findOrdinal: String {
        guard !findQuery.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return "" }
        guard let findIndex, findMatches.indices.contains(findIndex) else {
            return "0/\(findMatches.count)"
        }
        return "\(findIndex + 1)/\(findMatches.count)"
    }

    private var findBar: some View {
        HStack(spacing: 8) {
            Image(systemName: "magnifyingglass")
                .foregroundStyle(.secondary)
            TextField("Find in chat", text: $findQuery)
                .focused($findFocused)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .onSubmit { stepFind(1) }
                .onChange(of: findQuery) { _, _ in
                    findIndex = nextChatSearchIndex(
                        current: nil,
                        count: findMatches.count,
                        delta: 1
                    )
                }
                .accessibilityLabel("Find in chat")
                .accessibilityIdentifier("mobile-bot-find-field")
            if !findOrdinal.isEmpty {
                Text(findOrdinal)
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.secondary)
                    .accessibilityIdentifier("mobile-bot-find-count")
            }
            Button { stepFind(-1) } label: {
                Image(systemName: "chevron.up")
            }
            .disabled(findMatches.isEmpty)
            .accessibilityLabel("Previous match")
            .accessibilityIdentifier("mobile-bot-find-previous")
            Button { stepFind(1) } label: {
                Image(systemName: "chevron.down")
            }
            .disabled(findMatches.isEmpty)
            .accessibilityLabel("Next match")
            .accessibilityIdentifier("mobile-bot-find-next")
            Button { closeFind() } label: {
                Image(systemName: "xmark")
            }
            .accessibilityLabel("Close find")
            .accessibilityIdentifier("mobile-bot-find-close")
        }
        .buttonStyle(.plain)
        .padding(.horizontal, 14)
        .padding(.vertical, 8)
        .background(Color.white.opacity(0.97))
    }

    @MainActor
    private func openFind() {
        findPresented = true
        findIndex = nextChatSearchIndex(
            current: nil,
            count: findMatches.count,
            delta: 1
        )
        findFocused = true
    }

    @MainActor
    private func closeFind() {
        findPresented = false
        findQuery = ""
        findIndex = nil
        findFocused = false
    }

    @MainActor
    private func stepFind(_ delta: Int) {
        findIndex = nextChatSearchIndex(
            current: findIndex,
            count: findMatches.count,
            delta: delta
        )
    }

    @ViewBuilder
    private var replyBanner: some View {
        if let replyTargetId {
            let resolution = mobileResolveReplyReference(
                targetID: replyTargetId,
                entries: entries
            )
            let previewLabel = mobileReplyReferenceComposerLabel(resolution.preview)
            HStack(spacing: 8) {
                Image(systemName: replyIsFork ? "bubble.left.and.bubble.right" : "arrowshape.turn.up.left")
                Text(replyIsFork ? "Thread reply · \(previewLabel)" : "Replying · \(previewLabel)")
                    .font(.caption)
                    .lineLimit(1)
                Spacer()
                Button {
                    self.replyTargetId = nil
                    replyIsFork = false
                } label: {
                    Image(systemName: "xmark.circle.fill")
                }
            }
            .foregroundStyle(.secondary)
            .padding(.horizontal, 16)
            .padding(.top, 7)
        }
    }

    @ViewBuilder
    private var voiceStatusBanner: some View {
        if voiceRecorder.isRecording || transcribingVoice {
            HStack(spacing: 9) {
                if transcribingVoice {
                    ProgressView().controlSize(.small)
                    Text("正在离线转写…").font(.caption.weight(.semibold))
                } else {
                    Circle().fill(Color.red).frame(width: 8, height: 8)
                    Text("正在录音 \(voiceRecorder.elapsedSeconds / 60):\(String(format: "%02d", voiceRecorder.elapsedSeconds % 60))")
                        .font(.caption.weight(.semibold))
                    MobileVoiceWaveform(level: voiceRecorder.waveformLevel)
                        .frame(width: 58, height: 18)
                        .accessibilityHidden(true)
                }
                Spacer()
                Button("取消") { cancelVoiceInput() }
                    .font(.caption.weight(.semibold))
                    .disabled(transcribingVoice)
            }
            .padding(.horizontal, 16)
            .padding(.top, 7)
            .onChange(of: voiceRecorder.didReachMaximumDuration) { _, reachedMaximum in
                guard reachedMaximum, !transcribingVoice else { return }
                Task { await finishVoiceInput() }
            }
        }
    }

    private struct MobileVoiceWaveform: View {
        let level: Double

        var body: some View {
            HStack(alignment: .center, spacing: 2) {
                ForEach(0..<9, id: \.self) { index in
                    let emphasis = 0.55 + Double((index * 7) % 5) * 0.11
                    Capsule()
                        .fill(.secondary)
                        .frame(
                            width: 3,
                            height: max(3, 16 * (0.18 + min(1, level) * emphasis))
                        )
                }
            }
            .animation(.easeOut(duration: 0.16), value: level)
        }
    }

    private var editorSuggestionContext: MobileEditorSuggestionContext? {
        mobileEditorSuggestionContext(draft)
    }

    private var editorSuggestionAssistants: [MobileEditorSuggestionItem] {
        let canonicalRoster = availableBots.isEmpty ? [bot] : availableBots
        return projectMobileEditorMentionSuggestions(canonicalRoster)
    }

    private var editorSuggestionPrReferences: [MobileEditorSuggestionItem] {
        projectMobileEditorPrSuggestionItems(
            projectMobileEditorPrReferences(
                entries: entries,
                cloudInfos: cloudAgentInfoByBcId,
                ownedAccountKey: model.settingsNoticeAccountKey,
                currentAccountKey: model.settingsNoticeAccountKey,
                ownedAgentID: bot.id,
                currentAgentID: bot.id
            )
        )
    }

    private var editorSuggestionRows: [MobileEditorSuggestionItem] {
        mobileEditorSuggestionRows(
            context: editorSuggestionContext,
            assistants: editorSuggestionAssistants,
            workflows: editorSuggestionWorkflows,
            mcpReferences: editorSuggestionMcpReferences,
            prReferences: editorSuggestionPrReferences,
            recentKeys: editorSuggestionRecents
        )
    }

    private var editorSuggestionScopeFingerprint: String {
        [
            model.settingsNoticeAccountKey,
            model.mcpBackendAccountKey,
            bot.id,
            String(reconnectGeneration),
        ].joined(separator: "|")
    }

    @ViewBuilder
    private var editorSuggestionList: some View {
        let rows = editorSuggestionRows
        if editorSuggestionContext != nil {
            VStack(alignment: .leading, spacing: 2) {
                if rows.isEmpty {
                    if editorSuggestionStatus == .loading {
                        ProgressView("Loading suggestions…")
                            .controlSize(.small)
                            .padding(.horizontal, 12)
                            .padding(.vertical, 8)
                    } else if editorSuggestionStatus == .failed || editorSuggestionStatus == .unavailable {
                        Text("Suggestions unavailable")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .padding(.horizontal, 12)
                            .padding(.vertical, 8)
                    } else {
                        Text("No matches")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .padding(.horizontal, 12)
                            .padding(.vertical, 8)
                    }
                } else {
                    ScrollView {
                        LazyVStack(alignment: .leading, spacing: 0) {
                            ForEach(Array(rows.enumerated()), id: \.element.id) { index, item in
                                Button {
                                    chooseEditorSuggestion(item)
                                } label: {
                                    HStack(spacing: 9) {
                                        if item.category == .tools,
                                           let rawIconURL = item.iconURL,
                                           let iconURL = URL(string: rawIconURL) {
                                            AsyncImage(url: iconURL) { image in
                                                image.resizable().scaledToFit()
                                            } placeholder: {
                                                Image(systemName: "puzzlepiece.extension")
                                            }
                                            .frame(width: 18, height: 18)
                                            .foregroundStyle(.secondary)
                                        } else {
                                            Image(systemName: item.category == .assistants
                                                ? "person.crop.circle"
                                                : item.category == .automations
                                                    ? "bolt.circle"
                                                    : item.category == .tools
                                                        ? "puzzlepiece.extension"
                                                        : item.category == .pullRequests
                                                            ? "arrow.triangle.pull"
                                                            : "face.smiling")
                                                .foregroundStyle(.secondary)
                                        }
                                        VStack(alignment: .leading, spacing: 1) {
                                            Text(item.label)
                                                .lineLimit(1)
                                            if let subtitle = item.subtitle, !subtitle.isEmpty {
                                                Text(subtitle)
                                                    .font(.caption)
                                                    .foregroundStyle(.secondary)
                                                    .lineLimit(1)
                                            }
                                        }
                                        Spacer()
                                        if index == editorSuggestionActiveIndex {
                                            Image(systemName: "return")
                                                .font(.caption)
                                                .foregroundStyle(.secondary)
                                        }
                                    }
                                    .padding(.horizontal, 12)
                                    .padding(.vertical, 8)
                                    .contentShape(Rectangle())
                                    .background(
                                        index == editorSuggestionActiveIndex
                                            ? Color.accentColor.opacity(0.10)
                                            : Color.clear
                                    )
                                }
                                .buttonStyle(.plain)
                                .accessibilityLabel(
                                    item.subtitle.map { "\(item.label), \($0)" } ?? item.label
                                )
                                .accessibilityIdentifier("mobile-bot-editor-suggestion-\(index)")
                            }
                        }
                    }
                    .frame(maxHeight: 220)
                }
            }
            .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 12))
            .overlay(
                RoundedRectangle(cornerRadius: 12)
                    .stroke(Color.black.opacity(0.08))
            )
            .padding(.horizontal, 12)
            .accessibilityIdentifier("mobile-bot-editor-suggestions")
        }
    }

    private var composer: some View {
        VStack(spacing: 4) {
            editorSuggestionList
            composerAttachmentStrip
            HStack(alignment: .bottom, spacing: 8) {
                MobileComposerTextView(
                    text: $draft,
                    scopeKey: mobileBotConversationScopeKey(
                        accountScopeKey: model.settingsNoticeAccountKey,
                        agentID: bot.id
                    ),
                    focusGeneration: focusPromptGeneration &+ composerFocusGeneration,
                    onSubmit: {
                        if chooseActiveEditorSuggestion() { return }
                        if !busy {
                            Task { await send() }
                        }
                    },
                    onEscape: {
                        guard !editorSuggestionRows.isEmpty else { return false }
                        editorSuggestionActiveIndex = nil
                        return true
                    },
                    onSuggestionMove: { move in
                        let rows = editorSuggestionRows
                        guard !rows.isEmpty else { return false }
                        editorSuggestionActiveIndex = mobileEditorSuggestionNextIndex(
                            current: editorSuggestionActiveIndex,
                            count: rows.count,
                            move: move
                        )
                        return true
                    }
                )
                .background(
                    Color.black.opacity(0.055),
                    in: RoundedRectangle(cornerRadius: 18, style: .continuous)
                )
                .onChange(of: draft) { _, nextDraft in
                    composerMcpReferences = pruneMobileComposerMcpReferences(
                        draft: nextDraft,
                        references: composerMcpReferences
                    )
                    composerPrReferences = pruneMobileComposerPrReferences(
                        draft: nextDraft,
                        references: composerPrReferences
                    )
                    normalizeEditorSuggestionSelection()
                }

                attachmentButton
                miniAppButton
                voiceInputButton
                sendButton
            }
        }
        .padding(.horizontal, 12)
        .padding(.top, 8)
        .padding(.bottom, 10)
        .background(.ultraThinMaterial)
    }

    @MainActor
    private func normalizeEditorSuggestionSelection() {
        let rows = editorSuggestionRows
        guard !rows.isEmpty else {
            editorSuggestionActiveIndex = nil
            return
        }
        if let index = editorSuggestionActiveIndex, rows.indices.contains(index) {
            return
        }
        editorSuggestionActiveIndex = 0
    }

    @MainActor
    private func chooseEditorSuggestion(_ item: MobileEditorSuggestionItem) {
        guard let context = editorSuggestionContext else { return }
        draft = applyMobileEditorSuggestion(draft: draft, context: context, item: item)
        if let reference = item.mcpReference {
            composerMcpReferences = pruneMobileComposerMcpReferences(
                draft: draft,
                references: composerMcpReferences + [reference]
            )
        }
        if let reference = item.prReference {
            composerPrReferences = pruneMobileComposerPrReferences(
                draft: draft,
                references: composerPrReferences + [reference]
            )
        }
        let key = "\(item.category.rawValue):\(item.id)"
        editorSuggestionRecents = [key] + editorSuggestionRecents.filter { $0 != key }
        editorSuggestionRecents = Array(editorSuggestionRecents.prefix(50))
        editorSuggestionActiveIndex = nil
        composerFocusGeneration &+= 1
    }

    @MainActor
    @discardableResult
    private func chooseActiveEditorSuggestion() -> Bool {
        let rows = editorSuggestionRows
        guard !rows.isEmpty else { return false }
        let index = min(max(editorSuggestionActiveIndex ?? 0, 0), rows.count - 1)
        chooseEditorSuggestion(rows[index])
        return true
    }

    @MainActor
    private func invalidateEditorSuggestions() {
        editorSuggestionGeneration &+= 1
        editorSuggestionStatus = .cancelled
        editorSuggestionWorkflows = []
        editorSuggestionMcpReferences = []
        composerMcpReferences = []
        composerPrReferences = []
        editorSuggestionActiveIndex = nil
    }

    @MainActor
    private func adoptEditorMcpReferencesFromModel() {
        let ownedMcpAccount = model.mcpBackendAccountKey
        let ownedAppAccount = model.settingsNoticeAccountKey
        let ownedAgent = bot.id
        editorSuggestionMcpReferences = projectScopedMobileEditorMcpSuggestions(
            servers: model.mcpServers,
            catalog: model.connectorCatalog,
            ownedMcpAccountKey: ownedMcpAccount,
            currentMcpAccountKey: model.mcpBackendAccountKey,
            ownedAppAccountKey: ownedAppAccount,
            currentAppAccountKey: model.settingsNoticeAccountKey,
            ownedAgentID: ownedAgent,
            currentAgentID: bot.id
        )
        normalizeEditorSuggestionSelection()
    }

    @MainActor
    private func refreshEditorSuggestions() async {
        editorSuggestionGeneration &+= 1
        let generation = editorSuggestionGeneration
        let ownedAccount = model.settingsNoticeAccountKey
        let ownedMcpAccount = model.mcpBackendAccountKey
        let ownedAgent = bot.id
        let ownedReconnect = reconnectGeneration
        guard !ownedAccount.isEmpty, !ownedAgent.isEmpty else {
            editorSuggestionStatus = .unavailable
            editorSuggestionWorkflows = []
            editorSuggestionMcpReferences = []
            return
        }

        let previousWorkflows = editorSuggestionWorkflows
        let previousMcp = editorSuggestionMcpReferences
        var sourceFailed = false
        editorSuggestionStatus = .loading

        let requestId = "ios-editor-workflow-list-\(UUID().uuidString.lowercased())"
        do {
            _ = try await bridge.request(
                method: "feature.execute",
                params: [
                    "command": [
                        "type": "workflow.list",
                        "requestId": requestId,
                        "agentId": ownedAgent,
                    ],
                ]
            )
            let result = try await bridge.receiveFeatureEvent(
                deadlineMilliseconds: 8_000
            ) { event in
                event["type"] as? String == "workflow.listed"
                    && event["agentId"] as? String == ownedAgent
            }
            guard generation == editorSuggestionGeneration,
                  model.settingsNoticeAccountKey == ownedAccount,
                  bot.id == ownedAgent,
                  reconnectGeneration == ownedReconnect,
                  let event = result.value as? [String: Any],
                  let rows = event["workflows"] as? [[String: Any]]
            else { return }
            editorSuggestionWorkflows = projectMobileEditorWorkflowSuggestions(rows)
        } catch is CancellationError {
            guard generation == editorSuggestionGeneration else { return }
            editorSuggestionWorkflows = previousWorkflows
            sourceFailed = true
        } catch {
            guard generation == editorSuggestionGeneration,
                  model.settingsNoticeAccountKey == ownedAccount,
                  bot.id == ownedAgent,
                  reconnectGeneration == ownedReconnect
            else { return }
            editorSuggestionWorkflows = previousWorkflows
            sourceFailed = true
        }

        do {
            let snapshot = try await model.refreshMcpReferenceSnapshot()
            guard generation == editorSuggestionGeneration,
                  model.settingsNoticeAccountKey == ownedAccount,
                  bot.id == ownedAgent,
                  reconnectGeneration == ownedReconnect
            else { return }
            guard model.mcpBackendAccountKey == ownedMcpAccount else { return }
            editorSuggestionMcpReferences = projectScopedMobileEditorMcpSuggestions(
                servers: snapshot.servers,
                catalog: snapshot.catalog,
                ownedMcpAccountKey: ownedMcpAccount,
                currentMcpAccountKey: model.mcpBackendAccountKey,
                ownedAppAccountKey: ownedAccount,
                currentAppAccountKey: model.settingsNoticeAccountKey,
                ownedAgentID: ownedAgent,
                currentAgentID: bot.id
            )
        } catch is CancellationError {
            guard generation == editorSuggestionGeneration else { return }
            editorSuggestionMcpReferences = previousMcp
            sourceFailed = true
        } catch {
            guard generation == editorSuggestionGeneration,
                  model.settingsNoticeAccountKey == ownedAccount,
                  bot.id == ownedAgent,
                  reconnectGeneration == ownedReconnect
            else { return }
            editorSuggestionMcpReferences = previousMcp
            sourceFailed = true
        }

        if editorSuggestionWorkflows.isEmpty && editorSuggestionMcpReferences.isEmpty {
            editorSuggestionStatus = sourceFailed ? .failed : .empty
        } else {
            editorSuggestionStatus = .ready
        }
        normalizeEditorSuggestionSelection()
    }

    @ViewBuilder
    private var composerAttachmentStrip: some View {
        if !composerAttachments.isEmpty {
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 7) {
                    ForEach(Array(composerAttachments.enumerated()), id: \.offset) { index, attachment in
                        HStack(spacing: 6) {
                            Image(systemName: "paperclip")
                            VStack(alignment: .leading, spacing: 1) {
                                Text(attachment.name)
                                    .font(.caption.weight(.semibold))
                                    .lineLimit(1)
                                Text(ByteCountFormatter.string(
                                    fromByteCount: Int64(attachment.sizeBytes),
                                    countStyle: .file
                                ))
                                .font(.caption2)
                                .foregroundStyle(.secondary)
                            }
                            Button {
                                guard composerAttachments.indices.contains(index) else { return }
                                composerAttachments.remove(at: index)
                            } label: {
                                Image(systemName: "xmark.circle.fill")
                                    .foregroundStyle(.secondary)
                            }
                            .buttonStyle(.plain)
                            .accessibilityLabel("Remove \(attachment.name)")
                        }
                        .padding(.horizontal, 9)
                        .padding(.vertical, 6)
                        .background(Color.black.opacity(0.06), in: RoundedRectangle(cornerRadius: 10))
                        .accessibilityIdentifier("mobile-bot-composer-attachment-\(index)")
                    }
                }
                .padding(.horizontal, 1)
            }
            .frame(maxHeight: 54)
            .accessibilityIdentifier("mobile-bot-composer-attachments")
        }
    }

    @ViewBuilder
    private var attachmentButton: some View {
        if bot.miniAppId == nil {
            Button {
                attachmentImporterPresented = true
            } label: {
                if stagingAttachments {
                    ProgressView()
                        .controlSize(.small)
                        .frame(width: 39, height: 39)
                } else {
                    Image(systemName: "paperclip")
                        .font(.system(size: 16, weight: .semibold))
                        .frame(width: 39, height: 39)
                        .background(Color.black.opacity(0.075), in: Circle())
                }
            }
            .buttonStyle(.plain)
            .disabled(
                busy
                    || stagingAttachments
                    || voiceRecorder.isRecording
                    || transcribingVoice
                    || composerAttachments.count >= mobileComposerAttachmentLimit
            )
            .accessibilityLabel("Attach files")
            .accessibilityIdentifier("mobile-bot-attach")
        }
    }

    @ViewBuilder
    private var miniAppButton: some View {
        if bot.miniAppId == GlobalDharmaMiniAppBridge.globalDharmaId {
            Button {
                openedMiniApp = true
            } label: {
                Text(bot.menuButtonText ?? "打开应用")
                    .font(.caption.bold())
                    .lineLimit(1)
                    .padding(.horizontal, 10)
                    .frame(height: 39)
                    .background(Color.black.opacity(0.075), in: Capsule())
            }
            .buttonStyle(.plain)
            .accessibilityLabel(bot.menuButtonText ?? "打开应用")
            .accessibilityIdentifier("mobile-bot-open-miniapp")
        }
    }

    @ViewBuilder
    private var voiceInputButton: some View {
        if !busy {
            Button {
                if voiceRecorder.isRecording {
                    Task { await finishVoiceInput() }
                } else {
                    Task { await startVoiceInput() }
                }
            } label: {
                Image(systemName: voiceRecorder.isRecording ? "stop.fill" : "mic.fill")
                    .font(.system(size: 16, weight: .bold))
                    .foregroundStyle(.white)
                    .frame(width: 39, height: 39)
                    .background(voiceRecorder.isRecording ? Color.red : Color.black, in: Circle())
            }
            .disabled(transcribingVoice || stagingAttachments)
            .accessibilityIdentifier(voiceRecorder.isRecording ? "mobile-bot-voice-stop" : "mobile-bot-voice-start")
        }
    }

    private var sendButton: some View {
        Button {
            if busy {
                Task { await stop() }
            } else {
                Task { await send() }
            }
        } label: {
            Image(systemName: busy ? "stop.fill" : "arrow.up")
                .font(.system(size: 16, weight: .bold))
                .foregroundStyle(.white)
                .frame(width: 39, height: 39)
                .background(busy ? Color.red : Color.black, in: Circle())
        }
        .disabled(
            !busy && (
                !mobileComposerHasPayload(text: draft, attachments: composerAttachments)
                    || stagingAttachments
                    || voiceRecorder.isRecording
                    || transcribingVoice
            )
        )
        .accessibilityIdentifier(busy ? "mobile-bot-stop" : "mobile-bot-send")
    }

    @ViewBuilder
    private var miniAppCover: some View {
        if let miniAppId = bot.miniAppId,
           let plugin = model.plugins.first(where: { $0.pluginId == miniAppId }) {
            MiniAppWebMcpSurface(plugin: plugin, model: model)
        } else {
            VStack(spacing: 12) {
                ProgressView()
                Text("正在加载应用…")
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
            }
            .task {
                if model.plugins.first(where: { $0.pluginId == bot.miniAppId }) == nil {
                    await model.refresh()
                }
            }
        }
    }

    private var semanticFingerprint: String {
        let botIdFingerprint: String = bot.id
        let botNameFingerprint: String = bot.name
        let botDescriptionFingerprint: String = bot.description
        let botTitleFingerprint: String = bot.title ?? ""
        let notifyOnUpdatesFingerprint: String = String(bot.notifyOnUpdatesEnabled)
        let miniAppIdFingerprint: String = bot.miniAppId ?? ""
        let menuButtonTextFingerprint: String = bot.menuButtonText ?? ""
        let draftFingerprint: String = draft
        let busyFingerprint: String = String(busy)
        let openedMiniAppFingerprint: String = String(openedMiniApp)
        let activeOperationFingerprint: String = activeOperationId ?? ""
        let errorFingerprint: String = errorText ?? ""

        let entryFingerprints: [String] = entries.map { entry -> String in
            let entryId: String = "\(entry.id)"
            let entryKind: String = "\(entry.kind.rawValue)"
            let entryRole: String = "\(entry.role.rawValue)"
            let reactionFingerprint = entry.reactions
                .map { "\($0.emoji):\($0.by)" }
                .joined(separator: "|")
            return "\(entryId):\(entryKind):\(entryRole):\(reactionFingerprint)"
        }
        let entriesFingerprint: String = entryFingerprints.joined(separator: ",")

        let fields: [String] = [
            botIdFingerprint,
            botNameFingerprint,
            botDescriptionFingerprint,
            botTitleFingerprint,
            notifyOnUpdatesFingerprint,
            miniAppIdFingerprint,
            menuButtonTextFingerprint,
            draftFingerprint,
            busyFingerprint,
            openedMiniAppFingerprint,
            activeOperationFingerprint,
            errorFingerprint,
            entriesFingerprint,
        ]
        return fields.joined(separator: "|")
    }

    @MainActor
    private func publishAppAgentSurface() {
        var elements: [FabushiAppAgentSurface.Element] = [
            .init(agentId: "mobile-bot-chat", role: "application", name: "Bot \(String(bot.name.prefix(160)))"),
            .init(agentId: "mobile-bot-close", role: "button", name: "关闭 Bot 对话"),
            .init(agentId: "mobile-bot-settings", role: "button", name: "Bot 设置"),
            .init(agentId: "mobile-bot-find", role: "button", name: "Find in chat"),
            .init(agentId: "mobile-bot-full-conversation", role: "button", name: "Full conversation"),
            .init(agentId: "mobile-bot-async-tasks", role: "button", name: "Async tasks"),
            .init(agentId: "mobile-bot-draft", role: "textbox", name: "Bot 消息"),
        ]
        let sendId = busy ? "mobile-bot-stop" : "mobile-bot-send"
        elements.append(.init(
            agentId: sendId,
            role: "button",
            name: busy ? "停止 Bot" : "发送 Bot 消息",
            enabled: busy || !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        ))
        if bot.miniAppId == GlobalDharmaMiniAppBridge.globalDharmaId {
            elements.append(.init(
                agentId: "mobile-bot-open-miniapp",
                role: "button",
                name: bot.menuButtonText ?? "打开应用",
                enabled: !openedMiniApp
            ))
        }
        if errorText != nil {
            elements.append(.init(agentId: "mobile-bot-error", role: "status", name: "Bot 或 Mini App 调用失败"))
        }
        for entry in entries.suffix(50) {
            let id = Self.semanticId("mobile-bot-entry-\(entry.id)")
            let roleName = entry.role == .user ? "用户消息"
                : entry.kind == .handoff ? "等待用户接管"
                : entry.kind == .action ? "Bot 动作"
                : entry.kind == .thinking ? "Bot 思考"
                : entry.kind == .notice ? "通知"
                : entry.kind == .permissionRequest ? "权限请求记录"
                : entry.kind == .timelineEvent ? "时间线事件"
                : "Bot 消息"
            elements.append(.init(agentId: id, role: "log", name: roleName))
        }
        for entry in entries where entry.kind == .permissionRequest && entry.actionStatus == "pending" {
            let approveId = Self.semanticId("mobile-bot-approval-once-\(entry.id)")
            let denyId = Self.semanticId("mobile-bot-approval-deny-\(entry.id)")
            elements.append(.init(agentId: approveId, role: "button", name: "Allow once"))
            elements.append(.init(agentId: denyId, role: "button", name: "Deny"))
            if let proposed = entry.approvalProposedRule,
               !proposed.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                let alwaysId = Self.semanticId("mobile-bot-approval-always-\(entry.id)")
                elements.append(.init(agentId: alwaysId, role: "button", name: "Always allow"))
            }
        }
        for entry in entries where entry.kind == .handoff && entry.actionStatus == "pending" {
            let completeId = Self.semanticId("mobile-bot-handoff-complete-\(entry.id)")
            let dismissId = Self.semanticId("mobile-bot-handoff-dismiss-\(entry.id)")
            elements.append(.init(agentId: completeId, role: "button", name: "已完成并归还控制"))
            elements.append(.init(agentId: dismissId, role: "button", name: "无法完成此步骤"))
        }
        var actions: [String: FabushiAppAgentSurface.Action] = [
            "mobile-bot-close": .init(allowed: ["invoke"]) { _ in onClose() },
            "mobile-bot-settings": .init(allowed: ["invoke"]) { _ in onOpenSettings() },
            "mobile-bot-find": .init(allowed: ["invoke"]) { _ in openFind() },
            "mobile-bot-async-tasks": .init(allowed: ["invoke"]) { _ in asyncTasksPresented = true },
            "mobile-bot-draft": .init(allowed: ["setValue"]) { value in draft = value ?? "" },
        ]
        actions[sendId] = .init(allowed: ["invoke"]) { _ in
            if busy { Task { await stop() } } else { Task { await send() } }
        }
        if bot.miniAppId == GlobalDharmaMiniAppBridge.globalDharmaId {
            actions["mobile-bot-open-miniapp"] = .init(allowed: ["invoke"]) { _ in openedMiniApp = true }
        }
        for entry in entries where entry.kind == .permissionRequest && entry.actionStatus == "pending" {
            let approveId = Self.semanticId("mobile-bot-approval-once-\(entry.id)")
            let denyId = Self.semanticId("mobile-bot-approval-deny-\(entry.id)")
            actions[approveId] = .init(allowed: ["invoke"]) { _ in
                Task { await resolveApproval(entry, resolution: .approved) }
            }
            actions[denyId] = .init(allowed: ["invoke"]) { _ in
                Task { await resolveApproval(entry, resolution: .denied) }
            }
            if let proposed = entry.approvalProposedRule,
               !proposed.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                let alwaysId = Self.semanticId("mobile-bot-approval-always-\(entry.id)")
                actions[alwaysId] = .init(allowed: ["invoke"]) { _ in
                    Task { await resolveApproval(entry, resolution: .always) }
                }
            }
        }
        for entry in entries where entry.kind == .handoff && entry.actionStatus == "pending" {
            let completeId = Self.semanticId("mobile-bot-handoff-complete-\(entry.id)")
            let dismissId = Self.semanticId("mobile-bot-handoff-dismiss-\(entry.id)")
            actions[completeId] = .init(allowed: ["invoke"]) { _ in
                Task { await resolveBoxHandoff(entry, resolution: "completed") }
            }
            actions[dismissId] = .init(allowed: ["invoke"]) { _ in
                Task { await resolveBoxHandoff(entry, resolution: "dismissed") }
            }
        }
        try? appAgentSurface.publish(screen: "bot-chat", elements: elements, actions: actions)
    }

    private static func semanticId(_ value: String) -> String {
        String(value.map { character in
            character.isASCII && (character.isLetter || character.isNumber || "._:/@-".contains(character)) ? character : "-"
        }.prefix(200))
    }

    @ViewBuilder
    private func transcript(
        _ entry: MobileChatMessage,
        adjacency: MobileTranscriptAdjacency = .empty
    ) -> some View {
        if entry.kind == .thinking {
            HStack(spacing: 7) {
                MobileAgentAvatar(bot: bot, size: 22, activeOverride: true)
                Text(entry.actionTitle ?? "Thinking…").font(.caption).foregroundStyle(.secondary)
                ProgressView().controlSize(.mini)
            }
            .padding(.vertical, 4)
        } else if entry.kind == .handoff {
            VStack(alignment: .leading, spacing: 10) {
                HStack(spacing: 7) {
                    Image(systemName: "person.crop.circle.badge.exclamationmark")
                    Text("需要你完成一步").font(.caption.weight(.semibold))
                }
                Text(entry.text).font(.system(size: 15))
                if entry.actionStatus == "pending" {
                    HStack(spacing: 8) {
                        Button("已完成，继续") { Task { await resolveBoxHandoff(entry, resolution: "completed") } }
                            .buttonStyle(.borderedProminent)
                            .accessibilityIdentifier(Self.semanticId("mobile-bot-handoff-complete-\(entry.id)"))
                        Button("无法完成") { Task { await resolveBoxHandoff(entry, resolution: "dismissed") } }
                            .buttonStyle(.bordered)
                            .accessibilityIdentifier(Self.semanticId("mobile-bot-handoff-dismiss-\(entry.id)"))
                    }
                } else {
                    Text(entry.actionStatus == "completed" ? "已归还控制" : "已结束接管")
                        .font(.caption).foregroundStyle(.secondary)
                }
            }
            .padding(12)
            .background(Color.orange.opacity(0.10), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
        } else if entry.kind == .notice {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Image(systemName: "info.circle")
                    .foregroundStyle(.secondary)
                Text(entry.text)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Spacer(minLength: 8)
                Text(entry.createdAt, style: .time)
                    .font(.caption2)
                    .foregroundStyle(.tertiary)
            }
            .padding(.vertical, 4)
            .accessibilityIdentifier(Self.semanticId("mobile-bot-notice-\(entry.id)"))
        } else if entry.kind == .permissionRequest {
            VStack(alignment: .leading, spacing: 10) {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Image(systemName: "lock.shield").foregroundStyle(.secondary)
                    Text(entry.text).font(.caption)
                    Spacer(minLength: 8)
                    Text(entry.createdAt, style: .time).font(.caption2).foregroundStyle(.tertiary)
                }
                if let proposed = entry.approvalProposedRule,
                   !proposed.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                    Text(redactSandAutoReviewInlineSecrets(proposed))
                        .font(.caption2.monospaced()).foregroundStyle(.secondary).lineLimit(4)
                }
                if entry.actionStatus == "pending" {
                    HStack(spacing: 8) {
                        Button("Allow once") { Task { await resolveApproval(entry, resolution: .approved) } }
                            .buttonStyle(.borderedProminent)
                            .accessibilityIdentifier(Self.semanticId("mobile-bot-approval-once-\(entry.id)"))
                        if let proposed = entry.approvalProposedRule,
                           !proposed.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                            Button("Always allow") { Task { await resolveApproval(entry, resolution: .always) } }
                                .buttonStyle(.bordered)
                                .accessibilityIdentifier(Self.semanticId("mobile-bot-approval-always-\(entry.id)"))
                        }
                        Button("Deny", role: .destructive) { Task { await resolveApproval(entry, resolution: .denied) } }
                            .buttonStyle(.bordered)
                            .accessibilityIdentifier(Self.semanticId("mobile-bot-approval-deny-\(entry.id)"))
                    }
                } else if entry.actionStatus == "submitting" {
                    ProgressView("Applying decision…").controlSize(.small)
                } else if let status = entry.actionStatus {
                    Text(status == "always" ? "Always allowed"
                        : status == "approved" ? "Allowed once"
                        : status == "denied" ? "Denied"
                        : status == "stale" ? "This request is no longer pending."
                        : "Approval failed. You can retry from the next request.")
                        .font(.caption).foregroundStyle(status == "failed" ? .red : .secondary)
                }
            }
            .padding(10)
            .background(Color.black.opacity(0.035), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
            .accessibilityIdentifier(Self.semanticId("mobile-bot-permission-request-\(entry.id)"))
        } else if entry.kind == .timelineEvent {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Image(systemName: entry.timelineAutomationId == nil ? "clock" : "calendar")
                    .foregroundStyle(.secondary)
                Text(entry.text)
                    .font(.caption)
                    .lineLimit(1)
                if let automationId = entry.timelineAutomationId,
                   case let .automationChanged(_, automationName)? = entry.timelineEvent
                {
                    Button(automationName) {
                        onOpenAutomation(automationId)
                    }
                    .buttonStyle(.bordered)
                    .controlSize(.mini)
                    .accessibilityLabel("Open routine \(automationName)")
                    .accessibilityIdentifier(Self.semanticId("mobile-bot-open-routine-\(automationId)"))
                }
                Spacer(minLength: 8)
                Text(entry.createdAt, style: .time)
                    .font(.caption2)
                    .foregroundStyle(.tertiary)
            }
            .padding(.vertical, 4)
            .accessibilityIdentifier(Self.semanticId("mobile-bot-timeline-event-\(entry.id)"))
        } else if entry.kind == .toolCall {
            toolResultCard(entry)
        } else if entry.kind == .action {
            if let bcId = entry.cloudAgentBcId {
                cloudAgentCard(entry, bcId: bcId)
            } else if let connectors = entry.connectorNames {
                connectorCard(entry, connectors: connectors)
            } else if entry.localToolPermissionRequestId != nil {
                localToolPermissionCard(entry)
            } else if let widget = mobileTranscriptWidgetProjection(entry) {
                transcriptWidgetCard(entry, projection: widget)
            } else if let draft = mobileTranscriptDraftProjection(entry.canonicalTranscriptCard) {
                transcriptDraftCard(entry, draft: draft)
            } else if let secret = mobileSecretRequestProjection(entry.canonicalTranscriptCard) {
                secretRequestCard(entry, secret: secret)
            } else if let platform = entry.listenerPlatform {
                listenerIntegrationCard(entry, platform: platform)
            } else {
                HStack(spacing: 7) {
                    Circle().fill(entry.actionStatus == "failed" ? Color.red : Color.orange).frame(width: 7, height: 7)
                    Text(entry.actionTitle ?? "Working").font(.caption.weight(.medium))
                    if let detail = entry.actionDetail, !detail.isEmpty { Text(detail).font(.caption).foregroundStyle(.secondary).lineLimit(1) }
                }
                .padding(.vertical, 2)
            }
        } else if entry.role == .user {
            let seam = projectMobileMessageCardSeam(entry)
            HStack {
                Spacer(minLength: 54)
                VStack(alignment: .leading, spacing: 7) {
                    replyReferenceContent(entry)
                    messageTextContent(entry)
                    attachmentContent(entry)
                    reactionPills(entry)
                    threadAffordance(entry)
                    if let phase = entry.optimisticDeliveryPhase {
                        HStack(spacing: 5) {
                            if phase != .failed {
                                ProgressView().controlSize(.mini).tint(.white)
                            } else {
                                Image(systemName: "exclamationmark.circle.fill")
                            }
                            Text(
                                phase == .acceptedAwaitingEcho ? "Waiting for sync…"
                                    : phase == .failed ? "Failed to send"
                                    : "Sending…"
                            )
                            .font(.caption2.weight(.semibold))
                        }
                        .accessibilityIdentifier(Self.semanticId("mobile-bot-send-state-\(entry.id)"))
                        if phase == .failed,
                           let detail = entry.optimisticDeliveryError,
                           !detail.isEmpty {
                            Text(detail)
                                .font(.caption2)
                                .lineLimit(2)
                                .opacity(0.8)
                        }
                    }
                }
                .foregroundStyle(seam.isStandaloneEmoji ? Color.primary : Color.white)
                .tint(seam.isStandaloneEmoji ? Color.accentColor : Color.white)
                .padding(.horizontal, seam.isStandaloneEmoji ? 4 : 15)
                .padding(.vertical, seam.isStandaloneEmoji ? 2 : 10)
                .background(
                    seam.isStandaloneEmoji ? Color.clear : Color.black,
                    in: UnevenRoundedRectangle(
                        cornerRadii: .init(
                            topLeading: 18,
                            bottomLeading: 18,
                            bottomTrailing: adjacency.isContinuedToNext ? 8 : 18,
                            topTrailing: adjacency.isContinuedFromPrev ? 8 : 18
                        ),
                        style: .continuous
                    )
                )
                .contextMenu {
                    if mobileStableReplyTargetID(entry) != nil {
                        Button("Reply") { beginReply(to: entry, inThread: threadRootId != nil) }
                        Button(threadRootId == nil ? "Start Thread" : "Reply in Thread") { beginReply(to: entry, inThread: true) }
                    }
                    if mobileBotForwardMessageId(
                        entry,
                        sourceConversationId: bot.conversationId
                    ) != nil {
                        Button("Forward") { forwardMessage = entry }
                    }
                    if let copyText = seam.copyText {
                        Button("Copy") { UIPasteboard.general.string = copyText }
                    }
                    if entry.optimisticDeliveryPhase == .failed {
                        Button("Retry") {
                            Task { await retryFailedSend(entry) }
                        }
                    }
                    reactionMenu(entry)
                }
            }
        } else {
            let seam = projectMobileMessageCardSeam(entry)
            VStack(alignment: .leading, spacing: 3) {
                if adjacency.isRunStart {
                    Text(bot.name)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .padding(.leading, 12)
                }
                HStack(alignment: .bottom, spacing: 7) {
                    MobileAgentAvatar(bot: bot, size: 20)
                        .opacity(adjacency.isContinuedToNext ? 0 : 1)
                        .accessibilityHidden(adjacency.isContinuedToNext)
                    VStack(alignment: .leading, spacing: 7) {
                        replyReferenceContent(entry)
                        messageTextContent(entry)
                            .foregroundStyle(.black)
                        attachmentContent(entry)
                        reactionPills(entry)
                        threadAffordance(entry)
                    }
                    .padding(.horizontal, 15).padding(.vertical, 10)
                    .background(
                        Color.black.opacity(0.055),
                        in: UnevenRoundedRectangle(
                            cornerRadii: .init(
                                topLeading: adjacency.isContinuedFromPrev ? 8 : 18,
                                bottomLeading: adjacency.isContinuedToNext ? 8 : 18,
                                bottomTrailing: 18,
                                topTrailing: 18
                            ),
                            style: .continuous
                        )
                    )
                    .contextMenu {
                        if mobileStableReplyTargetID(entry) != nil {
                            Button("Reply") { beginReply(to: entry, inThread: threadRootId != nil) }
                            Button(threadRootId == nil ? "Start Thread" : "Reply in Thread") { beginReply(to: entry, inThread: true) }
                        }
                        if mobileBotForwardMessageId(
                            entry,
                            sourceConversationId: bot.conversationId
                        ) != nil {
                            Button("Forward") { forwardMessage = entry }
                        }
                        if let copyText = seam.copyText {
                            Button("Copy") { UIPasteboard.general.string = copyText }
                        }
                        reactionMenu(entry)
                    }
                    Spacer(minLength: 30)
                }
            }
        }
    }

    @ViewBuilder
    private func toolResultCard(_ entry: MobileChatMessage) -> some View {
        let snapshot = mobileToolResultForEntry(
            entry,
            cardsByAgent: model.toolResultCardsByAgent,
            agentId: bot.id
        )
        let heading = snapshot?.path
            ?? snapshot?.command
            ?? entry.toolName
            ?? entry.actionTitle
            ?? "Tool"
        let status = snapshot?.status.rawValue
            ?? entry.toolStatus
            ?? entry.actionStatus
            ?? "running"
        let detail: String = {
            guard let snapshot else {
                return entry.toolSummary ?? entry.actionDetail ?? ""
            }
            if !snapshot.summary.isEmpty { return snapshot.summary }
            if !snapshot.diff.isEmpty { return snapshot.diff }
            return snapshot.output
        }()

        DisclosureGroup {
            VStack(alignment: .leading, spacing: 8) {
                if let workingDirectory = snapshot?.workingDirectory, !workingDirectory.isEmpty {
                    Text(workingDirectory)
                        .font(.system(.caption, design: .monospaced))
                        .textSelection(.enabled)
                }
                if !detail.isEmpty {
                    Text(detail)
                        .font(.system(.caption, design: .monospaced))
                        .textSelection(.enabled)
                        .accessibilityLabel(
                            snapshot?.isStreaming == true
                                ? "Streaming tool result"
                                : "Tool result"
                        )
                }
                if let diff = snapshot?.diff, !diff.isEmpty {
                    Text(diff)
                        .font(.system(.caption, design: .monospaced))
                        .textSelection(.enabled)
                        .accessibilityLabel(snapshot?.path ?? snapshot?.kind.rawValue ?? "Tool diff")
                }
            }
            .padding(.top, 6)
        } label: {
            HStack(spacing: 8) {
                Text(heading)
                    .font(.system(.caption, design: .monospaced).weight(.semibold))
                    .lineLimit(2)
                Spacer(minLength: 8)
                Text(status)
                    .font(.caption2.weight(.semibold))
                    .foregroundStyle(.secondary)
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .background(
            Color.black.opacity(0.045),
            in: RoundedRectangle(cornerRadius: 10, style: .continuous)
        )
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier(
            Self.semanticId("mobile-bot-tool-result-\(entry.toolCallId ?? entry.id)")
        )
    }

    @MainActor
    private func beginReply(to entry: MobileChatMessage, inThread: Bool) {
        guard let targetID = mobileStableReplyTargetID(entry) else { return }
        replyTargetId = targetID
        replyIsFork = inThread
        if threadRootId != nil { threadRootId = nil }
    }

    @ViewBuilder
    private func replyReferenceContent(_ entry: MobileChatMessage) -> some View {
        if let targetID = entry.replyToMessageId {
            let resolution = mobileResolveReplyReference(
                targetID: targetID,
                entries: entries
            )
            HStack(spacing: 5) {
                switch resolution.preview {
                case .image:
                    Image(systemName: "photo")
                case .file:
                    Image(systemName: "doc")
                case .link:
                    Image(systemName: "link")
                case .userText, .assistantText, .missing:
                    Image(systemName: "arrowshape.turn.up.left")
                }
                Text(mobileReplyReferenceQuoteLabel(resolution.preview))
                    .lineLimit(2)
            }
            .font(.caption2)
            .opacity(0.72)
            .accessibilityElement(children: .combine)
            .accessibilityLabel(
                resolution.isResolved
                    ? "Reply to \(mobileReplyReferenceQuoteLabel(resolution.preview))"
                    : "Reply target deleted"
            )
            .accessibilityIdentifier(
                Self.semanticId("mobile-bot-reply-reference-\(entry.id)")
            )
        }
    }

    @ViewBuilder
    private func threadAffordance(_ entry: MobileChatMessage) -> some View {
        if threadRootId == nil {
            let rootId = mobileTranscriptCanonicalId(entry)
            let count = mobileThreadReplyCounts(entries)[rootId] ?? 0
            if count > 0 {
                Button {
                    openThread(rootId: rootId)
                } label: {
                    HStack(spacing: 5) {
                        Text("View thread")
                        Text(count == 1 ? "1 reply" : "\(count) replies")
                            .foregroundStyle(.secondary)
                    }
                    .font(.caption2.weight(.semibold))
                }
                .buttonStyle(.plain)
                .accessibilityLabel("View thread, \(count == 1 ? "1 reply" : "\(count) replies")")
                .accessibilityIdentifier(Self.semanticId("mobile-bot-view-thread-\(rootId)"))
            }
        }
    }

    @ViewBuilder
    private var threadSheet: some View {
        if let rootId = threadRootId {
            NavigationStack {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 9) {
                        if threadLoadingRootId == rootId {
                            ProgressView("Loading full thread…")
                                .controlSize(.small)
                        }
                        if let threadLoadError, !threadLoadError.isEmpty {
                            Text(threadLoadError)
                                .font(.caption)
                                .foregroundStyle(.red)
                        }
                        ForEach(mobileThreadEntries(entries, rootId: rootId)) { entry in
                            transcript(entry)
                                .id("thread:\(entry.id)")
                        }
                    }
                    .padding(16)
                }
                .navigationTitle("Thread")
                .navigationBarTitleDisplayMode(.inline)
                .safeAreaInset(edge: .bottom) {
                    Button {
                        replyTargetId = rootId
                        replyIsFork = true
                        threadRootId = nil
                    } label: {
                        Label("Reply in thread", systemImage: "arrowshape.turn.up.left")
                            .frame(maxWidth: .infinity)
                    }
                    .buttonStyle(.borderedProminent)
                    .padding()
                    .background(.ultraThinMaterial)
                    .accessibilityIdentifier(Self.semanticId("mobile-bot-thread-reply-\(rootId)"))
                }
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) {
                        Button("Close") { threadRootId = nil }
                    }
                }
            }
            .accessibilityIdentifier(Self.semanticId("mobile-bot-thread-\(rootId)"))
        }
    }

    @MainActor
    private func openThread(rootId: String) {
        threadRootId = rootId
        threadLoadError = nil
        Task { await loadCompleteThread(rootId: rootId) }
    }

    @MainActor
    private func loadCompleteThread(rootId: String) async {
        guard bot.miniAppId == nil,
              let conversationId = bot.conversationId?
                .trimmingCharacters(in: .whitespacesAndNewlines),
              !conversationId.isEmpty
        else { return }

        threadLoadGeneration &+= 1
        let generation = threadLoadGeneration
        let ownedBotId = bot.id
        let ownedAccountKey = model.settingsNoticeAccountKey
        threadLoadingRootId = rootId
        defer {
            if generation == threadLoadGeneration, threadLoadingRootId == rootId {
                threadLoadingRootId = nil
            }
        }

        var beforeMessageId: String?
        var fetched: [MobileChatMessage] = []
        var seenAnchors: Set<String> = []
        do {
            for page in 0..<50 {
                guard generation == threadLoadGeneration,
                      bot.id == ownedBotId,
                      model.settingsNoticeAccountKey == ownedAccountKey,
                      threadRootId == rootId
                else { return }

                let requestId = "ios-mobile-thread-\(page)-\(UUID().uuidString.lowercased())"
                var command: [String: Any] = [
                    "type": "conversation.openWindowed",
                    "requestId": requestId,
                    "conversationId": conversationId,
                    "limit": 200,
                ]
                if let beforeMessageId {
                    command["beforeMessageId"] = beforeMessageId
                }
                _ = try await bridge.request(
                    method: "feature.execute",
                    params: ["command": command]
                )
                let result = try await bridge.receiveFeatureEvent(
                    deadlineMilliseconds: 8_000
                ) { event in
                    event["type"] as? String == "conversation.windowOpened"
                        && event["requestId"] as? String == requestId
                        && event["conversationId"] as? String == conversationId
                }
                guard generation == threadLoadGeneration,
                      bot.id == ownedBotId,
                      model.settingsNoticeAccountKey == ownedAccountKey,
                      threadRootId == rootId,
                      let event = result.value as? [String: Any],
                      let rows = event["messages"] as? [[String: Any]]
                else { return }

                var pageEntries: [MobileChatMessage] = []
                for row in rows {
                    guard let projected = projectMobileConversationWindowEntries(row) else {
                        throw NSError(
                            domain: "Fabushi.MobileBotChat",
                            code: 42,
                            userInfo: [NSLocalizedDescriptionKey: "Host returned malformed thread history"]
                        )
                    }
                    pageEntries.append(contentsOf: projected)
                }
                fetched.append(contentsOf: pageEntries)
                if pageEntries.contains(where: {
                    $0.kind == .message && mobileTranscriptCanonicalId($0) == rootId
                }) {
                    entries = mergeMobileConversationHistory(current: entries, fetched: fetched)
                    threadLoadError = nil
                    return
                }

                guard let next = event["nextBeforeMessageId"] as? String,
                      !next.isEmpty
                else {
                    throw NSError(
                        domain: "Fabushi.MobileBotChat",
                        code: 43,
                        userInfo: [NSLocalizedDescriptionKey: "Thread root is no longer available in conversation history"]
                    )
                }
                guard seenAnchors.insert(next).inserted else {
                    throw NSError(
                        domain: "Fabushi.MobileBotChat",
                        code: 44,
                        userInfo: [NSLocalizedDescriptionKey: "Conversation history pagination repeated an anchor"]
                    )
                }
                beforeMessageId = next
            }
            throw NSError(
                domain: "Fabushi.MobileBotChat",
                code: 45,
                userInfo: [NSLocalizedDescriptionKey: "Thread history exceeded the bounded 10,000-message lookup"]
            )
        } catch is CancellationError {
            return
        } catch {
            guard generation == threadLoadGeneration,
                  bot.id == ownedBotId,
                  model.settingsNoticeAccountKey == ownedAccountKey,
                  threadRootId == rootId
            else { return }
            threadLoadError = error.localizedDescription
        }
    }

    @MainActor
    private func resolveTranscriptWidget(
        entry: MobileChatMessage,
        value: String?,
        dismiss: Bool
    ) async {
        guard !widgetPendingEntryIds.contains(entry.id),
              let conversationId = bot.conversationId?
                .trimmingCharacters(in: .whitespacesAndNewlines),
              !conversationId.isEmpty
        else { return }

        let answer = value?.trimmingCharacters(in: .whitespacesAndNewlines)
        if !dismiss, answer?.isEmpty != false { return }

        let ownedGeneration = widgetGeneration
        let ownedBotId = bot.id
        widgetPendingEntryIds.insert(entry.id)
        widgetErrors.removeValue(forKey: entry.id)
        defer {
            if widgetGeneration == ownedGeneration {
                widgetPendingEntryIds.remove(entry.id)
            }
        }

        do {
            let requestId = "ios-widget-\(UUID().uuidString.lowercased())"
            var command: [String: Any] = [
                "type": dismiss ? "widget.dismiss" : "widget.respond",
                "requestId": requestId,
                "conversationId": conversationId,
                "entryId": entry.id,
                "agentId": bot.id,
            ]
            if let answer { command["value"] = answer }
            let result = try await bridge.request(
                method: "feature.execute",
                params: ["command": command]
            )
            guard widgetGeneration == ownedGeneration, bot.id == ownedBotId else { return }
            let accepted = result.value as? [String: Any]
            let operationId = accepted?["operationId"] as? String

            let changed = try await bridge.receiveFeatureEvent(
                deadlineMilliseconds: 8_000
            ) { event in
                event["type"] as? String == "transcript.card"
                    && event["entryId"] as? String == entry.id
            }
            guard widgetGeneration == ownedGeneration, bot.id == ownedBotId,
                  let event = changed.value as? [String: Any],
                  let projected = projectMobileTranscriptCard(
                    event: event,
                    operationId: operationId
                  )
            else { return }
            if let index = entries.firstIndex(where: { $0.id == entry.id }) {
                entries[index] = projected
            }
            widgetCustomAnswers[entry.id] = ""

            guard let operationId, !operationId.isEmpty else { return }
            busy = true
            activeOperationId = operationId
            entries.append(MobileChatMessage(
                id: "thinking:\(operationId)",
                role: .assistant,
                text: "",
                kind: .thinking,
                operationId: operationId,
                actionTitle: "Continuing from your answer",
                actionStatus: "running"
            ))
            await pump(operationId: operationId)
            if widgetGeneration == ownedGeneration, bot.id == ownedBotId {
                activeOperationId = nil
                busy = false
            }
        } catch {
            guard widgetGeneration == ownedGeneration, bot.id == ownedBotId else { return }
            widgetErrors[entry.id] = error.localizedDescription
            activeOperationId = nil
            busy = false
        }
    }

    @ViewBuilder
    private func transcriptWidgetCard(
        _ entry: MobileChatMessage,
        projection: MobileTranscriptWidgetProjection
    ) -> some View {
        let widget = projection.widget
        let pending = widgetPendingEntryIds.contains(entry.id)
        let settled = projection.respondedValue != nil || projection.dismissed
        let options = widget.options
        VStack(alignment: .leading, spacing: 10) {
            Text(widget.prompt)
                .font(.body.weight(.semibold))
            if let help = widget.helpText, !help.isEmpty {
                Text(help)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            if let respondedValue = projection.respondedValue {
                Label(
                    getWidgetAnswerLabel(widget, answer: respondedValue),
                    systemImage: "checkmark.circle.fill"
                )
                .font(.caption.weight(.semibold))
                .foregroundStyle(.green)
            } else if projection.dismissed {
                Label("Dismissed", systemImage: "xmark.circle")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(.secondary)
            } else {
                ForEach(Array(options.enumerated()), id: \.offset) { index, option in
                    Button {
                        Task {
                            await resolveTranscriptWidget(
                                entry: entry,
                                value: option.value ?? option.label,
                                dismiss: false
                            )
                        }
                    } label: {
                        HStack(alignment: .top, spacing: 9) {
                            Text(String(UnicodeScalar(65 + index)!))
                                .font(.caption.monospaced().weight(.bold))
                                .foregroundStyle(.secondary)
                            VStack(alignment: .leading, spacing: 2) {
                                Text(option.label)
                                    .font(.caption.weight(.semibold))
                                if let description = option.description, !description.isEmpty {
                                    Text(description)
                                        .font(.caption2)
                                        .foregroundStyle(.secondary)
                                }
                            }
                            Spacer()
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    .buttonStyle(.bordered)
                    .disabled(pending)
                    .keyboardShortcut(
                        KeyEquivalent(Character(String(UnicodeScalar(97 + index)!))),
                        modifiers: []
                    )
                }

                if widget.allowCustom == true {
                    HStack(spacing: 8) {
                        TextField(
                            "Other answer",
                            text: Binding(
                                get: { widgetCustomAnswers[entry.id] ?? "" },
                                set: { widgetCustomAnswers[entry.id] = $0 }
                            )
                        )
                        .disabled(pending)
                        .onSubmit {
                            let value = (widgetCustomAnswers[entry.id] ?? "")
                                .trimmingCharacters(in: .whitespacesAndNewlines)
                            guard !pending, !value.isEmpty else { return }
                            Task {
                                await resolveTranscriptWidget(
                                    entry: entry,
                                    value: value,
                                    dismiss: false
                                )
                            }
                        }
                        Button("Submit") {
                            Task {
                                await resolveTranscriptWidget(
                                    entry: entry,
                                    value: widgetCustomAnswers[entry.id],
                                    dismiss: false
                                )
                            }
                        }
                        .buttonStyle(.borderedProminent)
                        .disabled(
                            pending
                                || (widgetCustomAnswers[entry.id] ?? "")
                                    .trimmingCharacters(in: .whitespacesAndNewlines)
                                    .isEmpty
                        )
                    }
                }

                Button("Dismiss", role: .cancel) {
                    Task {
                        await resolveTranscriptWidget(
                            entry: entry,
                            value: nil,
                            dismiss: true
                        )
                    }
                }
                .buttonStyle(.bordered)
                .disabled(pending)
            }

            if pending {
                ProgressView("Saving response…")
                    .controlSize(.small)
            }
            if let error = widgetErrors[entry.id], !error.isEmpty {
                Text(error)
                    .font(.caption2)
                    .foregroundStyle(.red)
            }
        }
        .padding(10)
        .background(Color.black.opacity(0.035), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .accessibilityIdentifier(Self.semanticId("mobile-bot-widget-\(entry.id)"))
        .accessibilityValue(settled ? "settled" : "waiting")
    }

    @MainActor
    private func resetTranscriptDraftUI() {
        transcriptDraftRecipients.removeAll()
        transcriptDraftSubjects.removeAll()
        transcriptDraftBodies.removeAll()
        transcriptDraftPendingEntryIds.removeAll()
        transcriptDraftStatuses.removeAll()
        transcriptDraftErrors.removeAll()
    }

    @MainActor
    private func resolveTranscriptDraft(
        entry: MobileChatMessage,
        draft: MobileTranscriptDraftProjection,
        action: String
    ) async {
        guard let payload = entry.canonicalTranscriptCard,
              !transcriptDraftPendingEntryIds.contains(entry.id)
        else { return }

        var overrides: [String: Any] = [:]
        if action == "send" {
            switch draft {
            case let .email(email):
                let recipientsValue = transcriptDraftRecipients[entry.id] ?? email.to.joined(separator: ", ")
                guard let recipients = mobileEmailRecipients(recipientsValue) else { return }
                let subject = transcriptDraftSubjects[entry.id] ?? email.subject
                let body = transcriptDraftBodies[entry.id] ?? email.body
                guard !body.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
                overrides["to"] = recipients
                overrides["subject"] = subject
                overrides["body"] = body
            case let .slack(slack):
                let body = transcriptDraftBodies[entry.id] ?? slack.body
                guard !body.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
                overrides["body"] = body
            }
        }

        transcriptDraftPendingEntryIds.insert(entry.id)
        transcriptDraftStatuses[entry.id] = action == "send" ? "sending" : "discarding"
        transcriptDraftErrors.removeValue(forKey: entry.id)
        defer { transcriptDraftPendingEntryIds.remove(entry.id) }

        do {
            let result = try await model.resolveTranscriptDraft(
                payload,
                overrides: overrides,
                action: action
            )
            transcriptDraftStatuses[entry.id] = result.status
            if let error = result.error, !error.isEmpty {
                transcriptDraftErrors[entry.id] = error
            }
        } catch {
            transcriptDraftStatuses[entry.id] = "failed"
            transcriptDraftErrors[entry.id] = error.localizedDescription
        }
    }

    @ViewBuilder
    private func transcriptDraftCard(
        _ entry: MobileChatMessage,
        draft: MobileTranscriptDraftProjection
    ) -> some View {
        let pending = transcriptDraftPendingEntryIds.contains(entry.id)
        switch draft {
        case let .email(email):
            let status = transcriptDraftStatuses[entry.id] ?? email.status
            let terminal = ["sent", "discarded"].contains(status)
            let recipients = transcriptDraftRecipients[entry.id] ?? email.to.joined(separator: ", ")
            let body = transcriptDraftBodies[entry.id] ?? email.body
            VStack(alignment: .leading, spacing: 9) {
                HStack {
                    Label("New email", systemImage: "envelope")
                        .font(.caption.weight(.semibold))
                    Spacer()
                    Text(status == "sending" ? "Sending…" : status.capitalized)
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                }
                if let from = email.from, !from.isEmpty {
                    LabeledContent("From", value: from).font(.caption2)
                }
                if !terminal {
                    TextField(
                        "name@example.com",
                        text: Binding(
                            get: { transcriptDraftRecipients[entry.id] ?? email.to.joined(separator: ", ") },
                            set: { transcriptDraftRecipients[entry.id] = $0 }
                        )
                    )
                    .textContentType(.emailAddress)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    TextField(
                        "Subject",
                        text: Binding(
                            get: { transcriptDraftSubjects[entry.id] ?? email.subject },
                            set: { transcriptDraftSubjects[entry.id] = $0 }
                        )
                    )
                    TextField(
                        "Write a message",
                        text: Binding(
                            get: { transcriptDraftBodies[entry.id] ?? email.body },
                            set: { transcriptDraftBodies[entry.id] = $0 }
                        ),
                        axis: .vertical
                    )
                    .lineLimit(4...10)
                    HStack {
                        Button("Send email") {
                            Task { await resolveTranscriptDraft(entry: entry, draft: draft, action: "send") }
                        }
                        .buttonStyle(.borderedProminent)
                        .disabled(pending || mobileEmailRecipients(recipients) == nil || body.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                        Button("Discard", role: .destructive) {
                            Task { await resolveTranscriptDraft(entry: entry, draft: draft, action: "discard") }
                        }
                        .buttonStyle(.bordered)
                        .disabled(pending)
                    }
                } else {
                    Text(status == "sent" ? "Sent to \(email.to.first ?? "") — “\(email.subject)”" : "Draft discarded")
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                }
                if let error = transcriptDraftErrors[entry.id] ?? email.error, !error.isEmpty {
                    Text(error).font(.caption2).foregroundStyle(.red)
                }
            }
            .padding(10)
            .background(Color.black.opacity(0.035), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
            .accessibilityIdentifier(Self.semanticId("mobile-bot-email-draft-\(entry.id)"))

        case let .slack(slack):
            let status = transcriptDraftStatuses[entry.id] ?? slack.status
            let terminal = ["sent", "discarded"].contains(status)
            let body = transcriptDraftBodies[entry.id] ?? slack.body
            VStack(alignment: .leading, spacing: 9) {
                HStack {
                    Label("Slack message", systemImage: "message")
                        .font(.caption.weight(.semibold))
                    Spacer()
                    Text(status == "sending" ? "Sending…" : status.capitalized)
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                }
                if let workspace = slack.workspace, !workspace.isEmpty {
                    LabeledContent("Workspace", value: workspace).font(.caption2)
                }
                LabeledContent("To", value: slack.target).font(.caption2)
                LabeledContent("Thread", value: slack.thread ?? "New message").font(.caption2)
                if !terminal {
                    TextField(
                        "Write a message",
                        text: Binding(
                            get: { transcriptDraftBodies[entry.id] ?? slack.body },
                            set: { transcriptDraftBodies[entry.id] = $0 }
                        ),
                        axis: .vertical
                    )
                    .lineLimit(4...10)
                    HStack {
                        Button("Send message") {
                            Task { await resolveTranscriptDraft(entry: entry, draft: draft, action: "send") }
                        }
                        .buttonStyle(.borderedProminent)
                        .disabled(pending || body.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                        Button("Discard", role: .destructive) {
                            Task { await resolveTranscriptDraft(entry: entry, draft: draft, action: "discard") }
                        }
                        .buttonStyle(.bordered)
                        .disabled(pending)
                    }
                } else {
                    Text(status == "sent" ? "Sent to \(slack.target)" : "Draft discarded")
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                }
                if let error = transcriptDraftErrors[entry.id] ?? slack.error, !error.isEmpty {
                    Text(error).font(.caption2).foregroundStyle(.red)
                }
            }
            .padding(10)
            .background(Color.black.opacity(0.035), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
            .accessibilityIdentifier(Self.semanticId("mobile-bot-slack-draft-\(entry.id)"))
        }
    }

    @MainActor
    private func resetSecretRequestUI() {
        secretRequestGeneration = secretRequestGeneration == Int.max ? 1 : secretRequestGeneration + 1
        secretDrafts.removeAll()
        secretPendingEntryIds.removeAll()
        secretProvidedEntryIds.removeAll()
        secretErrors.removeAll()
    }

    @MainActor
    private func submitSecretRequest(
        entry: MobileChatMessage,
        secret: MobileSecretRequestProjection
    ) async {
        let value = (secretDrafts[entry.id] ?? "")
            .trimmingCharacters(in: .whitespacesAndNewlines)
        guard !value.isEmpty,
              !secret.provided,
              !secretProvidedEntryIds.contains(entry.id),
              !secretPendingEntryIds.contains(entry.id)
        else { return }

        let fence = MobileSecretRequestFence(
            accountKey: model.settingsNoticeAccountKey,
            agentId: bot.id,
            generation: secretRequestGeneration
        )
        secretPendingEntryIds.insert(entry.id)
        secretErrors.removeValue(forKey: entry.id)
        defer {
            if fence.accepts(
                accountKey: model.settingsNoticeAccountKey,
                agentId: bot.id,
                generation: secretRequestGeneration
            ) {
                secretPendingEntryIds.remove(entry.id)
            }
        }
        do {
            try await model.provideTranscriptSecret(
                secretRequestId: secret.requestId,
                value: value
            )
            guard fence.accepts(
                accountKey: model.settingsNoticeAccountKey,
                agentId: bot.id,
                generation: secretRequestGeneration
            ) else { return }
            secretDrafts[entry.id] = ""
            secretProvidedEntryIds.insert(entry.id)
        } catch {
            guard fence.accepts(
                accountKey: model.settingsNoticeAccountKey,
                agentId: bot.id,
                generation: secretRequestGeneration
            ) else { return }
            secretErrors[entry.id] = error.localizedDescription
        }
    }

    @ViewBuilder
    private func secretRequestCard(
        _ entry: MobileChatMessage,
        secret: MobileSecretRequestProjection
    ) -> some View {
        let provided = secret.provided || secretProvidedEntryIds.contains(entry.id)
        let pending = secretPendingEntryIds.contains(entry.id)
        VStack(alignment: .leading, spacing: 9) {
            HStack(spacing: 8) {
                Image(systemName: provided ? "checkmark.shield.fill" : "key.fill")
                    .foregroundStyle(provided ? .green : .secondary)
                Text(secret.label)
                    .font(.caption.weight(.semibold))
                Spacer()
                if provided {
                    Text("Provided")
                        .font(.caption2.weight(.semibold))
                        .foregroundStyle(.green)
                }
            }
            if let description = secret.description,
               !description.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                Text(description)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
            if !provided {
                SecureField(
                    "Enter secret",
                    text: Binding(
                        get: { secretDrafts[entry.id] ?? "" },
                        set: { secretDrafts[entry.id] = $0 }
                    )
                )
                .textContentType(.password)
                .disabled(pending)
                .accessibilityIdentifier(Self.semanticId("mobile-bot-secret-input-\(entry.id)"))

                Button(pending ? "Submitting…" : "Submit") {
                    Task { await submitSecretRequest(entry: entry, secret: secret) }
                }
                .buttonStyle(.borderedProminent)
                .controlSize(.small)
                .disabled(
                    pending
                        || (secretDrafts[entry.id] ?? "")
                            .trimmingCharacters(in: .whitespacesAndNewlines)
                            .isEmpty
                )
                .accessibilityIdentifier(Self.semanticId("mobile-bot-secret-submit-\(entry.id)"))
            }
            if let error = secretErrors[entry.id], !error.isEmpty {
                Text(error)
                    .font(.caption2)
                    .foregroundStyle(.red)
            }
        }
        .padding(10)
        .background(Color.black.opacity(0.035), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .accessibilityIdentifier(Self.semanticId("mobile-bot-secret-request-\(entry.id)"))
    }

    @ViewBuilder
    private func connectorCard(_ entry: MobileChatMessage, connectors: [String]) -> some View {
        VStack(alignment: .leading, spacing: 9) {
            if let detail = entry.actionDetail,
               !detail.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                Text(detail).font(.caption).foregroundStyle(.secondary)
            }
            if connectors.isEmpty {
                Text("No connectors requested.")
                    .font(.caption).foregroundStyle(.secondary)
            } else {
                ForEach(Array(connectors.enumerated()), id: \.offset) { _, connector in
                    let server = MarketplaceModel.connectorServer(
                        model.mcpServers,
                        connector: connector,
                        serverIdHint: connectors.count == 1 ? entry.connectorServerIdHint : nil
                    )
                    let key = MarketplaceModel.connectorCardKey(connector)
                    let action = model.connectorCardActions[key]
                    HStack(spacing: 8) {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(connector).font(.caption.weight(.semibold))
                            Text(
                                server?.managedByTeamPluginPolicy == true || server?.isTeamServer == true
                                    ? "Managed by team"
                                    : server?.status ?? action ?? "Available"
                            )
                            .font(.caption2).foregroundStyle(.secondary)
                            if let error = model.connectorCardErrors[key], !error.isEmpty {
                                Text(error).font(.caption2).foregroundStyle(.red).lineLimit(2)
                            }
                        }
                        Spacer()
                        if server?.managedByTeamPluginPolicy == true || server?.isTeamServer == true {
                            Text("Managed").font(.caption2).foregroundStyle(.secondary)
                        } else if server?.status == "connected" || server?.status == "ready" || action == "ready" {
                            Label("Connected", systemImage: "checkmark.circle.fill")
                                .font(.caption2).foregroundStyle(.secondary)
                        } else if action == "waiting" {
                            Button("Reopen") {
                                model.reopenConnectorCard(connector: connector)
                            }
                            .buttonStyle(.bordered).controlSize(.small)
                        } else {
                            Button(action == "failed" ? "Retry" : "Connect") {
                                Task {
                                    await model.connectConnectorCard(
                                        connector: connector,
                                        serverIdHint: connectors.count == 1 ? entry.connectorServerIdHint : nil
                                    )
                                }
                            }
                            .buttonStyle(.borderedProminent).controlSize(.small)
                            .disabled(action == "installing" || action == "authenticating")
                        }
                    }
                }
            }
        }
        .padding(10)
        .background(Color.black.opacity(0.035), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .accessibilityIdentifier(Self.semanticId("mobile-bot-connectors-(entry.id)"))
    }

    @ViewBuilder
    private func cloudAgentCard(_ entry: MobileChatMessage, bcId: String) -> some View {
        let info = cloudAgentInfoByBcId[bcId]
        let error = cloudAgentErrorsByBcId[bcId]
        let title = info?.name ?? entry.actionTitle ?? "Cloud agent"
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                Image(systemName: info?.isTerminal == true ? "checkmark.circle" : "cloud")
                    .foregroundStyle(info?.status == "error" ? .red : .secondary)
                Text(title).font(.caption.weight(.semibold))
                Spacer(minLength: 8)
                Text(info?.status.capitalized ?? (error == nil ? "Loading" : "Unavailable"))
                    .font(.caption2.weight(.semibold))
                    .foregroundStyle(info?.status == "error" || error != nil ? .red : .secondary)
            }
            if let prompt = info?.prompt, !prompt.isEmpty {
                Text(prompt).font(.caption2).foregroundStyle(.secondary).lineLimit(3)
            }
            if let branch = info?.branchName, !branch.isEmpty {
                Label(branch, systemImage: "arrow.triangle.branch")
                    .font(.caption2.monospaced()).foregroundStyle(.secondary)
            }
            if let info,
               info.filesChanged != nil || info.linesAdded != nil || info.linesRemoved != nil {
                Text("Files \(info.filesChanged ?? 0) · +\(info.linesAdded ?? 0) −\(info.linesRemoved ?? 0)")
                    .font(.caption2.monospacedDigit()).foregroundStyle(.secondary)
            }
            if let error, !error.isEmpty {
                Text(error).font(.caption2).foregroundStyle(.red).lineLimit(2)
            }
            HStack(spacing: 8) {
                if let cursorURL = URL(string: "https://cursor.com/agents/\(bcId)") {
                    Link("Open in Cursor", destination: cursorURL)
                        .buttonStyle(.bordered)
                        .controlSize(.small)
                        .accessibilityIdentifier(Self.semanticId("mobile-bot-cloud-agent-open-\(bcId)"))
                }
                if let rawPR = info?.prURL,
                   let prURL = URL(string: rawPR),
                   prURL.scheme?.lowercased() == "https",
                   prURL.host != nil {
                    Link(info?.prNumber.map { "View PR #\($0)" } ?? "View PR", destination: prURL)
                        .buttonStyle(.bordered)
                        .controlSize(.small)
                        .accessibilityIdentifier(Self.semanticId("mobile-bot-cloud-agent-pr-\(bcId)"))
                }
            }
        }
        .padding(10)
        .background(Color.black.opacity(0.035), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .accessibilityIdentifier(Self.semanticId("mobile-bot-cloud-agent-\(bcId)"))
    }

    @MainActor
    private func resetLocalToolPermissionUI() {
        localToolPermissionGeneration &+= 1
        localToolPermissionPendingEntryIds.removeAll()
        localToolPermissionCeilings.removeAll()
        localToolPermissionCeilingLoadedEntryIds.removeAll()
        localToolPermissionErrors.removeAll()
    }

    @MainActor
    private func loadLocalToolPermissionPolicy(for entry: MobileChatMessage) async {
        guard entry.localToolPermissionStatus == "pending",
              entry.localToolPermissionRequestId != nil
        else { return }
        let ownedGeneration = localToolPermissionGeneration
        let ownedAccount = model.settingsNoticeAccountKey
        let ownedBotId = bot.id
        do {
            let state = try await model.loadLocalToolPermissionState()
            guard !Task.isCancelled,
                  localToolPermissionGeneration == ownedGeneration,
                  model.settingsNoticeAccountKey == ownedAccount,
                  bot.id == ownedBotId
            else { return }
            localToolPermissionCeilingLoadedEntryIds.insert(entry.id)
            if let ceiling = state.ceiling {
                localToolPermissionCeilings[entry.id] = ceiling
            } else {
                localToolPermissionCeilings.removeValue(forKey: entry.id)
            }
            localToolPermissionErrors.removeValue(forKey: entry.id)
        } catch is CancellationError {
            return
        } catch {
            guard !Task.isCancelled,
                  localToolPermissionGeneration == ownedGeneration,
                  model.settingsNoticeAccountKey == ownedAccount,
                  bot.id == ownedBotId
            else { return }
            // Fail closed for the standing Always choice until policy can be read.
            localToolPermissionCeilingLoadedEntryIds.remove(entry.id)
            localToolPermissionCeilings.removeValue(forKey: entry.id)
            localToolPermissionErrors[entry.id] = error.localizedDescription
        }
    }

    @MainActor
    private func resolveLocalToolPermission(
        _ entry: MobileChatMessage,
        resolution: String
    ) async {
        guard entry.localToolPermissionStatus == "pending",
              let requestId = entry.localToolPermissionRequestId,
              mobileLocalToolPermissionResolutions.contains(resolution),
              !localToolPermissionPendingEntryIds.contains(entry.id)
        else { return }

        let ownedGeneration = localToolPermissionGeneration
        let ownedAccount = model.settingsNoticeAccountKey
        let ownedBotId = bot.id
        localToolPermissionPendingEntryIds.insert(entry.id)
        localToolPermissionErrors.removeValue(forKey: entry.id)
        defer {
            if localToolPermissionGeneration == ownedGeneration,
               model.settingsNoticeAccountKey == ownedAccount,
               bot.id == ownedBotId {
                localToolPermissionPendingEntryIds.remove(entry.id)
            }
        }

        do {
            let authoritativeResolution = try await model.resolveLocalToolPermission(
                entryId: entry.id,
                requestId: requestId,
                agentId: ownedBotId,
                resolution: resolution
            )
            guard !Task.isCancelled,
                  localToolPermissionGeneration == ownedGeneration,
                  model.settingsNoticeAccountKey == ownedAccount,
                  bot.id == ownedBotId,
                  let index = entries.firstIndex(where: {
                      $0.id == entry.id && $0.localToolPermissionRequestId == requestId
                  })
            else { return }
            entries[index].localToolPermissionStatus = authoritativeResolution
            entries[index].actionStatus = authoritativeResolution
            entries[index].actionDetail = mobileLocalToolPermissionOutcomeText(authoritativeResolution)
        } catch is CancellationError {
            return
        } catch {
            guard !Task.isCancelled,
                  localToolPermissionGeneration == ownedGeneration,
                  model.settingsNoticeAccountKey == ownedAccount,
                  bot.id == ownedBotId
            else { return }
            localToolPermissionErrors[entry.id] = error.localizedDescription
        }
    }

    @ViewBuilder
    private func localToolPermissionCard(_ entry: MobileChatMessage) -> some View {
        let status = entry.localToolPermissionStatus ?? entry.actionStatus ?? "pending"
        let pending = status == "pending"
        let submitting = localToolPermissionPendingEntryIds.contains(entry.id)
        let alwaysBlocked = mobileLocalToolPermissionAlwaysBlocked(
            ceilingLoaded: localToolPermissionCeilingLoadedEntryIds.contains(entry.id),
            ceiling: localToolPermissionCeilings[entry.id]
        )

        VStack(alignment: .leading, spacing: 10) {
            if pending {
                Text("Allow Fabushi and all agents to run commands on your local device?")
                    .font(.subheadline.weight(.semibold))
                Text("This applies to Fabushi and every agent. You can always change it in Settings.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                if let error = localToolPermissionErrors[entry.id], !error.isEmpty {
                    Text("Your answer didn’t go through. Check your connection and try again.")
                        .font(.caption)
                        .foregroundStyle(.red)
                        .accessibilityHint(error)
                }
                HStack(spacing: 8) {
                    Button("Always allow") {
                        Task { await resolveLocalToolPermission(entry, resolution: "always") }
                    }
                    .buttonStyle(.borderedProminent)
                    .controlSize(.small)
                    .disabled(submitting || alwaysBlocked)
                    .help(alwaysBlocked ? "Always allow is unavailable until team policy permits it." : "")

                    Button("Allow once") {
                        Task { await resolveLocalToolPermission(entry, resolution: "allow-once") }
                    }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
                    .disabled(submitting)

                    Button("Never") {
                        Task { await resolveLocalToolPermission(entry, resolution: "never") }
                    }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
                    .disabled(submitting)

                    Button("Deny once") {
                        Task { await resolveLocalToolPermission(entry, resolution: "deny") }
                    }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
                    .disabled(submitting)
                    .keyboardShortcut(.cancelAction)
                }
            } else {
                Text(mobileLocalToolPermissionOutcomeText(status))
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        .padding(10)
        .background(Color.black.opacity(0.035), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Local tool permission")
        .accessibilityIdentifier(Self.semanticId("mobile-bot-local-tool-permission-\(entry.id)"))
        .task(
            id: [
                model.settingsNoticeAccountKey,
                bot.id,
                entry.id,
                String(reconnectGeneration),
                String(localToolPermissionGeneration),
            ].joined(separator: "|")
        ) {
            await loadLocalToolPermissionPolicy(for: entry)
        }
    }

    @ViewBuilder
    private func listenerIntegrationCard(
        _ entry: MobileChatMessage,
        platform rawPlatform: String
    ) -> some View {
        let platform = rawPlatform.lowercased()
        let integration = model.listenerIntegrationState(for: platform)
        let connected = integration?.isConnected ?? (entry.actionStatus == "completed")
        let connecting = model.listenerConnectingPlatform == platform
        let authorizing = model.listenerAuthorizingPlatform == platform
        let busy = connecting || authorizing
        let title = integration?.displayName
            ?? entry.actionTitle?.replacingOccurrences(of: "连接 ", with: "")
            ?? platform.capitalized
        VStack(alignment: .leading, spacing: 7) {
            HStack(spacing: 8) {
                Image(systemName: connected ? "checkmark.circle.fill" : "bolt.horizontal.circle")
                    .foregroundStyle(connected ? .green : .secondary)
                Text(connected ? "\(title) 已连接" : "连接 \(title)")
                    .font(.caption.weight(.semibold))
                Spacer(minLength: 8)
                if connected {
                    Text("Connected")
                        .font(.caption2.weight(.semibold))
                        .foregroundStyle(.green)
                } else {
                    Button(authorizing ? "授权中…" : (connecting ? "连接中…" : "连接")) {
                        Task { await model.connectListenerIntegration(platform: platform) }
                    }
                    .buttonStyle(.borderedProminent)
                    .controlSize(.small)
                    .disabled(busy)
                    .accessibilityIdentifier(Self.semanticId("mobile-bot-listener-connect-\(platform)"))
                }
            }
            if !connected {
                let detail = integration?.blurb ?? entry.actionDetail
                if let detail, !detail.isEmpty {
                    Text(detail)
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                }
            }
            if let error = integration?.error ?? model.listenerIntegrationErrors[platform],
               !error.isEmpty {
                Text(error)
                    .font(.caption2)
                    .foregroundStyle(.red)
            }
        }
        .padding(.vertical, 4)
        .accessibilityIdentifier(Self.semanticId("mobile-bot-listener-card-\(platform)"))
    }

    private static let quickReactionEmojis = ["👍", "👎", "❤️", "😂", "🎉", "😮"]
    private let reactionPickerColumnCount = 6

    @ViewBuilder
    private func reactionMenu(_ entry: MobileChatMessage) -> some View {
        if isMobileReactionActionable(entry) {
            Menu("React") {
                ForEach(Self.quickReactionEmojis, id: \.self) { emoji in
                    Button(emoji) { toggleReaction(entry, emoji: emoji) }
                }
                Divider()
                Button("More Reactions…") { openReactionPicker(entry) }
            }
        }
    }

    @ViewBuilder
    private func reactionPills(_ entry: MobileChatMessage) -> some View {
        let pills = projectMobileReactionPills(entry.reactions)
        let canToggle = isMobileReactionActionable(entry)
        if !pills.isEmpty {
            HStack(spacing: 5) {
                ForEach(pills) { pill in
                    Button {
                        toggleReaction(entry, emoji: pill.emoji)
                    } label: {
                        HStack(spacing: 3) {
                            Text(pill.emoji)
                            if pill.count > 1 { Text("\(pill.count)").font(.caption2) }
                        }
                        .padding(.horizontal, 7)
                        .padding(.vertical, 3)
                        .background(
                            pill.chosenByMe
                                ? Color.accentColor.opacity(0.16)
                                : Color.black.opacity(0.06),
                            in: Capsule()
                        )
                    }
                    .buttonStyle(.plain)
                    .disabled(!canToggle)
                    .accessibilityLabel(
                        "\(pill.emoji), \(pill.count) reaction\(pill.count == 1 ? "" : "s")"
                            + (pill.chosenByMe ? ", selected by you" : "")
                    )
                    .accessibilityHint(
                        canToggle
                            ? (pill.reactors.isEmpty
                                ? "Toggle reaction"
                                : "Reactors: \(pill.reactors.joined(separator: ", "))")
                            : "Reaction unavailable until this message is settled."
                    )
                    .accessibilityIdentifier(
                        Self.semanticId("mobile-bot-reaction-\(entry.id)-\(pill.emoji)")
                    )
                }
            }
        }
    }

    private var reactionPickerTarget: MobileChatMessage? {
        guard let targetId = reactionPickerTargetId,
              let entry = entries.first(where: { $0.id == targetId }),
              isMobileReactionActionable(entry)
        else { return nil }
        return entry
    }

    private var reactionPickerResults: [MobileReactionCatalogItem] {
        mobileReactionPickerResults(
            query: reactionPickerSearch,
            category: reactionPickerCategory,
            recentIds: reactionPickerRecentIds
        )
    }

    private var reactionPickerSheet: some View {
        NavigationStack {
            VStack(spacing: 0) {
                HStack(spacing: 12) {
                    Label("Category", systemImage: "square.grid.2x2")
                        .font(.subheadline.weight(.semibold))
                    Spacer()
                    Picker("Reaction category", selection: $reactionPickerCategory) {
                        ForEach(MobileReactionPickerCategory.allCases) { category in
                            Text(category.rawValue).tag(category)
                        }
                    }
                    .pickerStyle(.menu)
                    .accessibilityIdentifier("mobile-bot-reaction-picker-category")
                }
                .padding(.horizontal)
                .padding(.vertical, 10)

                Divider()

                if reactionPickerResults.isEmpty {
                    ContentUnavailableView.search(text: reactionPickerSearch)
                        .accessibilityIdentifier("mobile-bot-reaction-picker-empty")
                } else {
                    ScrollView {
                        LazyVGrid(
                            columns: Array(
                                repeating: GridItem(.flexible(minimum: 36), spacing: 8),
                                count: reactionPickerColumnCount
                            ),
                            spacing: 8
                        ) {
                            ForEach(Array(reactionPickerResults.enumerated()), id: \.element.id) { index, item in
                                let reacted = reactionPickerTarget?.myReactions.contains(item.emoji) == true
                                Button {
                                    submitReactionPicker(emoji: item.emoji, recentCatalogId: item.id)
                                } label: {
                                    ZStack(alignment: .topTrailing) {
                                        Text(item.emoji)
                                            .font(.title2)
                                            .frame(maxWidth: .infinity, minHeight: 44)
                                        if reacted {
                                            Image(systemName: "checkmark.circle.fill")
                                                .font(.caption)
                                                .symbolRenderingMode(.hierarchical)
                                                .accessibilityHidden(true)
                                        }
                                    }
                                    .padding(4)
                                    .background(
                                        reacted ? Color.accentColor.opacity(0.14) : Color.clear,
                                        in: RoundedRectangle(cornerRadius: 10)
                                    )
                                }
                                .buttonStyle(.plain)
                                .focused($reactionPickerFocusedId, equals: item.id)
                                .accessibilityLabel(
                                    mobileReactionPickerAccessibilityLabel(
                                        item,
                                        reactedByCurrentUser: reacted
                                    )
                                )
                                .accessibilityValue(reacted ? "Selected" : "Not selected")
                                .accessibilityHint(
                                    reacted
                                        ? "Activate to remove this reaction."
                                        : "Activate to add this reaction."
                                )
                                .accessibilityIdentifier(
                                    "mobile-bot-reaction-picker-item-\(index)"
                                )
                            }
                        }
                        .padding()
                        .onKeyPress(phases: .down) { press in
                            guard reactionPickerFocusedId != nil else { return .ignored }
                            let move: MobileReactionPickerMove?
                            switch press.key {
                            case .leftArrow:
                                move = .left
                            case .rightArrow:
                                move = .right
                            case .upArrow:
                                move = press.modifiers.contains(.command) ? .first : .up
                            case .downArrow:
                                move = press.modifiers.contains(.command) ? .last : .down
                            default:
                                move = nil
                            }
                            guard let move else { return .ignored }
                            moveReactionPickerFocus(move)
                            return .handled
                        }
                    }
                    .accessibilityIdentifier("mobile-bot-reaction-picker-catalog")
                }

                Divider()

                HStack(spacing: 8) {
                    TextField("Custom emoji", text: $reactionPickerDraft)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .accessibilityIdentifier("mobile-bot-reaction-picker-input")
                    Button("Add") { submitReactionPicker() }
                        .disabled(normalizeMobileReactionInput(reactionPickerDraft) == nil)
                        .accessibilityIdentifier("mobile-bot-reaction-picker-custom-add")
                }
                .padding(.horizontal)
                .padding(.vertical, 10)

                Text("Hardware keyboard: use arrow keys in the grid; ⌘↑ and ⌘↓ are the native first/last equivalents for Home/End.")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .padding(.horizontal)
                    .padding(.bottom, 8)
                    .accessibilityIdentifier("mobile-bot-reaction-picker-keyboard-help")
            }
            .navigationTitle("React")
            .searchable(
                text: $reactionPickerSearch,
                placement: .navigationBarDrawer(displayMode: .always),
                prompt: "Search reactions"
            )
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { reactionPickerPresented = false }
                }
            }
            .onAppear {
                normalizeReactionPickerFocus()
            }
            .onChange(of: reactionPickerSearch) { _, _ in
                normalizeReactionPickerFocus()
            }
            .onChange(of: reactionPickerCategory) { _, _ in
                normalizeReactionPickerFocus()
            }
        }
        .presentationDetents([.medium, .large])
        .accessibilityIdentifier("mobile-bot-reaction-picker")
    }

    @MainActor
    private func normalizeReactionPickerFocus() {
        let results = reactionPickerResults
        guard !results.isEmpty else {
            reactionPickerFocusedId = nil
            return
        }
        if let current = reactionPickerFocusedId,
           results.contains(where: { $0.id == current })
        {
            return
        }
        reactionPickerFocusedId = results[0].id
    }

    @MainActor
    private func moveReactionPickerFocus(_ move: MobileReactionPickerMove) {
        let results = reactionPickerResults
        let currentIndex = reactionPickerFocusedId.flatMap { focusedId in
            results.firstIndex(where: { $0.id == focusedId })
        }
        guard let next = mobileReactionPickerNextIndex(
            current: currentIndex,
            count: results.count,
            columns: reactionPickerColumnCount,
            move: move
        ) else {
            reactionPickerFocusedId = nil
            return
        }
        reactionPickerFocusedId = results[next].id
    }

    @MainActor
    private func invalidateReactionScope() {
        reactionGeneration &+= 1
        reactionPickerPresented = false
        reactionPickerTargetId = nil
        reactionPickerDraft = ""
        reactionPickerSearch = ""
        reactionPickerCategory = .all
        reactionPickerFocusedId = nil
    }

    @MainActor
    private func openReactionPicker(_ entry: MobileChatMessage) {
        guard isMobileReactionActionable(entry) else { return }
        reactionPickerTargetId = entry.id
        reactionPickerDraft = ""
        reactionPickerSearch = ""
        reactionPickerCategory = .all
        reactionPickerFocusedId = nil
        reactionPickerPresented = true
    }

    @MainActor
    private func submitReactionPicker(
        emoji rawEmoji: String? = nil,
        recentCatalogId: String? = nil
    ) {
        guard let targetId = reactionPickerTargetId,
              let emoji = normalizeMobileReactionInput(rawEmoji ?? reactionPickerDraft),
              let entry = entries.first(where: { $0.id == targetId }),
              isMobileReactionActionable(entry)
        else { return }
        if let recentCatalogId {
            reactionPickerRecentIds.removeAll { $0 == recentCatalogId }
            reactionPickerRecentIds.insert(recentCatalogId, at: 0)
            if reactionPickerRecentIds.count > 24 {
                reactionPickerRecentIds.removeLast(reactionPickerRecentIds.count - 24)
            }
        }
        reactionPickerPresented = false
        reactionPickerTargetId = nil
        reactionPickerDraft = ""
        reactionPickerSearch = ""
        reactionPickerCategory = .all
        reactionPickerFocusedId = nil
        toggleReaction(entry, emoji: emoji)
    }

    @MainActor
    private func toggleReaction(_ entry: MobileChatMessage, emoji rawEmoji: String) {
        guard let emoji = normalizeMobileReactionInput(rawEmoji),
              let index = entries.firstIndex(where: { $0.id == entry.id }),
              isMobileReactionActionable(entries[index]),
              let entryId = entries[index].canonicalMessageId
        else { return }
        let had = entries[index].myReactions.contains(emoji)
        if had {
            entries[index].reactions.removeAll { $0.emoji == emoji && $0.by == "me" }
            entries[index].myReactions.remove(emoji)
        } else {
            entries[index].reactions.append(.init(emoji: emoji, by: "me"))
            entries[index].myReactions.insert(emoji)
        }

        reactionGeneration &+= 1
        let fence = MobileReactionRequestFence(
            accountKey: model.settingsNoticeAccountKey,
            agentId: bot.id,
            generation: reactionGeneration
        )
        Task { @MainActor in
            do {
                let response = try await bridge.request(
                    method: "reactToMessage",
                    params: [
                        "entryId": entryId,
                        "emoji": emoji,
                        "agentId": fence.agentId,
                    ]
                )
                guard fence.accepts(
                    accountKey: model.settingsNoticeAccountKey,
                    agentId: bot.id,
                    generation: reactionGeneration
                ),
                      let object = response.value as? [String: Any],
                      object["applied"] as? Bool == true,
                      let currentIndex = entries.firstIndex(where: { $0.id == entry.id })
                else { return }
                let canonical = projectMobileTranscriptReactions(object["reactions"])
                entries[currentIndex].reactions = canonical
                entries[currentIndex].myReactions = Set(
                    canonical.filter { $0.by == "me" }.map(\.emoji)
                )
            } catch {
                // Desktop keeps the optimistic value until the authoritative transcript
                // reconciles. Preserve that behavior rather than inventing a second error owner.
            }
        }
    }

    @ViewBuilder
    private func messageTextContent(_ entry: MobileChatMessage) -> some View {
        let seam = projectMobileMessageCardSeam(entry)
        if let url = seam.url {
            MobileLinkMetadataCard(
                url: url,
                model: model,
                healingRevision: reconnectGeneration &+ linkMetadataFocusRevision
            )
                .accessibilityIdentifier(Self.semanticId("mobile-bot-message-url-card-\(entry.id)"))
        } else if let projection = entry.sendMessageTextProjection {
            switch projection.presentation {
            case .urlCard(let rawURL):
                if URL(string: rawURL) != nil {
                    MobileLinkMetadataCard(
                url: rawURL,
                model: model,
                healingRevision: reconnectGeneration &+ linkMetadataFocusRevision
            )
                        .accessibilityIdentifier(Self.semanticId("mobile-bot-url-card-\(projection.id)"))
                } else if !projection.content.isEmpty {
                    Text(projection.content)
                        .font(.system(size: 16))
                }
            case .text:
                if !projection.content.isEmpty {
                    Text(projection.content)
                        .overlay(alignment: .trailing) {
                            if projection.streaming {
                                Text("▌").foregroundStyle(.black.opacity(0.65))
                            }
                        }
                        .font(.system(size: 16))
                }
            }
            ForEach(Array(projection.images.enumerated()), id: \.offset) { index, image in
                if let url = URL(string: image.url),
                   let scheme = url.scheme?.lowercased(),
                   scheme == "http" || scheme == "https"
                {
                    AsyncImage(url: url) { phase in
                        switch phase {
                        case .success(let imageView):
                            imageView
                                .resizable()
                                .scaledToFit()
                                .frame(maxHeight: 220)
                                .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
                        case .failure:
                            Label(image.alt ?? "Image unavailable", systemImage: "photo")
                                .font(.caption)
                        case .empty:
                            ProgressView().controlSize(.small)
                        @unknown default:
                            EmptyView()
                        }
                    }
                    .accessibilityLabel(image.alt ?? "Attached image")
                    .accessibilityIdentifier(Self.semanticId("mobile-bot-text-image-\(projection.id)-\(index)"))
                } else {
                    Label(image.alt ?? "Attached image", systemImage: "photo")
                        .font(.caption)
                        .accessibilityIdentifier(Self.semanticId("mobile-bot-text-image-\(projection.id)-\(index)"))
                }
            }
        } else if !entry.text.isEmpty {
            Text(entry.text)
                .overlay(alignment: .trailing) {
                    if entry.streaming {
                        Text("▌").foregroundStyle(.black.opacity(0.65))
                    }
                }
                .font(.system(size: 16))
        }
    }

    @ViewBuilder
    private func attachmentContent(_ entry: MobileChatMessage) -> some View {
        if !entry.optimisticAttachments.isEmpty {
            VStack(alignment: .leading, spacing: 5) {
                ForEach(Array(entry.optimisticAttachments.enumerated()), id: \.offset) { _, attachment in
                    HStack(spacing: 6) {
                        Image(systemName: "paperclip")
                        Text(attachment.name)
                            .font(.caption.weight(.medium))
                            .lineLimit(1)
                        Text(ByteCountFormatter.string(
                            fromByteCount: Int64(attachment.sizeBytes),
                            countStyle: .file
                        ))
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                    }
                }
            }
            .accessibilityIdentifier(Self.semanticId("mobile-bot-optimistic-attachments-\(entry.id)"))
        } else if let attachment = entry.attachmentProjection {
            switch attachment.kind {
            case .box:
                VStack(alignment: .leading, spacing: 4) {
                    Label(
                        attachment.instruction ?? attachment.request ?? "Computer attachment",
                        systemImage: "desktopcomputer"
                    )
                    .font(.caption.weight(.medium))
                    if attachment.screenshotDataURL != nil {
                        Text("Computer snapshot attached")
                            .font(.caption2)
                            .foregroundStyle(.secondary)
                    }
                }
                .accessibilityIdentifier(Self.semanticId("mobile-bot-attachment-box-\(attachment.id)"))
            case .legacyLink:
                MobileLinkMetadataCard(
                url: attachment.url,
                model: model,
                healingRevision: reconnectGeneration &+ linkMetadataFocusRevision
            )
                    .accessibilityIdentifier(Self.semanticId("mobile-bot-attachment-\(attachment.id)"))
            case .media:
                MobileTranscriptMediaAttachmentView(
                    rawURL: attachment.url,
                    alt: attachment.alt ?? attachment.name
                )
                .accessibilityIdentifier(Self.semanticId("mobile-bot-attachment-\(attachment.id)"))
            case .file:
                let label = attachment.name ?? attachment.alt ?? "Open attachment"
                if let destination = attachmentDestinationURL(attachment.url) {
                    Link(destination: destination) {
                        Label(label, systemImage: "paperclip")
                            .font(.caption.weight(.medium))
                    }
                    .accessibilityIdentifier(Self.semanticId("mobile-bot-attachment-\(attachment.id)"))
                } else {
                    Label(label, systemImage: "paperclip")
                        .font(.caption.weight(.medium))
                        .accessibilityIdentifier(Self.semanticId("mobile-bot-attachment-\(attachment.id)"))
                }
            }
        } else if let rawURL = entry.attachmentURL,
                  let destination = attachmentDestinationURL(rawURL)
        {
            Link(destination: destination) {
                Label(
                    entry.attachmentFileName ?? entry.attachmentAlt ?? "Open attachment",
                    systemImage: "paperclip"
                )
                .font(.caption.weight(.medium))
            }
        }
    }

    private func attachmentDestinationURL(_ raw: String) -> URL? {
        let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return nil }
        if let url = URL(string: trimmed), url.scheme != nil {
            return url
        }
        if trimmed.hasPrefix("/") {
            return URL(fileURLWithPath: trimmed)
        }
        return nil
    }

    @MainActor
    private func startVoiceInput() async {
        guard !busy, !transcribingVoice, !voiceRecorder.isRecording else { return }
        voiceInputGeneration += 1
        errorText = nil
        await voiceRecorder.start()
        if let recorderError = voiceRecorder.errorMessage {
            errorText = recorderError
        }
    }

    @MainActor
    private func finishVoiceInput() async {
        guard !busy, !transcribingVoice, let recording = voiceRecorder.stop() else { return }
        let generation = voiceInputGeneration
        let agentId = bot.id
        let accountKey = model.settingsNoticeAccountKey
        transcribingVoice = true
        defer {
            transcribingVoice = false
            try? FileManager.default.removeItem(at: recording.url)
        }
        do {
            let text = try await voiceTranscriber.transcribe(fileURL: recording.url)
            guard generation == voiceInputGeneration,
                  agentId == bot.id,
                  accountKey == model.settingsNoticeAccountKey
            else { return }
            draft = mergeMobileComposerVoiceTranscript(existing: draft, transcript: text)
            errorText = nil
        } catch is CancellationError {
            return
        } catch {
            guard generation == voiceInputGeneration,
                  agentId == bot.id,
                  accountKey == model.settingsNoticeAccountKey
            else { return }
            errorText = error.localizedDescription
        }
    }

    @MainActor
    private func cancelVoiceInput() {
        voiceInputGeneration += 1
        voiceRecorder.cancel()
        voiceTranscriber.cancel()
        transcribingVoice = false
    }

    @MainActor
    private func invalidateComposerAttachmentStaging() {
        attachmentStageGeneration &+= 1
        stagingAttachments = false
        attachmentImporterPresented = false
    }

    @MainActor
    private func stageImportedComposerAttachments(
        _ result: Result<[URL], Error>
    ) async {
        let urls: [URL]
        do {
            urls = try result.get()
        } catch {
            errorText = error.localizedDescription
            return
        }

        let capacity = max(0, mobileComposerAttachmentLimit - composerAttachments.count)
        guard capacity > 0 else {
            errorText = "You can attach up to \(mobileComposerAttachmentLimit) files."
            return
        }
        let selected = Array(urls.prefix(capacity))
        let droppedForLimit = max(0, urls.count - selected.count)
        attachmentStageGeneration &+= 1
        let generation = attachmentStageGeneration
        let ownedAccount = model.settingsNoticeAccountKey
        let ownedAgent = bot.id
        stagingAttachments = true
        var stagingFailures: [MobileComposerStageFailure] = []
        errorText = droppedForLimit > 0
            ? "You can attach up to \(mobileComposerAttachmentLimit) files."
            : nil
        defer {
            if generation == attachmentStageGeneration {
                stagingAttachments = false
            }
        }

        for url in selected {
            guard generation == attachmentStageGeneration,
                  ownedAccount == model.settingsNoticeAccountKey,
                  ownedAgent == bot.id
            else { return }

            do {
                let prepared = try await Task.detached(priority: .userInitiated) {
                    let didAccess = url.startAccessingSecurityScopedResource()
                    defer {
                        if didAccess {
                            url.stopAccessingSecurityScopedResource()
                        }
                    }
                    let values = try url.resourceValues(
                        forKeys: [.fileSizeKey, .nameKey, .contentTypeKey]
                    )
                    let filename = mobileComposerStageFileName(
                        proposedName: values.name,
                        fallbackLastPathComponent: url.lastPathComponent,
                        mimeType: values.contentType?.preferredMIMEType
                    )
                    let limit = AttachmentLimits.attachmentByteLimit(forName: filename)
                    if let fileSize = values.fileSize, fileSize > limit {
                        throw AttachmentTooLargeError(limitBytes: limit)
                    }
                    let data = try Data(contentsOf: url, options: [.mappedIfSafe])
                    if data.isEmpty {
                        throw NSError(
                            domain: "Fabushi.MobileComposer",
                            code: 1,
                            userInfo: [
                                NSLocalizedDescriptionKey: "\"\(filename)\" is empty, so it wasn't attached.",
                                "fabushiAttachmentFilename": filename,
                                "fabushiAttachmentFailureReason": "empty",
                            ]
                        )
                    }
                    if data.count > limit {
                        throw AttachmentTooLargeError(limitBytes: limit)
                    }
                    let hash = SHA256.hash(data: data)
                        .map { String(format: "%02x", $0) }
                        .joined()
                    return (
                        name: filename,
                        mimeType: values.contentType?.preferredMIMEType,
                        sizeBytes: data.count,
                        bytesBase64: data.base64EncodedString(),
                        hash: hash
                    )
                }.value

                let requestId = "ios-composer-attachment-\(UUID().uuidString.lowercased())"
                var uploadCommand: [String: Any] = [
                    "type": "attachment.upload",
                    "requestId": requestId,
                    "agentId": ownedAgent,
                    "filename": prepared.name,
                    "bytesBase64": prepared.bytesBase64,
                ]
                if let mimeType = prepared.mimeType, !mimeType.isEmpty {
                    uploadCommand["mimeType"] = mimeType
                }
                _ = try await bridge.request(
                    method: "feature.execute",
                    params: ["command": uploadCommand]
                )
                let stored = try await bridge.receiveFeatureEvent(
                    deadlineMilliseconds: 15_000
                ) { event in
                    guard event["type"] as? String == "attachment.stored",
                          let attachment = event["attachment"] as? [String: Any]
                    else { return false }
                    return attachment["id"] as? String == prepared.hash
                        && attachment["agentId"] as? String == ownedAgent
                }

                guard generation == attachmentStageGeneration,
                      ownedAccount == model.settingsNoticeAccountKey,
                      ownedAgent == bot.id,
                      let event = stored.value as? [String: Any],
                      let attachment = event["attachment"] as? [String: Any],
                      let path = attachment["path"] as? String,
                      !path.isEmpty
                else { return }

                let sizeBytes = (attachment["sizeBytes"] as? NSNumber)?.intValue
                    ?? prepared.sizeBytes
                let mimeType = attachment["mimeType"] as? String ?? prepared.mimeType
                composerAttachments.append(.init(
                    id: prepared.hash,
                    name: (attachment["name"] as? String) ?? prepared.name,
                    path: path,
                    mimeType: mimeType,
                    sizeBytes: sizeBytes
                ))
            } catch is AttachmentTooLargeError {
                let filename = mobileComposerStageFileName(
                    proposedName: nil,
                    fallbackLastPathComponent: url.lastPathComponent,
                    mimeType: nil
                )
                stagingFailures.append(.init(name: filename, reason: .tooLarge))
            } catch {
                let nsError = error as NSError
                let filename = (nsError.userInfo["fabushiAttachmentFilename"] as? String)
                    ?? mobileComposerStageFileName(
                        proposedName: nil,
                        fallbackLastPathComponent: url.lastPathComponent,
                        mimeType: nil
                    )
                let reason: MobileComposerStageFailureReason =
                    (nsError.userInfo["fabushiAttachmentFailureReason"] as? String) == "empty"
                    ? .empty
                    : .failed
                stagingFailures.append(.init(name: filename, reason: reason))
            }
        }

        if let failureNotice = mobileComposerStageFailureNotice(stagingFailures) {
            errorText = droppedForLimit > 0
                ? "You can attach up to \(mobileComposerAttachmentLimit) files. \(failureNotice)"
                : failureNotice
        }
    }

    @MainActor
    private func loadInitialConversationTail() async {
        guard bot.miniAppId == nil,
              let conversationId = bot.conversationId?.trimmingCharacters(in: .whitespacesAndNewlines),
              !conversationId.isEmpty
        else { return }

        transcriptBaselineGeneration &+= 1
        transcriptBaselineError = nil
        let generation = transcriptBaselineGeneration
        let ownedBotID = bot.id
        let identitiesAtRequestStart = Set(entries.map { $0.canonicalMessageId ?? $0.id })
        let requestId = "ios-mobile-conversation-tail-\(UUID().uuidString.lowercased())"

        do {
            _ = try await bridge.request(
                method: "feature.execute",
                params: [
                    "command": [
                        "type": "conversation.openTail",
                        "requestId": requestId,
                        "conversationId": conversationId,
                        "limit": 200,
                    ],
                ]
            )
            let result = try await bridge.receiveFeatureEvent(
                deadlineMilliseconds: 8_000
            ) { event in
                event["type"] as? String == "conversation.windowOpened"
                    && event["requestId"] as? String == requestId
                    && event["conversationId"] as? String == conversationId
            }
            guard
                generation == transcriptBaselineGeneration,
                bot.id == ownedBotID,
                let event = result.value as? [String: Any],
                let rows = event["messages"] as? [[String: Any]]
            else { return }

            var baseline: [MobileChatMessage] = []
            for row in rows {
                guard let projected = projectMobileConversationWindowEntries(row) else {
                    throw NSError(
                        domain: "Fabushi.MobileBotChat",
                        code: 41,
                        userInfo: [NSLocalizedDescriptionKey: "Host returned a malformed conversation baseline"]
                    )
                }
                baseline.append(contentsOf: projected)
            }
            entries = reconcileMobileConversationBaseline(
                baseline: baseline,
                current: entries,
                identitiesAtRequestStart: identitiesAtRequestStart
            )
            transcriptOlderExhausted = rows.count < 200
            transcriptPaginationError = nil
            transcriptBaselineError = nil
        } catch is CancellationError {
            return
        } catch {
            guard generation == transcriptBaselineGeneration, bot.id == ownedBotID else { return }
            transcriptBaselineError = error.localizedDescription
        }
    }

    @MainActor
    private func loadOlderConversationEntries() async {
        guard !transcriptOlderLoading,
              !transcriptOlderExhausted,
              bot.miniAppId == nil,
              let conversationId = bot.conversationId?.trimmingCharacters(in: .whitespacesAndNewlines),
              !conversationId.isEmpty,
              let beforeMessageId = entries.compactMap({ $0.canonicalMessageId }).first
        else { return }

        transcriptPaginationGeneration &+= 1
        let generation = transcriptPaginationGeneration
        let ownedAccount = model.settingsNoticeAccountKey
        let ownedBotId = bot.id
        let oldFirstId = mobileMainTranscriptEntries(entries).first?.id
        transcriptOlderLoading = true
        transcriptPaginationError = nil
        defer {
            if generation == transcriptPaginationGeneration,
               ownedAccount == model.settingsNoticeAccountKey,
               ownedBotId == bot.id {
                transcriptOlderLoading = false
            }
        }

        let requestId = "ios-mobile-conversation-older-\(UUID().uuidString.lowercased())"
        do {
            _ = try await bridge.request(
                method: "feature.execute",
                params: [
                    "command": [
                        "type": "conversation.openWindowed",
                        "requestId": requestId,
                        "conversationId": conversationId,
                        "beforeMessageId": beforeMessageId,
                        "limit": 200,
                    ],
                ]
            )
            let result = try await bridge.receiveFeatureEvent(
                deadlineMilliseconds: 8_000
            ) { event in
                event["type"] as? String == "conversation.windowOpened"
                    && event["requestId"] as? String == requestId
                    && event["conversationId"] as? String == conversationId
            }
            guard generation == transcriptPaginationGeneration,
                  ownedAccount == model.settingsNoticeAccountKey,
                  ownedBotId == bot.id,
                  let event = result.value as? [String: Any],
                  let rows = event["messages"] as? [[String: Any]]
            else { return }

            var older: [MobileChatMessage] = []
            for row in rows {
                guard let projected = projectMobileConversationWindowEntries(row) else {
                    throw NSError(
                        domain: "Fabushi.MobileBotChat",
                        code: 42,
                        userInfo: [NSLocalizedDescriptionKey: "Host returned a malformed older conversation window"]
                    )
                }
                older.append(contentsOf: projected)
            }
            if !older.isEmpty {
                transcriptPrependAnchorId = oldFirstId
                entries = mergeMobileConversationOlderPage(older: older, current: entries)
            }
            transcriptOlderExhausted = rows.count < 200
            transcriptPaginationError = nil
        } catch is CancellationError {
            return
        } catch {
            guard generation == transcriptPaginationGeneration,
                  ownedAccount == model.settingsNoticeAccountKey,
                  ownedBotId == bot.id
            else { return }
            transcriptPaginationError = error.localizedDescription
        }
    }

    @MainActor
    private func resetAcknowledgementScope() {
        entries.removeAll { $0.optimisticDeliveryPhase != nil }
        activeOperationId = nil
        busy = false
    }

    @MainActor
    private func appendOptimisticUserMessage(
        requestId: String,
        text: String,
        attachments: [MobileComposerAttachment],
        mcpReferences: [MobileComposerMcpReference],
        prReferences: [MobileComposerPrReference],
        richText: String?,
        replyTarget: String?,
        sendAsFork: Bool,
        priorNonces: [String] = []
    ) {
        var row = MobileChatMessage(
            id: requestId,
            role: .user,
            text: text,
            canonicalMessageId: requestId,
            replyToMessageId: replyTarget,
            branched: sendAsFork,
            optimisticDeliveryPhase: .pending
        )
        row.optimisticAccountKey = model.settingsNoticeAccountKey
        row.optimisticAgentId = bot.id
        row.optimisticNonce = requestId
        row.optimisticPriorNonces = priorNonces
        row.optimisticAttachments = attachments
        row.optimisticMcpReferences = mcpReferences
        row.optimisticPrReferences = prReferences
        row.richText = richText
        entries.append(row)
    }

    @MainActor
    private func dispatchOptimisticUserMessage(
        requestId: String,
        text: String,
        attachments: [MobileComposerAttachment],
        mcpReferences: [MobileComposerMcpReference],
        richText: String?,
        replyTarget: String?,
        sendAsFork: Bool
    ) async {
        let ownedAccountKey = model.settingsNoticeAccountKey
        let ownedBotId = bot.id
        guard let startIndex = entries.firstIndex(where: {
            $0.id == requestId
                && $0.optimisticAccountKey == ownedAccountKey
                && $0.optimisticAgentId == ownedBotId
                && $0.optimisticNonce == requestId
        }) else { return }
        entries[startIndex].optimisticDeliveryPhase = .dispatching
        entries[startIndex].optimisticDeliveryError = nil

        if let miniAppId = bot.miniAppId {
            guard attachments.isEmpty else {
                let message = "Attachments aren't supported by this Mini App chat."
                entries[startIndex].optimisticDeliveryPhase = .failed
                entries[startIndex].optimisticDeliveryError = message
                errorText = message
                busy = false
                return
            }
            let succeeded = await sendMiniApp(
                pluginId: miniAppId,
                text: text,
                operationId: requestId
            )
            guard model.settingsNoticeAccountKey == ownedAccountKey,
                  bot.id == ownedBotId,
                  let index = entries.firstIndex(where: {
                      $0.id == requestId && $0.optimisticNonce == requestId
                  })
            else { return }
            entries[index].optimisticDeliveryPhase = succeeded ? nil : .failed
            entries[index].optimisticDeliveryError = succeeded ? nil : errorText
            if succeeded {
                clearComposerRecovery(requestId: requestId)
            }
            activeOperationId = nil
            busy = false
            return
        }

        do {
            var command: [String: Any] = [
                "type": "chat.send",
                "requestId": requestId,
                "text": text,
                "agentId": ownedBotId,
                "mode": "agent",
                "isFork": sendAsFork,
            ]
            if !attachments.isEmpty {
                command["attachments"] = attachments.map(mobileComposerAttachmentCommandPayload)
            }
            if !mcpReferences.isEmpty {
                command["mcpReferences"] = mcpReferences.map { reference in
                    var row: [String: Any] = [
                        "id": reference.workflowReferenceID,
                        "serverId": reference.serverId,
                        "serverIdentifier": reference.serverIdentifier,
                        "accountKey": reference.accountKey,
                        "label": reference.label,
                        "status": reference.status,
                    ]
                    if let iconURL = reference.iconURL, !iconURL.isEmpty {
                        row["iconUrl"] = iconURL
                    }
                    return row
                }
            }
            if let richText, !richText.isEmpty { command["richText"] = richText }
            if let replyTarget { command["replyToMessageId"] = replyTarget }
            let result = try await bridge.request(
                method: "feature.execute",
                params: ["command": command]
            )
            guard model.settingsNoticeAccountKey == ownedAccountKey,
                  bot.id == ownedBotId,
                  let index = entries.firstIndex(where: {
                      $0.id == requestId
                          && $0.optimisticAccountKey == ownedAccountKey
                          && $0.optimisticAgentId == ownedBotId
                          && $0.optimisticNonce == requestId
                  })
            else { return }

            let accepted = result.value as? [String: Any]
            let operationId = accepted?["operationId"] as? String ?? requestId
            entries[index].optimisticDeliveryPhase = .acceptedAwaitingEcho
            entries[index].optimisticDeliveryError = nil
            clearComposerRecovery(requestId: requestId)
            activeOperationId = operationId
            entries.append(MobileChatMessage(
                id: "thinking:\(operationId)",
                role: .assistant,
                text: "",
                kind: .thinking,
                operationId: operationId,
                actionTitle: "Thinking",
                actionStatus: "running"
            ))
            await pump(operationId: operationId)
        } catch {
            guard model.settingsNoticeAccountKey == ownedAccountKey,
                  bot.id == ownedBotId
            else { return }
            let message = error.localizedDescription
            if let index = entries.firstIndex(where: {
                $0.id == requestId
                    && $0.optimisticAccountKey == ownedAccountKey
                    && $0.optimisticAgentId == ownedBotId
                    && $0.optimisticNonce == requestId
            }) {
                entries[index].optimisticDeliveryPhase = .failed
                entries[index].optimisticDeliveryError = message
            }
            errorText = message
        }
        guard model.settingsNoticeAccountKey == ownedAccountKey, bot.id == ownedBotId else { return }
        activeOperationId = nil
        busy = false
    }

    @MainActor
    private func send() async {
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        let attachments = composerAttachments
        guard mobileComposerHasPayload(text: text, attachments: attachments),
              !busy,
              !stagingAttachments,
              !voiceRecorder.isRecording,
              !transcribingVoice
        else { return }
        let mcpReferences = pruneMobileComposerMcpReferences(
            draft: text,
            references: composerMcpReferences
        )
        let prReferences = pruneMobileComposerPrReferences(
            draft: text,
            references: composerPrReferences
        )
        if bot.miniAppId != nil && (!attachments.isEmpty || !mcpReferences.isEmpty || !prReferences.isEmpty) {
            errorText = "Attachments and rich references aren't supported by this Mini App chat."
            return
        }
        let richText = mobileComposerRichText(
            draft: text,
            references: mcpReferences,
            prReferences: prReferences
        )
        let requestId = "ios-mobile-bot-chat-\(UUID().uuidString.lowercased())"
        composerRecovery = .init(
            requestId: requestId,
            text: text,
            attachments: attachments,
            mcpReferences: mcpReferences,
            prReferences: prReferences
        )
        draft = ""
        composerAttachments = []
        composerMcpReferences = []
        composerPrReferences = []
        busy = true
        errorText = nil
        let replyTarget = replyTargetId
        let sendAsFork = replyIsFork
        replyTargetId = nil
        replyIsFork = false
        appendOptimisticUserMessage(
            requestId: requestId,
            text: text,
            attachments: attachments,
            mcpReferences: mcpReferences,
            prReferences: prReferences,
            richText: richText,
            replyTarget: replyTarget,
            sendAsFork: sendAsFork
        )
        await dispatchOptimisticUserMessage(
            requestId: requestId,
            text: text,
            attachments: attachments,
            mcpReferences: mcpReferences,
            richText: richText,
            replyTarget: replyTarget,
            sendAsFork: sendAsFork
        )
    }

    @MainActor
    private func retryFailedSend(_ entry: MobileChatMessage) async {
        guard !busy,
              entry.role == .user,
              entry.optimisticDeliveryPhase == .failed,
              entry.optimisticAccountKey == model.settingsNoticeAccountKey,
              entry.optimisticAgentId == bot.id,
              let oldNonce = entry.optimisticNonce,
              !oldNonce.isEmpty
        else { return }

        let logicalNonce = mobileAcknowledgementLogicalNonce(oldNonce)
        let freshNonce = mobileAcknowledgementRetryNonce(
            logicalNonce: logicalNonce,
            retryToken: UUID().uuidString.lowercased()
        )
        let priorNonces = entry.optimisticPriorNonces + [oldNonce]
        entries.removeAll { $0.id == entry.id }
        let retryReferences = pruneMobileComposerMcpReferences(
            draft: entry.text,
            references: entry.optimisticMcpReferences
        )
        let retryPrReferences = pruneMobileComposerPrReferences(
            draft: entry.text,
            references: entry.optimisticPrReferences
        )
        composerRecovery = .init(
            requestId: freshNonce,
            text: entry.text,
            attachments: entry.optimisticAttachments,
            mcpReferences: retryReferences,
            prReferences: retryPrReferences
        )
        busy = true
        errorText = nil
        appendOptimisticUserMessage(
            requestId: freshNonce,
            text: entry.text,
            attachments: entry.optimisticAttachments,
            mcpReferences: retryReferences,
            prReferences: retryPrReferences,
            richText: mobileComposerRichText(
                draft: entry.text,
                references: retryReferences,
                prReferences: retryPrReferences
            ),
            replyTarget: entry.replyToMessageId,
            sendAsFork: entry.branched,
            priorNonces: priorNonces
        )
        await dispatchOptimisticUserMessage(
            requestId: freshNonce,
            text: entry.text,
            attachments: entry.optimisticAttachments,
            mcpReferences: retryReferences,
            richText: mobileComposerRichText(
                draft: entry.text,
                references: retryReferences,
                prReferences: retryPrReferences
            ),
            replyTarget: entry.replyToMessageId,
            sendAsFork: entry.branched
        )
    }

    @MainActor
    private func clearComposerRecovery(requestId: String) {
        guard composerRecovery?.requestId == requestId else { return }
        composerRecovery = nil
    }

    @MainActor
    private func sendMiniApp(pluginId: String, text: String, operationId: String) async -> Bool {
        activeOperationId = operationId
        entries.append(MobileChatMessage(
            id: "thinking:\(operationId)",
            role: .assistant,
            text: "",
            kind: .thinking,
            operationId: operationId,
            actionTitle: "正在通过 WebMCP 理解并执行",
            actionStatus: "running"
        ))
        do {
            let bridge = GlobalDharmaMiniAppBridge(bridge: bridge)
            let routed = try await bridge.routeInput(pluginId: pluginId, input: text)
            guard let execution = routed["execution"] as? [String: Any] else {
                removeThinking(operationId)
                let reply = (routed["message"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines)
                entries.append(MobileChatMessage(
                    id: "assistant:\(operationId)",
                    role: .assistant,
                    text: reply?.isEmpty == false ? reply! : "全球法布施没有把这条输入解析成可执行命令。",
                    operationId: operationId
                ))
                return true
            }
            let command = routed["command"] as? [String: Any]
            let slash = command?["slash"] as? String ?? ""
            if routed["requiresApproval"] as? Bool == true {
                removeThinking(operationId)
                entries.append(MobileChatMessage(
                    id: "assistant:\(operationId)",
                    role: .assistant,
                    text: "已通过统一 Mini App 路由解析\(slash.isEmpty ? "" : "为 \(slash)")。该 Tool 需要宿主明确批准；iOS 不会静默执行写入或破坏性调用。",
                    operationId: operationId
                ))
                return true
            }
            guard (execution["kind"] as? String) == "mcp-http",
                  let tool = execution["tool"] as? String,
                  !tool.isEmpty
            else {
                throw MahayanaCoordinator.CoordinatorError.requestFailed("iOS Mini App Bot only accepts governed mcp-http execution")
            }
            let arguments = routed["arguments"] as? [String: Any] ?? [:]
            let result = try await bridge.callOfficialMcpTool(pluginId: pluginId, name: tool, arguments: arguments)
            if pluginId == GlobalDharmaMiniAppBridge.globalDharmaId {
                model.recordGlobalDharmaExecution(tool: tool, result: result, source: "bot")
            }
            removeThinking(operationId)
            entries.append(MobileChatMessage(
                id: "assistant:\(operationId)",
                role: .assistant,
                text: GlobalDharmaMiniAppBridge.resultText(result),
                operationId: operationId
            ))
            return true
        } catch {
            removeThinking(operationId)
            errorText = error.localizedDescription
            entries.append(MobileChatMessage(
                id: "assistant:\(operationId):error",
                role: .assistant,
                text: "Mini App 调用失败：\(error.localizedDescription)",
                operationId: operationId
            ))
            return false
        }
    }

    @MainActor
    private func resolveBoxHandoff(_ entry: MobileChatMessage, resolution: String) async {
        guard entry.actionStatus == "pending",
              let handoffRequestId = entry.handoffRequestId,
              let handoffAgentId = entry.handoffAgentId
        else { return }
        do {
            let result = try await bridge.request(
                method: "feature.execute",
                params: [
                    "command": [
                        "type": "box.handoff.resolve",
                        "requestId": "ios-box-handoff-\(UUID().uuidString.lowercased())",
                        "handoffRequestId": handoffRequestId,
                        "agentId": handoffAgentId,
                        "resolution": resolution,
                    ],
                ]
            )
            if let index = entries.firstIndex(where: { $0.handoffRequestId == handoffRequestId }) {
                entries[index].actionStatus = resolution
            }
            guard let accepted = result.value as? [String: Any],
                  let operationId = accepted["operationId"] as? String,
                  !operationId.isEmpty
            else { return }
            busy = true
            activeOperationId = operationId
            entries.append(MobileChatMessage(
                id: "thinking:\(operationId)",
                role: .assistant,
                text: "",
                kind: .thinking,
                operationId: operationId,
                actionTitle: "Resuming after handoff",
                actionStatus: "running"
            ))
            await pump(operationId: operationId)
            activeOperationId = nil
            busy = false
        } catch {
            errorText = error.localizedDescription
        }
    }

    @MainActor
    private func resolveApproval(_ entry: MobileChatMessage, resolution: MobileAutoReviewResolution) async {
        guard let approvalId = entry.approvalId,
              let initialIndex = entries.firstIndex(where: { $0.approvalId == approvalId }),
              entries[initialIndex].actionStatus == "pending" else { return }

        let ownedBotId = bot.id
        let ownedGeneration = approvalGeneration
        entries[initialIndex].actionStatus = "submitting"

        if resolution == .always, let proposedRule = entry.approvalProposedRule {
            do {
                let currentResult = try await bridge.request(method: "getAutoReviewInstructions", params: [:])
                let current = try decodeMobileAutoReviewInstructions(currentResult.value)
                if let next = appendMobileAutoReviewAllowRule(current, proposedRule: proposedRule) {
                    _ = try await bridge.request(method: "setAutoReviewInstructions", params: [
                        "isEnabled": next.isEnabled,
                        "allowInstructions": next.allowInstructions,
                        "blockInstructions": next.blockInstructions,
                    ])
                }
            } catch {
                // Desktop parity: durable-rule failure degrades to one-time allow.
            }
        }

        do {
            _ = try await bridge.request(method: "feature.approval.resolve", params: [
                "resolution": ["approvalId": approvalId, "decision": resolution.hostDecision],
            ])
            guard approvalGeneration == ownedGeneration, bot.id == ownedBotId,
                  let index = entries.firstIndex(where: { $0.approvalId == approvalId }) else { return }
            entries[index].actionStatus = resolution.rawValue
        } catch {
            guard approvalGeneration == ownedGeneration, bot.id == ownedBotId,
                  let index = entries.firstIndex(where: { $0.approvalId == approvalId }) else { return }
            entries[index].actionStatus = error.localizedDescription.localizedCaseInsensitiveContains("unknown approval")
                ? "stale" : "failed"
        }
    }

    private func stop() async {
        guard bot.miniAppId == nil, let activeOperationId else { return }
        _ = try? await bridge.request(method: "feature.interrupt", params: ["operationId": activeOperationId])
    }

    @MainActor
    private func pump(operationId: String) async {
        for _ in 0..<1800 {
            if Task.isCancelled { return }
            do {
                let ownedHandoffRequestIDs = Set(
                    entries.compactMap(\.handoffRequestId)
                )
                let result = try await bridge.receiveFeatureEvent(
                    deadlineMilliseconds: 450_000
                ) { event in
                    guard let type = event["type"] as? String else { return false }
                    if type == "box.handoff.resolved" {
                        guard let requestID = event["requestId"] as? String else { return false }
                        return ownedHandoffRequestIDs.contains(requestID)
                    }
                    if type == "approval.requested" {
                        return event["operationId"] as? String == operationId
                    }
                    if type == "host.transport" {
                        guard event["channel"] as? String == "transcript.reaction",
                              let payload = event["payload"] as? [String: Any]
                        else { return false }
                        return payload["agentId"] as? String == bot.id
                    }
                    let acceptedTypes: Set<String> = [
                        "box.handoff.requested",
                        "approval.requested",
                        "chat.message",
                        "chat.delta",
                        "agent.step",
                        "transcript.card",
                        "operation.started",
                        "operation.completed",
                        "operation.interrupted",
                        "operation.failed",
                        "model.routed",
                    ]
                    guard acceptedTypes.contains(type) else { return false }
                    return (event["operationId"] as? String ?? operationId) == operationId
                }
                guard let event = result.value as? [String: Any],
                      let type = event["type"] as? String
                else { continue }
                let eventOperationId = event["operationId"] as? String ?? operationId
                switch type {
                case "host.transport":
                    _ = applyMobileTranscriptReactionEvent(
                        event,
                        agentId: bot.id,
                        messages: &entries
                    )
                case "approval.requested":
                    guard let row = projectMobileApprovalRequest(event, operationId: operationId) else { continue }
                    if let index = entries.firstIndex(where: { $0.approvalId == row.approvalId }) {
                        entries[index] = row
                    } else {
                        entries.append(row)
                    }
                case "box.handoff.requested":
                    guard eventOperationId == operationId,
                          let requestId = event["requestId"] as? String,
                          let agentId = event["agentId"] as? String
                    else { continue }
                    let row = MobileChatMessage(
                        id: "handoff:\(requestId)",
                        role: .assistant,
                        text: event["instruction"] as? String ?? "Please complete the requested step.",
                        kind: .handoff,
                        operationId: operationId,
                        actionTitle: "Waiting for user help",
                        actionDetail: [event["reason"] as? String, event["domain"] as? String].compactMap { $0 }.joined(separator: " · "),
                        actionStatus: "pending",
                        handoffRequestId: requestId,
                        handoffAgentId: agentId
                    )
                    if let index = entries.firstIndex(where: { $0.handoffRequestId == requestId }) { entries[index] = row } else { entries.append(row) }
                case "box.handoff.resolved":
                    guard let requestId = event["requestId"] as? String else { continue }
                    if let index = entries.firstIndex(where: { $0.handoffRequestId == requestId }) {
                        entries[index].actionStatus = event["resolution"] as? String ?? "completed"
                    }
                case "chat.message":
                    if event["role"] as? String == "user" {
                        _ = applyMobileOptimisticUserEcho(
                            event,
                            accountKey: model.settingsNoticeAccountKey,
                            agentId: bot.id,
                            messages: &entries
                        )
                        continue
                    }
                    guard isMobileBotVisibleAssistantCompletion(event, operationId: operationId) else { continue }
                    removeThinking(operationId)
                    let eventText = event["text"] as? String ?? ""
                    let generatedAttachment = event["attachment"] as? [String: Any]
                    if eventText.isEmpty, generatedAttachment != nil, !entries.contains(where: { $0.kind == .message && $0.role == .assistant && $0.operationId == operationId }) {
                        entries.append(MobileChatMessage(id: "assistant:\(operationId)", role: .assistant, text: "", operationId: operationId))
                    } else {
                        upsertAssistant(operationId, text: eventText, append: false, streaming: false)
                    }
                    if let index = entries.lastIndex(where: { $0.kind == .message && $0.role == .assistant && $0.operationId == operationId }) {
                        let canonicalMessageId = event["messageId"] as? String
                        let attachmentBatchId = event["attachmentBatchId"] as? String
                        entries[index].canonicalMessageId = canonicalMessageId
                        entries[index].replyToMessageId = event["replyToMessageId"] as? String
                        entries[index].attachmentBatchId = attachmentBatchId
                        if let rawAttachment = event["attachment"] as? [String: Any],
                           let attachment = projectMobileChatMessageAttachment(
                                id: canonicalMessageId ?? "assistant:\(operationId)",
                                raw: rawAttachment,
                                batchId: attachmentBatchId,
                                timestampMs: event["timestampMs"]
                           )
                        {
                            entries[index].attachmentProjection = attachment
                            entries[index].attachmentURL = attachment.url
                            entries[index].attachmentFileName = attachment.name
                            entries[index].attachmentAlt = attachment.alt
                        } else {
                            entries[index].attachmentProjection = nil
                            entries[index].attachmentURL = nil
                            entries[index].attachmentFileName = nil
                            entries[index].attachmentAlt = nil
                        }
                        entries[index].branched = event["branched"] as? Bool ?? false
                    }
                case "chat.delta":
                    removeThinking(operationId)
                    upsertAssistant(operationId, text: event["delta"] as? String ?? "", append: true, streaming: true)
                case "agent.step":
                    let title = event["title"] as? String ?? "Working"
                    let detail = event["detail"] as? String
                    let status = event["status"] as? String ?? "completed"
                    if let rawToolCallId = event["toolCallId"] as? String {
                        let toolCallId = rawToolCallId.trimmingCharacters(in: .whitespacesAndNewlines)
                        if !toolCallId.isEmpty {
                            let id = "tool-call:\(toolCallId)"
                            var row = MobileChatMessage(
                                id: id,
                                role: .assistant,
                                text: "",
                                kind: .toolCall,
                                operationId: operationId,
                                actionTitle: title,
                                actionDetail: detail,
                                actionStatus: status
                            )
                            row.toolCallId = toolCallId
                            row.toolName = title
                            row.toolStatus = status
                            row.toolSummary = detail
                            if let index = entries.firstIndex(where: { $0.id == id }) {
                                entries[index] = row
                            } else {
                                entries.append(row)
                            }
                            continue
                        }
                    }
                    let id = "action:\(operationId):\((event["stepId"] as? String) ?? UUID().uuidString)"
                    let row = MobileChatMessage(
                        id: id,
                        role: .assistant,
                        text: "",
                        kind: .action,
                        operationId: operationId,
                        actionTitle: title,
                        actionDetail: detail,
                        actionStatus: status
                    )
                    if let index = entries.firstIndex(where: { $0.id == id }) {
                        entries[index] = row
                    } else {
                        entries.append(row)
                    }
                case "model.routed":
                    let id = "action:\(operationId):model"
                    let provider = event["provider"] as? String ?? ""
                    let model = event["model"] as? String ?? ""
                    let row = MobileChatMessage(id: id, role: .assistant, text: "", kind: .action, operationId: operationId, actionTitle: "Model", actionDetail: [provider, model].filter { !$0.isEmpty }.joined(separator: " · "), actionStatus: "completed")
                    if let index = entries.firstIndex(where: { $0.id == id }) { entries[index] = row } else { entries.append(row) }
                case "transcript.card":
                    guard let row = projectMobileTranscriptCardWithFallback(
                        event: event,
                        operationId: eventOperationId
                    ) else { continue }
                    if let index = entries.firstIndex(where: { $0.id == row.id }) { entries[index] = row } else { entries.append(row) }
                case "operation.completed":
                    removeThinking(operationId)
                    finishAssistant(operationId, toolStatus: "done")
                    return
                case "operation.interrupted":
                    removeThinking(operationId)
                    finishAssistant(operationId, toolStatus: "aborted")
                    return
                case "operation.failed":
                    removeThinking(operationId)
                    finishAssistant(operationId, toolStatus: "failed")
                    errorText = event["message"] as? String ?? "Bot run failed"
                    return
                default:
                    break
                }
            } catch {
                errorText = error.localizedDescription
                return
            }
            try? await Task.sleep(for: .milliseconds(60))
        }
    }

    private func removeThinking(_ operationId: String) {
        entries.removeAll { $0.kind == .thinking && $0.operationId == operationId }
    }

    private func upsertAssistant(_ operationId: String, text: String, append: Bool, streaming: Bool) {
        guard !text.isEmpty else { return }
        if let index = entries.lastIndex(where: { $0.kind == .message && $0.role == .assistant && $0.operationId == operationId }) {
            entries[index].text = append ? entries[index].text + text : text
            entries[index].streaming = streaming
        } else {
            entries.append(MobileChatMessage(id: "assistant:\(operationId)", role: .assistant, text: text, operationId: operationId, streaming: streaming))
        }
    }

    private func finishAssistant(_ operationId: String, toolStatus: String? = nil) {
        for index in entries.indices where entries[index].operationId == operationId && entries[index].role == .assistant {
            entries[index].streaming = false
            if entries[index].kind == .toolCall,
               ["pending", "running"].contains(entries[index].toolStatus ?? ""),
               let toolStatus {
                entries[index].toolStatus = toolStatus
                entries[index].actionStatus = toolStatus
            }
        }
    }
}
