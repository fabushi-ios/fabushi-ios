import Foundation
import os

public enum AppLifecyclePhase: String, Codable, Sendable, Equatable {
    case active
    case inactive
    case background
}

public enum ConnectionStatus: String, Codable, Sendable, Equatable {
    case connecting
    case updating
    case connected
    case offline
}

public struct AccountSessionSummary: Codable, Sendable, Equatable, Identifiable {
    public var id: UInt64 { userId }
    public let userId: UInt64
    public let username: String
    public let token: String
    public let sessionId: String
    public let serverURL: String
    public let firstName: String?

    public init(
        userId: UInt64,
        username: String,
        token: String,
        sessionId: String,
        serverURL: String,
        firstName: String? = nil
    ) {
        self.userId = userId
        self.username = username
        self.token = token
        self.sessionId = sessionId
        self.serverURL = serverURL
        self.firstName = firstName
    }

    enum CodingKeys: String, CodingKey {
        case userId = "user_id"
        case username
        case token
        case sessionId = "session_id"
        case serverURL = "server_url"
        case firstName = "first_name"
    }
}

public struct RuntimeBootstrapState: Codable, Sendable, Equatable {
    public let serverURL: String
    public let appSupportPath: String
    public let isAuthenticated: Bool
    public let activeAccount: AccountSessionSummary?
    public let accounts: [AccountSessionSummary]
    public let connectionStatus: ConnectionStatus
    public let version: String

    public init(
        serverURL: String,
        appSupportPath: String,
        isAuthenticated: Bool,
        activeAccount: AccountSessionSummary?,
        accounts: [AccountSessionSummary],
        connectionStatus: ConnectionStatus,
        version: String = "1.3.0"
    ) {
        self.serverURL = serverURL
        self.appSupportPath = appSupportPath
        self.isAuthenticated = isAuthenticated
        self.activeAccount = activeAccount
        self.accounts = accounts
        self.connectionStatus = connectionStatus
        self.version = version
    }
}

/// Structured host RPC error (`PROJECT.md § IC.1`) — never swallowed as `"acknowledged"`.
public struct HostError: LocalizedError, Codable, Sendable, Equatable {
    public let code: String
    public let httpStatus: Int?
    public let message: String
    public let retryAfterSeconds: Int?

    public init(
        code: String,
        httpStatus: Int? = nil,
        message: String,
        retryAfterSeconds: Int? = nil
    ) {
        self.code = code
        self.httpStatus = httpStatus
        self.message = message
        self.retryAfterSeconds = retryAfterSeconds
    }

    public var errorDescription: String? {
        if let httpStatus {
            return "[\(code) HTTP \(httpStatus)] \(message)"
        }
        return "[\(code)] \(message)"
    }

    enum CodingKeys: String, CodingKey {
        case code
        case httpStatus = "http_status"
        case message
        case retryAfterSeconds = "retry_after_seconds"
    }
}

// MARK: - Realtime Push Event DTOs (`PROJECT.md § IC.1`)

public struct DialogSummaryDTO: Codable, Sendable, Equatable, Identifiable {
    public let id: Int64
    public let title: String
    public let unreadCount: Int
    public let lastMessage: String

    public init(id: Int64, title: String, unreadCount: Int = 0, lastMessage: String = "") {
        self.id = id
        self.title = title
        self.unreadCount = unreadCount
        self.lastMessage = lastMessage
    }
}

public struct MessageItemDTO: Codable, Sendable, Equatable, Identifiable {
    public let id: Int64
    public let chatId: Int64
    public let senderId: UInt64
    public let text: String
    public let timestampMs: Int64

    public init(id: Int64, chatId: Int64, senderId: UInt64, text: String, timestampMs: Int64) {
        self.id = id
        self.chatId = chatId
        self.senderId = senderId
        self.text = text
        self.timestampMs = timestampMs
    }
}

public struct PeerTypingDTO: Codable, Sendable, Equatable {
    public let userId: UInt64
    public let isTyping: Bool

    public init(userId: UInt64, isTyping: Bool) {
        self.userId = userId
        self.isTyping = isTyping
    }
}

public struct OutboxEventDTO: Codable, Sendable, Equatable {
    public let clientNonce: UInt64
    public let state: String
    public let messageId: Int64?

    public init(clientNonce: UInt64, state: String, messageId: Int64?) {
        self.clientNonce = clientNonce
        self.state = state
        self.messageId = messageId
    }
}

public struct ReactionUpdateDTO: Codable, Sendable, Equatable {
    public let chatId: Int64
    public let messageId: Int64
    public let emoji: String

