import Foundation
import SwiftUI

internal let mobileMermaidSourceByteCap = 64 * 1024
internal let mobileMermaidLineCap = 1_024
internal let mobileMermaidNodeCap = 256
internal let mobileMermaidEdgeCap = 512
internal let mobileMermaidLabelCharacterCap = 512
internal let mobileMermaidRenderCacheLimit = 64
internal let mobileMermaidMinZoom: CGFloat = 0.1
internal let mobileMermaidMaxZoom: CGFloat = 8
internal let mobileMermaidZoomStep: CGFloat = 1.4

internal enum MobileMermaidParseError: LocalizedError, Equatable, Sendable {
    case empty
    case tooLarge
    case tooComplex
    case unsafeDirective
    case unsupportedDiagram(String)
    case malformed(String)

    var errorDescription: String? {
        switch self {
        case .empty:
            return "The Mermaid diagram is empty."
        case .tooLarge:
            return "The Mermaid diagram exceeds the offline preview size limit."
        case .tooComplex:
            return "The Mermaid diagram exceeds the offline preview complexity limit."
        case .unsafeDirective:
            return "This Mermaid diagram contains a directive that is not allowed in the strict native renderer."
        case .unsupportedDiagram(let kind):
            return "This Mermaid diagram type is not yet supported by the native renderer: \(kind)."
        case .malformed(let detail):
            return "Couldn't parse this Mermaid diagram: \(detail)"
        }
    }
}

internal struct MobileMermaidNode: Identifiable, Equatable, Sendable {
    let id: String
    let label: String
}

internal struct MobileMermaidEdge: Identifiable, Equatable, Sendable {
    let id: Int
    let from: String
    let to: String
    let label: String?
}

internal enum MobileMermaidDirection: Equatable, Sendable {
    case topBottom
    case leftRight
    case bottomTop
    case rightLeft
}

internal struct MobileMermaidSequenceMessage: Identifiable, Equatable, Sendable {
    let id: Int
    let from: String
    let to: String
    let label: String
    let dashed: Bool
}

internal struct MobileMermaidPieSlice: Identifiable, Equatable, Sendable {
    let id: Int
    let label: String
    let value: Double
}

internal enum MobileMermaidDiagram: Equatable, Sendable {
    case graph(
        direction: MobileMermaidDirection,
        nodes: [MobileMermaidNode],
        edges: [MobileMermaidEdge]
    )
    case sequence(
        participants: [MobileMermaidNode],
        messages: [MobileMermaidSequenceMessage]
    )
    case pie(slices: [MobileMermaidPieSlice])
}

internal enum MobileAssistantRichSegmentKind: Equatable {
    case text
    case mermaid
}

internal struct MobileAssistantRichSegment: Identifiable, Equatable {
    let id: Int
    let kind: MobileAssistantRichSegmentKind
    let text: String
}

internal func splitMobileAssistantMermaid(_ text: String) -> [MobileAssistantRichSegment] {
    guard !text.isEmpty else {
        return [.init(id: 0, kind: .text, text: "")]
    }
    let fence = String(repeating: "\u{60}", count: 3)
    let escapedFence = NSRegularExpression.escapedPattern(for: fence)
    let pattern = escapedFence + #"[ \t]*mermaid[ \t]*\r?\n([\s\S]*?)\r?\n"# + escapedFence
    guard let regex = try? NSRegularExpression(pattern: pattern, options: [.caseInsensitive]) else {
        return [.init(id: 0, kind: .text, text: text)]
    }
    let matches = regex.matches(
        in: text,
        range: NSRange(text.startIndex..<text.endIndex, in: text)
    )
    guard !matches.isEmpty else {
        return [.init(id: 0, kind: .text, text: text)]
    }

    var output: [MobileAssistantRichSegment] = []
    var cursor = text.startIndex
    var nextID = 0
    for match in matches {
        guard let full = Range(match.range(at: 0), in: text),
              let source = Range(match.range(at: 1), in: text)
        else { continue }
        if cursor < full.lowerBound {
            output.append(.init(
                id: nextID,
                kind: .text,
                text: String(text[cursor..<full.lowerBound])
            ))
            nextID += 1
        }
        output.append(.init(
            id: nextID,
            kind: .mermaid,
            text: String(text[source])
        ))
        nextID += 1
        cursor = full.upperBound
    }
    if cursor < text.endIndex {
        output.append(.init(
            id: nextID,
            kind: .text,
            text: String(text[cursor...])
        ))
    }
    return output.isEmpty ? [.init(id: 0, kind: .text, text: text)] : output
}

internal func mobileMermaidRenderScopeID(
    accountKey: String,
    agentID: String,
    conversationID: String,
    entryID: String
) -> String {
    [accountKey, agentID, conversationID, entryID]
        .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
        .joined(separator: "\u{0}")
}

