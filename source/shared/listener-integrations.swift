import Foundation

enum ListenerIntegrationPlatform: String, Codable, CaseIterable, Equatable, Sendable {
    case github
    case slack
}

struct ListenerIntegrationManifest: Codable, Equatable, Sendable {
    let platform: ListenerIntegrationPlatform
    let displayName: String
    let blurb: String
}

let LISTENER_INTEGRATIONS: [ListenerIntegrationManifest] = [
    .init(
        platform: .github,
        displayName: "GitHub",
        blurb: "Let automations watch a repo's PRs, comments, issues, and CI."
    ),
    .init(
        platform: .slack,
        displayName: "Slack",
        blurb: "Wake automations on Slack messages, mentions, and reactions."
    ),
]

func listenerIntegrationManifest(_ platform: String) -> ListenerIntegrationManifest? {
    LISTENER_INTEGRATIONS.first { $0.platform.rawValue == platform }
}
