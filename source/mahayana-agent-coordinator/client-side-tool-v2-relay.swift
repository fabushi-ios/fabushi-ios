import Foundation

@MainActor
final class ClientSideToolV2Relay {
    enum RelayError: LocalizedError, Equatable {
        case duplicateCall
        case unknownCall

        var errorDescription: String? {
            switch self {
            case .duplicateCall: "tool call id is empty or already pending"
            case .unknownCall: "unknown client-side tool call"
            }
        }
    }

    private struct AgentFence {
        var epoch: String
        var sequence: UInt64 = 0
        var retiredEpochs: Set<String> = []
        var updatesByToolCallID: [String: [ClientSideToolV2TransportEvent]] = [:]
    }

    private var agents: [String: AgentFence] = [:]
    private var pendingCompatibilityCalls: [String: String] = [:]

    func accept(_ event: ClientSideToolV2TransportEvent) -> ClientSideToolV2RendererEvent? {
        guard event.version == ClientSideToolV2Transport.wireVersion,
              event.accountSlot == ClientSideToolV2Transport.accountSlot,
              !event.agentId.isEmpty,
              !event.epoch.isEmpty,
              event.sequence >= 1
        else { return nil }

        var fence = agents[event.agentId] ?? AgentFence(epoch: event.epoch)
        if event.epoch != fence.epoch {
            guard !fence.retiredEpochs.contains(event.epoch) else { return nil }
            if !fence.epoch.isEmpty { fence.retiredEpochs.insert(fence.epoch) }
            fence.epoch = event.epoch
            fence.sequence = 0
            fence.updatesByToolCallID.removeAll()
        }
        guard event.sequence > fence.sequence else { return nil }
        fence.sequence = event.sequence
        // Match Desktop exactly: once an envelope passes identity/ordering fences,
        // its sequence and epoch transition are consumed even if payload decoding
        // or Call/Result settlement later rejects the event.
        agents[event.agentId] = fence

        if case .reset = event {
            fence.updatesByToolCallID.removeAll()
            agents[event.agentId] = fence
            return materialize(event)
        }

        guard let message = event.message,
              let (_, toolCallID) = decodeMessage(event: event, message: message)
        else { return nil }

        switch event {
        case .update(_, .call, _, _, _, _, _):
            fence.updatesByToolCallID[toolCallID] = [event]
        case .update(_, .result, _, _, _, _, _):
            guard let lifecycle = fence.updatesByToolCallID[toolCallID],
                  lifecycle.first.map(isCall) == true
            else { return nil }
            fence.updatesByToolCallID[toolCallID] = [lifecycle[0], event]
        case .reset:
            break
        }

        agents[event.agentId] = fence
        return materialize(event)
    }

    func replay() -> [ClientSideToolV2RendererEvent] {
        agents.values
            .flatMap { $0.updatesByToolCallID.values.flatMap { $0 } }
            .sorted { $0.sequence < $1.sequence }
            .compactMap(materialize)
    }

    func clear() {
        agents.removeAll()
        pendingCompatibilityCalls.removeAll()
    }

    func begin(callID: String, toolName: String) throws {
        guard !callID.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
              pendingCompatibilityCalls[callID] == nil
        else { throw RelayError.duplicateCall }
        pendingCompatibilityCalls[callID] = toolName
    }

    func settle(callID: String, output: CoordinatorPayload) throws -> (tool: String, output: CoordinatorPayload) {
        guard let tool = pendingCompatibilityCalls.removeValue(forKey: callID) else {
            throw RelayError.unknownCall
        }
        return (tool, output)
    }

    private func materialize(_ event: ClientSideToolV2TransportEvent) -> ClientSideToolV2RendererEvent? {
        if case .reset = event {
            return .init(
                version: event.version,
                kind: event.kindName,
                accountSlot: event.accountSlot,
                agentId: event.agentId,
                epoch: event.epoch,
                sequence: event.sequence,
                messageType: nil,
                bytes: nil
            )
        }
        guard let message = event.message,
              let (bytes, _) = decodeMessage(event: event, message: message)
        else { return nil }
        return .init(
            version: event.version,
            kind: event.kindName,
            accountSlot: event.accountSlot,
            agentId: event.agentId,
            epoch: event.epoch,
            sequence: event.sequence,
            messageType: message.messageType,
            bytes: bytes
        )
    }

    private func isCall(_ event: ClientSideToolV2TransportEvent) -> Bool {
        if case .update(_, .call, _, _, _, _, _) = event { return true }
        return false
    }

    private func decodeMessage(
        event: ClientSideToolV2TransportEvent,
        message: ClientSideToolV2WireMessage
    ) -> (Data, String)? {
        let expectedType: String
        let toolCallField: UInt32
        switch event {
        case .update(_, .call, _, _, _, _, _):
            expectedType = "aiserver.v1.ClientSideToolV2Call"
            toolCallField = 3
        case .update(_, .result, _, _, _, _, _):
            expectedType = "aiserver.v1.ClientSideToolV2Result"
            toolCallField = 35
        case .reset:
            return nil
        }
        guard message.messageType == expectedType,
              let data = message.decodedBytes,
              !data.isEmpty,
              let toolCallBytes = extractLengthDelimitedField(Array(data), wantedField: toolCallField),
              let toolCallID = String(bytes: toolCallBytes, encoding: .utf8),
              !toolCallID.isEmpty
        else { return nil }
        return (data, toolCallID)
    }

    private func extractLengthDelimitedField(_ bytes: [UInt8], wantedField: UInt32) -> [UInt8]? {
        var cursor = 0
        while cursor < bytes.count {
            guard let key = readVarint(bytes, cursor: &cursor) else { return nil }
            let field = UInt32(key >> 3)
            let wire = UInt8(key & 0x07)
            switch wire {
            case 0:
                guard readVarint(bytes, cursor: &cursor) != nil else { return nil }
            case 1:
                guard cursor + 8 <= bytes.count else { return nil }
                cursor += 8
            case 2:
                guard let rawLength = readVarint(bytes, cursor: &cursor),
                      rawLength <= UInt64(Int.max)
                else { return nil }
                let length = Int(rawLength)
                guard cursor + length <= bytes.count else { return nil }
                if field == wantedField { return Array(bytes[cursor ..< cursor + length]) }
                cursor += length
            case 5:
                guard cursor + 4 <= bytes.count else { return nil }
                cursor += 4
            default:
                return nil
            }
        }
        return nil
    }

    private func readVarint(_ bytes: [UInt8], cursor: inout Int) -> UInt64? {
        var value: UInt64 = 0
        var shift: UInt64 = 0
        while cursor < bytes.count, shift <= 63 {
            let byte = bytes[cursor]
            cursor += 1
            value |= UInt64(byte & 0x7f) << shift
            if byte & 0x80 == 0 { return value }
            shift += 7
        }
        return nil
    }
}