private let mobileMermaidArrowTokens = [
    "||--o{", "}o--||", "}|--|{", "}o..o{", "||..o{",
    "<|--", "*--", "o--", "..>", "-.->", "==>", "-->", "---"
]

private func mobileMermaidCleanLabel(_ raw: String) -> String {
    var value = raw.trimmingCharacters(in: .whitespacesAndNewlines)
    if value.count > mobileMermaidLabelCharacterCap {
        value = String(value.prefix(mobileMermaidLabelCharacterCap))
    }
    let pairs: [(Character, Character)] = [
        ("\"", "\""), ("'", "'"), ("[", "]"), ("(", ")"), ("{", "}"),
    ]
    var changed = true
    while changed, value.count >= 2 {
        changed = false
        for (left, right) in pairs where value.first == left && value.last == right {
            value.removeFirst()
            value.removeLast()
            value = value.trimmingCharacters(in: .whitespacesAndNewlines)
            changed = true
            break
        }
    }
    return value.replacingOccurrences(of: "<br/>", with: "\n")
        .replacingOccurrences(of: "<br>", with: "\n")
}

private func mobileMermaidNodeToken(_ raw: String) -> (id: String, label: String)? {
    let value = raw.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !value.isEmpty else { return nil }

    var idEnd = value.startIndex
    while idEnd < value.endIndex {
        let character = value[idEnd]
        if character.isLetter || character.isNumber || "_-.:".contains(character) {
            idEnd = value.index(after: idEnd)
        } else {
            break
        }
    }
    guard idEnd > value.startIndex else { return nil }
    let id = String(value[..<idEnd])
    var label = String(value[idEnd...]).trimmingCharacters(in: .whitespacesAndNewlines)
    if label.isEmpty { label = id }

    if label.hasPrefix("[[") && label.hasSuffix("]]") && label.count >= 4 {
        label = String(label.dropFirst(2).dropLast(2))
    } else if label.hasPrefix("((") && label.hasSuffix("))") && label.count >= 4 {
        label = String(label.dropFirst(2).dropLast(2))
    } else if label.hasPrefix("([") && label.hasSuffix("])") && label.count >= 4 {
        label = String(label.dropFirst(2).dropLast(2))
    }
    label = mobileMermaidCleanLabel(label)
    return (id, label.isEmpty ? id : label)
}

private func mobileMermaidEarliestArrow(in value: String) -> (range: Range<String.Index>, token: String)? {
    var best: (Range<String.Index>, String)?
    for token in mobileMermaidArrowTokens {
        guard let range = value.range(of: token) else { continue }
        if let current = best {
            if range.lowerBound < current.0.lowerBound
                || (range.lowerBound == current.0.lowerBound && token.count > current.1.count)
            {
                best = (range, token)
            }
        } else {
            best = (range, token)
        }
    }
    return best
}

private func mobileMermaidExtractEdgeLabelAndDestination(_ raw: String) -> (String?, String) {
    var value = raw.trimmingCharacters(in: .whitespacesAndNewlines)
    var label: String?
    if value.first == "|", let close = value.dropFirst().firstIndex(of: "|") {
        let start = value.index(after: value.startIndex)
        label = mobileMermaidCleanLabel(String(value[start..<close]))
        value = String(value[value.index(after: close)...])
            .trimmingCharacters(in: .whitespacesAndNewlines)
    }
    return (label, value)
}

private func mobileMermaidStateDeclaration(_ line: String) -> MobileMermaidNode? {
    let value = line.trimmingCharacters(in: .whitespacesAndNewlines)
    guard value.hasPrefix("state ") else { return nil }
    let body = String(value.dropFirst("state ".count))
        .trimmingCharacters(in: .whitespacesAndNewlines)
    if body.first == "\"", let closing = body.dropFirst().firstIndex(of: "\"") {
        let labelStart = body.index(after: body.startIndex)
        let label = String(body[labelStart..<closing])
        let remainder = String(body[body.index(after: closing)...])
            .trimmingCharacters(in: .whitespacesAndNewlines)
        guard remainder.hasPrefix("as ") else { return nil }
        let id = String(remainder.dropFirst(3))
            .trimmingCharacters(in: .whitespacesAndNewlines)
        guard !id.isEmpty else { return nil }
        return .init(id: id, label: mobileMermaidCleanLabel(label))
    }
    return mobileMermaidNodeToken(body).map { .init(id: $0.id, label: $0.label) }
}

