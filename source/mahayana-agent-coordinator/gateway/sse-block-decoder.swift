import Foundation

struct ServerSentEventBlock: Equatable, Sendable {
    let event: String?
    let id: String?
    let data: String
}

struct SSEBlockDecoder {
    private var buffer = Data()

    mutating func append(_ chunk: String) -> [ServerSentEventBlock] {
        append(Data(chunk.utf8))
    }

    mutating func append(_ chunk: Data) -> [ServerSentEventBlock] {
        buffer.append(chunk)
        var result: [ServerSentEventBlock] = []

        while let boundary = Self.firstBoundary(in: buffer) {
            let block = buffer.subdata(in: 0..<boundary.start)
            buffer.removeSubrange(0..<boundary.end)
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

    private static func firstBoundary(in data: Data) -> (start: Int, end: Int)? {
        let bytes = [UInt8](data)
        guard bytes.count >= 2 else { return nil }
        for index in 0..<(bytes.count - 1) {
            if bytes[index] == 0x0A, bytes[index + 1] == 0x0A {
                return (index, index + 2)
            }
            if index + 3 < bytes.count,
               bytes[index] == 0x0D,
               bytes[index + 1] == 0x0A,
               bytes[index + 2] == 0x0D,
               bytes[index + 3] == 0x0A {
                return (index, index + 4)
            }
        }
        return nil
    }

    private static func decode(_ bytes: Data) -> ServerSentEventBlock? {
        guard let raw = String(data: bytes, encoding: .utf8) else { return nil }
        let block = raw.replacingOccurrences(of: "\r\n", with: "\n")
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
