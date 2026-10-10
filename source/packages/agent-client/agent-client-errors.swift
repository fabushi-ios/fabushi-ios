import Foundation

/// iOS-native counterpart of Grok's agent-client error classifier.
///
/// The reference client classifies Connect/backend failures into a small set
/// of product decisions. iOS keeps that policy independent of transport
/// implementation so Coordinator/Host callers can preserve retry and
/// action-required behavior without importing ConnectRPC JavaScript types.
enum AgentClientErrorKind: Equatable, Sendable {
    case retriable
    case actionRequired(String)
    case nonRetriable
    case cancelled
}

struct AgentClientDisplayInfo: Equatable, Sendable {
    let title: String?
    let detail: String?
    let isRetryable: Bool?
    let connectCode: Int?
    let errorCode: String?
    let inferenceRequestErrorType: String?

    init(
        title: String? = nil,
        detail: String? = nil,
        isRetryable: Bool? = nil,
        connectCode: Int? = nil,
        errorCode: String? = nil,
        inferenceRequestErrorType: String? = nil
    ) {
        self.title = title
        self.detail = detail
        self.isRetryable = isRetryable
        self.connectCode = connectCode
        self.errorCode = errorCode
        self.inferenceRequestErrorType = inferenceRequestErrorType
    }
}

struct AgentClientClassifiedError: Error, Equatable, Sendable {
    let kind: AgentClientErrorKind
    let message: String
    let requestId: String?
    let displayInfo: AgentClientDisplayInfo?
    let isTransport: Bool
}

struct AgentClientConnectFailure: Equatable, Sendable {
    let code: Int
    let message: String
    let metadata: [String: String]
    let errorCode: String?
    let title: String?
    let detail: String?
    let isRetryable: Bool?
    let backendActionRequired: String?
    let transportEvidence: [String]

    init(
        code: Int,
        message: String,
        metadata: [String: String] = [:],
        errorCode: String? = nil,
        title: String? = nil,
        detail: String? = nil,
        isRetryable: Bool? = nil,
        backendActionRequired: String? = nil,
        transportEvidence: [String] = []
    ) {
        self.code = code
        self.message = message
        self.metadata = metadata
        self.errorCode = errorCode
        self.title = title
        self.detail = detail
        self.isRetryable = isRetryable
        self.backendActionRequired = backendActionRequired
        self.transportEvidence = transportEvidence
    }
}

enum AgentClientFailure: Equatable, Sendable {
    case connect(AgentClientConnectFailure)
    case abort(message: String)
    case blobNotFound(message: String)
    case standard(message: String, name: String, code: String?, causes: [String])
    case opaque(String)
}

enum AgentClientConnectCode {
    /// Connect/gRPC canonical status values.
    static let cancelled = 1
    static let aborted = 10
    static let unauthenticated = 16
}

private let agentClientAuthCodes: Set<String> = [
    "NOT_LOGGED_IN",
    "AGENT_REQUIRES_LOGIN",
    "AUTH_TOKEN_NOT_FOUND",
    "AUTH_TOKEN_EXPIRED",
    "INVALID_AUTH_ID",
    "UNAUTHORIZED",
    "GITHUB_NO_USER_CREDENTIALS",
    "GITHUB_USER_NO_ACCESS",
]

private let agentClientUpgradeCodes: Set<String> = [
    "FREE_USER_USAGE_LIMIT",
    "FREE_USER_RATE_LIMIT_EXCEEDED",
    "PRO_USER_ONLY",
    "PRO_USER_USAGE_LIMIT",
    "PRO_USER_RATE_LIMIT_EXCEEDED",
    "RATE_LIMITED",
    "RATE_LIMITED_CHANGEABLE",
    "GENERIC_RATE_LIMIT_EXCEEDED",
]

private let agentClientPaymentCodes: Set<String> = [
    "USAGE_PRICING_REQUIRED",
    "USAGE_PRICING_REQUIRED_CHANGEABLE",
]

private let agentClientConfigCodes: Set<String> = [
    "BAD_API_KEY",
    "BAD_USER_API_KEY",
    "OUTDATED_CLIENT",
]

private let agentClientCancelledCodes: Set<String> = [
    "USER_ABORTED_REQUEST",
    "DEBOUNCED",
]

private let agentClientTerminalMessageCodes: Set<String> = [
    "CUSTOM_MESSAGE",
]

private let agentClientTransportPatterns = [
    "NGHTTP2",
    "ECONNRESET",
    "ECONNREFUSED",
    "ETIMEDOUT",
    "EPIPE",
    "socket hang up",
    "Premature close",
    "ERR_STREAM",
    "protocol error",
    "http/2 stream",
    "ERR_HTTP2_SESSION_ERROR",
    "Session closed with error code",
    "connection aborted",
]

