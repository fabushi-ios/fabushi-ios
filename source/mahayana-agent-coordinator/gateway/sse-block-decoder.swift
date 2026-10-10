import Foundation

struct ServerSentEventBlock: Equatable, Sendable {
    let event: String?
    let id: String?
    let data: String
}

struct SSEBlockDecoder {
    private var buffer = ""

    mutating func append(_ chunk: String) -> [ServerSentEventBlock] {
        buffer += chunk.replacingOccurrences(of: "\r\n", with: "\n")
        var result: [ServerSentEventBlock] = []

        while let range = buffer.range(of: "\n\n") {
            let block = String(buffer[..<range.lowerBound])
            buffer.removeSubrange(buffer.startIndex..<range.upperBound)
            if let decoded = Self.decode(block) {
                result.append(decoded)
            }
        }
        return result
    }

    mutating func finish() -> ServerSentEventBlock? {
        defer { buffer.removeAll(keepingCapacity: false) }
        guard !buffer.isEmpty else { return nil }
        return Self.decode(buffer)
    }

    private static func decode(_ block: String) -> ServerSentEventBlock? {
        var event: String?
        var id: String?
        var dataLines: [String] = []

        for rawLine in block.split(separator: "\n", omittingEmptySubsequences: false) {
            let line = String(rawLine)
            if line.hasPrefix(":") { continue }
            let pair = line.split(separator: ":", maxSplits: 1, omittingEmptySubsequences: false)
            let field = String(pair.first ?? "")
            var value = pair.count == 2 ? String(pair[1]) : ""
            if value.hasPrefix(" ") { value.removeFirst() }
            switch field {
            case "event": event = value
            case "id": id = value
            case "data": dataLines.append(value)
            default: continue
            }
        }

        guard !dataLines.isEmpty || event != nil || id != nil else { return nil }
        return .init(event: event, id: id, data: dataLines.joined(separator: "\n"))
    }
}
