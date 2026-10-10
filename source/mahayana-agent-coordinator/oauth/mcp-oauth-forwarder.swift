import Foundation

@MainActor
final class MCPOAuthForwarder {
    typealias Forward = @MainActor (_ providerIdentifier: String, _ callbackURL: URL) async throws -> Void

    private let listener: MCPOAuthCallbackListener
    private let forward: Forward

    init(listener: MCPOAuthCallbackListener, forward: @escaping Forward) {
        self.listener = listener
        self.forward = forward
    }

    func handleOpenURL(_ url: URL) async throws {
        let accepted = try await listener.accept(url)
        try await forward(accepted.providerIdentifier, accepted.callbackURL)
    }
}
