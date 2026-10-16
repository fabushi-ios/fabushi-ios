import Foundation

enum SendMessagePreviewInput: Equatable, Sendable {
    case text(String)
    case attachment(url: String)
    case widget(prompt: String)
    case cursorAgent(title: String?)
    case secretRequest(label: String)
    case emailDraft(subject: String, body: String)
    case slackDraft(body: String)
    case permissionRequest(title: String)
    case autoReviewApproval(summary: String)
    case localToolPermission(target: String)
    case connector(variant: String, connector: String)
    case connectors([String])
    case listenerConnect(platform: String)
}

enum SendMessagePreview {
    static let cursorAgentFallback = "Cursor cloud agent"

    static func cursorAgentText(title: String?) -> String {
        let trimmed = title?.trimmingCharacters(in: .whitespacesAndNewlines)
        return (trimmed?.isEmpty == false) ? "Cursor agent: \(trimmed!)" : cursorAgentFallback
    }

    static func connectorsText(_ connectors: [String]) -> String {
        connectors.isEmpty ? "Connect tools" : "Connect \(connectors.joined(separator: ", "))"
    }

    static func text(for message: SendMessagePreviewInput) -> String {
        switch message {
        case .text(let content):
            content
        case .attachment(let url):
            url
        case .widget(let prompt):
            prompt
        case .cursorAgent(let title):
            cursorAgentText(title: title)
        case .secretRequest(let label):
            label
        case .emailDraft(let subject, let body):
            subject.isEmpty ? body : subject
        case .slackDraft(let body):
            body
        case .permissionRequest(let title):
            title
        case .autoReviewApproval(let summary):
            "Approval required: \(summary)"
        case .localToolPermission(let target):
            "Permission required: \(target)"
        case .connector(let variant, let connector):
            variant == "connected" ? "\(connector) connected" : "Connect \(connector)"
        case .connectors(let connectors):
            connectorsText(connectors)
        case .listenerConnect(let platform):
            "Connect \(platform == "slack" ? "Slack" : "GitHub")"
        }
    }
}