    public init(chatId: Int64, messageId: Int64, emoji: String) {
        self.chatId = chatId
        self.messageId = messageId
        self.emoji = emoji
    }
}

public struct PollDTO: Codable, Sendable, Equatable {
    public let pollId: String
    public let payloadJSON: String

    public init(pollId: String, payloadJSON: String) {
        self.pollId = pollId
        self.payloadJSON = payloadJSON
    }
}

public struct CallSignalEventDTO: Codable, Sendable, Equatable {
    public let callId: String
    public let kind: String
    public let payloadJSON: String

    public init(callId: String, kind: String, payloadJSON: String) {
        self.callId = callId
        self.kind = kind
        self.payloadJSON = payloadJSON
    }
}

public struct TalkEventDTO: Codable, Sendable, Equatable {
    public let rms: Float
    public let bargeInCount: UInt64

    public init(rms: Float, bargeInCount: UInt64) {
        self.rms = rms
        self.bargeInCount = bargeInCount
    }
}

public struct HermesChunkDTO: Codable, Sendable, Equatable {
    public let sessionId: String
    public let delta: String
    public let isFinal: Bool

    public init(sessionId: String, delta: String, isFinal: Bool) {
        self.sessionId = sessionId
        self.delta = delta
        self.isFinal = isFinal
    }
}

public struct ApprovalGateDTO: Codable, Sendable, Equatable {
    public let approvalId: String
    public let toolName: String
    public let summary: String

    public init(approvalId: String, toolName: String, summary: String) {
        self.approvalId = approvalId
        self.toolName = toolName
        self.summary = summary
    }
}

public struct ComputerApprovalDTO: Codable, Sendable, Equatable {
    public let nodeId: String
    public let operation: String
    public let generation: UInt64

    public init(nodeId: String, operation: String, generation: UInt64) {
        self.nodeId = nodeId
        self.operation = operation
        self.generation = generation
    }
}

/// Canonical realtime push events surfaced by `FabushiHostClient` (`PROJECT.md § IC.1`).
public enum HostRealtimeEvent: Sendable, Equatable {
    case connectionStateChanged(ConnectionStatus)
    case dialogsUpdated([DialogSummaryDTO])
    case messagesUpdated(chatId: Int64, [MessageItemDTO])
    case typingChanged(chatId: Int64, [PeerTypingDTO])
    case outboxChanged(OutboxEventDTO)
    case reactionUpdated(ReactionUpdateDTO)
    case pollUpdated(PollDTO)
    case callEvent(CallSignalEventDTO)
    case talkBargeIn(TalkEventDTO)
    case hermesStreamChunk(HermesChunkDTO)
    case actionApprovalRequested(ApprovalGateDTO)
    case tailnetLeaseApproval(ComputerApprovalDTO)
    case raw(type: String, payloadJSON: String)
}

/// Thread-safe Swift 6 `Sendable` fan-out broadcaster backed by `OSAllocatedUnfairLock`.
public final class HostEventBroadcaster: Sendable {
    private let subscribers = OSAllocatedUnfairLock(
        initialState: [UUID: AsyncStream<HostRealtimeEvent>.Continuation]()
    )

    public init() {}

    public func makeStream() -> AsyncStream<HostRealtimeEvent> {
        let id = UUID()
        return AsyncStream(bufferingPolicy: .unbounded) { continuation in
            subscribers.withLock { $0[id] = continuation }
            continuation.onTermination = { [weak self] _ in
                self?.subscribers.withLock { _ = $0.removeValue(forKey: id) }
            }
        }
    }

    public func publish(_ event: HostRealtimeEvent) {
        let targets = subscribers.withLock { Array($0.values) }
        for target in targets {
            target.yield(event)
        }
    }

    public func finishAll() {
        let targets = subscribers.withLock { map -> [AsyncStream<HostRealtimeEvent>.Continuation] in
            let all = Array(map.values)
            map.removeAll()
            return all
        }
        for target in targets {
            target.finish()
        }
    }
}

/// Primary Host Bridge protocol shared by M1–M6 (`PROJECT.md § IC.1`).
public protocol FabushiHostClientProtocol: Sendable {
    var eventStream: AsyncStream<HostRealtimeEvent> { get }

    func initialize(
        appSupportDir: URL?,
        defaultServerURL: String,
        keychainBridge: KeychainProvider,
        biometricsBridge: BiometricsProvider
    ) async throws -> RuntimeBootstrapState

    func handleScenePhaseChange(_ phase: AppLifecyclePhase) async

    func dispatchRaw(method: String, payloadJSON: Data) async throws -> Data

    func call<Params: Encodable & Sendable, Response: Decodable & Sendable>(
        _ method: String,
        params: Params
    ) async throws -> Response
}
