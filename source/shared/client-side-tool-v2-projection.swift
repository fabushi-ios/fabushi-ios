import Foundation

enum MobileToolResultKind: String, Equatable, Sendable {
    case fileEdit = "file-edit"
    case fileWrite = "file-write"
    case shell
}

enum MobileToolResultStatus: String, Equatable, Sendable {
    case running
    case success
    case error
    case denied
    case rejected
    case cancelled
    case background
}

struct MobileToolResultCard: Identifiable, Equatable, Sendable {
    let agentId: String
    let toolCallId: String
    let kind: MobileToolResultKind
    let status: MobileToolResultStatus
    let path: String?
    let command: String?
    let workingDirectory: String?
    let summary: String
    let output: String
    let diff: String
    let isStreaming: Bool
    let isBackground: Bool
    let sequence: UInt64

    var id: String { "\(agentId):\(toolCallId)" }
}

private enum MobileProtoValue {
    case varint(UInt64)
    case bytes(Data)
}

private struct MobileProtoMessage {
    private(set) var fields: [Int: [MobileProtoValue]] = [:]

    static func decode(_ data: Data) -> MobileProtoMessage? {
        let bytes = [UInt8](data)
        var offset = 0
        var message = MobileProtoMessage()
        while offset < bytes.count {
            guard let key = readVarint(bytes, offset: &offset), key > 0 else { return nil }
            let field = Int(key >> 3)
            let wire = Int(key & 7)
            guard field > 0 else { return nil }
            switch wire {
            case 0:
                guard let value = readVarint(bytes, offset: &offset) else { return nil }
                message.fields[field, default: []].append(.varint(value))
            case 1:
                guard offset + 8 <= bytes.count else { return nil }
                offset += 8
            case 2:
                guard let length = readVarint(bytes, offset: &offset),
                      length <= UInt64(Int.max)
                else { return nil }
                let count = Int(length)
                guard count >= 0, offset + count <= bytes.count else { return nil }
                message.fields[field, default: []].append(
                    .bytes(Data(bytes[offset..<(offset + count)]))
                )
                offset += count
            case 5:
                guard offset + 4 <= bytes.count else { return nil }
                offset += 4
            default:
                return nil
            }
        }
        return message
    }

    func varint(_ field: Int) -> UInt64? {
        for value in fields[field] ?? [] {
            if case .varint(let raw) = value { return raw }
        }
        return nil
    }

    func bool(_ field: Int) -> Bool {
        varint(field) != 0
    }

    func bytes(_ field: Int) -> Data? {
        for value in fields[field] ?? [] {
            if case .bytes(let data) = value { return data }
        }
        return nil
    }

    func allBytes(_ field: Int) -> [Data] {
        (fields[field] ?? []).compactMap {
            if case .bytes(let data) = $0 { return data }
            return nil
        }
    }

    func string(_ field: Int) -> String? {
        guard let data = bytes(field),
              let value = String(data: data, encoding: .utf8),
              !value.isEmpty
        else { return nil }
        return value
    }

    private static func readVarint(_ bytes: [UInt8], offset: inout Int) -> UInt64? {
        var value: UInt64 = 0
        var shift: UInt64 = 0
        for _ in 0..<10 {
            guard offset < bytes.count else { return nil }
            let byte = bytes[offset]
            offset += 1
            let payload = UInt64(byte & 0x7f)
            if shift == 63, payload > 1 { return nil }
            value |= payload << shift
            if byte & 0x80 == 0 { return value }
            shift += 7
        }
        return nil
    }
}

private struct MobileToolCallProjection {
    let tool: UInt64
    let toolCallId: String
    let kind: MobileToolResultKind
    let path: String?
    let command: String?
    let workingDirectory: String?
    let isBackground: Bool
}

private struct MobileToolMergeState {
    let call: MobileToolCallProjection
    var card: MobileToolResultCard
    var lastSequence: UInt64
}

struct MobileToolResultRendererEvent: Equatable, Sendable {
    let kind: String
    let agentId: String
    let sequence: UInt64
    let messageType: String?
    let bytes: Data?

