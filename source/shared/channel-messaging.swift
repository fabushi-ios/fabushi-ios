import Foundation

let CHANNEL_CONFIG_FILENAME = "connection.json"
let CHANNEL_MAX_LABEL_LENGTH = 80
let CHANNEL_INBOUND_WAKE_CUE = "[inbound]"
let CHANNEL_DELIVERY_FAILED_WAKE_CUE = "[channel-delivery-failed]"
let CHANNEL_CREDENTIAL_FIELD = "token"

func clampChannelLabel(_ label: String) -> String {
    SandText.clampLine(label, maxLength: CHANNEL_MAX_LABEL_LENGTH)
}

struct ChannelOutboundImage: Equatable, Sendable {
    let url: String
}

enum ChannelOutboundInput: Equatable, Sendable {
    case text(content: String, images: [ChannelOutboundImage] = [])
    case attachment(url: String, alt: String?)
    case unsupported(type: String)
}

enum ChannelOutboundMessage: Equatable, Sendable {
    case text(String)
    case attachment(url: String, caption: String?)
}

func buildChannelOutboundMessage(_ message: ChannelOutboundInput) -> ChannelOutboundMessage? {
    switch message {
    case .text(let content, let images):
        if let image = images.first(where: { !$0.url.isEmpty }) {
            return .attachment(url: image.url, caption: content.isEmpty ? nil : content)
        }
        return content.isEmpty ? nil : .text(content)
    case .attachment(let url, let alt):
        let caption = (alt?.isEmpty == false) ? alt : nil
        if !url.isEmpty { return .attachment(url: url, caption: caption) }
        return caption.map(ChannelOutboundMessage.text)
    case .unsupported:
        return nil
    }
}

struct ChannelConnectionSummary: Equatable, Sendable {
    let platform: String
    let label: String
    let status: String
    var detail: String? = nil
}

private func describeChannelConnection(_ connection: ChannelConnectionSummary) -> String {
    let platformName = findConnectorManifest(connection.platform)?.displayName ?? connection.platform
    let detail = connection.status == "error" && connection.detail != nil ? " (\(connection.detail!))" : ""
    return "- \(platformName) \"\(connection.label)\" [\(connection.status)]\(detail). Address people on it as \(connection.platform):<chat id>"
}

func renderChannelsSystemPrompt(
    manifests: [ConnectorManifest],
    connections: [ChannelConnectionSummary],
    location: String?
) -> String {
    guard let location, hasChannelsToShow(manifests: manifests, connections: connections) else { return "" }
    let available = manifests.filter { $0.availability == .available }
    var lines = [
        "Channels: outside messaging surfaces you can talk on, beyond this Fabushi chat.",
        "Each connected channel lives in a subfolder at \(location) holding a \(CHANNEL_CONFIG_FILENAME). Credentials stay in a separate secret store and must never be requested in plain chat or written into files.",
        "Every channel conversation has an address shaped like platform:chat. Use that address deliberately for outbound routing.",
        "Inbound channel activity is prefixed with \(CHANNEL_INBOUND_WAKE_CUE); reply to the same address unless the user explicitly asks otherwise.",
        "Channel messages carry concise text and attachments rather than in-app-only widgets.",
        "Platforms you can connect:",
    ]
    for manifest in available {
        lines.append("- \(manifest.displayName): \(manifest.blurb)")
        for guideLine in manifest.connectGuide.split(separator: "\n", omittingEmptySubsequences: false) {
            lines.append("  \(guideLine)")
        }
    }
    let comingSoon = manifests.filter { $0.availability == .comingSoon }
    if !comingSoon.isEmpty {
        lines.append("Coming soon (not connectable yet): \(comingSoon.map(\.displayName).joined(separator: ", ")).")
    }
    if connections.isEmpty {
        lines.append("No channels connected yet.")
    } else {
        lines.append("Currently connected:")
        lines.append(contentsOf: connections.map(describeChannelConnection))
    }
    return lines.joined(separator: "\n")
}

