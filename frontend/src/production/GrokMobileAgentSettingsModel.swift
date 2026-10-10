import Foundation

internal enum GrokMobileAgentSettingsModel {
    enum Pending: Equatable {
        case profile
        case notifications
    }

    struct Profile: Equatable {
        let name: String
        let title: String?
        let description: String
    }

    struct MutationFence: Equatable {
        let accountScopeKey: String
        let agentId: String
        let generation: Int
    }

    static func profile(from agent: MobileBotSummary) -> Profile {
        Profile(
            name: agent.name,
            title: agent.isGroup ? nil : agent.title,
            description: agent.description
        )
    }

    static func normalizedProfile(
        name: String,
        title: String?,
        description: String,
        isGroup: Bool
    ) -> Profile? {
        let normalizedName = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !normalizedName.isEmpty else { return nil }
        let normalizedTitle = isGroup
            ? nil
            : title?.trimmingCharacters(in: .whitespacesAndNewlines)
        return Profile(
            name: normalizedName,
            title: normalizedTitle,
            description: description.trimmingCharacters(in: .whitespacesAndNewlines)
        )
    }

    static func accepts(
        _ fence: MutationFence,
        accountScopeKey: String,
        agentId: String,
        generation: Int
    ) -> Bool {
        fence.accountScopeKey == accountScopeKey
            && fence.agentId == agentId
            && fence.generation == generation
    }
}