private func mobileMermaidGraph(
    lines: [String],
    direction: MobileMermaidDirection,
    bodyStart: Int
) throws -> MobileMermaidDiagram {
    var nodesByID: [String: MobileMermaidNode] = [:]
    var nodeOrder: [String] = []
    var edges: [MobileMermaidEdge] = []

    func register(_ token: (id: String, label: String)) throws {
        if nodesByID[token.id] == nil {
            guard nodeOrder.count < mobileMermaidNodeCap else {
                throw MobileMermaidParseError.tooComplex
            }
            nodeOrder.append(token.id)
        }
        nodesByID[token.id] = .init(id: token.id, label: token.label)
    }

    for rawLine in lines.dropFirst(bodyStart) {
        var line = rawLine.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !line.isEmpty, !line.hasPrefix("%%") else { continue }
        if line == "end"
            || line.hasPrefix("subgraph ")
            || line.hasPrefix("direction ")
            || line.hasPrefix("classDef ")
            || line.hasPrefix("class ")
            || line.hasPrefix("style ")
            || line.hasPrefix("linkStyle ")
        {
            continue
        }
        if let state = mobileMermaidStateDeclaration(line) {
            try register((state.id, state.label))
            continue
        }

        guard let firstArrow = mobileMermaidEarliestArrow(in: line) else {
            if line.hasSuffix("{") || line == "}" { continue }
            if let token = mobileMermaidNodeToken(line) {
                try register(token)
            }
            continue
        }

        guard let source = mobileMermaidNodeToken(String(line[..<firstArrow.range.lowerBound])) else {
            throw MobileMermaidParseError.malformed("missing source node")
        }
        try register(source)
        var current = source
        line = String(line[firstArrow.range.upperBound...])

        while true {
            let extracted = mobileMermaidExtractEdgeLabelAndDestination(line)
            var remainder = extracted.1
            let nextArrow = mobileMermaidEarliestArrow(in: remainder)
            var destinationRaw = nextArrow.map { String(remainder[..<$0.range.lowerBound]) } ?? remainder
            var trailingLabel = extracted.0
            if nextArrow == nil, let colon = destinationRaw.firstIndex(of: ":") {
                let before = String(destinationRaw[..<colon])
                let after = String(destinationRaw[destinationRaw.index(after: colon)...])
                if mobileMermaidNodeToken(before) != nil {
                    destinationRaw = before
                    let clean = mobileMermaidCleanLabel(after)
                    if !clean.isEmpty { trailingLabel = clean }
                }
            }
            guard let destination = mobileMermaidNodeToken(destinationRaw) else {
                throw MobileMermaidParseError.malformed("missing destination node")
            }
            try register(destination)
            guard edges.count < mobileMermaidEdgeCap else {
                throw MobileMermaidParseError.tooComplex
            }
            edges.append(.init(
                id: edges.count,
                from: current.id,
                to: destination.id,
                label: trailingLabel?.isEmpty == true ? nil : trailingLabel
            ))
            current = destination
            guard let nextArrow else { break }
            remainder = String(remainder[nextArrow.range.upperBound...])
            line = remainder
        }
    }

    let nodes = nodeOrder.compactMap { nodesByID[$0] }
    guard !nodes.isEmpty else {
        throw MobileMermaidParseError.malformed("diagram contains no nodes")
    }
    return .graph(direction: direction, nodes: nodes, edges: edges)
}

private func mobileMermaidSequence(lines: [String]) throws -> MobileMermaidDiagram {
    var participantsByID: [String: MobileMermaidNode] = [:]
    var participantOrder: [String] = []
    var messages: [MobileMermaidSequenceMessage] = []

    func register(_ id: String, _ label: String? = nil) throws {
        let cleanID = id.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !cleanID.isEmpty else { throw MobileMermaidParseError.malformed("empty participant") }
        if participantsByID[cleanID] == nil {
            guard participantOrder.count < mobileMermaidNodeCap else {
                throw MobileMermaidParseError.tooComplex
            }
            participantOrder.append(cleanID)
        }
        participantsByID[cleanID] = .init(
            id: cleanID,
            label: mobileMermaidCleanLabel(label ?? cleanID)
        )
    }

    let arrows = ["-->>", "->>", "-->", "->", "--x", "-x", "--)", "-)"]
    for rawLine in lines.dropFirst() {
        let line = rawLine.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !line.isEmpty, !line.hasPrefix("%%") else { continue }
        if line.hasPrefix("participant ") || line.hasPrefix("actor ") {
            let prefix = line.hasPrefix("participant ") ? "participant " : "actor "
            let body = String(line.dropFirst(prefix.count))
            if let asRange = body.range(of: " as ") {
                try register(
                    String(body[..<asRange.lowerBound]),
                    String(body[asRange.upperBound...])
                )
            } else {
                try register(body)
            }
            continue
        }
        if ["activate ", "deactivate ", "Note ", "loop ", "alt ", "else", "opt ", "par ", "and ", "rect ", "critical ", "break ", "end", "autonumber"]
            .contains(where: { line == $0 || line.hasPrefix($0) })
        {
            continue
        }
        guard let match = arrows.compactMap({ arrow -> (String, Range<String.Index>)? in
            line.range(of: arrow).map { (arrow, $0) }
        }).min(by: { $0.1.lowerBound < $1.1.lowerBound }) else {
            continue
        }
        let from = String(line[..<match.1.lowerBound])
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let tail = String(line[match.1.upperBound...])
        let colon = tail.firstIndex(of: ":")
        let toPart: Substring = colon.map { tail[..<$0] } ?? Substring(tail)
        let to = String(toPart).trimmingCharacters(in: .whitespacesAndNewlines)
        let label = colon.map {
            mobileMermaidCleanLabel(String(tail[tail.index(after: $0)...]))
        } ?? ""
        try register(from)
        try register(to)
        guard messages.count < mobileMermaidEdgeCap else {
            throw MobileMermaidParseError.tooComplex
        }
        messages.append(.init(
            id: messages.count,
            from: from,
            to: to,
            label: label,
            dashed: match.0.hasPrefix("--")
        ))
    }

    let participants = participantOrder.compactMap { participantsByID[$0] }
    guard !participants.isEmpty else {
        throw MobileMermaidParseError.malformed("sequence diagram contains no participants")
    }
    return .sequence(participants: participants, messages: messages)
}