    static func fromFoundation(_ value: Any) -> MobileToolResultRendererEvent? {
        guard let object = value as? [String: Any],
              let version = integer(object["version"]),
              version == ClientSideToolV2Transport.wireVersion,
              let kind = object["kind"] as? String,
              ["call", "result", "reset"].contains(kind),
              object["accountSlot"] as? String == ClientSideToolV2Transport.accountSlot,
              let agentId = nonEmptyString(object["agentId"]),
              let sequence = unsignedInteger(object["sequence"]),
              sequence >= 1
        else { return nil }

        if kind == "reset" {
            return .init(kind: kind, agentId: agentId, sequence: sequence, messageType: nil, bytes: nil)
        }
        let expectedType = kind == "call"
            ? "aiserver.v1.ClientSideToolV2Call"
            : "aiserver.v1.ClientSideToolV2Result"
        guard object["messageType"] as? String == expectedType,
              let byteValues = object["bytes"] as? [Any],
              !byteValues.isEmpty
        else { return nil }
        var bytes = [UInt8]()
        bytes.reserveCapacity(byteValues.count)
        for value in byteValues {
            guard let number = integer(value), (0...255).contains(number) else { return nil }
            bytes.append(UInt8(number))
        }
        return .init(
            kind: kind,
            agentId: agentId,
            sequence: sequence,
            messageType: expectedType,
            bytes: Data(bytes)
        )
    }

    private static func nonEmptyString(_ value: Any?) -> String? {
        guard let value = value as? String, !value.isEmpty else { return nil }
        return value
    }

    private static func integer(_ value: Any?) -> Int? {
        if let value = value as? Int { return value }
        if let value = value as? NSNumber {
            let double = value.doubleValue
            guard double.isFinite, double.rounded() == double else { return nil }
            return value.intValue
        }
        return nil
    }

    private static func unsignedInteger(_ value: Any?) -> UInt64? {
        let maxSafe = 9_007_199_254_740_991.0
        if let value = value as? UInt64, Double(value) <= maxSafe { return value }
        guard let value = value as? NSNumber else {
            if let value = value as? Int, value >= 0, Double(value) <= maxSafe { return UInt64(value) }
            return nil
        }
        let double = value.doubleValue
        guard double.isFinite, double >= 0, double <= maxSafe, double.rounded() == double else { return nil }
        return UInt64(double)
    }
}

@MainActor
final class MobileToolResultStore {
    private var stateByID: [String: MobileToolMergeState] = [:]
    private(set) var cardsByAgent: [String: [MobileToolResultCard]] = [:]

    func resetAll() {
        stateByID.removeAll()
        cardsByAgent.removeAll()
    }

    @discardableResult
    func consume(_ event: MobileToolResultRendererEvent) -> Bool {
        if event.kind == "reset" {
            stateByID = stateByID.filter { !$0.key.hasPrefix(event.agentId + ":") }
            cardsByAgent.removeValue(forKey: event.agentId)
            return true
        }
        guard let bytes = event.bytes,
              let message = MobileProtoMessage.decode(bytes)
        else { return false }

        if event.kind == "call" {
            guard let call = projectCall(message),
                  call.toolCallId.count > 0
            else { return false }
            let key = event.agentId + ":" + call.toolCallId
            if let previous = stateByID[key], event.sequence <= previous.lastSequence { return false }
            let card = MobileToolResultCard(
                agentId: event.agentId,
                toolCallId: call.toolCallId,
                kind: call.kind,
                status: .running,
                path: call.path,
                command: call.command,
                workingDirectory: call.workingDirectory,
                summary: "",
                output: "",
                diff: "",
                isStreaming: true,
                isBackground: call.isBackground,
                sequence: event.sequence
            )
            stateByID[key] = .init(call: call, card: card, lastSequence: event.sequence)
            publish(agentId: event.agentId)
            return true
        }

        guard event.kind == "result",
              let tool = message.varint(1),
              let toolCallId = message.string(35)
        else { return false }
        let key = event.agentId + ":" + toolCallId
        guard var state = stateByID[key],
              event.sequence > state.lastSequence,
              state.call.tool == tool
        else { return false }
        state.lastSequence = event.sequence
        guard let next = projectResult(
            message,
            call: state.call,
            previous: state.card,
            agentId: event.agentId,
            sequence: event.sequence
        ) else {
            stateByID[key] = state
            return false
        }
        state.card = next
        stateByID[key] = state
        publish(agentId: event.agentId)
        return true
    }

    private func publish(agentId: String) {
        cardsByAgent[agentId] = stateByID.values
            .filter { $0.card.agentId == agentId }
            .map(\.card)
            .sorted {
                if $0.sequence != $1.sequence { return $0.sequence < $1.sequence }
                return $0.toolCallId < $1.toolCallId
            }
    }