private let agentClientNetworkCodes: Set<String> = [
    "ENOTFOUND",
    "EAI_AGAIN",
    "EAI_FAIL",
    "ENODATA",
    "ESERVFAIL",
    "EHOSTUNREACH",
    "ENETDOWN",
    "ENETUNREACH",
]

private let agentClientInferenceHeader = "x-cursor-inference-request-error-type"

func classifyAgentClientError(
    _ failure: AgentClientFailure,
    requestId: String? = nil
) -> AgentClientClassifiedError {
    switch failure {
    case .connect(let failure):
        return classifyAgentClientConnectFailure(failure, requestId: requestId)

    case .blobNotFound(let message):
        return .init(
            kind: .nonRetriable,
            message: message,
            requestId: requestId,
            displayInfo: .init(
                title: "Conversation data missing",
                detail: message,
                isRetryable: false
            ),
            isTransport: false
        )

    case .abort(let message):
        return .init(
            kind: .cancelled,
            message: message,
            requestId: requestId,
            displayInfo: nil,
            isTransport: false
        )

    case .standard(let message, let name, let code, let causes):
        let evidence = [name + ": " + message] + (code.map { [$0] } ?? []) + causes
        let transport = evidence.contains(where: agentClientMatchesTransportEvidence)
        return .init(
            kind: transport ? .retriable : .retriable,
            message: message,
            requestId: requestId,
            displayInfo: nil,
            isTransport: transport
        )

    case .opaque(let value):
        return .init(
            kind: .retriable,
            message: value,
            requestId: requestId,
            displayInfo: nil,
            isTransport: false
        )
    }
}

private func classifyAgentClientConnectFailure(
    _ failure: AgentClientConnectFailure,
    requestId: String?
) -> AgentClientClassifiedError {
    let transport = ([failure.message] + failure.transportEvidence)
        .contains(where: agentClientMatchesTransportEvidence)
    let display = AgentClientDisplayInfo(
        title: failure.title,
        detail: failure.detail,
        isRetryable: failure.isRetryable,
        connectCode: failure.code,
        errorCode: failure.errorCode,
        inferenceRequestErrorType: failure.metadata[agentClientInferenceHeader]
    )
    let message = agentClientDisplayMessage(
        fallback: failure.message,
        title: failure.title,
        detail: failure.detail
    )

    let kind: AgentClientErrorKind
    if failure.code == AgentClientConnectCode.cancelled || failure.code == AgentClientConnectCode.aborted {
        kind = transport ? .retriable : .cancelled
    } else if let errorCode = failure.errorCode {
        if agentClientCancelledCodes.contains(errorCode) {
            kind = .cancelled
        } else if let action = failure.backendActionRequired?.trimmingCharacters(in: .whitespacesAndNewlines),
                  !action.isEmpty {
            kind = .actionRequired(action)
        } else if agentClientAuthCodes.contains(errorCode) {
            kind = .actionRequired("login")
        } else if agentClientUpgradeCodes.contains(errorCode) {
            kind = .actionRequired("upgrade")
        } else if agentClientPaymentCodes.contains(errorCode) {
            kind = .actionRequired("payment")
        } else if agentClientConfigCodes.contains(errorCode) {
            kind = .actionRequired("config")
        } else if agentClientTerminalMessageCodes.contains(errorCode), failure.isRetryable != true {
            kind = .nonRetriable
        } else if failure.isRetryable == false {
            kind = .nonRetriable
        } else {
            kind = .retriable
        }
    } else if failure.code == AgentClientConnectCode.unauthenticated {
        kind = .actionRequired("login")
    } else {
        kind = .retriable
    }

    return .init(
        kind: kind,
        message: message,
        requestId: requestId,
        displayInfo: display,
        isTransport: transport
    )
}

private func agentClientDisplayMessage(
    fallback: String,
    title: String?,
    detail: String?
) -> String {
    switch (title?.trimmingCharacters(in: .whitespacesAndNewlines),
            detail?.trimmingCharacters(in: .whitespacesAndNewlines)) {
    case let (title?, detail?) where !title.isEmpty && !detail.isEmpty:
        return "\(title) \(detail)"
    case let (title?, _) where !title.isEmpty:
        return title
    case let (_, detail?) where !detail.isEmpty:
        return detail
    default:
        return fallback
    }
}

private func agentClientMatchesTransportEvidence(_ value: String) -> Bool {
    if agentClientTransportPatterns.contains(where: value.contains) {
        return true
    }
    let tokens = value.split { !$0.isLetter && !$0.isNumber && $0 != "_" }
    return tokens.contains { agentClientNetworkCodes.contains(String($0)) }
}