private func mobileMermaidPie(lines: [String]) throws -> MobileMermaidDiagram {
    var slices: [MobileMermaidPieSlice] = []
    for rawLine in lines.dropFirst() {
        let line = rawLine.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !line.isEmpty, !line.hasPrefix("%%"), !line.hasPrefix("title ") else { continue }
        guard let colon = line.lastIndex(of: ":") else { continue }
        let label = mobileMermaidCleanLabel(String(line[..<colon]))
        let rawValue = String(line[line.index(after: colon)...])
            .trimmingCharacters(in: .whitespacesAndNewlines)
        guard let value = Double(rawValue), value.isFinite, value >= 0 else {
            throw MobileMermaidParseError.malformed("pie slice has an invalid value")
        }
        guard slices.count < mobileMermaidNodeCap else {
            throw MobileMermaidParseError.tooComplex
        }
        slices.append(.init(id: slices.count, label: label, value: value))
    }
    guard !slices.isEmpty, slices.contains(where: { $0.value > 0 }) else {
        throw MobileMermaidParseError.malformed("pie diagram contains no positive slices")
    }
    return .pie(slices: slices)
}

internal func parseMobileMermaidDiagram(_ source: String) throws -> MobileMermaidDiagram {
    guard !source.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
        throw MobileMermaidParseError.empty
    }
    guard source.utf8.count <= mobileMermaidSourceByteCap else {
        throw MobileMermaidParseError.tooLarge
    }
    let lower = source.lowercased()
    let forbidden = ["%%{", "<script", "javascript:", "click ", " href", "target=_blank", "target=\"_blank"]
    if forbidden.contains(where: lower.contains) {
        throw MobileMermaidParseError.unsafeDirective
    }

    let lines = source.components(separatedBy: .newlines)
    guard lines.count <= mobileMermaidLineCap else {
        throw MobileMermaidParseError.tooComplex
    }
    guard let first = lines.first(where: {
        let value = $0.trimmingCharacters(in: .whitespacesAndNewlines)
        return !value.isEmpty && !value.hasPrefix("%%")
    })?.trimmingCharacters(in: .whitespacesAndNewlines)
    else {
        throw MobileMermaidParseError.empty
    }
    let firstLower = first.lowercased()

    if firstLower.hasPrefix("flowchart ") || firstLower.hasPrefix("graph ") {
        let token = first.split(whereSeparator: { $0.isWhitespace }).dropFirst().first
            .map { String($0).uppercased() } ?? "TD"
        let direction: MobileMermaidDirection
        switch token {
        case "LR": direction = .leftRight
        case "RL": direction = .rightLeft
        case "BT": direction = .bottomTop
        default: direction = .topBottom
        }
        return try mobileMermaidGraph(lines: lines, direction: direction, bodyStart: 1)
    }
    if firstLower == "statediagram" || firstLower == "statediagram-v2" {
        return try mobileMermaidGraph(lines: lines, direction: .topBottom, bodyStart: 1)
    }
    if firstLower == "classdiagram" || firstLower == "erdiagram" {
        return try mobileMermaidGraph(lines: lines, direction: .topBottom, bodyStart: 1)
    }
    if firstLower == "sequencediagram" {
        return try mobileMermaidSequence(lines: lines)
    }
    if firstLower == "pie" || firstLower.hasPrefix("pie ") {
        return try mobileMermaidPie(lines: lines)
    }

    let kind = first.split(whereSeparator: { $0.isWhitespace }).first.map(String.init) ?? first
    throw MobileMermaidParseError.unsupportedDiagram(kind)
}

internal enum MobileMermaidRenderResult: Equatable, Sendable {
    case ready(MobileMermaidDiagram)
    case failed(MobileMermaidParseError)
}

