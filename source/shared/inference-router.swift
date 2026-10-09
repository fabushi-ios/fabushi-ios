import Foundation

enum SandInferenceProvider: String, Codable, CaseIterable, Hashable, Sendable {
    case cursor
    case claudeCode = "claude-code"
    case codex
    case fabushi
    case openrouter
}

enum SandInferenceUsageSource: String, Equatable, Sendable {
    case fabushi
    case cursor
    case external
}

struct SandInferenceProviderDescriptor: Equatable, Sendable {
    let provider: SandInferenceProvider
    let label: String
    let description: String
    let usageDescription: String
    let usageSource: SandInferenceUsageSource
}

let SAND_INFERENCE_PROVIDER_DESCRIPTORS: [SandInferenceProviderDescriptor] = [
    .init(
        provider: .fabushi,
        label: "Fabushi",
        description: "Use Fabushi's native Coordinator and account-backed inference route.",
        usageDescription: "Usage is managed by the signed-in Fabushi account.",
        usageSource: .fabushi
    ),
    .init(
        provider: .cursor,
        label: "Cursor",
        description: "Use the signed-in Cursor account and hosted agent models.",
        usageDescription: "Included and on-demand usage is managed by the Cursor account.",
        usageSource: .cursor
    ),
    .init(
        provider: .claudeCode,
        label: "Claude Code",
        description: "Route agent requests through Anthropic Claude Code.",
        usageDescription: "Usage is managed by the connected Anthropic account.",
        usageSource: .external
    ),
    .init(
        provider: .codex,
        label: "Codex",
        description: "Route agent requests through OpenAI Codex.",
        usageDescription: "Usage is managed by the connected OpenAI account.",
        usageSource: .external
    ),
    .init(
        provider: .openrouter,
        label: "OpenRouter",
        description: "Route requests through models and billing in OpenRouter.",
        usageDescription: "Usage and spend are managed in OpenRouter.",
        usageSource: .external
    ),
]

func sandInferenceProviderDescriptor(
    _ provider: SandInferenceProvider
) -> SandInferenceProviderDescriptor {
    SAND_INFERENCE_PROVIDER_DESCRIPTORS.first { $0.provider == provider }
        ?? SAND_INFERENCE_PROVIDER_DESCRIPTORS[0]
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
