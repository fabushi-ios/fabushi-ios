import Foundation

/// iOS replacement for Grok's Node localhost:8787 OAuth listener.
///
/// The reference file owns OAuth state/TTL/completion semantics. iOS preserves
/// those semantics but receives the callback through its registered app URL
/// scheme / ASWebAuthenticationSession instead of binding a TCP listener.
let MCP_OAUTH_IOS_CALLBACK_URL = "fabushi://auth/callback"
let MCP_OAUTH_LOOPBACK_CALLBACK_URL = MCP_OAUTH_IOS_CALLBACK_URL
let BACKEND_MCP_OAUTH_PENDING_STATE_TTL_MS = 15 * 60 * 1_000
let MCP_OAUTH_PENDING_TTL_MS = BACKEND_MCP_OAUTH_PENDING_STATE_TTL_MS + 60_000
let MCP_OAUTH_COMPLETION_RETRY_DELAY_MS = 500

struct ParsedMcpOAuthAuthorization: Equatable, Sendable {
    let state: String
}

private func oauthCallbackIdentity(_ url: URL) -> (scheme: String, host: String, port: Int?, path: String)? {
    guard let scheme = url.scheme?.lowercased(),
          let host = url.host?.lowercased() else { return nil }
    return (scheme, host, url.port, url.path)
}

func isMcpOAuthIOSCallback(
    _ url: URL,
    callback: String = MCP_OAUTH_IOS_CALLBACK_URL
) -> Bool {
    guard let expected = URL(string: callback),
          let lhs = oauthCallbackIdentity(url),
          let rhs = oauthCallbackIdentity(expected) else { return false }
    return lhs.scheme == rhs.scheme
        && lhs.host == rhs.host
        && lhs.port == rhs.port
        && lhs.path == rhs.path
}

func parseMcpOAuthLoopbackAuthorization(
    _ authorizationUrl: String,
    callback: String = MCP_OAUTH_IOS_CALLBACK_URL
) -> ParsedMcpOAuthAuthorization? {
    guard let authorization = URLComponents(string: authorizationUrl),
          let rawRedirect = authorization.queryItems?.first(where: { $0.name == "redirect_uri" })?.value,
          let state = authorization.queryItems?.first(where: { $0.name == "state" })?.value,
          !state.isEmpty,
          let redirect = URL(string: rawRedirect),
          isMcpOAuthIOSCallback(redirect, callback: callback) else {
        return nil
    }
    return .init(state: state)
}

enum McpOAuthCallbackFailureReason: String, Equatable, Sendable {
    case providerError = "provider_error"
    case missingCode = "missing_code"
    case completionRejected = "completion_rejected"
    case completionTimeout = "completion_timeout"
}

enum McpOAuthCallbackOutcome: Equatable, Sendable {
    case success
    case refused(McpOAuthCallbackFailureReason)
    case failed(McpOAuthCallbackFailureReason, retryable: Bool)
    case notFound
    case unsupportedURL
}

struct McpOAuthCallbackEvent: Equatable, Sendable {
    let phase: String
    let outcome: String
    var serverName: String? = nil
    var failureReason: McpOAuthCallbackFailureReason? = nil
}

private func defaultMcpOAuthRetryable(_ error: Error) -> Bool {
    guard let urlError = error as? URLError else { return false }
    switch urlError.code {
    case .timedOut, .networkConnectionLost, .notConnectedToInternet,
         .cannotConnectToHost, .cannotFindHost, .dnsLookupFailed:
        return true
    default:
        return false
    }
}