internal actor MobileMermaidRenderQueue {
    static let shared = MobileMermaidRenderQueue(capacity: mobileMermaidRenderCacheLimit)

    private let capacity: Int
    private var cache: [String: MobileMermaidRenderResult] = [:]
    private var order: [String] = []

    init(capacity: Int) {
        self.capacity = max(1, capacity)
    }

    func resolve(source: String, theme: String) -> MobileMermaidRenderResult {
        let key = theme + "\u{0}" + source
        if let cached = cache[key] {
            if let index = order.firstIndex(of: key) {
                order.remove(at: index)
            }
            order.append(key)
            return cached
        }

        let result: MobileMermaidRenderResult
        do {
            result = .ready(try parseMobileMermaidDiagram(source))
        } catch let error as MobileMermaidParseError {
            result = .failed(error)
        } catch {
            result = .failed(.malformed(error.localizedDescription))
        }

        cache[key] = result
        order.append(key)
        while order.count > capacity {
            let oldest = order.removeFirst()
            cache.removeValue(forKey: oldest)
        }
        return result
    }

    func entryCount() -> Int {
        cache.count
    }
}

private struct MobileMermaidGraphLayout {
    static let nodeWidth: CGFloat = 150
    static let nodeHeight: CGFloat = 54
    static let columnGap: CGFloat = 70
    static let rowGap: CGFloat = 58
    static let margin: CGFloat = 34

    let positions: [String: CGPoint]
    let size: CGSize

    init(nodes: [MobileMermaidNode], edges: [MobileMermaidEdge], direction: MobileMermaidDirection) {
        var indegree = Dictionary(uniqueKeysWithValues: nodes.map { ($0.id, 0) })
        var outgoing: [String: [String]] = [:]
        for edge in edges where edge.from != edge.to {
            indegree[edge.to, default: 0] += 1
            outgoing[edge.from, default: []].append(edge.to)
        }
        var queue = nodes.compactMap { indegree[$0.id] == 0 ? $0.id : nil }
        var levels = Dictionary(uniqueKeysWithValues: nodes.map { ($0.id, 0) })
        var consumed = Set<String>()
        var cursor = 0
        while cursor < queue.count {
            let id = queue[cursor]
            cursor += 1
            consumed.insert(id)
            for next in outgoing[id] ?? [] {
                levels[next] = max(levels[next] ?? 0, (levels[id] ?? 0) + 1)
                indegree[next, default: 0] -= 1
                if indegree[next] == 0 { queue.append(next) }
            }
        }
        if consumed.count != nodes.count {
            let columns = max(1, Int(ceil(sqrt(Double(max(1, nodes.count))))))
            for (index, node) in nodes.enumerated() {
                levels[node.id] = index / columns
            }
        }

        var groups: [Int: [String]] = [:]
        for node in nodes {
            groups[levels[node.id] ?? 0, default: []].append(node.id)
        }
        let maxLevel = groups.keys.max() ?? 0
        let maxCrossCount = max(1, groups.values.map(\.count).max() ?? 1)
        let primary = CGFloat(maxLevel + 1) * Self.nodeHeight
            + CGFloat(maxLevel) * Self.rowGap
            + Self.margin * 2
        let cross = CGFloat(maxCrossCount) * Self.nodeWidth
            + CGFloat(maxCrossCount - 1) * Self.columnGap
            + Self.margin * 2

        var projected: [String: CGPoint] = [:]
        for level in 0...maxLevel {
            let ids = groups[level] ?? []
            let rowWidth = CGFloat(ids.count) * Self.nodeWidth
                + CGFloat(max(0, ids.count - 1)) * Self.columnGap
            for (index, id) in ids.enumerated() {
                let crossPosition = Self.margin + (cross - Self.margin * 2 - rowWidth) / 2
                    + Self.nodeWidth / 2
                    + CGFloat(index) * (Self.nodeWidth + Self.columnGap)
                let primaryPosition = Self.margin + Self.nodeHeight / 2
                    + CGFloat(level) * (Self.nodeHeight + Self.rowGap)
                let point: CGPoint
                switch direction {
                case .topBottom:
                    point = .init(x: crossPosition, y: primaryPosition)
                case .bottomTop:
                    point = .init(x: crossPosition, y: primary - primaryPosition)
                case .leftRight:
                    point = .init(x: primaryPosition, y: crossPosition)
                case .rightLeft:
                    point = .init(x: primary - primaryPosition, y: crossPosition)
                }
                projected[id] = point
            }
        }
        positions = projected
        switch direction {
        case .topBottom, .bottomTop:
            size = .init(width: cross, height: primary)
        case .leftRight, .rightLeft:
            size = .init(width: primary, height: cross)
        }
    }
}

private struct MobileMermaidGraphView: View {
    let nodes: [MobileMermaidNode]
    let edges: [MobileMermaidEdge]
    let direction: MobileMermaidDirection