struct ChannelReaction: Equatable, Sendable {
    let emoji: String
    var messageQuote: String? = nil
}

struct ChannelInboundEnvelope: Equatable, Sendable {
    let address: ChannelAddress
    let sender: String
    let text: String
    var reaction: ChannelReaction? = nil
}

func formatChannelReactionSummary(_ reaction: ChannelReaction) -> String {
    if let quote = reaction.messageQuote, !quote.isEmpty {
        return "reacted \(reaction.emoji) to your message: \"\(quote)\""
    }
    return "reacted \(reaction.emoji) to a message"
}

func buildChannelInboundWakePrompt(_ envelopes: [ChannelInboundEnvelope]) -> String {
    var order: [String] = []
    var grouped: [String: [ChannelInboundEnvelope]] = [:]
    for envelope in envelopes {
        let key = formatChannelAddress(envelope.address)
        if grouped[key] == nil { order.append(key) }
        grouped[key, default: []].append(envelope)
    }
    let blocks = order.compactMap { addressToken -> String? in
        guard let bucket = grouped[addressToken], let head = bucket.first else { return nil }
        let platformName = findConnectorManifest(head.address.platform)?.displayName ?? head.address.platform
        let transcript = bucket.map { envelope in
            if let reaction = envelope.reaction {
                return "  \(envelope.sender) \(formatChannelReactionSummary(reaction))"
            }
            return "  \(envelope.sender): \(envelope.text)"
        }.joined(separator: "\n")
        return "On \(platformName), from \(addressToken):\n\(transcript)"
    }
    let hasMessage = envelopes.contains { $0.reaction == nil }
    let plural = envelopes.count == 1 ? "" : "s"
    let opening = hasMessage
        ? "\(CHANNEL_INBOUND_WAKE_CUE) New message\(plural) on a channel you are connected to."
        : "\(CHANNEL_INBOUND_WAKE_CUE) New reaction\(plural) on a channel you are connected to."
    let closing = hasMessage
        ? "Reply using the channel address shown above and keep each message concise."
        : "No reply is required for a reaction unless acting on it is useful."
    return [opening, "This is activity from someone on an outside platform, not the user typing in this app.", "", blocks.joined(separator: "\n\n"), "", closing].joined(separator: "\n")
}

func humanizeChannelDeliveryFailure(_ addressToken: String, rawMessage: String) -> String {
    let address = parseChannelAddress(addressToken)
    let platformName = address.flatMap { findConnectorManifest($0.platform)?.displayName ?? $0.platform }
    let trimmed = rawMessage.trimmingCharacters(in: .whitespacesAndNewlines)
    if address == nil || trimmed.localizedCaseInsensitiveContains("not a valid channel address") {
        return "\"\(addressToken)\" isn't a valid channel address, so that message wasn't delivered."
    }
    if trimmed == "No channel delivery mechanism is registered." {
        return "Channel messaging isn't available on this computer, so that message wasn't delivered."
    }
    if trimmed.lowercased().contains("no live") && trimmed.lowercased().contains("connection") {
        return "\(platformName!) isn't connected on this computer, so that message wasn't delivered."
    }
    return "Couldn't deliver that message to \(platformName!): \(trimmed)"
}

struct ChannelDeliveryFailure: Equatable, Sendable {
    let addressToken: String
    let reason: String
}

func buildChannelDeliveryFailureWakePrompt(_ failures: [ChannelDeliveryFailure]) -> String {
    let lines = failures.map { "- To \($0.addressToken): \($0.reason)" }
    return ([
        "\(CHANNEL_DELIVERY_FAILED_WAKE_CUE) A message you tried to send to a channel did not go through.",
        "This is a system notice about your outbound send, not a new user message.",
    ] + lines + [
        "Correct the record in this in-app chat; do not silently retry the same failed channel.",
    ]).joined(separator: "\n")
}
