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


let LISTENER_INTEGRATIONS_DASHBOARD_URL = "https://cursor.com/dashboard?tab=integrations"

struct ListenerScopeIssue: Equatable, Sendable {
    let kind: String
    let scope: String
}

func listenerScopeDiagnostic(_ issues: [ListenerScopeIssue]) -> String {
    let missingBot = issues
        .filter { $0.kind == "bot-not-in-channel" }
        .map(\.scope)
    let notFound = issues
        .filter { $0.kind == "not-found" }
        .map(\.scope)
    var parts: [String] = []
    if !missingBot.isEmpty {
        parts.append(
            "Invite @Cursor to \(missingBot.joined(separator: ", ")) in Slack — messages there can't reach this listener until the bot joins."
        )
    }
    if !notFound.isEmpty {
        parts.append(
            "Couldn't find \(notFound.joined(separator: ", ")) — check the name, or invite @Cursor to it first if it's a private channel."
        )
    }
    return parts.joined(separator: " ")
}

func listenerConnectURL(
    platform: ListenerIntegrationPlatform,
    slackInstallURL: String?
) -> String {
    if platform == .slack,
       let url = slackInstallURL?.trimmingCharacters(in: .whitespacesAndNewlines),
       url.hasPrefix("https://"),
       !url.isEmpty
    {
        return url
    }
    return LISTENER_INTEGRATIONS_DASHBOARD_URL
}

func filterListenerAgentChannels(_ channels: [[String: Any]]) -> [[String: Any]] {
    channels.filter { channel in
        guard let platform = channel["platform"] as? String else { return false }
        return platform == ListenerIntegrationPlatform.github.rawValue
            || platform == ListenerIntegrationPlatform.slack.rawValue
    }
}

func listenerAgentChannelsView(_ channels: [[String: Any]]) -> [String: Any] {
    [
        "manifests": LISTENER_INTEGRATIONS.map { ["platform": $0.platform.rawValue] },
        "connections": filterListenerAgentChannels(channels),
    ]
}