    var body: some View {
        let layout = MobileMermaidGraphLayout(nodes: nodes, edges: edges, direction: direction)
        ZStack {
            Canvas { context, _ in
                for edge in edges {
                    guard let start = layout.positions[edge.from],
                          let end = layout.positions[edge.to]
                    else { continue }
                    var path = Path()
                    path.move(to: start)
                    path.addLine(to: end)
                    context.stroke(path, with: .color(Color.secondary), lineWidth: 1.5)

                    let angle = atan2(end.y - start.y, end.x - start.x)
                    let arrowLength: CGFloat = 9
                    let left = CGPoint(
                        x: end.x - arrowLength * cos(angle - .pi / 6),
                        y: end.y - arrowLength * sin(angle - .pi / 6)
                    )
                    let right = CGPoint(
                        x: end.x - arrowLength * cos(angle + .pi / 6),
                        y: end.y - arrowLength * sin(angle + .pi / 6)
                    )
                    var arrow = Path()
                    arrow.move(to: left)
                    arrow.addLine(to: end)
                    arrow.addLine(to: right)
                    context.stroke(arrow, with: .color(Color.secondary), lineWidth: 1.5)

                    if let label = edge.label, !label.isEmpty {
                        context.draw(
                            Text(label).font(.caption2).foregroundStyle(.secondary),
                            at: .init(x: (start.x + end.x) / 2, y: (start.y + end.y) / 2 - 8)
                        )
                    }
                }
            }
            ForEach(nodes) { node in
                Text(node.label)
                    .font(.caption.weight(.semibold))
                    .multilineTextAlignment(.center)
                    .lineLimit(3)
                    .padding(.horizontal, 8)
                    .frame(
                        width: MobileMermaidGraphLayout.nodeWidth,
                        height: MobileMermaidGraphLayout.nodeHeight
                    )
                    .background(
                        Color(uiColor: .secondarySystemBackground),
                        in: RoundedRectangle(cornerRadius: 10, style: .continuous)
                    )
                    .overlay(
                        RoundedRectangle(cornerRadius: 10, style: .continuous)
                            .stroke(Color.secondary.opacity(0.45), lineWidth: 1)
                    )
                    .position(layout.positions[node.id] ?? .zero)
            }
        }
        .frame(width: layout.size.width, height: layout.size.height)
        .accessibilityElement(children: .combine)
        .accessibilityLabel("Mermaid diagram with \(nodes.count) nodes and \(edges.count) connections")
    }
}

private struct MobileMermaidSequenceView: View {
    let participants: [MobileMermaidNode]
    let messages: [MobileMermaidSequenceMessage]

    private let laneWidth: CGFloat = 145
    private let top: CGFloat = 52
    private let messageGap: CGFloat = 56

    var body: some View {
        let width = max(360, CGFloat(participants.count) * laneWidth + 40)
        let height = max(180, top + CGFloat(max(1, messages.count)) * messageGap + 48)
        let indexByID = Dictionary(uniqueKeysWithValues: participants.enumerated().map { ($0.element.id, $0.offset) })
        Canvas { context, _ in
            for (index, participant) in participants.enumerated() {
                let x = 20 + laneWidth / 2 + CGFloat(index) * laneWidth
                var lane = Path()
                lane.move(to: .init(x: x, y: top))
                lane.addLine(to: .init(x: x, y: height - 20))
                context.stroke(
                    lane,
                    with: .color(Color.secondary.opacity(0.5)),
                    style: .init(lineWidth: 1, dash: [5, 4])
                )
                context.draw(
                    Text(participant.label).font(.caption.weight(.semibold)),
                    at: .init(x: x, y: 22)
                )
            }
            for (index, message) in messages.enumerated() {
                guard let fromIndex = indexByID[message.from], let toIndex = indexByID[message.to] else { continue }
                let y = top + 30 + CGFloat(index) * messageGap
                let fromX = 20 + laneWidth / 2 + CGFloat(fromIndex) * laneWidth
                let toX = 20 + laneWidth / 2 + CGFloat(toIndex) * laneWidth
                var line = Path()
                line.move(to: .init(x: fromX, y: y))
                line.addLine(to: .init(x: toX, y: y))
                context.stroke(
                    line,
                    with: .color(Color.primary),
                    style: .init(lineWidth: 1.4, dash: message.dashed ? [5, 4] : [])
                )
                let direction: CGFloat = toX >= fromX ? 1 : -1
                var arrow = Path()
                arrow.move(to: .init(x: toX - 8 * direction, y: y - 5))
                arrow.addLine(to: .init(x: toX, y: y))
                arrow.addLine(to: .init(x: toX - 8 * direction, y: y + 5))
                context.stroke(arrow, with: .color(Color.primary), lineWidth: 1.4)
                if !message.label.isEmpty {
                    context.draw(
                        Text(message.label).font(.caption2),
                        at: .init(x: (fromX + toX) / 2, y: y - 13)
                    )
                }
            }
        }
        .frame(width: width, height: height)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Mermaid sequence diagram with \(participants.count) participants and \(messages.count) messages")
    }
}

