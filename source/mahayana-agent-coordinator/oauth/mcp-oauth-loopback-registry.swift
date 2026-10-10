import Foundation

actor MCPOAuthCallbackRegistry {
    struct Pending: Equatable, Sendable {
        let state: String
        let providerIdentifier: String
        let createdAt: Date
    }

    private var pending: [String: Pending] = [:]

    func register(state: String, providerIdentifier: String, now: Date = Date()) {
        guard !state.isEmpty, !providerIdentifier.isEmpty else { return }
        pending[state] = .init(state: state, providerIdentifier: providerIdentifier, createdAt: now)
    }

    func consume(
        state: String,
        maxAge: TimeInterval = TimeInterval(MCP_OAUTH_PENDING_TTL_MS) / 1_000,
        now: Date = Date()
    ) -> Pending? {
        guard let value = pending.removeValue(forKey: state),
              now.timeIntervalSince(value.createdAt) <= maxAge
        else { return nil }
        return value
    }

    func clearExpired(
        maxAge: TimeInterval = TimeInterval(MCP_OAUTH_PENDING_TTL_MS) / 1_000,
        now: Date = Date()
    ) {
        pending = pending.filter { now.timeIntervalSince($0.value.createdAt) <= maxAge }
    }
}
