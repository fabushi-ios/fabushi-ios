import Foundation

enum SandInferenceProvider: String, Codable, CaseIterable, Hashable, Sendable {
    case cursor
    case claudeCode = "claude-code"
    case codex
    case fabushi
    case openrouter
}

let SAND_INFERENCE_PROVIDERS = SandInferenceProvider.allCases.map(\.rawValue)

struct SandInferenceRouterUsageProvider: Codable, Equatable, Sendable {
    var requests: Int
    var inputTokens: Int
    var outputTokens: Int
    var cacheReadTokens: Int
    var cacheWriteTokens: Int
    var lastUsedAt: String?
}

struct SandInferenceRouterUsage: Codable, Equatable, Sendable {
    let schemaVersion: Int
    var providers: [SandInferenceProvider: SandInferenceRouterUsageProvider]
}

func isSandInferenceProvider(_ value: Any) -> Bool {
    guard let value = value as? String else { return false }
    return SandInferenceProvider(rawValue: value) != nil
}

func emptySandInferenceRouterUsage() -> SandInferenceRouterUsage {
    func empty() -> SandInferenceRouterUsageProvider {
        .init(
            requests: 0,
            inputTokens: 0,
            outputTokens: 0,
            cacheReadTokens: 0,
            cacheWriteTokens: 0,
            lastUsedAt: nil
        )
    }
    return .init(
        schemaVersion: 1,
        providers: Dictionary(
            uniqueKeysWithValues: SandInferenceProvider.allCases.map { ($0, empty()) }
        )
    )
}