private struct MobileMermaidPieView: View {
    let slices: [MobileMermaidPieSlice]

    var body: some View {
        HStack(alignment: .top, spacing: 18) {
            Canvas { context, size in
                let total = slices.reduce(0) { $0 + $1.value }
                let center = CGPoint(x: size.width / 2, y: size.height / 2)
                let radius = max(1, min(size.width, size.height) / 2 - 4)
                var start = -Double.pi / 2
                for (index, slice) in slices.enumerated() {
                    let fraction = slice.value / total
                    let end = start + fraction * Double.pi * 2
                    var path = Path()
                    path.move(to: center)
                    path.addArc(
                        center: center,
                        radius: radius,
                        startAngle: .radians(start),
                        endAngle: .radians(end),
                        clockwise: false
                    )
                    path.closeSubpath()
                    context.fill(
                        path,
                        with: .color(Color.accentColor.opacity(0.35 + 0.55 * Double((index % 5) + 1) / 5))
                    )
                    start = end
                }
            }
            .frame(width: 190, height: 190)

            VStack(alignment: .leading, spacing: 7) {
                ForEach(slices) { slice in
                    HStack(spacing: 6) {
                        Circle().fill(Color.accentColor.opacity(0.7)).frame(width: 8, height: 8)
                        Text("\(slice.label): \(slice.value.formatted())")
                            .font(.caption)
                    }
                }
            }
            .frame(maxWidth: 220, alignment: .leading)
        }
        .padding(8)
        .accessibilityElement(children: .combine)
        .accessibilityLabel("Mermaid pie diagram with \(slices.count) slices")
    }
}

private struct MobileMermaidDiagramSurface: View {
    let diagram: MobileMermaidDiagram

    @ViewBuilder
    var body: some View {
        switch diagram {
        case .graph(let direction, let nodes, let edges):
            MobileMermaidGraphView(nodes: nodes, edges: edges, direction: direction)
        case .sequence(let participants, let messages):
            MobileMermaidSequenceView(participants: participants, messages: messages)
        case .pie(let slices):
            MobileMermaidPieView(slices: slices)
        }
    }
}

internal func mobileMermaidNaturalSize(_ diagram: MobileMermaidDiagram) -> CGSize {
    switch diagram {
    case .graph(let direction, let nodes, let edges):
        return MobileMermaidGraphLayout(
            nodes: nodes,
            edges: edges,
            direction: direction
        ).size
    case .sequence(let participants, let messages):
        let laneWidth: CGFloat = 145
        let top: CGFloat = 52
        let messageGap: CGFloat = 56
        return CGSize(
            width: max(360, CGFloat(participants.count) * laneWidth + 40),
            height: max(180, top + CGFloat(max(1, messages.count)) * messageGap + 48)
        )
    case .pie:
        return CGSize(width: 444, height: 206)
    }
}

internal func mobileMermaidFitScale(
    diagram: CGSize,
    viewport: CGSize
) -> CGFloat {
    guard diagram.width > 0, diagram.height > 0,
          viewport.width > 0, viewport.height > 0
    else { return 1 }
    let raw = min(viewport.width / diagram.width, viewport.height / diagram.height)
    guard raw.isFinite, raw > 0 else { return mobileMermaidMinZoom }
    return min(1, max(mobileMermaidMinZoom, min(mobileMermaidMaxZoom, raw)))
}

private struct MobileMermaidFullscreenView: View {
    let diagram: MobileMermaidDiagram
    let onClose: () -> Void
    @State private var scale: CGFloat = 1
    @GestureState private var liveMagnification: CGFloat = 1

    private var naturalSize: CGSize {
        mobileMermaidNaturalSize(diagram)
    }

    private var effectiveScale: CGFloat {
        min(
            mobileMermaidMaxZoom,
            max(mobileMermaidMinZoom, scale * liveMagnification)
        )
    }

