import Foundation

enum IOSInferenceProvider: String, Codable, CaseIterable, Sendable {
    case cursor
    case codex
    case claudeCode = "claude-code"
    case openRouter = "openrouter"
}

@MainActor
final class CoordinatorInferenceRouter {
    typealias SendPrompt = @MainActor (
        _ provider: IOSInferenceProvider,
        _ agentId: String,
        _ prompt: String,
        _ tools: RoutedMCPBridge?
    ) async throws -> String

    private(set) var provider: IOSInferenceProvider
    private let sendPrompt: SendPrompt

    init(provider: IOSInferenceProvider = .codex, sendPrompt: @escaping SendPrompt) {
        self.provider = provider
        self.sendPrompt = sendPrompt
    }

    func setProvider(_ provider: IOSInferenceProvider) {
        self.provider = provider
    }

    func send(agentId: String, prompt: String, tools: RoutedMCPBridge? = nil) async throws -> String {
        guard !agentId.isEmpty, !prompt.isEmpty else { return "" }
        return try await sendPrompt(provider, agentId, prompt, tools)
    }
}