actor SandMcpOAuthCallbackLifecycle {
    typealias CompleteOAuth = @Sendable (_ stateId: String, _ code: String) async throws -> Void

    private struct CompletionResult: Sendable {
        let outcome: McpOAuthCallbackOutcome
    }

    private struct Pending {
        var serverName: String?
        var expiresAtMs: Int64
        var completing: Task<CompletionResult, Never>?
    }

    private let completeOAuth: CompleteOAuth
    private let callback: String
    private let nowMs: @Sendable () -> Int64
    private let sleepMs: @Sendable (Int) async -> Void
    private let shouldRetry: @Sendable (Error) -> Bool
    private let onCallback: (@Sendable (McpOAuthCallbackEvent) -> Void)?
    private var pending: [String: Pending] = [:]
    private var disposed = false

    init(
        callback: String = MCP_OAUTH_IOS_CALLBACK_URL,
        completeOAuth: @escaping CompleteOAuth,
        nowMs: @escaping @Sendable () -> Int64 = {
            Int64(Date().timeIntervalSince1970 * 1_000)
        },
        sleepMs: @escaping @Sendable (Int) async -> Void = { milliseconds in
            try? await Task.sleep(for: .milliseconds(milliseconds))
        },
        shouldRetry: @escaping @Sendable (Error) -> Bool = defaultMcpOAuthRetryable,
        onCallback: (@Sendable (McpOAuthCallbackEvent) -> Void)? = nil
    ) {
        self.callback = callback
        self.completeOAuth = completeOAuth
        self.nowMs = nowMs
        self.sleepMs = sleepMs
        self.shouldRetry = shouldRetry
        self.onCallback = onCallback
    }

    func registerPendingAuthFromUrl(
        authorizationUrl: String,
        serverName: String? = nil
    ) -> Bool {
        guard !disposed,
              let parsed = parseMcpOAuthLoopbackAuthorization(
                authorizationUrl,
                callback: callback
              ) else { return false }
        let expiry = nowMs() + Int64(MCP_OAUTH_PENDING_TTL_MS)
        if var known = pending[parsed.state] {
            known.expiresAtMs = expiry
            if known.serverName == nil { known.serverName = serverName }
            pending[parsed.state] = known
        } else {
            pending[parsed.state] = .init(
                serverName: serverName,
                expiresAtMs: expiry,
                completing: nil
            )
        }
        return true
    }

    func hasPendingState(_ state: String) -> Bool {
        expirePending()
        return pending[state] != nil
    }

    func handleCallback(_ url: URL) async -> McpOAuthCallbackOutcome {
        guard !disposed else { return .notFound }
        guard isMcpOAuthIOSCallback(url, callback: callback) else {
            return .unsupportedURL
        }
        expirePending()

        guard let components = URLComponents(url: url, resolvingAgainstBaseURL: false),
              let state = components.queryItems?.first(where: { $0.name == "state" })?.value,
              var auth = pending[state] else {
            return .notFound
        }

        if components.queryItems?.contains(where: { $0.name == "error" }) == true {
            pending.removeValue(forKey: state)
            emit(.providerError, serverName: auth.serverName, outcome: "failed")
            return .refused(.providerError)
        }
        guard let code = components.queryItems?.first(where: { $0.name == "code" })?.value,
              !code.isEmpty else {
            pending.removeValue(forKey: state)
            emit(.missingCode, serverName: auth.serverName, outcome: "failed")
            return .refused(.missingCode)
        }

        let task: Task<CompletionResult, Never>
        if let existing = auth.completing {
            task = existing
        } else {
            let completeOAuth = self.completeOAuth
            let shouldRetry = self.shouldRetry
            let sleepMs = self.sleepMs
            task = Task {
                var lastError: Error?
                for attempt in 0..<2 {
                    do {
                        try await completeOAuth(state, code)
                        return .init(outcome: .success)
                    } catch {
                        lastError = error
                        guard attempt == 0, shouldRetry(error) else { break }
                        await sleepMs(MCP_OAUTH_COMPLETION_RETRY_DELAY_MS)
                    }
                }
                let retryable = lastError.map(shouldRetry) ?? false
                let reason: McpOAuthCallbackFailureReason =
                    (lastError as? URLError)?.code == .timedOut
                        ? .completionTimeout
                        : .completionRejected
                return .init(outcome: .failed(reason, retryable: retryable))
            }
            auth.completing = task
            pending[state] = auth
        }

        let result = await task.value
        switch result.outcome {
        case .success:
            pending.removeValue(forKey: state)
            onCallback?(.init(
                phase: "callback_received",
                outcome: "ok",
                serverName: auth.serverName
            ))
        case .failed(let reason, let retryable):
            if retryable {
                if var current = pending[state] {
                    current.completing = nil
                    pending[state] = current
                }
            } else {
                pending.removeValue(forKey: state)
            }
            emit(reason, serverName: auth.serverName, outcome: "failed")
        case .refused(let reason):
            pending.removeValue(forKey: state)
            emit(reason, serverName: auth.serverName, outcome: "failed")
        case .notFound, .unsupportedURL:
            break
        }
        return result.outcome
    }

    func expirePending() {
        let now = nowMs()
        pending = pending.filter { $0.value.expiresAtMs > now }
    }

    func dispose() {
        disposed = true
        for item in pending.values {
            item.completing?.cancel()
        }
        pending.removeAll()
    }

    private func emit(
        _ reason: McpOAuthCallbackFailureReason,
        serverName: String?,
        outcome: String
    ) {
        onCallback?(.init(
            phase: "callback_received",
            outcome: outcome,
            serverName: serverName,
            failureReason: reason
        ))
    }
}
