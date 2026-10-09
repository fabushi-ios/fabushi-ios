import Foundation

enum ClientSideToolV2Transport {
    static let family = "client-side-tool-v2"
    static let wireVersion = 1
    static let accountSlot = "host"
}

enum ClientSideToolV2MessageKind: String, Codable, Sendable {
    case call
    case result
}

struct ClientSideToolV2WireMessage: Codable, Equatable, Sendable {
    let encoding: String
    let messageType: String
    let bytes: String

    init(messageType: String, bytes: Data) {
        encoding = "protobuf-base64"
        self.messageType = messageType
        self.bytes = bytes.base64EncodedString()
    }

    init(encoding: String, messageType: String, bytes: String) {
        self.encoding = encoding
        self.messageType = messageType
        self.bytes = bytes
    }

    var decodedBytes: Data? {
        guard encoding == "protobuf-base64",
              let data = Data(base64Encoded: bytes),
              data.base64EncodedString() == bytes
        else { return nil }
        return data
    }
}

enum ClientSideToolV2TransportEvent: Equatable, Sendable {
    case update(
        version: Int,
        kind: ClientSideToolV2MessageKind,
        accountSlot: String,
        agentId: String,
        epoch: String,
        sequence: UInt64,
        message: ClientSideToolV2WireMessage
    )
    case reset(version: Int, accountSlot: String, agentId: String, epoch: String, sequence: UInt64)

    var version: Int {
        switch self {
        case .update(let version, _, _, _, _, _, _), .reset(let version, _, _, _, _): version
        }
    }

    var accountSlot: String {
        switch self {
        case .update(_, _, let value, _, _, _, _), .reset(_, let value, _, _, _): value
        }
    }

    var agentId: String {
        switch self {
        case .update(_, _, _, let value, _, _, _), .reset(_, _, let value, _, _): value
        }
    }

    var epoch: String {
        switch self {
        case .update(_, _, _, _, let value, _, _), .reset(_, _, _, let value, _): value
        }
    }

    var sequence: UInt64 {
        switch self {
        case .update(_, _, _, _, _, let sequence, _), .reset(_, _, _, _, let sequence): sequence
        }
    }

    var kindName: String {
        switch self {
        case .update(_, let kind, _, _, _, _, _): kind.rawValue
        case .reset: "reset"
        }
    }

    var message: ClientSideToolV2WireMessage? {
        switch self {
        case .update(_, _, _, _, _, _, let message): message
        case .reset: nil
        }
    }

    static func fromFoundation(_ value: Any) -> ClientSideToolV2TransportEvent? {
        guard let object = value as? [String: Any],
              let version = integer(object["version"]),
              version == ClientSideToolV2Transport.wireVersion,
              let kind = object["kind"] as? String,
              let accountSlot = nonEmptyString(object["accountSlot"]),
              let agentId = nonEmptyString(object["agentId"]),
              let epoch = nonEmptyString(object["epoch"]),
              let sequence = unsignedInteger(object["sequence"]),
              sequence >= 1
        else { return nil }

        if kind == "reset" {
            return .reset(
                version: version,
                accountSlot: accountSlot,
                agentId: agentId,
                epoch: epoch,
                sequence: sequence
            )
        }

        guard let messageKind = ClientSideToolV2MessageKind(rawValue: kind),
              let messageObject = object["message"] as? [String: Any],
              let encoding = messageObject["encoding"] as? String,
              let messageType = messageObject["messageType"] as? String,
              let bytes = messageObject["bytes"] as? String
        else { return nil }

        let message = ClientSideToolV2WireMessage(
            encoding: encoding,
            messageType: messageType,
            bytes: bytes
        )
        let expectedType = messageKind == .call
            ? "aiserver.v1.ClientSideToolV2Call"
            : "aiserver.v1.ClientSideToolV2Result"
        guard message.messageType == expectedType,
              let decoded = message.decodedBytes,
              !decoded.isEmpty
        else { return nil }

        return .update(
            version: version,
            kind: messageKind,
            accountSlot: accountSlot,
            agentId: agentId,
            epoch: epoch,
            sequence: sequence,
            message: message
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
        if let value = value as? UInt64 { return value }
        if let value = value as? Int, value >= 0 { return UInt64(value) }
        if let value = value as? NSNumber {
            let double = value.doubleValue
            guard double.isFinite, double >= 0, double.rounded() == double else { return nil }
            return UInt64(value.uint64Value)
        }
        return nil
    }
}

struct ClientSideToolV2RendererEvent: Equatable, Sendable {
    let version: Int
    let kind: String
    let accountSlot: String
    let agentId: String
    let epoch: String
    let sequence: UInt64
    let messageType: String?
    let bytes: Data?

    var coordinatorPayload: CoordinatorPayload {
        var object: [String: CoordinatorPayload] = [
            "version": .number(Double(version)),
            "kind": .string(kind),
            "accountSlot": .string(accountSlot),
            "agentId": .string(agentId),
            "epoch": .string(epoch),
            "sequence": .number(Double(sequence)),
        ]
        object["messageType"] = messageType.map(CoordinatorPayload.string) ?? .null
        object["bytes"] = bytes.map { data in
            .array(data.map { .number(Double($0)) })
        } ?? .null
        return .object(object)
    }
}
