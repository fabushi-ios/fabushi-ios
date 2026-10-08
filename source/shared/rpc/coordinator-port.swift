import Foundation

enum CoordinatorProtocol {
    static let version = 1
    static let unknownMethod = "unknown-method"
    static let cancelled = "cancelled"
    static let disconnected = "COORDINATOR_DISCONNECTED"
    static let transportStateFamily = "coordinator-transport-state"
}

enum CoordinatorPayloadError: Error, Equatable {
    case unsupportedFoundationValue(String)
}

indirect enum CoordinatorPayload: Codable, Equatable, Sendable {
    case null
    case bool(Bool)
    case number(Double)
    case string(String)
    case array([CoordinatorPayload])
    case object([String: CoordinatorPayload])

    init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() {
            self = .null
        } else if let value = try? container.decode(Bool.self) {
            self = .bool(value)
        } else if let value = try? container.decode(Double.self) {
            self = .number(value)
        } else if let value = try? container.decode(String.self) {
            self = .string(value)
        } else if let value = try? container.decode([CoordinatorPayload].self) {
            self = .array(value)
        } else if let value = try? container.decode([String: CoordinatorPayload].self) {
            self = .object(value)
        } else {
            throw DecodingError.dataCorruptedError(in: container, debugDescription: "Unsupported coordinator payload")
        }
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        switch self {
        case .null:
            try container.encodeNil()
        case .bool(let value):
            try container.encode(value)
        case .number(let value):
            try container.encode(value)
        case .string(let value):
            try container.encode(value)
        case .array(let value):
            try container.encode(value)
        case .object(let value):
            try container.encode(value)
        }
    }

    static func fromFoundation(_ value: Any) throws -> CoordinatorPayload {
        switch value {
        case is NSNull:
            return .null
        case let value as Bool:
            return .bool(value)
        case let value as String:
            return .string(value)
        case let value as Int:
            return .number(Double(value))
        case let value as Int8:
            return .number(Double(value))
        case let value as Int16:
            return .number(Double(value))
        case let value as Int32:
            return .number(Double(value))
        case let value as Int64:
            return .number(Double(value))
        case let value as UInt:
            return .number(Double(value))
        case let value as UInt8:
            return .number(Double(value))
        case let value as UInt16:
            return .number(Double(value))
        case let value as UInt32:
            return .number(Double(value))
        case let value as UInt64:
            return .number(Double(value))
        case let value as Float:
            return .number(Double(value))
        case let value as Double:
            return .number(value)
        case let value as NSNumber:
            return .number(value.doubleValue)
        case let value as [Any]:
            return .array(try value.map(CoordinatorPayload.fromFoundation))
        case let value as [String: Any]:
            return .object(try value.mapValues(CoordinatorPayload.fromFoundation))
        default:
            throw CoordinatorPayloadError.unsupportedFoundationValue(String(describing: type(of: value)))
        }
    }

    var foundationValue: Any {
        switch self {
        case .null:
            return NSNull()
        case .bool(let value):
            return value
        case .number(let value):
            return value
        case .string(let value):
            return value
        case .array(let value):
            return value.map(\.foundationValue)
        case .object(let value):
            return value.mapValues(\.foundationValue)
        }
    }
}

struct CoordinatorFailure: Codable, Equatable, Sendable {
    let code: String
    let message: String
    let transportKind: String?

    init(code: String, message: String, transportKind: String? = nil) {
        self.code = code
        self.message = message
        self.transportKind = transportKind
    }
}

enum CoordinatorReplyOutcome: Equatable, Sendable, Codable {
    case ok(CoordinatorPayload)
    case failed(CoordinatorFailure)

    private enum CodingKeys: String, CodingKey { case status, value, failure }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let status = try container.decode(String.self, forKey: .status)
        switch status {
        case "ok":
            guard container.contains(.value) else {
                throw DecodingError.keyNotFound(CodingKeys.value, .init(codingPath: decoder.codingPath, debugDescription: "ok reply requires value"))
            }
            self = .ok(try container.decode(CoordinatorPayload.self, forKey: .value))
        case "failed":
            self = .failed(try container.decode(CoordinatorFailure.self, forKey: .failure))
        default:
            throw DecodingError.dataCorruptedError(forKey: .status, in: container, debugDescription: "reply status must be ok or failed")
        }
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .ok(let value):
            try container.encode("ok", forKey: .status)
            try container.encode(value, forKey: .value)
        case .failed(let failure):
            try container.encode("failed", forKey: .status)
            try container.encode(failure, forKey: .failure)
        }
    }
}

enum CoordinatorShutdownReason: String, Codable, Equatable, Sendable {
    case requested
    case protocolError = "protocol-error"
}

enum CoordinatorFrame: Equatable, Sendable, Codable {
    case hello(protocolVersion: Int)
    case ready(protocolVersion: Int)
    case shutdown(reason: CoordinatorShutdownReason, detail: String?)
    case request(requestId: String, method: String, args: CoordinatorPayload)
    case cancel(requestId: String)
    case reply(requestId: String, outcome: CoordinatorReplyOutcome)
    case event(family: String, payload: CoordinatorPayload)

    private enum CodingKeys: String, CodingKey {
        case kind, phase, protocolVersion, reason, detail, requestId, method, args, outcome, family, payload
    }