    private func projectCall(_ message: MobileProtoMessage) -> MobileToolCallProjection? {
        guard let tool = message.varint(1),
              [UInt64(7), 15, 38].contains(tool),
              let toolCallId = message.string(3)
        else { return nil }

        if tool == 7 {
            guard let payload = message.bytes(13),
                  let params = MobileProtoMessage.decode(payload),
                  let path = params.string(1)
            else { return nil }
            return .init(tool: tool, toolCallId: toolCallId, kind: .fileEdit, path: path, command: nil, workingDirectory: nil, isBackground: false)
        }
        if tool == 38 {
            guard let payload = message.bytes(50),
                  let params = MobileProtoMessage.decode(payload),
                  let path = params.string(1)
            else { return nil }
            return .init(tool: tool, toolCallId: toolCallId, kind: .fileEdit, path: path, command: nil, workingDirectory: nil, isBackground: false)
        }
        guard let payload = message.bytes(23),
              let params = MobileProtoMessage.decode(payload),
              let command = params.string(1)
        else { return nil }
        return .init(
            tool: tool,
            toolCallId: toolCallId,
            kind: .shell,
            path: nil,
            command: command,
            workingDirectory: params.string(2),
            isBackground: params.bool(5)
        )
    }

    private func projectResult(
        _ message: MobileProtoMessage,
        call: MobileToolCallProjection,
        previous: MobileToolResultCard,
        agentId: String,
        sequence: UInt64
    ) -> MobileToolResultCard? {
        if let errorData = message.bytes(8),
           let error = MobileProtoMessage.decode(errorData),
           let summary = error.string(1) {
            return copy(
                previous,
                status: summary.hasPrefix("Permission denied:") ? .denied : .error,
                summary: summary,
                isStreaming: false,
                sequence: sequence
            )
        }

        if call.tool == 7 {
            guard let payload = message.bytes(10),
                  let result = MobileProtoMessage.decode(payload)
            else { return nil }
            if result.bool(5) {
                return copy(previous, status: .rejected, isStreaming: false, sequence: sequence)
            }
            if result.bool(3) || result.bytes(11) != nil {
                return copy(previous, status: .error, isStreaming: false, sequence: sequence)
            }
            guard result.bool(2) else { return nil }
            return copy(previous, status: .success, diff: diffText(result.bytes(1)), isStreaming: false, sequence: sequence)
        }

        if call.tool == 38 {
            guard let payload = message.bytes(51),
                  let result = MobileProtoMessage.decode(payload)
            else { return nil }
            return copy(
                previous,
                kind: result.bool(2) ? .fileWrite : .fileEdit,
                status: result.bool(4) ? .rejected : .success,
                diff: diffText(result.bytes(3)),
                isStreaming: false,
                sequence: sequence
            )
        }

        guard let payload = message.bytes(24),
              let result = MobileProtoMessage.decode(payload)
        else { return nil }
        let output = result.string(12) ?? result.string(1) ?? ""
        if result.bool(3) {
            return copy(previous, status: .rejected, output: output, isStreaming: false, sequence: sequence)
        }
        if result.bool(4) || result.bool(5) {
            return copy(previous, status: .background, output: output, isStreaming: false, isBackground: true, sequence: sequence)
        }
        switch result.varint(9) {
        case 1:
            return copy(previous, status: .success, output: output, workingDirectory: result.string(7) ?? previous.workingDirectory, isStreaming: false, sequence: sequence)
        case 2:
            return copy(previous, status: .cancelled, output: output, workingDirectory: result.string(7) ?? previous.workingDirectory, isStreaming: false, sequence: sequence)
        case 3, 4, 5:
            return copy(previous, status: .error, output: output, workingDirectory: result.string(7) ?? previous.workingDirectory, isStreaming: false, sequence: sequence)
        default:
            return nil
        }
    }

    private func diffText(_ data: Data?) -> String {
        guard let data,
              let diff = MobileProtoMessage.decode(data)
        else { return "" }
        return diff.allBytes(1).compactMap { chunkData in
            MobileProtoMessage.decode(chunkData)?.string(1)
        }.joined()
    }

    private func copy(
        _ source: MobileToolResultCard,
        kind: MobileToolResultKind? = nil,
        status: MobileToolResultStatus? = nil,
        summary: String? = nil,
        output: String? = nil,
        diff: String? = nil,
        workingDirectory: String? = nil,
        isStreaming: Bool? = nil,
        isBackground: Bool? = nil,
        sequence: UInt64
    ) -> MobileToolResultCard {
        .init(
            agentId: source.agentId,
            toolCallId: source.toolCallId,
            kind: kind ?? source.kind,
            status: status ?? source.status,
            path: source.path,
            command: source.command,
            workingDirectory: workingDirectory ?? source.workingDirectory,
            summary: summary ?? source.summary,
            output: output ?? source.output,
            diff: diff ?? source.diff,
            isStreaming: isStreaming ?? source.isStreaming,
            isBackground: isBackground ?? source.isBackground,
            sequence: sequence
        )
    }
}
