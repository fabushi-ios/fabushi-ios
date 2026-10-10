import Foundation

/// iOS does not open a desktop loopback listener. OAuth returns through the
/// registered app/universal-link callback and is validated against the state registry.
actor MCPOAuthCallbackListener {
    enum CallbackError: LocalizedError, Equatable {
        case unsupportedURL
        case missingState
        case stateMismatch

        var errorDescription: String? {
            switch self {
            case .unsupportedURL: "unsupported OAuth callback URL"
            case .missingState: "OAuth callback is missing state"
            case .stateMismatch: "OAuth callback state is stale or unknown"
            }
        }
    }

    private let registry: MCPOAuthCallbackRegistry

    init(registry: MCPOAuthCallbackRegistry) {
        self.registry = registry
    }

    func accept(_ url: URL) async throws -> (providerIdentifier: String, callbackURL: URL) {
        guard isMcpOAuthIOSCallback(url) else {
            throw CallbackError.unsupportedURL
        }
        let components = URLComponents(url: url, resolvingAgainstBaseURL: false)
        guard let state = components?.queryItems?.first(where: { $0.name == "state" })?.value, !state.isEmpty else {
            throw CallbackError.missingState
        }
        guard let pending = await registry.consume(state: state) else {
            throw CallbackError.stateMismatch
        }
        return (pending.providerIdentifier, url)
    }
}