    private static func requireNonEmpty(_ value: String, _ field: String, decoder: Decoder) throws -> String {
        guard !value.isEmpty else {
            throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "\(field) must be non-empty"))
        }
        return value
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let kind = try container.decode(String.self, forKey: .kind)
        switch kind {
        case "lifecycle":
            let phase = try container.decode(String.self, forKey: .phase)
            switch phase {
            case "hello":
                self = .hello(protocolVersion: try container.decode(Int.self, forKey: .protocolVersion))
            case "ready":
                self = .ready(protocolVersion: try container.decode(Int.self, forKey: .protocolVersion))
            case "shutdown":
                let reason = try container.decode(CoordinatorShutdownReason.self, forKey: .reason)
                let detail = try container.decodeIfPresent(String.self, forKey: .detail)
                if reason == .protocolError && (detail?.isEmpty != false) {
                    throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "protocol-error shutdown requires detail"))
                }
                if reason == .requested && detail != nil {
                    throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "requested shutdown detail must be null"))
                }
                self = .shutdown(reason: reason, detail: detail)
            default:
                throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "unknown lifecycle phase"))
            }
        case "request":
            let requestId = try Self.requireNonEmpty(try container.decode(String.self, forKey: .requestId), "requestId", decoder: decoder)
            let method = try Self.requireNonEmpty(try container.decode(String.self, forKey: .method), "method", decoder: decoder)
            guard container.contains(.args) else {
                throw DecodingError.keyNotFound(CodingKeys.args, .init(codingPath: decoder.codingPath, debugDescription: "request args missing"))
            }
            self = .request(requestId: requestId, method: method, args: try container.decode(CoordinatorPayload.self, forKey: .args))
        case "cancel":
            let requestId = try Self.requireNonEmpty(try container.decode(String.self, forKey: .requestId), "requestId", decoder: decoder)
            self = .cancel(requestId: requestId)
        case "reply":
            let requestId = try Self.requireNonEmpty(try container.decode(String.self, forKey: .requestId), "requestId", decoder: decoder)
            self = .reply(requestId: requestId, outcome: try container.decode(CoordinatorReplyOutcome.self, forKey: .outcome))
        case "event":
            let family = try Self.requireNonEmpty(try container.decode(String.self, forKey: .family), "family", decoder: decoder)
            guard container.contains(.payload) else {
                throw DecodingError.keyNotFound(CodingKeys.payload, .init(codingPath: decoder.codingPath, debugDescription: "event payload missing"))
            }
            self = .event(family: family, payload: try container.decode(CoordinatorPayload.self, forKey: .payload))
        default:
            throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "unknown coordinator frame kind"))
        }
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .hello(let protocolVersion):
            try container.encode("lifecycle", forKey: .kind)
            try container.encode("hello", forKey: .phase)
            try container.encode(protocolVersion, forKey: .protocolVersion)
        case .ready(let protocolVersion):
            try container.encode("lifecycle", forKey: .kind)
            try container.encode("ready", forKey: .phase)
            try container.encode(protocolVersion, forKey: .protocolVersion)
        case .shutdown(let reason, let detail):
            try container.encode("lifecycle", forKey: .kind)
            try container.encode("shutdown", forKey: .phase)
            try container.encode(reason, forKey: .reason)
            try container.encodeIfPresent(detail, forKey: .detail)
            if detail == nil { try container.encodeNil(forKey: .detail) }
        case .request(let requestId, let method, let args):
            try container.encode("request", forKey: .kind)
            try container.encode(requestId, forKey: .requestId)
            try container.encode(method, forKey: .method)
            try container.encode(args, forKey: .args)
        case .cancel(let requestId):
            try container.encode("cancel", forKey: .kind)
            try container.encode(requestId, forKey: .requestId)
        case .reply(let requestId, let outcome):
            try container.encode("reply", forKey: .kind)
            try container.encode(requestId, forKey: .requestId)
            try container.encode(outcome, forKey: .outcome)
        case .event(let family, let payload):
            try container.encode("event", forKey: .kind)
            try container.encode(family, forKey: .family)
            try container.encode(payload, forKey: .payload)
        }
    }
}

struct CoordinatorBootstrap: Codable, Equatable, Sendable {
    struct ProcessConfig: Codable, Equatable, Sendable {
        let appVersion: String
        let isPackaged: Bool
        let dataDir: String
        let localHumanId: String?
    }

    let processConfig: ProcessConfig
}

struct CoordinatorRequestEnvelope: Equatable, Sendable {
    let protocolVersion: Int
    let requestId: String
    let sessionId: String
    let method: String
    let params: CoordinatorPayload
    let deadlineMilliseconds: Int?

    init(
        protocolVersion: Int = CoordinatorProtocol.version,
        requestId: String = UUID().uuidString.lowercased(),
        sessionId: String,
        method: String,
        params: CoordinatorPayload = .object([:]),
        deadlineMilliseconds: Int? = nil
    ) {
        precondition(!requestId.isEmpty)
        precondition(!sessionId.isEmpty)
        precondition(!method.isEmpty)
        self.protocolVersion = protocolVersion
        self.requestId = requestId
        self.sessionId = sessionId
        self.method = method
        self.params = params
        self.deadlineMilliseconds = deadlineMilliseconds
    }
}

struct CoordinatorReplyEnvelope: Equatable, Sendable {
    let requestId: String
    let outcome: CoordinatorReplyOutcome
}

struct CoordinatorEventEnvelope: Equatable, Sendable {
    let eventId: String
    let sessionId: String
    let sequence: UInt64
    let type: String
    let payload: CoordinatorPayload
}

struct CoordinatorCancelEnvelope: Equatable, Sendable {
    let requestId: String
    let reason: String?
}

@MainActor
protocol CoordinatorPort: AnyObject {
    func post(_ frame: CoordinatorFrame)
    func close()
}