    var body: some View {
        GeometryReader { geometry in
            NavigationStack {
                ScrollViewReader { scrollProxy in
                    ScrollView([.horizontal, .vertical]) {
                        MobileMermaidDiagramSurface(diagram: diagram)
                            .scaleEffect(effectiveScale, anchor: .topLeading)
                            .frame(
                                width: naturalSize.width * effectiveScale,
                                height: naturalSize.height * effectiveScale,
                                alignment: .topLeading
                            )
                            .padding(24)
                            .id("mobile-mermaid-origin")
                    }
                    .background(Color(uiColor: .systemBackground))
                    .simultaneousGesture(
                        MagnificationGesture()
                            .updating($liveMagnification) { value, state, _ in
                                state = value
                            }
                            .onEnded { value in
                                scale = min(
                                    mobileMermaidMaxZoom,
                                    max(mobileMermaidMinZoom, scale * value)
                                )
                            }
                    )
                    .navigationTitle("Diagram")
                    .navigationBarTitleDisplayMode(.inline)
                    .toolbar {
                        ToolbarItem(placement: .topBarLeading) {
                            Button("Close", action: onClose)
                        }
                        ToolbarItemGroup(placement: .topBarTrailing) {
                            Button {
                                scale = max(
                                    mobileMermaidMinZoom,
                                    scale / mobileMermaidZoomStep
                                )
                            } label: {
                                Image(systemName: "minus.magnifyingglass")
                            }
                            .accessibilityLabel("Zoom out")

                            Button {
                                scale = min(
                                    mobileMermaidMaxZoom,
                                    scale * mobileMermaidZoomStep
                                )
                            } label: {
                                Image(systemName: "plus.magnifyingglass")
                            }
                            .accessibilityLabel("Zoom in")

                            Button {
                                scale = mobileMermaidFitScale(
                                    diagram: naturalSize,
                                    viewport: CGSize(
                                        width: max(1, geometry.size.width - 48),
                                        height: max(1, geometry.size.height - 96)
                                    )
                                )
                                DispatchQueue.main.async {
                                    scrollProxy.scrollTo(
                                        "mobile-mermaid-origin",
                                        anchor: .topLeading
                                    )
                                }
                            } label: {
                                Image(systemName: "arrow.down.right.and.arrow.up.left")
                            }
                            .accessibilityLabel("Fit diagram")
                        }
                    }
                    .onAppear {
                        scale = mobileMermaidFitScale(
                            diagram: naturalSize,
                            viewport: CGSize(
                                width: max(1, geometry.size.width - 48),
                                height: max(1, geometry.size.height - 96)
                            )
                        )
                    }
                }
            }
        }
    }
}

internal struct MobileMermaidDiagramView: View {
    private enum LoadState {
        case loading
        case ready(MobileMermaidDiagram)
        case failed(String)
    }

    let source: String
    let renderScopeID: String

    @Environment(\.colorScheme) private var colorScheme
    @State private var state: LoadState = .loading
    @State private var activeRequestID = ""
    @State private var fullscreen = false

    private var requestID: String {
        renderScopeID + "\u{1f}" + source
    }

    private var renderRequestID: String {
        requestID + "\u{1e}" + (colorScheme == .dark ? "dark" : "light")
    }

    var body: some View {
        Group {
            switch state {
            case .loading:
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("Rendering diagram…").font(.caption).foregroundStyle(.secondary)
                }
                .frame(minHeight: 72)
            case .failed(let message):
                VStack(alignment: .leading, spacing: 6) {
                    Label("Couldn't render this diagram.", systemImage: "exclamationmark.triangle")
                        .font(.caption.weight(.semibold))
                    Text(message).font(.caption2).foregroundStyle(.secondary)
                    ScrollView(.horizontal) {
                        Text(source)
                            .font(.system(.caption, design: .monospaced))
                            .textSelection(.enabled)
                    }
                }
                .padding(10)
                .background(
                    Color(uiColor: .secondarySystemBackground),
                    in: RoundedRectangle(cornerRadius: 10, style: .continuous)
                )
            case .ready(let diagram):
                ZStack(alignment: .topTrailing) {
                    ScrollView([.horizontal, .vertical]) {
                        MobileMermaidDiagramSurface(diagram: diagram)
                            .padding(12)
                    }
                    .frame(minHeight: 180, maxHeight: 280)
                    Button {
                        fullscreen = true
                    } label: {
                        Image(systemName: "arrow.up.left.and.arrow.down.right")
                            .padding(7)
                            .background(.thinMaterial, in: Circle())
                    }
                    .buttonStyle(.plain)
                    .padding(8)
                    .accessibilityLabel("Open diagram full screen")
                }
                .background(
                    Color(uiColor: .secondarySystemBackground),
                    in: RoundedRectangle(cornerRadius: 12, style: .continuous)
                )
            }
        }
        .task(id: renderRequestID) {
            let request = renderRequestID
            activeRequestID = request
            state = .loading
            let result = await MobileMermaidRenderQueue.shared.resolve(
                source: source,
                theme: colorScheme == .dark ? "dark" : "light"
            )
            guard !Task.isCancelled,
                  activeRequestID == request,
                  renderRequestID == request
            else { return }
            switch result {
            case .ready(let diagram):
                state = .ready(diagram)
            case .failed(let error):
                state = .failed(error.localizedDescription)
            }
        }
        .onDisappear {
            activeRequestID = ""
        }
        .sheet(isPresented: $fullscreen) {
            if case .ready(let diagram) = state {
                MobileMermaidFullscreenView(diagram: diagram) {
                    fullscreen = false
                }
            }
        }
    }
}
