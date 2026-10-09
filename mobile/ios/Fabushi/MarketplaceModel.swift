import AuthenticationServices
import Foundation
import Observation
import UIKit

enum MobileChatRole: String, Equatable {
    case user
    case assistant
}

enum MobileChatEntryKind: String, Equatable {
    case message
    case action
    case thinking
    case handoff
    case notice
    case permissionRequest
    case timelineEvent
}

enum MahayanaChatPumpOutcome: Equatable {
    case terminal
    case nonTerminal

    var shouldSettleLifecycle: Bool { self == .terminal }
}

struct MobileLinkMetadata: Equatable {
    let url: String
    var title: String?
    var description: String?
    var hostname: String?
    var imageURL: String?
    var imageDataURL: String?
    var faviconDataURL: String?

    var displayTitle: String {
        for candidate in [title, hostname] {
            if let candidate, !candidate.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                return candidate
            }
        }
        return url
    }
}

struct MobileSendMessageTextImage: Equatable {
    let url: String
    var alt: String?
}

enum MobileSendMessageTextPresentation: Equatable {
    case text
    case urlCard(String)
}

struct MobileSendMessageTextProjection: Equatable {
    let id: String
    let content: String
    let images: [MobileSendMessageTextImage]
    var channel: String?
    let streaming: Bool
    var timestampMs: Double?
    let presentation: MobileSendMessageTextPresentation
}

enum MobileAttachmentCardKind: String, Equatable {
    case box
    case legacyLink
    case media
    case file
}

struct MobileAttachmentCardProjection: Equatable {
    let id: String
    let kind: MobileAttachmentCardKind
    let url: String
    var name: String?
    var alt: String?
    var instruction: String?
    var request: String?
    var requestId: String?
    var resolution: String?
    var screenshotDataURL: String?
    var byteSize: Double?
    var width: Double?
    var height: Double?
    var timestampMs: Double?
    var batchId: String?
    var replyTo: String?
    var clientNonce: String?
}

struct MobileTranscriptReaction: Equatable, Hashable {
    let emoji: String
    let by: String
}

func projectMobileTranscriptReactions(_ raw: Any?) -> [MobileTranscriptReaction] {
    guard let rows = raw as? [[String: Any]] else { return [] }
    return rows.compactMap { row in
        guard let emoji = (row["emoji"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines),
              !emoji.isEmpty,
              let by = (row["by"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines),
              !by.isEmpty
        else { return nil }
        return MobileTranscriptReaction(emoji: emoji, by: by)
    }
}

struct MobileReactionPillProjection: Identifiable, Equatable {
    var id: String { emoji }
    let emoji: String
    let count: Int
    let reactors: [String]
    let chosenByMe: Bool
}

func projectMobileReactionPills(
    _ reactions: [MobileTranscriptReaction]
) -> [MobileReactionPillProjection] {
    var order: [String] = []
    var reactorsByEmoji: [String: [String]] = [:]
    for reaction in reactions {
        if reactorsByEmoji[reaction.emoji] == nil {
            order.append(reaction.emoji)
            reactorsByEmoji[reaction.emoji] = []
        }
        if reactorsByEmoji[reaction.emoji]?.contains(reaction.by) == false {
            reactorsByEmoji[reaction.emoji]?.append(reaction.by)
        }
    }
    return order.map { emoji in
        let reactors = reactorsByEmoji[emoji] ?? []
        return MobileReactionPillProjection(
            emoji: emoji,
            count: reactors.count,
            reactors: reactors,
            chosenByMe: reactors.contains("me")
        )
    }
}

func normalizeMobileReactionInput(_ value: String) -> String? {
    let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !trimmed.isEmpty, trimmed.utf16.count <= 16 else { return nil }
    return trimmed
}

struct MobileReactionRequestFence: Equatable {
    let accountKey: String
    let agentId: String
    let generation: Int

    func accepts(accountKey: String, agentId: String, generation: Int) -> Bool {
        self.accountKey == accountKey
            && self.agentId == agentId
            && self.generation == generation
    }
}

struct MobileListenerIntegrationProjection: Equatable, Sendable {
    let platform: String
    let displayName: String
    let blurb: String
    let isConnected: Bool
    var accountLabel: String?
    var error: String?
}

func projectMobileListenerIntegration(_ raw: Any?) -> MobileListenerIntegrationProjection? {
    guard let row = raw as? [String: Any],
          let platformValue = row["platform"] as? String,
          let displayName = row["displayName"] as? String,
          let blurb = row["blurb"] as? String,
          let isConnected = row["isConnected"] as? Bool
    else { return nil }
    let platform = platformValue.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
    guard !platform.isEmpty else { return nil }
    let accountLabel = (row["accountLabel"] as? String)?
        .trimmingCharacters(in: .whitespacesAndNewlines)
    let error = (row["error"] as? String)?
        .trimmingCharacters(in: .whitespacesAndNewlines)
    return .init(
        platform: platform,
        displayName: displayName,
        blurb: blurb,
        isConnected: isConnected,
        accountLabel: accountLabel?.isEmpty == false ? accountLabel : nil,
        error: error?.isEmpty == false ? error : nil
    )
}

func projectMobileListenerIntegrations(_ raw: Any?) -> [String: MobileListenerIntegrationProjection]? {
    guard let rows = raw as? [Any] else { return nil }
    var projected: [String: MobileListenerIntegrationProjection] = [:]
    for row in rows {
        guard let integration = projectMobileListenerIntegration(row) else { continue }
        projected[integration.platform] = integration
    }
    return projected
}

func validatedMobileListenerAuthorizationURL(_ raw: String) -> URL? {
    guard let components = URLComponents(string: raw),
          components.scheme?.lowercased() == "https",
          components.host?.isEmpty == false
    else { return nil }
    return components.url
}

struct MobileCanonicalTranscriptCardPayload: Equatable {
    let kind: String
    let json: String
}

struct MobileTranscriptWidgetProjection: Equatable {
    let widget: SandWidget
    let respondedValue: String?
    let dismissed: Bool
    let skipped: Bool
}

func mobileTranscriptWidgetProjection(
    _ entry: MobileChatMessage
) -> MobileTranscriptWidgetProjection? {
    guard let payload = entry.canonicalTranscriptCard,
          payload.kind == "widget",
          let data = payload.json.data(using: .utf8),
          let card = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
          let rawWidget = card["widget"] as? [String: Any],
          let prompt = rawWidget["prompt"] as? String,
          let rawOptions = rawWidget["options"] as? [[String: Any]]
    else { return nil }

    let options = rawOptions.compactMap { row -> SandWidgetChoiceOption? in
        guard let label = row["label"] as? String else { return nil }
        if row["value"] != nil, row["value"] is String == false { return nil }
        if row["description"] != nil, row["description"] is String == false { return nil }

        let style: WidgetActionStyle?
        switch row["style"] as? String {
        case "primary", "success":
            style = .primary
        case "danger":
            style = .danger
        default:
            style = nil
        }

        return .init(
            label: label,
            value: row["value"] as? String,
            description: row["description"] as? String,
            style: style
        )
    }
    guard options.count == rawOptions.count else { return nil }

    let widget = SandWidget(
        prompt: prompt,
        helpText: rawWidget["helpText"] as? String,
        options: options,
        allowCustom: rawWidget["allowCustom"] as? Bool ?? false,
        dismissOnMoveOn: rawWidget["dismissOnMoveOn"] as? Bool ?? false
    )
    guard widget.isValid else { return nil }
    if card["respondedValue"] != nil, card["respondedValue"] is String == false { return nil }
    if card["widgetDismissed"] != nil, card["widgetDismissed"] is Bool == false { return nil }
    if card["widgetSkipped"] != nil, card["widgetSkipped"] is Bool == false { return nil }
    return .init(
        widget: widget,
        respondedValue: card["respondedValue"] as? String,
        dismissed: card["widgetDismissed"] as? Bool ?? false,
        skipped: card["widgetSkipped"] as? Bool ?? false
    )
}

struct MobileEmailDraftProjection: Equatable {
    let id: String
    let from: String?
    let to: [String]
    let cc: [String]?
    let subject: String
    let body: String
    let status: String
    let error: String?
}

struct MobileSlackDraftProjection: Equatable {
    let id: String
    let workspace: String?
    let target: String
    let thread: String?
    let body: String
    let status: String
    let error: String?
}

enum MobileTranscriptDraftProjection: Equatable {
    case email(MobileEmailDraftProjection)
    case slack(MobileSlackDraftProjection)
}

struct MobileDraftResolution: Equatable {
    let status: String
    let error: String?
}

func mobileTranscriptDraftProjection(
    _ payload: MobileCanonicalTranscriptCardPayload?
) -> MobileTranscriptDraftProjection? {
    guard let payload,
          ["emailDraft", "slackDraft"].contains(payload.kind),
          let data = payload.json.data(using: .utf8),
          let card = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
          let draft = card["draft"] as? [String: Any],
          let id = draft["id"] as? String,
          !id.isEmpty,
          let status = draft["status"] as? String
    else { return nil }

    if payload.kind == "emailDraft" {
        guard draft["kind"] as? String == "email",
              let to = draft["to"] as? [String],
              let subject = draft["subject"] as? String,
              let body = draft["body"] as? String
        else { return nil }
        return .email(.init(
            id: id,
            from: draft["from"] as? String,
            to: to,
            cc: draft["cc"] as? [String],
            subject: subject,
            body: body,
            status: status,
            error: draft["error"] as? String
        ))
    }

    guard draft["kind"] as? String == "slack",
          let target = draft["target"] as? String,
          !target.isEmpty,
          let body = draft["body"] as? String
    else { return nil }
    return .slack(.init(
        id: id,
        workspace: draft["workspace"] as? String,
        target: target,
        thread: draft["thread"] as? String,
        body: body,
        status: status,
        error: draft["error"] as? String
    ))
}

func mobileEmailRecipients(_ value: String) -> [String]? {
    let recipients = value
        .split(separator: ",")
        .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
        .filter { !$0.isEmpty }
    guard !recipients.isEmpty,
          recipients.allSatisfy({
              $0.range(
                  of: #"^[^\s@]+@[^\s@]+\.[^\s@]+$"#,
                  options: .regularExpression
              ) != nil
          })
    else { return nil }
    return recipients
}

struct MobileSecretRequestProjection: Equatable {
    let requestId: String
    let label: String
    let description: String?
    let provided: Bool
}

func mobileSecretRequestProjection(
    _ payload: MobileCanonicalTranscriptCardPayload?
) -> MobileSecretRequestProjection? {
    guard payload?.kind == "secretRequest",
          let json = payload?.json,
          let data = json.data(using: .utf8),
          let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
          let requestId = object["requestId"] as? String,
          !requestId.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
          let label = object["label"] as? String,
          !label.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
          let provided = object["provided"] as? Bool
    else { return nil }
    if object["description"] != nil, object["description"] is String == false { return nil }
    return .init(
        requestId: requestId,
        label: label,
        description: object["description"] as? String,
        provided: provided
    )
}

private func mobileTranscriptCardPayload(
    kind: String,
    card: [String: Any]
) -> MobileCanonicalTranscriptCardPayload? {
    guard JSONSerialization.isValidJSONObject(card),
          let data = try? JSONSerialization.data(withJSONObject: card, options: [.sortedKeys]),
          let json = String(data: data, encoding: .utf8)
    else { return nil }
    return .init(kind: kind, json: json)
}

private func mobileTranscriptCardNonEmptyString(_ value: Any?) -> String? {
    guard let value = value as? String else { return nil }
    let normalized = value.trimmingCharacters(in: .whitespacesAndNewlines)
    return normalized.isEmpty ? nil : normalized
}

private func mobileTranscriptCardOptionalString(
    _ object: [String: Any],
    key: String
) -> Bool {
    object[key] == nil || object[key] is String
}

private func mobileTranscriptDraftStatus(_ value: Any?) -> String? {
    guard let value = value as? String,
          ["editable", "sending", "sent", "discarded", "failed"].contains(value)
    else { return nil }
    return value
}

func projectMobileCanonicalHostTranscriptCard(
    event: [String: Any],
    operationId: String?
) -> MobileChatMessage? {
    guard let card = event["card"] as? [String: Any],
          let kind = card["kind"] as? String
    else { return nil }
    let entryId = (event["entryId"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        ?? "transcript-card:\(UUID().uuidString.lowercased())"
    let createdAt = mobileTranscriptCardDate(event["timestampMs"] ?? card["timestampMs"])
    guard let payload = mobileTranscriptCardPayload(kind: kind, card: card) else { return nil }

    switch kind {
    case "widget":
        guard let rawWidget = card["widget"] as? [String: Any],
              let prompt = mobileTranscriptCardNonEmptyString(rawWidget["prompt"]),
              let options = rawWidget["options"] as? [[String: Any]],
              (1...6).contains(options.count),
              rawWidget["allowCustom"] == nil || rawWidget["allowCustom"] is Bool,
              rawWidget["dismissOnMoveOn"] == nil || rawWidget["dismissOnMoveOn"] is Bool,
              mobileTranscriptCardOptionalString(rawWidget, key: "helpText"),
              card["respondedValue"] == nil || card["respondedValue"] is String,
              card["widgetDismissed"] == nil || card["widgetDismissed"] is Bool,
              card["widgetSkipped"] == nil || card["widgetSkipped"] is Bool
        else { return nil }
        for option in options {
            guard mobileTranscriptCardNonEmptyString(option["label"]) != nil,
                  mobileTranscriptCardOptionalString(option, key: "value"),
                  mobileTranscriptCardOptionalString(option, key: "description"),
                  mobileTranscriptCardOptionalString(option, key: "style")
            else { return nil }
        }
        let responded = (card["respondedValue"] as? String)?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let dismissed = card["widgetDismissed"] as? Bool ?? false
        return .init(
            id: entryId, role: .assistant, text: "", kind: .action,
            operationId: operationId,
            actionTitle: prompt,
            actionDetail: rawWidget["helpText"] as? String,
            actionStatus: (responded?.isEmpty == false || dismissed) ? "completed" : "waiting",
            canonicalTranscriptCard: payload,
            createdAt: createdAt
        )

    case "emailDraft":
        guard let draft = card["draft"] as? [String: Any],
              draft["kind"] as? String == "email",
              mobileTranscriptCardNonEmptyString(draft["id"]) != nil,
              let recipients = draft["to"] as? [String],
              recipients.allSatisfy({ !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }),
              let subject = draft["subject"] as? String,
              draft["body"] is String,
              let status = mobileTranscriptDraftStatus(draft["status"]),
              mobileTranscriptCardOptionalString(draft, key: "from"),
              draft["cc"] == nil || draft["cc"] is [String],
              mobileTranscriptCardOptionalString(draft, key: "error")
        else { return nil }
        return .init(
            id: entryId, role: .assistant, text: "", kind: .action,
            operationId: operationId,
            actionTitle: subject.isEmpty ? "Email draft" : subject,
            actionDetail: recipients.isEmpty ? "Email draft" : "To: \(recipients.joined(separator: ", "))",
            actionStatus: status,
            canonicalTranscriptCard: payload,
            createdAt: createdAt
        )

    case "slackDraft":
        guard let draft = card["draft"] as? [String: Any],
              draft["kind"] as? String == "slack",
              mobileTranscriptCardNonEmptyString(draft["id"]) != nil,
              let target = mobileTranscriptCardNonEmptyString(draft["target"]),
              let body = draft["body"] as? String,
              let status = mobileTranscriptDraftStatus(draft["status"]),
              mobileTranscriptCardOptionalString(draft, key: "workspace"),
              mobileTranscriptCardOptionalString(draft, key: "thread"),
              mobileTranscriptCardOptionalString(draft, key: "error")
        else { return nil }
        return .init(
            id: entryId, role: .assistant, text: "", kind: .action,
            operationId: operationId,
            actionTitle: "Slack draft · \(target)",
            actionDetail: body,
            actionStatus: status,
            canonicalTranscriptCard: payload,
            createdAt: createdAt
        )

    case "secretRequest":
        guard mobileTranscriptCardNonEmptyString(card["requestId"]) != nil,
              let label = mobileTranscriptCardNonEmptyString(card["label"]),
              let provided = card["provided"] as? Bool,
              mobileTranscriptCardOptionalString(card, key: "description")
        else { return nil }
        return .init(
            id: entryId, role: .assistant, text: "", kind: .action,
            operationId: operationId,
            actionTitle: provided ? "\(label) provided" : "Secret required · \(label)",
            actionDetail: card["description"] as? String,
            actionStatus: provided ? "completed" : "waiting",
            canonicalTranscriptCard: payload,
            createdAt: createdAt
        )

    case "event":
        guard let item = card["event"] as? [String: Any],
              mobileTranscriptCardNonEmptyString(item["source"]) != nil,
              mobileTranscriptCardNonEmptyString(item["event"]) != nil,
              let title = mobileTranscriptCardNonEmptyString(item["title"]),
              let summary = item["summary"] as? String,
              mobileTranscriptCardOptionalString(item, key: "url"),
              mobileTranscriptCardOptionalString(item, key: "actor")
        else { return nil }
        if let fields = item["fields"] {
            guard let rows = fields as? [[String: Any]],
                  rows.allSatisfy({
                      mobileTranscriptCardNonEmptyString($0["label"]) != nil
                          && $0["value"] is String
                  })
            else { return nil }
        }
        if item["occurredAtMs"] != nil,
           GrokMobileBotService.int64Value(item["occurredAtMs"]) == nil {
            return nil
        }
        return .init(
            id: entryId, role: .assistant,
            text: summary.isEmpty ? title : "\(title) — \(summary)",
            kind: .notice, operationId: operationId,
            canonicalTranscriptCard: payload,
            createdAt: createdAt
        )

    case "pdf":
        guard let name = mobileTranscriptCardNonEmptyString(card["name"]),
              mobileTranscriptCardOptionalString(card, key: "url"),
              mobileTranscriptCardOptionalString(card, key: "dataBase64")
        else { return nil }
        if card["pageCount"] != nil {
            guard let count = GrokMobileBotService.int64Value(card["pageCount"]), count >= 0
            else { return nil }
        }
        return .init(
            id: entryId, role: .assistant, text: "PDF · \(name)",
            kind: .notice, operationId: operationId,
            canonicalTranscriptCard: payload,
            createdAt: createdAt
        )

    case "spreadsheet":
        guard let name = mobileTranscriptCardNonEmptyString(card["name"]),
              let sheets = card["sheets"] as? [[String: Any]]
        else { return nil }
        for sheet in sheets {
            guard mobileTranscriptCardNonEmptyString(sheet["name"]) != nil,
                  sheet["rows"] is [[String]]
            else { return nil }
        }
        return .init(
            id: entryId, role: .assistant,
            text: "Spreadsheet · \(name) · \(sheets.count) sheets",
            kind: .notice, operationId: operationId,
            canonicalTranscriptCard: payload,
            createdAt: createdAt
        )

    case "miniApp":
        guard mobileTranscriptCardNonEmptyString(card["miniAppId"]) != nil,
              let name = mobileTranscriptCardNonEmptyString(card["name"]),
              card["html"] is String,
              mobileTranscriptCardOptionalString(card, key: "description")
        else { return nil }
        return .init(
            id: entryId, role: .assistant, text: "", kind: .action,
            operationId: operationId,
            actionTitle: "Open \(name)",
            actionDetail: card["description"] as? String,
            actionStatus: "waiting",
            canonicalTranscriptCard: payload,
            createdAt: createdAt
        )

    default:
        return nil
    }
}

enum MobileOptimisticDeliveryPhase: String, Equatable {
    case pending
    case acceptedAwaitingEcho
    case failed
}

struct MobileChatMessage: Identifiable, Equatable {
    let id: String
    let role: MobileChatRole
    var text: String
    var kind: MobileChatEntryKind = .message
    var operationId: String?
    var actionTitle: String?
    var actionDetail: String?
    var actionStatus: String?
    var listenerPlatform: String?
    var canonicalTranscriptCard: MobileCanonicalTranscriptCardPayload?
    var handoffRequestId: String?
    var handoffAgentId: String?
    var approvalId: String?
    var approvalKind: String?
    var approvalProposedRule: String?
    var canonicalMessageId: String?
    var replyToMessageId: String?
    var attachmentBatchId: String?
    var attachmentURL: String?
    var attachmentFileName: String?
    var attachmentAlt: String?
    var attachmentProjection: MobileAttachmentCardProjection?
    var sendMessageTextProjection: MobileSendMessageTextProjection?
    var timelineEvent: SandTimelineEvent?
    var timelineAutomationId: String?
    var reactions: [MobileTranscriptReaction] = []
    var myReactions: Set<String> = []
    var branched = false
    var streaming = false
    var optimisticDeliveryPhase: MobileOptimisticDeliveryPhase?
    var optimisticDeliveryError: String?
    var createdAt = Date()
}

enum MobileAutoReviewResolution: String, Equatable {
    case approved
    case always
    case denied

    var hostDecision: String {
        switch self {
        case .approved, .always: "allow-once"
        case .denied: "deny"
        }
    }
}

enum MobileAutoReviewProjectionError: Error {
    case invalidInstructions
}

func decodeMobileAutoReviewInstructions(_ value: Any) throws -> SandAutoReviewInstructions {
    guard let object = value as? [String: Any],
          let isEnabled = object["isEnabled"] as? Bool,
          let allow = object["allowInstructions"] as? [Any],
          let block = object["blockInstructions"] as? [Any]
    else {
        throw MobileAutoReviewProjectionError.invalidInstructions
    }
    return normalizeSandAutoReviewInstructions(
        isEnabled: isEnabled,
        allowInstructions: allow,
        blockInstructions: block
    )
}

struct MobileConfigurationSettingsSnapshot: Equatable {
    let autoReview: SandAutoReviewInstructions
    let inferenceProvider: SandInferenceProvider
    let privacyModeEnabled: Bool
}

func appendMobileAutoReviewAllowRule(
    _ current: SandAutoReviewInstructions,
    proposedRule: String
) -> SandAutoReviewInstructions? {
    let redacted = redactSandAutoReviewInlineSecrets(proposedRule)
    let canonicalSpacing = redacted
        .split(whereSeparator: { $0.isWhitespace })
        .joined(separator: " ")
    let bounded = String(canonicalSpacing.prefix(SAND_AUTO_REVIEW_INSTRUCTION_MAX_CHARS))
        .trimmingCharacters(in: .whitespacesAndNewlines)
    guard !bounded.isEmpty else { return nil }
    var allow = current.allowInstructions
    if !allow.contains(bounded) {
        allow.append(bounded)
        if allow.count > SAND_AUTO_REVIEW_INSTRUCTION_MAX_ENTRIES {
            allow = Array(allow.suffix(SAND_AUTO_REVIEW_INSTRUCTION_MAX_ENTRIES))
        }
    }
    return normalizeSandAutoReviewInstructions(
        isEnabled: current.isEnabled,
        allowInstructions: allow,
        blockInstructions: current.blockInstructions
    )
}

func projectMobileApprovalRequest(
    _ event: [String: Any],
    operationId: String
) -> MobileChatMessage? {
    guard event["type"] as? String == "approval.requested",
          event["operationId"] as? String == operationId,
          let approvalId = (event["approvalId"] as? String)?
            .trimmingCharacters(in: .whitespacesAndNewlines),
          !approvalId.isEmpty
    else { return nil }

    let title = [
        event["subject"] as? String,
        event["detail"] as? String,
        event["reason"] as? String,
        event["capability"] as? String,
    ]
    .compactMap { $0?.trimmingCharacters(in: .whitespacesAndNewlines) }
    .first { !$0.isEmpty } ?? "This action needs your approval."

    return MobileChatMessage(
        id: "approval:\(approvalId)",
        role: .assistant,
        text: title,
        kind: .permissionRequest,
        operationId: operationId,
        actionStatus: "pending",
        approvalId: approvalId,
        approvalKind: event["kind"] as? String,
        approvalProposedRule: event["proposedRule"] as? String
    )
}

@discardableResult
func applyMobileTranscriptReactionEvent(
    _ event: [String: Any],
    agentId: String,
    messages: inout [MobileChatMessage]
) -> Bool {
    guard event["type"] as? String == "host.transport",
          event["channel"] as? String == "transcript.reaction",
          let payload = event["payload"] as? [String: Any],
          payload["agentId"] as? String == agentId,
          let entryId = (payload["entryId"] as? String)?
            .trimmingCharacters(in: .whitespacesAndNewlines),
          !entryId.isEmpty,
          let index = messages.firstIndex(where: {
              ($0.canonicalMessageId ?? $0.id) == entryId
          })
    else { return false }

    let canonical = projectMobileTranscriptReactions(payload["reactions"])
    let myReactions: Set<String>
    if let rawMine = payload["myReactions"] as? [String] {
        myReactions = Set(rawMine.compactMap(normalizeMobileReactionInput))
    } else {
        myReactions = Set(canonical.filter { $0.by == "me" }.map(\.emoji))
    }
    guard messages[index].reactions != canonical
        || messages[index].myReactions != myReactions
    else { return false }

    messages[index].reactions = canonical
    messages[index].myReactions = myReactions
    return true
}

private func normalizeMobileLinkURL(_ value: Any?) -> String? {
    guard let string = value as? String else { return nil }
    let trimmed = string.trimmingCharacters(in: .whitespacesAndNewlines)
    guard trimmed.range(of: #"^https?://"#, options: [.regularExpression, .caseInsensitive]) != nil,
          let url = URL(string: trimmed),
          let scheme = url.scheme?.lowercased(),
          (scheme == "http" || scheme == "https"),
          let host = url.host,
          !host.isEmpty
    else { return nil }
    var components = URLComponents(url: url, resolvingAgainstBaseURL: false)
    if components?.path.isEmpty == true {
        components?.path = "/"
    }
    return components?.url?.absoluteString ?? url.absoluteString
}

private func extractMobileBareLink(_ content: String) -> String? {
    let trimmed = content.trimmingCharacters(in: .whitespacesAndNewlines)
    if let expression = try? NSRegularExpression(
        pattern: #"^\[[^\]\n]*\]\(\s*([^\)\s]+)(?:\s+[^)]*)?\)\s*$"#
    ) {
        let range = NSRange(trimmed.startIndex..<trimmed.endIndex, in: trimmed)
        if let match = expression.firstMatch(in: trimmed, range: range),
           match.numberOfRanges > 1,
           let targetRange = Range(match.range(at: 1), in: trimmed)
        {
            return normalizeMobileLinkURL(String(trimmed[targetRange]))
        }
    }
    return normalizeMobileLinkURL(trimmed)
}

/// Native projector for Desktop transcript-card/send-message-text.ts.
/// It preserves the exact non-streaming, image-free bare-link presentation gate.
func projectMobileSendMessageText(
    _ value: [String: Any]
) -> MobileSendMessageTextProjection? {
    guard value["kind"] as? String == "send-message",
          let id = value["id"] as? String,
          !id.isEmpty,
          let message = value["message"] as? [String: Any],
          message["type"] as? String == "text",
          let content = message["content"] as? String
    else { return nil }

    if let rawStreaming = value["streaming"], !(rawStreaming is Bool) {
        return nil
    }
    let streaming = value["streaming"] as? Bool ?? false

    let timestampMs: Double?
    if let rawTimestamp = value["timestampMs"] {
        guard let parsed = finiteAttachmentNumber(rawTimestamp) else { return nil }
        timestampMs = parsed
    } else {
        timestampMs = nil
    }

    let channel: String?
    if let rawChannel = message["channel"] {
        if rawChannel is NSNull {
            channel = nil
        } else {
            guard let parsed = rawChannel as? String else { return nil }
            channel = parsed
        }
    } else {
        channel = nil
    }

    var images: [MobileSendMessageTextImage] = []
    if let rawImages = message["images"] {
        guard let values = rawImages as? [[String: Any]] else { return nil }
        for image in values {
            guard let url = image["url"] as? String, !url.isEmpty else { return nil }
            if let alt = image["alt"], !(alt is String) { return nil }
            images.append(.init(url: url, alt: image["alt"] as? String))
        }
    }

    let presentation: MobileSendMessageTextPresentation
    if !streaming, images.isEmpty, let url = extractMobileBareLink(content) {
        presentation = .urlCard(url)
    } else {
        presentation = .text
    }

    return .init(
        id: id,
        content: content,
        images: images,
        channel: channel,
        streaming: streaming,
        timestampMs: timestampMs,
        presentation: presentation
    )
}

private let mobileAttachmentImageExtensions: Set<String> = [
    ".avif", ".bmp", ".gif", ".ico", ".jpeg", ".jpg", ".png", ".svg", ".webp",
]
private let mobileAttachmentVideoExtensions: Set<String> = [
    ".m4v", ".mov", ".mp4", ".ogv", ".webm",
]
private let mobileAttachmentAudioExtensions: Set<String> = [
    ".aac", ".flac", ".m4a", ".mp3", ".oga", ".ogg", ".wav",
]

enum MobileAttachmentMediaPresentation: Equatable {
    case image
    case video
    case audio
    case file
}

func mobileAttachmentMediaPresentation(_ value: String) -> MobileAttachmentMediaPresentation {
    let ext = mobileAttachmentExtension(value)
    if mobileAttachmentImageExtensions.contains(ext) { return .image }
    if mobileAttachmentVideoExtensions.contains(ext) { return .video }
    if mobileAttachmentAudioExtensions.contains(ext) { return .audio }
    return .file
}

private func mobileAttachmentExtension(_ value: String) -> String {
    let normalized = value.replacingOccurrences(of: "\\", with: "/")
    let path: String
    if let url = URL(string: value),
       let scheme = url.scheme?.lowercased(),
       ["http", "https", "file"].contains(scheme)
    {
        path = url.path
    } else {
        let withoutQuery = normalized.split(
            separator: "?",
            maxSplits: 1,
            omittingEmptySubsequences: false
        ).first.map(String.init) ?? normalized
        path = withoutQuery.split(
            separator: "#",
            maxSplits: 1,
            omittingEmptySubsequences: false
        ).first.map(String.init) ?? withoutQuery
    }
    let lower = path.lowercased()
    guard let dot = lower.lastIndex(of: ".") else { return "" }
    return String(lower[dot...])
}

private func isMobileAttachmentMediaURL(_ value: String) -> Bool {
    mobileAttachmentMediaPresentation(value) != .file
}

private func normalizedMobileHTTPURL(_ value: String) -> String? {
    let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !trimmed.isEmpty,
          !trimmed.contains(where: \.isWhitespace),
          let url = URL(string: trimmed),
          let scheme = url.scheme?.lowercased(),
          scheme == "http" || scheme == "https",
          let host = url.host,
          !host.isEmpty
    else { return nil }
    return url.absoluteString
}

func classifyMobileAttachmentURL(_ value: String) -> MobileAttachmentCardKind {
    if value == "sand://box" || value.hasPrefix("sand://box?") {
        return .box
    }
    if normalizedMobileHTTPURL(value) != nil, !isMobileAttachmentMediaURL(value) {
        return .legacyLink
    }
    return isMobileAttachmentMediaURL(value) ? .media : .file
}

private func finiteAttachmentNumber(_ value: Any?) -> Double? {
    guard !(value is Bool), let number = value as? NSNumber else { return nil }
    let result = number.doubleValue
    return result.isFinite ? result : nil
}

private func optionalNonNegativeAttachmentNumber(
    _ value: Any?
) -> (valid: Bool, value: Double?) {
    guard let value else { return (true, nil) }
    guard let number = finiteAttachmentNumber(value), number >= 0 else {
        return (false, nil)
    }
    return (true, number)
}

private func optionalNonEmptyAttachmentString(
    _ value: Any?
) -> (valid: Bool, value: String?) {
    guard let value else { return (true, nil) }
    guard let string = value as? String, !string.isEmpty else {
        return (false, nil)
    }
    return (true, string)
}

private func mobileAttachmentBasename(_ value: String) -> String {
    let normalized = value.replacingOccurrences(of: "\\", with: "/")
    let leaf = normalized.split(separator: "/", omittingEmptySubsequences: false).last
        .map(String.init) ?? ""
    return leaf.isEmpty ? "Attachment" : leaf
}

/// Native data projection for Desktop transcript-card/attachment-data.ts.
/// Malformed metadata fails closed instead of becoming a generic attachment.
func projectMobileAttachmentCard(
    _ value: [String: Any]
) -> MobileAttachmentCardProjection? {
    guard let rawKind = value["kind"] as? String else { return nil }

    if rawKind == "send-message" {
        guard let id = value["id"] as? String, !id.isEmpty,
              let message = value["message"] as? [String: Any],
              message["type"] as? String == "attachment",
              let rawURL = message["url"] as? String
        else { return nil }

        let timestamp: Double?
        if value["timestampMs"] == nil {
            timestamp = nil
        } else {
            guard let parsed = finiteAttachmentNumber(value["timestampMs"]) else { return nil }
            timestamp = parsed
        }

        let kind = classifyMobileAttachmentURL(rawURL)
        let projectedURL = kind == .legacyLink
            ? (normalizedMobileHTTPURL(rawURL) ?? rawURL)
            : rawURL

        var projection = MobileAttachmentCardProjection(
            id: id,
            kind: kind,
            url: projectedURL,
            timestampMs: timestamp
        )
        if kind == .media {
            projection.alt = message["alt"] as? String
        }
        if kind == .box {
            projection.instruction = value["boxInstruction"] as? String
            projection.request = value["boxRequest"] as? String
            projection.requestId = value["boxRequestId"] as? String
            if value["boxResolution"] is NSNull {
                projection.resolution = nil
            } else if let resolution = value["boxResolution"] as? String {
                projection.resolution = resolution
            } else if value["boxResolution"] != nil {
                return nil
            }
            projection.screenshotDataURL = value["boxSnapshot"] as? String
        }
        return projection
    }

    if rawKind == "user-attachment" {
        guard let id = value["id"] as? String, !id.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
              let filePath = value["file_path"] as? String,
              !filePath.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        else { return nil }

        if let fileName = value["file_name"], !(fileName is String) {
            return nil
        }
        let byteSize = optionalNonNegativeAttachmentNumber(value["byteSize"])
        let width = optionalNonNegativeAttachmentNumber(value["width"])
        let height = optionalNonNegativeAttachmentNumber(value["height"])
        let timestamp = optionalNonNegativeAttachmentNumber(value["timestampMs"])
        let batchId = optionalNonEmptyAttachmentString(value["batchId"])
        let replyTo = optionalNonEmptyAttachmentString(value["replyTo"])
        let clientNonce = optionalNonEmptyAttachmentString(value["clientNonce"])
        guard byteSize.valid, width.valid, height.valid, timestamp.valid,
              batchId.valid, replyTo.valid, clientNonce.valid
        else { return nil }

        let explicitName = value["file_name"] as? String
        return MobileAttachmentCardProjection(
            id: id,
            kind: isMobileAttachmentMediaURL(filePath) ? .media : .file,
            url: filePath,
            name: explicitName == nil || explicitName?.isEmpty == true
                ? mobileAttachmentBasename(filePath)
                : explicitName,
            byteSize: byteSize.value,
            width: width.value,
            height: height.value,
            timestampMs: timestamp.value,
            batchId: batchId.value,
            replyTo: replyTo.value,
            clientNonce: clientNonce.value
        )
    }

    return nil
}

func projectMobileChatMessageAttachment(
    id: String,
    raw: [String: Any],
    batchId: String?,
    timestampMs: Any? = nil
) -> MobileAttachmentCardProjection? {
    guard let rawURL = raw["url"] as? String else { return nil }
    var message: [String: Any] = [
        "type": "attachment",
        "url": rawURL,
    ]
    if let alt = raw["alt"] as? String {
        message["alt"] = alt
    }
    var envelope: [String: Any] = [
        "kind": "send-message",
        "id": id,
        "message": message,
    ]
    if let timestampMs {
        envelope["timestampMs"] = timestampMs
    }
    guard var projection = projectMobileAttachmentCard(envelope) else { return nil }
    if let fileName = raw["file_name"] as? String, !fileName.isEmpty {
        projection.name = fileName
    }
    if let batchId, !batchId.isEmpty {
        projection.batchId = batchId
    }
    return projection
}

private func mobileTranscriptCardDate(_ value: Any?) -> Date {
    if let milliseconds = value as? NSNumber {
        return Date(timeIntervalSince1970: milliseconds.doubleValue / 1_000)
    }
    return Date()
}

private func projectMobileTimelineEvent(
    _ raw: [String: Any]
) -> (event: SandTimelineEvent, automationId: String?)? {
    guard let type = raw["type"] as? String else { return nil }
    switch type {
    case "name-changed":
        guard let to = raw["to"] as? String else { return nil }
        return (.nameChanged(to: to), nil)
    case "channel-connected":
        guard let label = raw["label"] as? String else { return nil }
        return (.channelConnected(label: label), nil)
    case "channel-disconnected":
        guard let label = raw["label"] as? String else { return nil }
        return (.channelDisconnected(label: label), nil)
    case "automation-changed":
        guard let automationId = raw["automationId"] as? String, !automationId.isEmpty,
              let action = raw["action"] as? String, !action.isEmpty,
              let automationName = raw["automationName"] as? String, !automationName.isEmpty
        else { return nil }
        return (.automationChanged(action: action, automationName: automationName), automationId)
    default:
        return nil
    }
}

/// Native projection for the recovered Desktop transcript-card family.
/// Unknown/malformed cards fail closed rather than becoming generic messages.
func projectMobileTranscriptCard(
    event: [String: Any],
    operationId: String?
) -> MobileChatMessage? {
    guard let card = event["card"] as? [String: Any],
          let kind = card["kind"] as? String
    else { return nil }

    if kind == "listenerConnect" {
        return projectListenerConnectTranscriptCard(event: event, operationId: operationId)
    }
    if let hostCard = projectMobileCanonicalHostTranscriptCard(
        event: event,
        operationId: operationId
    ) {
        return hostCard
    }

    let entryId = (event["entryId"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        ?? (card["id"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        ?? "transcript-card:\(UUID().uuidString.lowercased())"
    let createdAt = mobileTranscriptCardDate(event["timestampMs"] ?? card["timestampMs"])

    if kind == "send-message" {
        guard let message = card["message"] as? [String: Any],
              let type = message["type"] as? String
        else { return nil }
        if type == "text" {
            guard let projection = projectMobileSendMessageText(card) else { return nil }
            return MobileChatMessage(
                id: projection.id,
                role: .assistant,
                text: projection.content,
                operationId: operationId,
                sendMessageTextProjection: projection,
                streaming: projection.streaming,
                createdAt: projection.timestampMs.map {
                    Date(timeIntervalSince1970: $0 / 1_000)
                } ?? createdAt
            )
        }
        if type == "attachment" {
            guard let attachment = projectMobileAttachmentCard(card) else { return nil }
            return MobileChatMessage(
                id: attachment.id,
                role: .assistant,
                text: "",
                operationId: operationId,
                attachmentBatchId: attachment.batchId,
                attachmentURL: attachment.url,
                attachmentFileName: attachment.name,
                attachmentAlt: attachment.alt,
                attachmentProjection: attachment,
                createdAt: attachment.timestampMs.map {
                    Date(timeIntervalSince1970: $0 / 1_000)
                } ?? createdAt
            )
        }
        return nil
    }

    if kind == "user-attachment" {
        guard let attachment = projectMobileAttachmentCard(card) else { return nil }
        return MobileChatMessage(
            id: attachment.id,
            role: .user,
            text: "",
            operationId: operationId,
            replyToMessageId: attachment.replyTo,
            attachmentBatchId: attachment.batchId,
            attachmentURL: attachment.url,
            attachmentFileName: attachment.name,
            attachmentAlt: attachment.alt,
            attachmentProjection: attachment,
            createdAt: attachment.timestampMs.map {
                Date(timeIntervalSince1970: $0 / 1_000)
            } ?? createdAt
        )
    }

    switch kind {
    case "notice":
        guard let text = card["text"] as? String, !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return nil }
        return MobileChatMessage(
            id: entryId, role: .assistant, text: text, kind: .notice,
            operationId: operationId, createdAt: createdAt
        )

    case "permissionRequest", "permission-request":
        let nestedTitle = (card["permission"] as? [String: Any])?["title"] as? String
        guard let title = (nestedTitle ?? card["title"] as? String),
              !title.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        else { return nil }
        return MobileChatMessage(
            id: entryId, role: .assistant, text: title, kind: .permissionRequest,
            operationId: operationId, createdAt: createdAt
        )

    case "timelineEvent", "timeline-event", "event":
        guard let rawEvent = card["event"] as? [String: Any],
              let projection = projectMobileTimelineEvent(rawEvent)
        else { return nil }
        return MobileChatMessage(
            id: entryId, role: .assistant, text: describeTimelineEvent(projection.event),
            kind: .timelineEvent, operationId: operationId,
            timelineEvent: projection.event, timelineAutomationId: projection.automationId,
            createdAt: createdAt
        )

    default:
        return nil
    }
}

func projectListenerConnectTranscriptCard(
    event: [String: Any],
    operationId: String?
) -> MobileChatMessage? {
    guard let card = event["card"] as? [String: Any],
          card["kind"] as? String == "listenerConnect",
          let platform = card["platform"] as? String,
          !platform.isEmpty
    else { return nil }

    let displayName: String
    switch platform.lowercased() {
    case "slack": displayName = "Slack"
    case "github": displayName = "GitHub"
    case "git": displayName = "Git"
    case "teams": displayName = "Microsoft Teams"
    case "linear": displayName = "Linear"
    case "sentry": displayName = "Sentry"
    case "pagerduty": displayName = "PagerDuty"
    default: displayName = platform
    }

    let connected = card["connected"] as? Bool ?? false
    let pending = card["pending"] as? Bool ?? false
    let reason = (card["reason"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines)
    let detail = reason?.isEmpty == false
        ? reason
        : (connected ? "\(displayName) 已连接。" : "连接 \(displayName) 后，此例程才能接收对应事件。")
    let entryId = (event["entryId"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        ?? "listener-connect:\(platform.lowercased())"

    return MobileChatMessage(
        id: entryId,
        role: .assistant,
        text: "",
        kind: .action,
        operationId: operationId,
        actionTitle: connected ? "\(displayName) 已连接" : "连接 \(displayName)",
        actionDetail: detail,
        actionStatus: connected ? "completed" : (pending ? "pending" : "waiting"),
        listenerPlatform: platform.lowercased()
    )
}

struct MiniAppToolContract: Equatable, Sendable {
    let name: String
    let description: String
    let approval: String
    let title: String?
    let inputSchemaJSON: String?

    init(
        name: String,
        description: String,
        approval: String,
        title: String? = nil,
        inputSchemaJSON: String? = nil
    ) {
        self.name = name
        self.description = description
        self.approval = approval
        self.title = title
        self.inputSchemaJSON = inputSchemaJSON
    }

    var inputSchemaObject: [String: Any]? {
        guard let inputSchemaJSON,
              let data = inputSchemaJSON.data(using: .utf8),
              let raw = try? JSONSerialization.jsonObject(with: data),
              let object = raw as? [String: Any]
        else { return nil }
        return object
    }
}

struct MarketplacePlugin: Identifiable, Equatable, Sendable {
    let pluginId: String
    let displayName: String
    let description: String
    let latestVersion: String?
    let sourceRef: String?
    let tools: [MiniAppToolContract]

    init(
        pluginId: String,
        displayName: String,
        description: String,
        latestVersion: String?,
        sourceRef: String? = nil,
        tools: [MiniAppToolContract]
    ) {
        self.pluginId = pluginId
        self.displayName = displayName
        self.description = description
        self.latestVersion = latestVersion
        self.sourceRef = sourceRef
        self.tools = tools
    }

    var id: String { pluginId }

    func replacingTools(_ tools: [MiniAppToolContract]) -> MarketplacePlugin {
        MarketplacePlugin(
            pluginId: pluginId,
            displayName: displayName,
            description: description,
            latestVersion: latestVersion,
            sourceRef: sourceRef,
            tools: tools
        )
    }
}


enum MarketplaceBrowserTab: String, CaseIterable, Identifiable, Sendable {
    case marketplace = "Marketplace"
    case yours = "Yours"
    var id: String { rawValue }
}

enum MarketplaceSkillOwnershipFilter: String, CaseIterable, Identifiable, Sendable {
    case all = "全部"
    case team = "团队"
    case publicItems = "公开"
    var id: String { rawValue }
}

struct MarketplaceSkillPublishTarget: Identifiable, Equatable, Sendable {
    let teamId: Int
    let name: String
    var id: Int { teamId }
}

struct MarketplacePrivateSkill: Identifiable {
    let id: String
    let name: String
    let description: String
    let body: String
    let source: String
    let sourceRef: String?
    let pluginId: String?
    let publishedByCurrentUser: Bool
    let isEnabledForAgent: Bool
    let triggerSchedule: String?
    let triggerEnabled: Bool?

    var canEdit: Bool { source == "workflow" }
    var canToggle: Bool { source == "workflow" }
    var sourceLabel: String {
        switch source {
        case "managed": return "Managed by Cursor"
        case "plugin": return "Shared with your team"
        default: return "Private skill"
        }
    }
}

struct PluginPermissionRequest: Identifiable, Equatable {
    let pluginId: String
    let runtime: String
    let permissions: [String]
    var id: String { pluginId }
}

struct MarketplaceMcpServer: Identifiable, Equatable, Sendable {
    let serverId: String
    let name: String
    let serverIdentifier: String
    let accountKey: String
    let transport: String
    let status: String
    let statusDetail: String?
    let toolCount: Int
    let disabledToolCount: Int
    let isTeamServer: Bool
    let pluginId: String?
    let isRequired: Bool
    let managedByTeamPluginPolicy: Bool

    var id: String { "\(serverId):\(accountKey)" }
    var isDisabledByTeamAdminPolicy: Bool { status == "disabledByTeamAdminPolicy" }
}

struct MarketplaceMcpTool: Identifiable, Equatable, Sendable {
    let name: String
    let title: String?
    let description: String?
    let isDisabled: Bool
    var id: String { name }
}

private final class BrowserAuthPresentationContext: NSObject, ASWebAuthenticationPresentationContextProviding {
    func presentationAnchor(for session: ASWebAuthenticationSession) -> ASPresentationAnchor {
        let scenes = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
        if let keyWindow = scenes.flatMap(\.windows).first(where: \.isKeyWindow) {
            return keyWindow
        }
        if let window = scenes.flatMap(\.windows).first {
            return window
        }
        return ASPresentationAnchor()
    }
}

enum AccountFeedbackCode: String, CaseIterable, Sendable {
    case accessDenied = "access-denied"
    case invalidFeedback = "invalid-feedback"
    case notSignedIn = "not-signed-in"
    case rateLimited = "rate-limited"
    case subscriptionRequired = "subscription-required"
    case unavailable

    static func normalize(_ rawValue: Any?) -> AccountFeedbackCode {
        guard let rawValue = rawValue as? String,
              let code = AccountFeedbackCode(rawValue: rawValue)
        else { return .unavailable }
        return code
    }

    var localizedMessage: String {
        switch self {
        case .accessDenied: "当前账号无权提交反馈。"
        case .invalidFeedback: "反馈内容无效，请检查后重试。"
        case .notSignedIn: "请先登录后再提交反馈。"
        case .rateLimited: "提交过于频繁，请稍后重试。"
        case .subscriptionRequired: "当前订阅无法提交反馈。"
        case .unavailable: "反馈服务暂时不可用，请稍后重试。"
        }
    }
}

struct AccountFeedbackError: LocalizedError, Equatable {
    let code: AccountFeedbackCode
    var errorDescription: String? { code.localizedMessage }
}

@MainActor
@Observable
final class MarketplaceModel {
    var query = ""
    var message = "Mahayana Rust Host 正在启动"
    var loading = false
    var installingPluginId: String?
    var accountRosterRevision = 0
    var plugins: [MarketplacePlugin] = []
    var pluginBrowserTab: MarketplaceBrowserTab = .marketplace
    var privateSkillOwnershipFilter: MarketplaceSkillOwnershipFilter = .all
    var privateSkillQuery = ""
    var privateSkills: [MarketplacePrivateSkill] = []
    var privateSkillsLoading = false
    var privateSkillMutatingId: String?
    var privateSkillPublishingId: String?
    var privateSkillError: String?
    var skillPublishTargets: [MarketplaceSkillPublishTarget] = []
    var selectedSkillPublishTeamId: Int?
    var skillPublishTargetsLoading = false
    var privateSkillNameDrafts: [String: String] = [:]
    var privateSkillDescriptionDrafts: [String: String] = [:]
    var privateSkillBodyDrafts: [String: String] = [:]
    var privateSkillAgentId: String?
    var privateSkillAgentName: String?
    var permissionRequest: PluginPermissionRequest?
    var listenerIntegrations: [String: MobileListenerIntegrationProjection] = [:]
    var listenerIntegrationsLoading = false
    var listenerConnectingPlatform: String?
    var listenerAuthorizingPlatform: String?
    var listenerIntegrationErrors: [String: String] = [:]
    var mcpServers: [MarketplaceMcpServer] = []
    var mcpToolsByServerId: [String: [MarketplaceMcpTool]] = [:]
    var mcpLoading = false
    var mcpLoadingServerId: String?
    var mcpMutatingToolKey: String?
    var mcpError: String?
    var mcpBackendLoggedIn = false
    var mcpBackendEmail = ""
    var mcpBackendBusy = false
    var mcpNewAccountDraftByServerId: [String: String] = [:]
    var mcpRenameDraftByIdentity: [String: String] = [:]
    var featureHostSmokeStatus: String?
    var authResolved = false
    var loggedIn = false
    var accountName = "Fabushi"
    var accountEmail = ""
    var accountUsage: AccountUsageProjection?
    var accountUsageLoading = false
    var accountUsageError: String?
    var onboardingStep: Int
    var onboardingRouteResolved = false
    var browserLoginAttemptId: String?
    var browserLoginURL: URL?
    var loginBusy = false
    var loginError: String?
    var chatDraft = ""
    var chatMessages: [MobileChatMessage] = []
    var chatBusy = false
    var activeOperationId: String?
    let globalDharmaCommerce: GlobalDharmaCommerceModel
    @ObservationIgnored let settingsNoticeController = SettingsNoticeController()

    private let bridge: IOSPreloadBridge
    private let globalDharmaBridge: GlobalDharmaMiniAppBridge
    // Read-only migration source for builds that predate the canonical SandSettingsStore owner.\n    private static let legacyOnboardingKeyPrefix = "fabushi.mobile.onboarding-complete.v2:"
    @ObservationIgnored private var onboardingRouteGeneration = 0
    @ObservationIgnored private var globalDharmaAccountScope: String?
    @ObservationIgnored private var globalDharmaExecution: [String: Any]?
    private static let globalDharmaExecutionKeyPrefix = "fabushi.ios.miniapp-execution.v1:"
    @ObservationIgnored private let browserAuthPresentationContext = BrowserAuthPresentationContext()
    @ObservationIgnored private var webAuthenticationSession: ASWebAuthenticationSession?
    @ObservationIgnored private var mcpOAuthSession: ASWebAuthenticationSession?
    @ObservationIgnored private var mcpBackendLoginAttemptId: String?
    @ObservationIgnored private var linkMetadataCache: [String: MobileLinkMetadata] = [:]
    @ObservationIgnored private var linkMetadataTasks: [String: Task<MobileLinkMetadata, Error>] = [:]
    @ObservationIgnored private var listenerRequestSerial = 0
    @ObservationIgnored private var mcpOAuthGeneration = 0
    @ObservationIgnored private var mcpAccountEpoch = 0
    @ObservationIgnored private var mcpServerRequestSerial = 0
    @ObservationIgnored private var mcpToolRequestSerial: [String: Int] = [:]
    @ObservationIgnored private var mcpMutationSerial: [String: Int] = [:]
    @ObservationIgnored private var privateSkillRequestSerial = 0
    @ObservationIgnored private var privateSkillScopeGeneration = 0

    init(bridge: IOSPreloadBridge) {
        self.bridge = bridge
        globalDharmaBridge = GlobalDharmaMiniAppBridge(bridge: bridge)
        globalDharmaCommerce = GlobalDharmaCommerceModel(bridge: bridge)
        onboardingStep = MobileSignedInOnboardingStep.meet.rawValue
    }

    var settingsNoticeAccountKey: String {
        guard loggedIn else { return "logged-out" }
        if let globalDharmaAccountScope, !globalDharmaAccountScope.isEmpty {
            return globalDharmaAccountScope
        }
        let email = accountEmail.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        return email.isEmpty ? "account" : email
    }

    private func publishPluginsNotice(
        _ operation: PluginsNoticeOperation,
        kind: SurfaceNoticeKind,
        message: String,
        fence: SettingsNoticeFence
    ) {
        SurfaceNoticePublisher.publish(
            RootSettingsNoticeEvent(
                kind: kind,
                operation: .plugins(operation),
                message: message
            ),
            controller: settingsNoticeController,
            fence: fence
        )
    }

    nonisolated static func projectLinkMetadata(
        url: String,
        value: Any
    ) -> MobileLinkMetadata? {
        guard let normalizedURL = normalizeMobileLinkURL(url),
              let object = value as? [String: Any]
        else { return nil }

        func optionalString(_ key: String) -> String? {
            guard let raw = object[key] else { return nil }
            if raw is NSNull { return nil }
            guard let string = raw as? String else { return nil }
            let trimmed = string.trimmingCharacters(in: .whitespacesAndNewlines)
            return trimmed.isEmpty ? nil : trimmed
        }

        if object["title"] != nil, !(object["title"] is String), !(object["title"] is NSNull) {
            return nil
        }
        if object["description"] != nil, !(object["description"] is String), !(object["description"] is NSNull) {
            return nil
        }
        if object["hostname"] != nil, !(object["hostname"] is String), !(object["hostname"] is NSNull) {
            return nil
        }
        if object["imageUrl"] != nil, !(object["imageUrl"] is String), !(object["imageUrl"] is NSNull) {
            return nil
        }
        if object["imageDataUrl"] != nil, !(object["imageDataUrl"] is String), !(object["imageDataUrl"] is NSNull) {
            return nil
        }
        if object["faviconDataUrl"] != nil, !(object["faviconDataUrl"] is String), !(object["faviconDataUrl"] is NSNull) {
            return nil
        }

        return .init(
            url: normalizedURL,
            title: optionalString("title"),
            description: optionalString("description"),
            hostname: optionalString("hostname"),
            imageURL: optionalString("imageUrl"),
            imageDataURL: optionalString("imageDataUrl"),
            faviconDataURL: optionalString("faviconDataUrl")
        )
    }

    func linkMetadata(for rawURL: String) async throws -> MobileLinkMetadata {
        guard let url = normalizeMobileLinkURL(rawURL) else {
            throw MahayanaCoordinator.CoordinatorError.requestFailed("Invalid HTTP(S) link")
        }
        if let cached = linkMetadataCache[url] {
            return cached
        }
        if let pending = linkMetadataTasks[url] {
            return try await pending.value
        }

        let bridge = self.bridge
        let task = Task { @MainActor in
            let response = try await bridge.request(
                method: "getLinkMetadata",
                params: ["url": url]
            )
            guard let projected = Self.projectLinkMetadata(
                url: url,
                value: response.value
            ) else {
                throw MahayanaCoordinator.CoordinatorError.requestFailed(
                    "Invalid link metadata response"
                )
            }
            return projected
        }
        linkMetadataTasks[url] = task
        do {
            let metadata = try await task.value
            linkMetadataTasks[url] = nil
            linkMetadataCache[url] = metadata
            return metadata
        } catch {
            linkMetadataTasks[url] = nil
            throw error
        }
    }

    func resetLinkMetadataCache() {
        for task in linkMetadataTasks.values {
            task.cancel()
        }
        linkMetadataTasks.removeAll()
        linkMetadataCache.removeAll()
    }

    static func nextGlobalDharmaExecution(
        previous: [String: Any]?,
        tool: String,
        result: Any,
        source: String
    ) -> [String: Any] {
        let previousRevision = (previous?["revision"] as? NSNumber)?.intValue ?? 0
        return [
            "protocol": "fabushi.miniapp.execution.v1",
            "miniAppId": GlobalDharmaMiniAppBridge.globalDharmaId,
            "revision": previousRevision + 1,
            "source": source,
            "phase": "completed",
            "tool": tool,
            "result": result,
        ]
    }

    static func globalDharmaRuntime(from execution: [String: Any]) -> [String: Any]? {
        guard execution["protocol"] as? String == "fabushi.miniapp.execution.v1",
              execution["miniAppId"] as? String == GlobalDharmaMiniAppBridge.globalDharmaId,
              let revision = (execution["revision"] as? NSNumber)?.intValue,
              revision > 0
        else { return nil }
        return [
            "protocol": "fabushi.miniapp.runtime.v1",
            "miniAppId": GlobalDharmaMiniAppBridge.globalDharmaId,
            "revision": revision,
            "state": execution,
        ]
    }

    static func bridgeGlobalDharmaStatusResult(
        _ result: [String: Any],
        runtime: [String: Any]
    ) -> [String: Any] {
        var bridged = result
        var structured = (result["structuredContent"] as? [String: Any]) ?? [:]
        structured["runtime"] = runtime
        bridged["structuredContent"] = structured
        return bridged
    }

    private static let canonicalAccountIdentityKeys = [
        "principalId",
        "principal_id",
        "id",
        "userId",
        "user_id",
        "userNo",
        "user_no",
        "username",
    ]

    private static func stableGlobalDharmaAccountIdentity(in auth: [String: Any]) -> String? {
        if let user = auth["user"] as? [String: Any],
           let identity = stableGlobalDharmaIdentityComponent(in: user) {
            return identity
        }
        return stableGlobalDharmaIdentityComponent(in: auth)
    }

    private static func stableGlobalDharmaIdentityComponent(in object: [String: Any]) -> String? {
        for key in canonicalAccountIdentityKeys {
            guard let raw = object[key] else { continue }
            if let value = raw as? String {
                let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
                if !trimmed.isEmpty { return trimmed }
                continue
            }
            if let number = raw as? NSNumber,
               CFGetTypeID(number) != CFBooleanGetTypeID() {
                return number.stringValue
            }
        }
        return nil
    }

    static func globalDharmaScope(for auth: [String: Any]) -> String? {
        guard let raw = stableGlobalDharmaAccountIdentity(in: auth) else { return nil }
        return Data(raw.utf8)
            .base64EncodedString()
            .replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "+", with: "-")
            .replacingOccurrences(of: "=", with: "")
    }

    private static func globalDharmaExecutionKey(scope: String) -> String {
        globalDharmaExecutionKeyPrefix + scope
    }

    private static func loadGlobalDharmaExecution(scope: String) -> [String: Any]? {
        let key = globalDharmaExecutionKey(scope: scope)
        guard let data = UserDefaults.standard.data(forKey: key),
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              object["protocol"] as? String == "fabushi.miniapp.execution.v1",
              object["miniAppId"] as? String == GlobalDharmaMiniAppBridge.globalDharmaId,
              ((object["revision"] as? NSNumber)?.intValue ?? 0) > 0
        else { return nil }
        return object
    }

    func recordGlobalDharmaExecution(tool: String, result: Any, source: String) {
        guard loggedIn, let scope = globalDharmaAccountScope else { return }
        let execution = Self.nextGlobalDharmaExecution(
            previous: globalDharmaExecution,
            tool: tool,
            result: result,
            source: source
        )
        guard JSONSerialization.isValidJSONObject(execution),
              let data = try? JSONSerialization.data(withJSONObject: execution)
        else { return }
        globalDharmaExecution = execution
        UserDefaults.standard.set(data, forKey: Self.globalDharmaExecutionKey(scope: scope))
    }

    func globalDharmaSharedRuntime() throws -> [String: Any] {
        guard loggedIn,
              let execution = globalDharmaExecution,
              let runtime = Self.globalDharmaRuntime(from: execution)
        else {
            throw MahayanaCoordinator.CoordinatorError.requestFailed(
                "Global Dharma has no completed account-scoped Bot execution to restore"
            )
        }
        return runtime
    }

    private func resetMcpState() {
        mcpAccountEpoch = mcpAccountEpoch == Int.max ? 1 : mcpAccountEpoch + 1
        mcpServerRequestSerial = mcpServerRequestSerial == Int.max ? 1 : mcpServerRequestSerial + 1
        listenerRequestSerial = listenerRequestSerial == Int.max ? 1 : listenerRequestSerial + 1
        mcpOAuthGeneration = mcpOAuthGeneration == Int.max ? 1 : mcpOAuthGeneration + 1
        mcpOAuthSession?.cancel()
        mcpOAuthSession = nil
        listenerIntegrations = [:]
        listenerIntegrationsLoading = false
        listenerConnectingPlatform = nil
        listenerAuthorizingPlatform = nil
        listenerIntegrationErrors = [:]
        mcpToolRequestSerial.removeAll()
        mcpMutationSerial.removeAll()
        mcpServers = []
        mcpToolsByServerId = [:]
        mcpLoading = false
        mcpLoadingServerId = nil
        mcpMutatingToolKey = nil
        mcpError = nil
        mcpNewAccountDraftByServerId = [:]
        mcpRenameDraftByIdentity = [:]
    }

    func initializeApp() async {
        authResolved = false
        do {
            let result = try await bridge.request(method: "feature.auth.status")
            applyAuth(result.value as? [String: Any])
            authResolved = true
            if loggedIn {
                await resolveSignedInOnboardingRoute()
                await refreshAccountUsage()
                await refresh()
                await refreshMcpBackendStatus()
                await refreshMcpServers()
            }
        } catch {
            authResolved = true
            message = "账号状态加载失败：\(error.localizedDescription)"
        }
    }

    private func applyAuth(_ object: [String: Any]?, defaultLoggedIn: Bool = false) {
        let previousMcpScope = globalDharmaAccountScope
        let auth = (object?["auth"] as? [String: Any]) ?? object
        loggedIn = auth?["loggedIn"] as? Bool ?? defaultLoggedIn
        if !loggedIn {
            accountUsage = nil
            accountUsageError = nil
        }

        if loggedIn, let auth, let scope = Self.globalDharmaScope(for: auth) {
            globalDharmaAccountScope = scope
            globalDharmaExecution = Self.loadGlobalDharmaExecution(scope: scope)
        } else {
            // Account-scoped Mini App state must fail closed whenever the Host
            // cannot supply the same stable account identity used by the
            // Coordinator. Never retain a previous account's runtime.
            globalDharmaAccountScope = nil
            globalDharmaExecution = nil
        }

        if previousMcpScope != globalDharmaAccountScope || !loggedIn {
            resetMcpState()
        }

        guard let user = auth?["user"] as? [String: Any] else {
            accountName = "Fabushi"
            accountEmail = ""
            return
        }
        accountName = (user["nickname"] as? String)
            ?? (user["username"] as? String)
            ?? (user["email"] as? String)
            ?? "Fabushi"
        accountEmail = user["email"] as? String ?? ""
    }

    nonisolated static func normalizedAccountDisplayName(_ value: String) -> String {
        value
            .split(whereSeparator: { $0.isWhitespace })
            .joined(separator: " ")
            .trimmingCharacters(in: .whitespacesAndNewlines)
    }

    func updateAccountDisplayName(_ value: String) async throws {
        let normalized = Self.normalizedAccountDisplayName(value)
        guard !normalized.isEmpty, normalized.count <= 200 else {
            throw NSError(
                domain: "Fabushi.AccountMenu",
                code: 1,
                userInfo: [NSLocalizedDescriptionKey: "名称必须为 1–200 个字符。"]
            )
        }
        _ = try await bridge.request(
            method: "updateCursorAccountName",
            params: ["name": normalized]
        )
        let status = try await bridge.request(method: "feature.auth.status")
        applyAuth(status.value as? [String: Any], defaultLoggedIn: true)
        if accountName == "Fabushi" || accountName.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            accountName = normalized
        }
    }

    func openAccountHelp() async throws {
        _ = try await bridge.request(
            method: "openExternal",
            params: ["url": "https://cursor.com/help"]
        )
    }

    func loadConfigurationSettings() async throws -> MobileConfigurationSettingsSnapshot {
        async let review = bridge.request(method: "getAutoReviewInstructions")
        async let provider = bridge.request(method: "getInferenceProvider")
        async let privacy = bridge.request(method: "getCursorPrivacyModeEnabled")
        let (reviewResult, providerResult, privacyResult) = try await (review, provider, privacy)
        let autoReview = try decodeMobileAutoReviewInstructions(reviewResult.value)
        guard let object = providerResult.value as? [String: Any],
              let rawProvider = object["provider"] as? String,
              let inferenceProvider = SandInferenceProvider(rawValue: rawProvider),
              let privacyModeEnabled = privacyResult.value as? Bool
        else {
            throw MahayanaCoordinator.CoordinatorError.invalidResponse
        }
        return .init(
            autoReview: autoReview,
            inferenceProvider: inferenceProvider,
            privacyModeEnabled: privacyModeEnabled
        )
    }

    func updateAutoReviewSettings(
        _ value: SandAutoReviewInstructions
    ) async throws -> SandAutoReviewInstructions {
        let result = try await bridge.request(
            method: "setAutoReviewInstructions",
            params: [
                "isEnabled": value.isEnabled,
                "allowInstructions": value.allowInstructions,
                "blockInstructions": value.blockInstructions,
            ]
        )
        return try decodeMobileAutoReviewInstructions(result.value)
    }

    func updateInferenceProvider(
        _ provider: SandInferenceProvider
    ) async throws -> SandInferenceProvider {
        let result = try await bridge.request(
            method: "setInferenceProvider",
            params: ["provider": provider.rawValue]
        )
        guard let object = result.value as? [String: Any],
              let rawProvider = object["provider"] as? String,
              let authoritative = SandInferenceProvider(rawValue: rawProvider)
        else {
            throw MahayanaCoordinator.CoordinatorError.invalidResponse
        }
        return authoritative
    }

    func submitAccountFeedback(_ value: String, conversationId: String? = nil) async throws {
        let message = value.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !message.isEmpty, message.count <= 10_000 else {
            throw AccountFeedbackError(code: .invalidFeedback)
        }
        let slot = (globalDharmaAccountScope ?? accountEmail)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        guard !slot.isEmpty, slot.count <= 512 else {
            throw AccountFeedbackError(code: .notSignedIn)
        }

        var params: [String: Any] = [
            "accountSlot": slot,
            "message": message,
            "submissionId": UUID().uuidString.lowercased(),
        ]
        if let conversationId {
            let normalizedConversationId = conversationId.trimmingCharacters(in: .whitespacesAndNewlines)
            if !normalizedConversationId.isEmpty {
                params["conversationId"] = normalizedConversationId
            }
        }

        do {
            let result = try await bridge.request(method: "submitFeedback", params: params)
            guard let response = result.value as? [String: Any],
                  let ok = response["ok"] as? Bool
            else {
                throw AccountFeedbackError(code: .unavailable)
            }
            guard ok else {
                throw AccountFeedbackError(code: AccountFeedbackCode.normalize(response["code"]))
            }
        } catch let error as AccountFeedbackError {
            throw error
        } catch {
            throw AccountFeedbackError(code: .unavailable)
        }
    }

    var signedInOnboardingStep: MobileSignedInOnboardingStep {
        get { MobileSignedInOnboardingStep(rawValue: onboardingStep) ?? .meet }
        set { onboardingStep = newValue.rawValue }
    }

    private func legacyOnboardingKey(accountKey: String) -> String {
        Self.legacyOnboardingKeyPrefix + accountKey
    }

    func resolveSignedInOnboardingRoute() async {
        guard loggedIn else {
            onboardingRouteGeneration &+= 1
            onboardingRouteResolved = false
            signedInOnboardingStep = .meet
            return
        }
        onboardingRouteGeneration &+= 1
        let generation = onboardingRouteGeneration
        let accountKey = settingsNoticeAccountKey
        onboardingRouteResolved = false

        let canonicalSeen: Bool
        do {
            let result = try await bridge.request(method: "getOnboardingSeen")
            guard let seen = result.value as? Bool else {
                throw MahayanaCoordinator.CoordinatorError.invalidResponse
            }
            canonicalSeen = seen
        } catch {
            canonicalSeen = false
        }
        guard generation == onboardingRouteGeneration,
              loggedIn,
              settingsNoticeAccountKey == accountKey
        else { return }

        if canonicalSeen {
            signedInOnboardingStep = .completed
            onboardingRouteResolved = true
            return
        }

        let legacyKey = legacyOnboardingKey(accountKey: accountKey)
        if UserDefaults.standard.bool(forKey: legacyKey) {
            do {
                _ = try await bridge.request(
                    method: "setOnboardingSeen",
                    params: ["seen": true]
                )
                guard generation == onboardingRouteGeneration,
                      loggedIn,
                      settingsNoticeAccountKey == accountKey
                else { return }
                UserDefaults.standard.removeObject(forKey: legacyKey)
            } catch {
                // Preserve the legacy marker until the canonical store accepts it.
            }
            guard generation == onboardingRouteGeneration,
                  loggedIn,
                  settingsNoticeAccountKey == accountKey
            else { return }
            signedInOnboardingStep = .completed
            onboardingRouteResolved = true
            return
        }

        let roster: [MobileBotSummary]
        do {
            roster = try await GrokMobileBotService(bridge: bridge).loadOnboardingAgents()
        } catch {
            roster = []
        }
        guard generation == onboardingRouteGeneration,
              loggedIn,
              settingsNoticeAccountKey == accountKey
        else { return }

        if !roster.isEmpty {
            completeSignedInOnboarding()
        } else {
            signedInOnboardingStep = .meet
        }
        onboardingRouteResolved = true
    }

    func advanceOnboarding() {
        let step = signedInOnboardingStep
        guard step != .create, step != .handOff, step != .completed else { return }
        signedInOnboardingStep = step.next
    }

    func retreatOnboarding() {
        guard let previous = signedInOnboardingStep.previous,
              signedInOnboardingStep != .handOff,
              signedInOnboardingStep != .completed
        else { return }
        signedInOnboardingStep = previous
    }

    func beginOnboardingHandOff() {
        signedInOnboardingStep = .handOff
    }

    func completeSignedInOnboarding() {
        guard loggedIn else { return }
        let accountKey = settingsNoticeAccountKey
        signedInOnboardingStep = .completed
        onboardingRouteResolved = true
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                _ = try await self.bridge.request(
                    method: "setOnboardingSeen",
                    params: ["seen": true]
                )
                guard self.loggedIn,
                      self.settingsNoticeAccountKey == accountKey
                else { return }
                UserDefaults.standard.removeObject(
                    forKey: self.legacyOnboardingKey(accountKey: accountKey)
                )
            } catch {
                // Desktop also treats this write as asynchronous. Keep the UI
                // completed while leaving any legacy migration marker intact.
            }
        }
    }

    func skipSignedInOnboarding() {
        completeSignedInOnboarding()
    }

    func beginBrowserLogin() async {
        guard !loginBusy else { return }
        loginBusy = true
        loginError = nil
        do {
            let result = try await bridge.request(method: "feature.auth.browserStart")
            guard let object = result.value as? [String: Any],
                  let attemptId = object["attemptId"] as? String,
                  let loginURLString = (object["loginUrl"] as? String) ?? (object["authorizationUrl"] as? String),
                  let loginURL = URL(string: loginURLString)
            else { throw MahayanaCoordinator.CoordinatorError.requestFailed(
                    "Draft returned an unsupported terminal status."
                ) }
            browserLoginAttemptId = attemptId
            browserLoginURL = loginURL
            loginBusy = false
            if loginURLString.hasPrefix("about:blank#fabushi-test-browser-login") {
                await completeBrowserLogin(attemptId: attemptId)
            } else {
                presentBrowserLogin(loginURL)
            }
        } catch {
            browserLoginAttemptId = nil
            browserLoginURL = nil
            loginBusy = false
            loginError = error.localizedDescription
        }
    }

    func reopenBrowserLogin() async {
        guard let attemptId = browserLoginAttemptId else { return }
        do {
            let result = try await bridge.request(method: "feature.auth.browserReopen", params: ["attemptId": attemptId])
            guard let object = result.value as? [String: Any],
                  let loginURLString = (object["loginUrl"] as? String) ?? (object["authorizationUrl"] as? String),
                  let loginURL = URL(string: loginURLString)
            else { throw MahayanaCoordinator.CoordinatorError.invalidResponse }
            browserLoginURL = loginURL
            if loginURLString.hasPrefix("about:blank#fabushi-test-browser-login") {
                await completeBrowserLogin(attemptId: attemptId)
            } else {
                presentBrowserLogin(loginURL)
            }
        } catch { loginError = error.localizedDescription }
    }

    func cancelBrowserLogin() async {
        webAuthenticationSession?.cancel()
        webAuthenticationSession = nil
        await cancelBrowserLoginAttempt()
    }

    private func cancelBrowserLoginAttempt() async {
        guard let attemptId = browserLoginAttemptId else { return }
        do {
            _ = try await bridge.request(method: "feature.auth.browserCancel", params: ["attemptId": attemptId])
        } catch { loginError = error.localizedDescription }
        browserLoginAttemptId = nil
        browserLoginURL = nil
        loginBusy = false
        message = "登录授权已取消"
    }

    private func presentBrowserLogin(_ loginURL: URL) {
        webAuthenticationSession?.cancel()
        let session = ASWebAuthenticationSession(
            url: loginURL,
            callbackURLScheme: "fabushi"
        ) { [weak self] callbackURL, error in
            Task { @MainActor in
                guard let self else { return }
                self.webAuthenticationSession = nil
                if let callbackURL {
                    self.handleDeepLink(callbackURL)
                    return
                }
                if let authError = error as? ASWebAuthenticationSessionError, authError.code == .canceledLogin {
                    await self.cancelBrowserLoginAttempt()
                    return
                }
                if let error {
                    self.loginError = error.localizedDescription
                    self.message = "登录页面未能完成，请重试"
                }
            }
        }
        session.presentationContextProvider = browserAuthPresentationContext
        session.prefersEphemeralWebBrowserSession = false
        webAuthenticationSession = session
        if !session.start() {
            webAuthenticationSession = nil
            loginError = "无法打开应用内登录页面"
            message = "登录页面未能打开，请重试"
        }
    }

    func runFeatureHostSmokeIfRequested() async {
        guard ProcessInfo.processInfo.environment["FABUSHI_FEATURE_HOST_SMOKE"] == "1" else { return }
        featureHostSmokeStatus = "running"
        do {
            let infoResult = try await bridge.request(method: "feature.info")
            guard let info = infoResult.value as? [String: Any],
                  info["platform"] as? String == "ios",
                  let protocolVersion = info["protocolVersion"] as? String,
                  !protocolVersion.isEmpty,
                  (info["runtimeVersion"] as? String)?.contains("test") == true
            else {
                throw MahayanaCoordinator.CoordinatorError.invalidResponse
            }

            _ = try await bridge.request(method: "feature.auth.status")
            let providers = try await bridge.request(method: "feature.auth.providers")
            guard let providerRows = providers.value as? [[String: Any]],
                  providerRows.contains(where: { $0["id"] as? String == "google" })
            else {
                throw MahayanaCoordinator.CoordinatorError.invalidResponse
            }

            let oauth = try await bridge.request(
                method: "feature.auth.oauthStart",
                params: ["provider": "google"]
            )
            guard let oauthObject = oauth.value as? [String: Any],
                  let attemptId = oauthObject["attemptId"] as? String
            else {
                throw MahayanaCoordinator.CoordinatorError.invalidResponse
            }
            let oauthCompleted = try await bridge.request(
                method: "feature.auth.oauthPoll",
                params: ["attemptId": attemptId]
            )
            guard let completedObject = oauthCompleted.value as? [String: Any],
                  completedObject["status"] as? String == "completed"
            else {
                throw MahayanaCoordinator.CoordinatorError.invalidResponse
            }

            _ = try await executeFeatureCommand(
                type: "chat.send",
                requestId: "ios-chat",
                fields: ["text": "请用一句话说明自动化测试状态"]
            )
            _ = try await executeFeatureCommand(
                type: "marketplace.install",
                requestId: "ios-install",
                fields: ["miniAppId": "global-dharma"]
            )
            _ = try await executeFeatureCommand(
                type: "miniapp.open",
                requestId: "ios-open",
                fields: ["miniAppId": "global-dharma"]
            )
            _ = try await executeFeatureCommand(
                type: "capability.request",
                requestId: "ios-capability",
                fields: [
                    "miniAppId": "global-dharma",
                    "capability": "camera",
                    "reason": "cross-platform UI automation",
                ]
            )
            let approval = try await receiveFeatureEvent(type: "approval.requested")
            guard let approvalId = approval["approvalId"] as? String else {
                throw MahayanaCoordinator.CoordinatorError.invalidResponse
            }
            _ = try await bridge.request(
                method: "feature.approval.resolve",
                params: [
                    "resolution": [
                        "approvalId": approvalId,
                        "decision": "allow-once",
                    ],
                ]
            )

            let longTask = try await executeFeatureCommand(
                type: "runtime.longTask",
                requestId: "ios-long-task",
                fields: ["label": "iOS simulated user operation"]
            )
            guard let operationId = longTask["operationId"] as? String else {
                throw MahayanaCoordinator.CoordinatorError.invalidResponse
            }
            _ = try await bridge.request(
                method: "feature.interrupt",
                params: ["operationId": operationId]
            )

            _ = try await executeFeatureCommand(
                type: "session.clear",
                requestId: "ios-session-clear"
            )
            featureHostSmokeStatus = "passed"
        } catch {
            featureHostSmokeStatus = "failed: \(error.localizedDescription)"
        }
    }

    private func executeFeatureCommand(
        type: String,
        requestId: String,
        fields: [String: Any] = [:]
    ) async throws -> [String: Any] {
        var command = fields
        command["type"] = type
        command["requestId"] = requestId
        let result = try await bridge.request(
            method: "feature.execute",
            params: ["command": command]
        )
        guard let accepted = result.value as? [String: Any],
              accepted["requestId"] as? String == requestId
        else {
            throw MahayanaCoordinator.CoordinatorError.invalidResponse
        }
        return accepted
    }

    func resolveTranscriptDraft(
        _ payload: MobileCanonicalTranscriptCardPayload,
        overrides: [String: Any] = [:],
        action: String
    ) async throws -> MobileDraftResolution {
        guard ["send", "discard"].contains(action),
              let projection = mobileTranscriptDraftProjection(payload),
              let data = payload.json.data(using: .utf8),
              let card = try JSONSerialization.jsonObject(with: data) as? [String: Any],
              var draft = card["draft"] as? [String: Any],
              loggedIn, let accountScope = globalDharmaAccountScope
        else {
            throw MahayanaCoordinator.CoordinatorError.requestFailed(
                "Draft is no longer available."
            )
        }
        let draftId: String
        switch projection {
        case let .email(value): draftId = value.id
        case let .slack(value): draftId = value.id
        }
        for (key, value) in overrides {
            draft[key] = value
        }

        let requestId = "ios-draft-resolve-\(UUID().uuidString.lowercased())"
        _ = try await executeFeatureCommand(
            type: "draft.resolve",
            requestId: requestId,
            fields: [
                "draft": draft,
                "action": action,
            ]
        )

        while true {
            let result = try await bridge.receiveFeatureEvent(
                deadlineMilliseconds: 12_000
            ) { event in
                event["type"] as? String == "draft.changed"
                    && event["draftId"] as? String == draftId
            }
            guard loggedIn, globalDharmaAccountScope == accountScope,
                  let event = result.value as? [String: Any],
                  let status = event["status"] as? String
            else {
                throw MahayanaCoordinator.CoordinatorError.requestFailed(
                    "Draft scope changed before completion."
                )
            }
            let error = event["error"] as? String
            if status == "sending" { continue }
            guard ["sent", "discarded", "failed"].contains(status) else {
                throw MahayanaCoordinator.CoordinatorError.invalidResponse
            }
            return .init(status: status, error: error)
        }
    }

    func provideTranscriptSecret(
        secretRequestId rawSecretRequestId: String,
        value rawValue: String
    ) async throws {
        let secretRequestId = rawSecretRequestId.trimmingCharacters(in: .whitespacesAndNewlines)
        let value = rawValue.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !secretRequestId.isEmpty, !value.isEmpty,
              loggedIn, let accountScope = globalDharmaAccountScope
        else {
            throw MahayanaCoordinator.CoordinatorError.requestFailed(
                "Secret request is no longer available."
            )
        }

        let requestId = "ios-secret-provide-\(UUID().uuidString.lowercased())"
        _ = try await executeFeatureCommand(
            type: "secret.provide",
            requestId: requestId,
            fields: [
                "secretRequestId": secretRequestId,
                "value": value,
            ]
        )
        let result = try await bridge.receiveFeatureEvent(
            deadlineMilliseconds: 8_000
        ) { event in
            event["type"] as? String == "secret.provided"
                && event["secretRequestId"] as? String == secretRequestId
        }
        guard loggedIn, globalDharmaAccountScope == accountScope,
              let event = result.value as? [String: Any],
              event["secretRequestId"] as? String == secretRequestId
        else {
            throw MahayanaCoordinator.CoordinatorError.requestFailed(
                "Secret request scope changed before completion."
            )
        }
    }

    func listenerIntegrationState(for rawPlatform: String) -> MobileListenerIntegrationProjection? {
        let platform = rawPlatform.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        return listenerIntegrations[platform]
    }

    func refreshListenerIntegrations() async {
        guard loggedIn, globalDharmaAccountScope != nil else {
            listenerIntegrations = [:]
            return
        }
        guard !listenerIntegrationsLoading else { return }
        listenerIntegrationsLoading = true
        listenerRequestSerial = listenerRequestSerial == Int.max ? 1 : listenerRequestSerial + 1
        let serial = listenerRequestSerial
        let accountScope = globalDharmaAccountScope
        defer {
            if listenerRequestSerial == serial {
                listenerIntegrationsLoading = false
            }
        }

        do {
            _ = try await executeFeatureCommand(
                type: "listener.list",
                requestId: "ios-listener-list-\(UUID().uuidString.lowercased())"
            )
            let result = try await bridge.receiveFeatureEvent(
                deadlineMilliseconds: 5_120
            ) { event in
                event["type"] as? String == "listener.listed"
            }
            guard serial == listenerRequestSerial,
                  loggedIn,
                  globalDharmaAccountScope == accountScope,
                  let event = result.value as? [String: Any],
                  let projected = projectMobileListenerIntegrations(event["integrations"])
            else { return }
            listenerIntegrations = projected
            for (platform, integration) in projected where integration.isConnected {
                listenerIntegrationErrors.removeValue(forKey: platform)
            }
        } catch is CancellationError {
            return
        } catch {
            guard serial == listenerRequestSerial,
                  loggedIn,
                  globalDharmaAccountScope == accountScope
            else { return }
            for platform in listenerIntegrations.keys {
                listenerIntegrationErrors[platform] = error.localizedDescription
            }
        }
    }

    func connectListenerIntegration(platform rawPlatform: String) async {
        let platform = rawPlatform.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard !platform.isEmpty,
              loggedIn,
              let accountScope = globalDharmaAccountScope,
              listenerIntegrations[platform]?.isConnected != true,
              listenerConnectingPlatform != platform,
              listenerAuthorizingPlatform != platform
        else { return }

        listenerConnectingPlatform = platform
        listenerIntegrationErrors.removeValue(forKey: platform)
        let requestId = "ios-listener-connect-\(platform)-\(UUID().uuidString.lowercased())"
        do {
            _ = try await executeFeatureCommand(
                type: "listener.connect",
                requestId: requestId,
                fields: ["platform": platform]
            )
            let result = try await bridge.receiveFeatureEvent(
                deadlineMilliseconds: 5_120
            ) { event in
                guard let type = event["type"] as? String else { return false }
                if type == "connector.oauthRequested" {
                    return (event["connectorId"] as? String)?.lowercased() == platform
                }
                if type == "connector.changed" {
                    return ((event["connector"] as? [String: Any])?["id"] as? String)?
                        .lowercased() == platform
                }
                if type == "listener.changed" {
                    return ((event["integration"] as? [String: Any])?["platform"] as? String)?
                        .lowercased() == platform
                }
                return false
            }
            guard loggedIn, globalDharmaAccountScope == accountScope,
                  let event = result.value as? [String: Any],
                  let type = event["type"] as? String
            else {
                listenerConnectingPlatform = nil
                return
            }

            if type == "connector.oauthRequested" {
                guard let rawURL = event["authorizationUrl"] as? String,
                      let url = validatedMobileListenerAuthorizationURL(rawURL)
                else {
                    throw MahayanaCoordinator.CoordinatorError.requestFailed(
                        "Listener authorization URL must use HTTPS"
                    )
                }
                listenerConnectingPlatform = nil
                listenerAuthorizingPlatform = platform
                presentMcpOAuth(url, listenerPlatform: platform)
                return
            }

            if type == "listener.changed",
               let integration = projectMobileListenerIntegration(event["integration"]) {
                listenerIntegrations[integration.platform] = integration
                if integration.isConnected {
                    listenerIntegrationErrors.removeValue(forKey: integration.platform)
                }
            } else {
                await refreshListenerIntegrations()
            }
            if listenerConnectingPlatform == platform {
                listenerConnectingPlatform = nil
            }
        } catch is CancellationError {
            if listenerConnectingPlatform == platform {
                listenerConnectingPlatform = nil
            }
        } catch {
            guard loggedIn, globalDharmaAccountScope == accountScope else {
                if listenerConnectingPlatform == platform {
                    listenerConnectingPlatform = nil
                }
                return
            }
            listenerIntegrationErrors[platform] = error.localizedDescription
            if listenerConnectingPlatform == platform {
                listenerConnectingPlatform = nil
            }
        }
    }

    private func receiveFeatureEvent(type expectedType: String) async throws -> [String: Any] {
        let result = try await bridge.receiveFeatureEvent(
            deadlineMilliseconds: 5_120
        ) { event in
            event["type"] as? String == expectedType
        }
        guard let event = result.value as? [String: Any] else {
            throw MahayanaCoordinator.CoordinatorError.invalidResponse
        }
        return event
    }

    func handleDeepLink(_ url: URL) {
        if isMcpOAuthIOSCallback(url) {
            Task { @MainActor in
                do {
                    let result = try await bridge.request(
                        method: "coordinator.mcp.oauthCallback",
                        params: ["url": url.absoluteString]
                    )
                    let outcome = (result.value as? [String: Any])?["outcome"] as? String
                    if outcome != "success", outcome != "notFound" {
                        mcpError = outcome ?? "MCP OAuth callback failed."
                    }
                    await refreshMcpServers()
                } catch {
                    mcpError = error.localizedDescription
                }
            }
            return
        }
        guard url.scheme?.lowercased() == "fabushi",
              url.user == nil,
              url.password == nil,
              url.port == nil,
              url.host?.lowercased() == "auth"
        else { return }
        let parts = url.pathComponents.filter { $0 != "/" && !$0.isEmpty }
        guard parts == ["complete"], let components = URLComponents(url: url, resolvingAgainstBaseURL: false) else { return }
        let allowedNames = Set(["attemptId", "status"])
        var params: [String: String] = [:]
        for item in components.queryItems ?? [] {
            guard allowedNames.contains(item.name), params[item.name] == nil, let value = item.value else { return }
            params[item.name] = value
        }
        let attemptId = params["attemptId"] ?? ""
        let status = (params["status"] ?? "completed").lowercased()
        guard attemptId.range(of: "^[A-Za-z0-9_-]{8,96}$", options: .regularExpression) != nil,
              ["completed", "cancelled", "failed"].contains(status)
        else { return }
        message = status == "completed" ? "登录授权已完成，正在同步账号状态" : "登录授权状态：\(status)"
        if status == "completed" { Task { await completeBrowserLogin(attemptId: attemptId) } }
    }

    func completeBrowserLogin(attemptId: String) async {
        message = "登录授权已完成，正在通过 Rust Host 同步账号状态"
        do {
            let result = try await bridge.request(
                method: "feature.auth.browserPoll",
                params: ["attemptId": attemptId]
            )
            guard let object = result.value as? [String: Any],
                  let status = object["status"] as? String
            else {
                throw MahayanaCoordinator.CoordinatorError.invalidResponse
            }
            switch status {
            case "completed":
                if let auth = object["auth"] as? [String: Any] {
                    applyAuth(auth, defaultLoggedIn: true)
                } else {
                    loggedIn = true
                }
                browserLoginAttemptId = nil
                browserLoginURL = nil
                webAuthenticationSession = nil
                loginError = nil
                await resolveSignedInOnboardingRoute()
                await refreshAccountUsage()
                await refresh()
                await refreshMcpServers()
                message = "登录成功，账号状态已同步"
            case "cancelled":
                message = "登录授权已取消"
            case "failed":
                message = "登录授权失败"
            default:
                message = "登录结果尚未可用，请返回浏览器重试"
            }
        } catch {
            message = "登录状态同步失败：\(error.localizedDescription)"
        }
    }

    func logout() async {
        if let operationId = activeOperationId {
            _ = try? await bridge.request(method: "feature.interrupt", params: ["operationId": operationId])
        }
        do {
            let scopeToClear = globalDharmaAccountScope
            let result = try await bridge.request(method: "feature.auth.logout")
            if let scopeToClear {
                UserDefaults.standard.removeObject(
                    forKey: Self.globalDharmaExecutionKey(scope: scopeToClear)
                )
            }
            applyAuth(result.value as? [String: Any])
        } catch {
            message = "退出登录失败：\(error.localizedDescription)"
            return
        }
        loggedIn = false
        onboardingRouteGeneration &+= 1
        onboardingRouteResolved = false
        signedInOnboardingStep = .meet
        accountUsage = nil
        accountUsageError = nil
        chatMessages = []
        activeOperationId = nil
        chatBusy = false
        message = "已退出登录"
    }

    func sendChat() async {
        let text = chatDraft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, loggedIn, !chatBusy else { return }
        chatDraft = ""
        chatBusy = true
        let requestId = "ios-chat-\(UUID().uuidString.lowercased())"
        chatMessages.append(MobileChatMessage(id: requestId, role: .user, text: text))
        do {
            let accepted = try await executeFeatureCommand(
                type: "chat.send",
                requestId: requestId,
                fields: ["text": text, "agentId": "mahayana-assistant", "mode": "agent"]
            )
            let operationId = accepted["operationId"] as? String ?? requestId
            activeOperationId = operationId
            chatMessages.append(MobileChatMessage(
                id: "thinking:\(operationId)", role: .assistant, text: "", kind: .thinking, operationId: operationId,
                actionTitle: "正在思考", actionStatus: "running"
            ))
            let outcome = await pumpChatEvents(operationId: operationId)
            if outcome.shouldSettleLifecycle {
                chatBusy = false
                activeOperationId = nil
            }
        } catch is CancellationError {
            // View-driven cancellation is a normal lifecycle path.
            if activeOperationId == nil {
                chatBusy = false
                activeOperationId = nil
            }
            return
        } catch {
            message = "发送失败：\(error.localizedDescription)"
            if activeOperationId == nil {
                chatBusy = false
                activeOperationId = nil
            }
            return
        }
    }

    func resolveBoxHandoff(_ entry: MobileChatMessage, resolution: String) async {
        guard entry.actionStatus == "pending",
              let handoffRequestId = entry.handoffRequestId,
              let handoffAgentId = entry.handoffAgentId
        else { return }
        do {
            let accepted = try await executeFeatureCommand(
                type: "box.handoff.resolve",
                requestId: "ios-box-handoff-\(UUID().uuidString.lowercased())",
                fields: [
                    "handoffRequestId": handoffRequestId,
                    "agentId": handoffAgentId,
                    "resolution": resolution,
                ]
            )
            if let index = chatMessages.firstIndex(where: { $0.handoffRequestId == handoffRequestId }) {
                chatMessages[index].actionStatus = resolution
            }
            guard let operationId = accepted["operationId"] as? String, !operationId.isEmpty else { return }
            chatBusy = true
            activeOperationId = operationId
            chatMessages.append(MobileChatMessage(
                id: "thinking:\(operationId)",
                role: .assistant,
                text: "",
                kind: .thinking,
                operationId: operationId,
                actionTitle: "正在从接管状态恢复",
                actionStatus: "running"
            ))
            let outcome = await pumpChatEvents(operationId: operationId)
            if outcome.shouldSettleLifecycle {
                chatBusy = false
                activeOperationId = nil
            }
        } catch {
            message = "恢复 Agent 失败：\(error.localizedDescription)"
        }
    }

    func stopChat() async {
        guard let operationId = activeOperationId else { return }
        _ = try? await bridge.request(method: "feature.interrupt", params: ["operationId": operationId])
    }

    private func pumpChatEvents(operationId: String) async -> MahayanaChatPumpOutcome {
        for _ in 0..<1800 {
            if Task.isCancelled { return .nonTerminal }
            do {
                let ownedHandoffRequestIDs = Set(
                    chatMessages.compactMap(\.handoffRequestId)
                )
                let result = try await bridge.receiveFeatureEvent(
                    deadlineMilliseconds: 450_000
                ) { event in
                    guard let type = event["type"] as? String else { return false }
                    if type == "box.handoff.resolved" {
                        guard let requestID = event["requestId"] as? String else { return false }
                        return ownedHandoffRequestIDs.contains(requestID)
                    }
                    if type == "host.transport" {
                        guard event["channel"] as? String == "transcript.reaction",
                              let payload = event["payload"] as? [String: Any]
                        else { return false }
                        return payload["agentId"] as? String == "mahayana-assistant"
                    }
                    let acceptedTypes: Set<String> = [
                        "box.handoff.requested",
                        "model.routed",
                        "operation.started",
                        "chat.message",
                        "chat.delta",
                        "agent.step",
                        "transcript.card",
                        "operation.completed",
                        "operation.interrupted",
                        "operation.failed",
                    ]
                    guard acceptedTypes.contains(type) else { return false }
                    return (event["operationId"] as? String ?? operationId) == operationId
                }
                guard let event = result.value as? [String: Any],
                      let type = event["type"] as? String
                else { continue }
                switch type {
                case "host.transport":
                    _ = applyMobileTranscriptReactionEvent(
                        event,
                        agentId: "mahayana-assistant",
                        messages: &chatMessages
                    )
                case "box.handoff.requested":
                    let eventOperationId = event["operationId"] as? String ?? operationId
                    guard eventOperationId == operationId,
                          let requestId = event["requestId"] as? String,
                          let agentId = event["agentId"] as? String
                    else { continue }
                    let row = MobileChatMessage(
                        id: "handoff:\(requestId)",
                        role: .assistant,
                        text: event["instruction"] as? String ?? "请完成 Agent 请求的本机步骤。",
                        kind: .handoff,
                        operationId: operationId,
                        actionTitle: "等待用户接管",
                        actionDetail: [event["reason"] as? String, event["domain"] as? String].compactMap { $0 }.joined(separator: " · "),
                        actionStatus: "pending",
                        handoffRequestId: requestId,
                        handoffAgentId: agentId
                    )
                    if let index = chatMessages.firstIndex(where: { $0.handoffRequestId == requestId }) { chatMessages[index] = row } else { chatMessages.append(row) }
                case "box.handoff.resolved":
                    guard let requestId = event["requestId"] as? String else { continue }
                    if let index = chatMessages.firstIndex(where: { $0.handoffRequestId == requestId }) {
                        chatMessages[index].actionStatus = event["resolution"] as? String ?? "completed"
                    }
                case "model.routed":
                    guard (event["operationId"] as? String ?? operationId) == operationId else { continue }
                    let provider = event["provider"] as? String ?? ""
                    let model = event["model"] as? String ?? ""
                    upsertAction(operationId: operationId, stepId: "model-route", title: "选择模型", detail: [provider, model].filter { !$0.isEmpty }.joined(separator: " · "), status: "completed")
                case "operation.started":
                    guard event["operationId"] as? String == operationId else { continue }
                    if !chatMessages.contains(where: { $0.kind == .thinking && $0.operationId == operationId }) {
                        chatMessages.append(MobileChatMessage(id: "thinking:\(operationId)", role: .assistant, text: "", kind: .thinking, operationId: operationId, actionTitle: event["label"] as? String ?? "正在思考", actionStatus: "running"))
                    }
                case "chat.message":
                    let eventOperationId = event["operationId"] as? String ?? operationId
                    guard eventOperationId == operationId else { continue }
                    let role = (event["role"] as? String) == "user" ? MobileChatRole.user : .assistant
                    let eventText = event["text"] as? String ?? ""
                    if role == .assistant {
                        removeThinking(operationId: operationId)
                        let generatedAttachment = event["attachment"] as? [String: Any]
                        if eventText.isEmpty, generatedAttachment != nil, !chatMessages.contains(where: { $0.kind == .message && $0.role == .assistant && $0.operationId == operationId }) {
                            chatMessages.append(MobileChatMessage(id: "assistant:\(operationId)", role: .assistant, text: "", operationId: operationId))
                        } else {
                            upsertAssistantMessage(operationId: operationId, text: eventText, append: false)
                        }
                        if let index = chatMessages.lastIndex(where: { $0.kind == .message && $0.role == .assistant && $0.operationId == operationId }) {
                            let canonicalMessageId = event["messageId"] as? String
                            let attachmentBatchId = event["attachmentBatchId"] as? String
                            chatMessages[index].canonicalMessageId = canonicalMessageId
                            chatMessages[index].replyToMessageId = event["replyToMessageId"] as? String
                            chatMessages[index].attachmentBatchId = attachmentBatchId
                            if let rawAttachment = event["attachment"] as? [String: Any],
                               let attachment = projectMobileChatMessageAttachment(
                                    id: canonicalMessageId ?? "assistant:\(operationId)",
                                    raw: rawAttachment,
                                    batchId: attachmentBatchId,
                                    timestampMs: event["timestampMs"]
                               )
                            {
                                chatMessages[index].attachmentProjection = attachment
                                chatMessages[index].attachmentURL = attachment.url
                                chatMessages[index].attachmentFileName = attachment.name
                                chatMessages[index].attachmentAlt = attachment.alt
                            } else {
                                chatMessages[index].attachmentProjection = nil
                                chatMessages[index].attachmentURL = nil
                                chatMessages[index].attachmentFileName = nil
                                chatMessages[index].attachmentAlt = nil
                            }
                            chatMessages[index].branched = event["branched"] as? Bool ?? false
                        }
                    } else if !chatMessages.contains(where: { $0.role == .user && $0.text == eventText }) {
                        chatMessages.append(MobileChatMessage(id: "user:\(UUID().uuidString)", role: .user, text: eventText))
                    }
                case "chat.delta":
                    guard event["operationId"] as? String == operationId else { continue }
                    removeThinking(operationId: operationId)
                    upsertAssistantMessage(operationId: operationId, text: event["delta"] as? String ?? "", append: true)
                case "agent.step":
                    let eventOperationId = event["operationId"] as? String ?? operationId
                    guard eventOperationId == operationId else { continue }
                    let title = event["title"] as? String ?? "助手动作"
                    let stepId = event["stepId"] as? String ?? "step-\(UUID().uuidString)"
                    upsertAction(operationId: operationId, stepId: stepId, title: title, detail: event["detail"] as? String, status: event["status"] as? String ?? "completed")
                case "transcript.card":
                    guard let row = projectMobileTranscriptCard(
                        event: event,
                        operationId: event["operationId"] as? String ?? operationId
                    ) else { continue }
                    if let index = chatMessages.firstIndex(where: { $0.id == row.id }) {
                        chatMessages[index] = row
                    } else {
                        chatMessages.append(row)
                    }
                case "operation.completed", "operation.interrupted":
                    guard event["operationId"] as? String == operationId else { continue }
                    removeThinking(operationId: operationId)
                    settleActions(operationId: operationId, status: type == "operation.completed" ? "completed" : "failed")
                    return .terminal
                case "operation.failed":
                    guard event["operationId"] as? String == operationId else { continue }
                    removeThinking(operationId: operationId)
                    settleActions(operationId: operationId, status: "failed")
                    message = event["message"] as? String ?? "本次任务失败"
                    return .terminal
                default:
                    break
                }
            } catch {
                message = "消息流中断：\(error.localizedDescription)"
                if Task.isCancelled { return .nonTerminal }
                try? await Task.sleep(nanoseconds: 80_000_000)
                continue
            }
            try? await Task.sleep(nanoseconds: 80_000_000)
        }
        if chatBusy { message = "任务仍在后台运行，稍后会继续同步事件" }
        return .nonTerminal
    }

    private func removeThinking(operationId: String) {
        chatMessages.removeAll { $0.kind == .thinking && $0.operationId == operationId }
    }

    private func settleActions(operationId: String, status: String) {
        for index in chatMessages.indices where chatMessages[index].kind == .action &&
            chatMessages[index].operationId == operationId &&
            chatMessages[index].actionStatus == "running" {
            chatMessages[index].actionStatus = status
        }
    }

    private func upsertAssistantMessage(operationId: String, text: String, append: Bool) {
        guard !text.isEmpty else { return }
        if let index = chatMessages.lastIndex(where: { $0.kind == .message && $0.role == .assistant && $0.operationId == operationId }) {
            if append { chatMessages[index].text += text } else { chatMessages[index].text = text }
            return
        }
        chatMessages.append(MobileChatMessage(id: "assistant:\(operationId)", role: .assistant, text: text, operationId: operationId))
    }

    private func upsertAction(operationId: String, stepId: String, title: String, detail: String?, status: String) {
        let id = "action:\(operationId):\(stepId)"
        let entry = MobileChatMessage(id: id, role: .assistant, text: "", kind: .action, operationId: operationId, actionTitle: title, actionDetail: detail, actionStatus: status)
        if let index = chatMessages.firstIndex(where: { $0.id == id }) { chatMessages[index] = entry } else { chatMessages.append(entry) }
    }

    func refreshAccountUsage() async {
        guard loggedIn else {
            accountUsage = nil
            accountUsageError = nil
            accountUsageLoading = false
            return
        }

        accountUsageLoading = true
        defer { accountUsageLoading = false }

        do {
            let result = try await bridge.request(method: "feature.usage.status")
            guard let payload = result.value as? [String: Any],
                  let usage = AccountUsageProjection(payload: payload)
            else {
                throw MahayanaCoordinator.CoordinatorError.invalidResponse
            }
            accountUsage = usage
            accountUsageError = nil
        } catch {
            // Usage is supplementary account UI. A temporarily unavailable
            // budget endpoint must not turn a valid authenticated session into
            // a login/marketplace failure.
            accountUsage = nil
            accountUsageError = "usage_unavailable"
        }
    }

    private static func canonicalInputSchemaJSON(_ value: Any?) -> String? {
        guard let object = value as? [String: Any],
              JSONSerialization.isValidJSONObject(object),
              let data = try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys])
        else { return nil }
        return String(data: data, encoding: .utf8)
    }

    static func marketplacePlugin(from item: [String: Any]) -> MarketplacePlugin? {
        guard let id = item["pluginId"] as? String, !id.isEmpty else { return nil }
        let source = item["source"] as? [String: Any]
        let releaseManifest = item["releaseManifest"] as? [String: Any]
        let install = item["install"] as? [String: Any]
            ?? releaseManifest?["install"] as? [String: Any]
        let commands = source?["commands"] as? [[String: Any]]
            ?? item["commands"] as? [[String: Any]]
            ?? releaseManifest?["commands"] as? [[String: Any]]
            ?? []
        let installSource = install?["source"] as? [String: Any]
        let sourceRef = (installSource?["sourceRef"] as? String)?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let latestVersion = [
            item["latestVersion"] as? String,
            item["version"] as? String,
            releaseManifest?["version"] as? String,
            install?["version"] as? String,
        ]
        .compactMap { $0?.trimmingCharacters(in: .whitespacesAndNewlines) }
        .first { !$0.isEmpty }

        return MarketplacePlugin(
            pluginId: id,
            displayName: item["displayName"] as? String ?? item["title"] as? String ?? id,
            description: item["description"] as? String ?? "无描述",
            latestVersion: latestVersion,
            sourceRef: sourceRef?.isEmpty == false ? sourceRef : nil,
            tools: commands.compactMap(Self.toolContract(from:))
        )
    }

    static func webMcpToolContract(from item: [String: Any]) -> MiniAppToolContract? {
        guard let name = (item["name"] as? String)?
            .trimmingCharacters(in: .whitespacesAndNewlines),
            !name.isEmpty,
            name.range(of: #"^[A-Za-z0-9_.-]{1,128}$"#, options: .regularExpression) != nil
        else { return nil }
        let annotations = item["annotations"] as? [String: Any]
        let approval: String
        if annotations?["readOnlyHint"] as? Bool == true {
            approval = "none"
        } else if annotations?["destructiveHint"] as? Bool == true {
            approval = "destructive"
        } else {
            approval = "required"
        }
        let description = (item["description"] as? String)?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let title = (item["title"] as? String)?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return MiniAppToolContract(
            name: name,
            description: description?.isEmpty == false ? description! : name,
            approval: approval,
            title: title?.isEmpty == false ? title : nil,
            inputSchemaJSON: canonicalInputSchemaJSON(item["inputSchema"])
        )
    }

    static let globalDharmaStatusFallbackTool = MiniAppToolContract(
        name: "status",
        description: "读取全球法布施 canonical shared runtime 状态",
        approval: "none"
    )

    static func activeLocalInstallSatisfies(
        plugin: MarketplacePlugin,
        pointer: [String: Any]?
    ) -> Bool {
        guard let pointer,
              pointer["pluginId"] as? String == plugin.pluginId
        else { return false }
        guard let targetVersion = plugin.latestVersion, !targetVersion.isEmpty else {
            return true
        }
        return pointer["version"] as? String == targetVersion
    }


    static func marketplacePrivateSkill(from row: [String: Any]) -> MarketplacePrivateSkill? {
        guard let id = (row["id"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines),
              !id.isEmpty,
              let name = row["name"] as? String,
              let source = row["source"] as? String,
              ["workflow", "managed", "plugin"].contains(source)
        else { return nil }
        let publishedByCurrentUser = row["publishedByCurrentUser"] as? Bool ?? false
        if source == "plugin" && !publishedByCurrentUser {
            return nil
        }
        let trigger = row["trigger"] as? [String: Any]
        return MarketplacePrivateSkill(
            id: id,
            name: name,
            description: row["description"] as? String ?? "",
            body: row["body"] as? String ?? "",
            source: source,
            sourceRef: row["sourceRef"] as? String,
            pluginId: row["pluginId"] as? String,
            publishedByCurrentUser: publishedByCurrentUser,
            isEnabledForAgent: row["isEnabledForAgent"] as? Bool ?? true,
            triggerSchedule: trigger?["schedule"] as? String,
            triggerEnabled: trigger?["isEnabled"] as? Bool
        )
    }

    nonisolated static func filterPrivateSkills(
        _ skills: [MarketplacePrivateSkill],
        query: String,
        ownership: MarketplaceSkillOwnershipFilter
    ) -> [MarketplacePrivateSkill] {
        let needle = query.trimmingCharacters(in: .whitespacesAndNewlines)
        return skills.filter { skill in
            let ownershipMatches: Bool
            switch ownership {
            case .all:
                ownershipMatches = true
            case .team:
                ownershipMatches = skill.source == "plugin"
            case .publicItems:
                ownershipMatches = skill.source != "plugin"
            }
            guard ownershipMatches else { return false }
            guard !needle.isEmpty else { return true }
            return skill.name.localizedCaseInsensitiveContains(needle)
                || skill.description.localizedCaseInsensitiveContains(needle)
        }
    }

    var visiblePrivateSkills: [MarketplacePrivateSkill] {
        Self.filterPrivateSkills(
            privateSkills,
            query: privateSkillQuery,
            ownership: privateSkillOwnershipFilter
        )
    }

    static func mcpServer(from row: [String: Any]) -> MarketplaceMcpServer? {
        guard let id = row["id"] as? String,
              !id.isEmpty,
              let name = row["name"] as? String,
              let identifier = row["serverIdentifier"] as? String,
              let transport = row["transport"] as? String,
              let status = row["status"] as? String
        else { return nil }
        return .init(
            serverId: id,
            name: name,
            serverIdentifier: identifier,
            accountKey: (row["accountKey"] as? String) ?? DEFAULT_MCP_ACCOUNT_KEY,
            transport: transport,
            status: status,
            statusDetail: row["statusDetail"] as? String,
            toolCount: (row["toolCount"] as? NSNumber)?.intValue ?? 0,
            disabledToolCount: (row["disabledToolCount"] as? NSNumber)?.intValue ?? 0,
            isTeamServer: row["isTeamServer"] as? Bool ?? false,
            pluginId: row["pluginId"] as? String,
            isRequired: row["isRequired"] as? Bool ?? false,
            managedByTeamPluginPolicy: row["managedByTeamPluginPolicy"] as? Bool ?? false
        )
    }

    private static func mcpTool(from row: [String: Any]) -> MarketplaceMcpTool? {
        guard let name = row["name"] as? String, !name.isEmpty,
              let isDisabled = row["isDisabled"] as? Bool
        else { return nil }
        return .init(
            name: name,
            title: row["title"] as? String,
            description: row["description"] as? String,
            isDisabled: isDisabled
        )
    }

    nonisolated static func optimisticallySetMcpToolEnabled(
        _ tools: [MarketplaceMcpTool],
        toolName: String,
        enabled: Bool
    ) -> [MarketplaceMcpTool] {
        tools.map { tool in
            guard tool.name == toolName else { return tool }
            return .init(
                name: tool.name,
                title: tool.title,
                description: tool.description,
                isDisabled: !enabled
            )
        }
    }

    func refreshMcpBackendStatus() async {
        guard loggedIn else {
            mcpBackendLoggedIn = false
            mcpBackendEmail = ""
            return
        }
        do {
            let response = try await bridge.request(method: "coordinator.mcp.cursorAuth.status")
            guard let auth = response.value as? [String: Any] else {
                throw MahayanaCoordinator.CoordinatorError.invalidResponse
            }
            mcpBackendLoggedIn = auth["loggedIn"] as? Bool ?? false
            mcpBackendEmail = auth["email"] as? String ?? ""
        } catch {
            mcpBackendLoggedIn = false
            mcpBackendEmail = ""
            mcpError = error.localizedDescription
        }
    }

    func beginMcpBackendLogin() async {
        guard loggedIn, !mcpBackendBusy else { return }
        mcpBackendBusy = true
        mcpError = nil
        do {
            let response = try await bridge.request(method: "coordinator.mcp.cursorAuth.start")
            guard let object = response.value as? [String: Any],
                  let attemptId = object["attemptId"] as? String,
                  let rawURL = object["loginUrl"] as? String,
                  let url = URL(string: rawURL)
            else {
                throw MahayanaCoordinator.CoordinatorError.invalidResponse
            }
            mcpBackendLoginAttemptId = attemptId
            await UIApplication.shared.open(url)
            for _ in 0..<150 {
                try? await Task.sleep(for: .seconds(1))
                guard mcpBackendLoginAttemptId == attemptId else {
                    mcpBackendBusy = false
                    return
                }
                let poll = try await bridge.request(
                    method: "coordinator.mcp.cursorAuth.poll",
                    params: ["attemptId": attemptId]
                )
                guard let value = poll.value as? [String: Any] else {
                    throw MahayanaCoordinator.CoordinatorError.invalidResponse
                }
                if value["completed"] as? Bool == true {
                    mcpBackendLoginAttemptId = nil
                    mcpBackendBusy = false
                    await refreshMcpBackendStatus()
                    await refreshMcpServers()
                    return
                }
            }
            throw MahayanaCoordinator.CoordinatorError.requestFailed(
                "MCP backend sign-in timed out."
            )
        } catch {
            if let attemptId = mcpBackendLoginAttemptId {
                _ = try? await bridge.request(
                    method: "coordinator.mcp.cursorAuth.cancel",
                    params: ["attemptId": attemptId]
                )
            }
            mcpBackendLoginAttemptId = nil
            mcpBackendBusy = false
            mcpError = error.localizedDescription
        }
    }

    func logoutMcpBackend() async {
        guard !mcpBackendBusy else { return }
        mcpBackendBusy = true
        defer { mcpBackendBusy = false }
        do {
            _ = try await bridge.request(method: "coordinator.mcp.cursorAuth.logout")
            mcpBackendLoggedIn = false
            mcpBackendEmail = ""
            await refreshMcpServers()
        } catch {
            mcpError = error.localizedDescription
        }
    }

    func authenticateMcpServer(
        serverId: String,
        accountKey: String,
        forceReauth: Bool = false
    ) async {
        let key = accountKey.trimmingCharacters(in: .whitespacesAndNewlines)
        guard loggedIn, mcpBackendLoggedIn, !key.isEmpty else { return }
        mcpError = nil
        do {
            let response = try await bridge.request(
                method: "coordinator.mcp.authenticate",
                params: [
                    "serverId": serverId,
                    "accountKey": key,
                    "forceReauth": forceReauth,
                ]
            )
            guard let value = response.value as? [String: Any],
                  let status = value["status"] as? String
            else {
                throw MahayanaCoordinator.CoordinatorError.invalidResponse
            }
            if status == McpAuthStartStatus.started.rawValue,
               let rawURL = value["authorizationUrl"] as? String,
               let url = URL(string: rawURL) {
                presentMcpOAuth(url)
            } else {
                if let message = value["message"] as? String, !message.isEmpty {
                    mcpError = message
                }
                await refreshMcpServers()
            }
        } catch {
            mcpError = error.localizedDescription
        }
    }

    private func presentMcpOAuth(
        _ url: URL,
        listenerPlatform: String? = nil
    ) {
        mcpOAuthGeneration = mcpOAuthGeneration == Int.max ? 1 : mcpOAuthGeneration + 1
        let generation = mcpOAuthGeneration
        mcpOAuthSession?.cancel()
        if let listenerPlatform {
            listenerAuthorizingPlatform = listenerPlatform
        }
        let session = ASWebAuthenticationSession(
            url: url,
            callbackURLScheme: "fabushi"
        ) { [weak self] callbackURL, error in
            Task { @MainActor in
                guard let self, self.mcpOAuthGeneration == generation else { return }
                self.mcpOAuthSession = nil
                defer {
                    if let listenerPlatform,
                       self.listenerAuthorizingPlatform == listenerPlatform {
                        self.listenerAuthorizingPlatform = nil
                    }
                }
                if let callbackURL {
                    do {
                        let result = try await self.bridge.request(
                            method: "coordinator.mcp.oauthCallback",
                            params: ["url": callbackURL.absoluteString]
                        )
                        let outcome = (result.value as? [String: Any])?["outcome"] as? String
                        if outcome != "success" {
                            let message = outcome ?? "MCP OAuth callback failed."
                            self.mcpError = message
                            if let listenerPlatform {
                                self.listenerIntegrationErrors[listenerPlatform] = message
                            }
                        }
                        await self.refreshMcpServers()
                        if listenerPlatform != nil {
                            await self.refreshListenerIntegrations()
                        }
                    } catch {
                        self.mcpError = error.localizedDescription
                        if let listenerPlatform {
                            self.listenerIntegrationErrors[listenerPlatform] = error.localizedDescription
                        }
                    }
                } else if let error,
                          (error as? ASWebAuthenticationSessionError)?.code
                            != .canceledLogin {
                    self.mcpError = error.localizedDescription
                    if let listenerPlatform {
                        self.listenerIntegrationErrors[listenerPlatform] = error.localizedDescription
                    }
                }
            }
        }
        session.presentationContextProvider = browserAuthPresentationContext
        session.prefersEphemeralWebBrowserSession = false
        mcpOAuthSession = session
        if !session.start() {
            if mcpOAuthGeneration == generation {
                mcpOAuthSession = nil
                let message = "Unable to start MCP authentication."
                mcpError = message
                if let listenerPlatform {
                    listenerIntegrationErrors[listenerPlatform] = message
                    if listenerAuthorizingPlatform == listenerPlatform {
                        listenerAuthorizingPlatform = nil
                    }
                }
            }
        }
    }

    func logoutMcpAccount(serverId: String, accountKey: String) async {
        await mutateMcpAccount(
            method: "coordinator.mcp.logoutAccount",
            params: ["serverId": serverId, "accountKey": accountKey]
        )
    }

    func removeMcpAccount(serverId: String, accountKey: String) async {
        await mutateMcpAccount(
            method: "coordinator.mcp.removeAccount",
            params: ["serverId": serverId, "accountKey": accountKey]
        )
    }

    func renameMcpAccount(
        serverId: String,
        accountKey: String,
        newAccountKey: String
    ) async {
        let next = newAccountKey.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !next.isEmpty else { return }
        await mutateMcpAccount(
            method: "coordinator.mcp.renameAccount",
            params: [
                "serverId": serverId,
                "accountKey": accountKey,
                "newAccountKey": next,
            ]
        )
    }

    private func mutateMcpAccount(
        method: String,
        params: [String: Any]
    ) async {
        guard loggedIn, mcpBackendLoggedIn else { return }
        mcpError = nil
        do {
            _ = try await bridge.request(method: method, params: params)
            await refreshMcpServers()
        } catch {
            mcpError = error.localizedDescription
        }
    }

    func refreshMcpServers() async {
        let noticeFence = settingsNoticeController.makeFence()
        guard loggedIn else {
            resetMcpState()
            return
        }
        let epoch = mcpAccountEpoch
        mcpServerRequestSerial = mcpServerRequestSerial == Int.max ? 1 : mcpServerRequestSerial + 1
        let serial = mcpServerRequestSerial
        mcpLoading = true
        mcpError = nil
        defer {
            if epoch == mcpAccountEpoch, serial == mcpServerRequestSerial {
                mcpLoading = false
            }
        }
        do {
            let response = try await bridge.request(method: "coordinator.mcp.servers")
            guard epoch == mcpAccountEpoch, serial == mcpServerRequestSerial else { return }
            guard let object = response.value as? [String: Any],
                  let rows = object["servers"] as? [[String: Any]]
            else {
                throw MahayanaCoordinator.CoordinatorError.invalidResponse
            }
            let next = rows.compactMap(Self.mcpServer(from:))
            mcpServers = next
            let validIds = Set(next.map(\.serverId))
            mcpToolsByServerId = mcpToolsByServerId.filter { validIds.contains($0.key) }
        } catch {
            guard epoch == mcpAccountEpoch, serial == mcpServerRequestSerial else { return }
            let notice = error.localizedDescription
            mcpError = notice
            publishPluginsNotice(.load, kind: .error, message: notice, fence: noticeFence)
        }
    }

    func loadMcpTools(serverId: String) async {
        guard loggedIn else { return }
        let noticeFence = settingsNoticeController.makeFence()
        let epoch = mcpAccountEpoch
        let previous = mcpToolRequestSerial[serverId] ?? 0
        let serial = previous == Int.max ? 1 : previous + 1
        mcpToolRequestSerial[serverId] = serial
        mcpLoadingServerId = serverId
        mcpError = nil
        defer {
            if epoch == mcpAccountEpoch,
               mcpToolRequestSerial[serverId] == serial,
               mcpLoadingServerId == serverId {
                mcpLoadingServerId = nil
            }
        }
        do {
            let response = try await bridge.request(
                method: "coordinator.mcp.tools",
                params: ["serverId": serverId]
            )
            guard epoch == mcpAccountEpoch,
                  mcpToolRequestSerial[serverId] == serial
            else { return }
            guard let object = response.value as? [String: Any],
                  let rows = object["tools"] as? [[String: Any]]
            else {
                throw MahayanaCoordinator.CoordinatorError.invalidResponse
            }
            mcpToolsByServerId[serverId] = rows.compactMap(Self.mcpTool(from:))
        } catch {
            guard epoch == mcpAccountEpoch,
                  mcpToolRequestSerial[serverId] == serial
            else { return }
            let notice = error.localizedDescription
            mcpError = notice
            publishPluginsNotice(.serverToolsLoad, kind: .error, message: notice, fence: noticeFence)
        }
    }

    func setMcpToolEnabled(
        serverId: String,
        toolName: String,
        enabled: Bool
    ) async {
        guard loggedIn else { return }
        let noticeFence = settingsNoticeController.makeFence()
        let epoch = mcpAccountEpoch
        let key = "\(serverId):\(toolName)"
        let previous = mcpMutationSerial[key] ?? 0
        let serial = previous == Int.max ? 1 : previous + 1
        mcpMutationSerial[key] = serial
        mcpMutatingToolKey = key
        mcpError = nil
        if let current = mcpToolsByServerId[serverId] {
            mcpToolsByServerId[serverId] = Self.optimisticallySetMcpToolEnabled(
                current,
                toolName: toolName,
                enabled: enabled
            )
        }
        do {
            let response = try await bridge.request(
                method: "coordinator.mcp.setToolDisabled",
                params: [
                    "serverId": serverId,
                    "tool": toolName,
                    "disabled": !enabled,
                ]
            )
            guard epoch == mcpAccountEpoch,
                  mcpMutationSerial[key] == serial
            else { return }
            guard let object = response.value as? [String: Any],
                  let rows = object["tools"] as? [[String: Any]]
            else {
                throw MahayanaCoordinator.CoordinatorError.invalidResponse
            }
            mcpToolsByServerId[serverId] = rows.compactMap(Self.mcpTool(from:))
            if mcpMutatingToolKey == key { mcpMutatingToolKey = nil }
            publishPluginsNotice(
                .serverToolToggle,
                kind: .success,
                message: enabled ? "\(toolName) 已启用" : "\(toolName) 已停用",
                fence: noticeFence
            )
            await refreshMcpServers()
        } catch {
            guard epoch == mcpAccountEpoch,
                  mcpMutationSerial[key] == serial
            else { return }
            if mcpMutatingToolKey == key { mcpMutatingToolKey = nil }
            let notice = error.localizedDescription
            mcpError = notice
            publishPluginsNotice(.serverToolToggle, kind: .error, message: notice, fence: noticeFence)
        }
    }



    var hasPrivateSkillAgentScope: Bool {
        privateSkillAgentId?.isEmpty == false
    }

    func bindPrivateSkillAgentScope(agentId: String, agentName: String) {
        let normalizedId = agentId.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !normalizedId.isEmpty else {
            clearPrivateSkillAgentScope()
            return
        }
        let normalizedName = agentName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard privateSkillAgentId != normalizedId else {
            privateSkillAgentName = normalizedName.isEmpty ? normalizedId : normalizedName
            return
        }
        privateSkillScopeGeneration = privateSkillScopeGeneration == Int.max ? 1 : privateSkillScopeGeneration + 1
        privateSkillRequestSerial = privateSkillRequestSerial == Int.max ? 1 : privateSkillRequestSerial + 1
        privateSkillAgentId = normalizedId
        privateSkillAgentName = normalizedName.isEmpty ? normalizedId : normalizedName
        resetPrivateSkillProjection()
    }

    func clearPrivateSkillAgentScope(agentId: String? = nil) {
        if let agentId, privateSkillAgentId != agentId { return }
        privateSkillScopeGeneration = privateSkillScopeGeneration == Int.max ? 1 : privateSkillScopeGeneration + 1
        privateSkillRequestSerial = privateSkillRequestSerial == Int.max ? 1 : privateSkillRequestSerial + 1
        privateSkillAgentId = nil
        privateSkillAgentName = nil
        resetPrivateSkillProjection()
    }

    private func resetPrivateSkillProjection() {
        privateSkills = []
        privateSkillsLoading = false
        privateSkillMutatingId = nil
        privateSkillPublishingId = nil
        privateSkillError = nil
        skillPublishTargets = []
        selectedSkillPublishTeamId = nil
        skillPublishTargetsLoading = false
        privateSkillNameDrafts = [:]
        privateSkillDescriptionDrafts = [:]
        privateSkillBodyDrafts = [:]
    }

    func refreshPrivateSkills() async {
        guard !privateSkillsLoading else { return }
        let noticeFence = settingsNoticeController.makeFence()
        guard let agentId = privateSkillAgentId, !agentId.isEmpty else {
            resetPrivateSkillProjection()
            let notice = "请从具体 Agent 的设置中打开 Yours；Skills 必须绑定明确的 Agent。"
            privateSkillError = notice
            publishPluginsNotice(.privateSkillsLoad, kind: .error, message: notice, fence: noticeFence)
            return
        }
        privateSkillsLoading = true
        privateSkillError = nil
        privateSkillRequestSerial = privateSkillRequestSerial == Int.max ? 1 : privateSkillRequestSerial + 1
        let serial = privateSkillRequestSerial
        let scopeGeneration = privateSkillScopeGeneration
        do {
            // Refresh the external installed-plugin projection first. This is
            // best-effort so private workflows remain available when Cursor
            // publishing auth is not connected.
            _ = try? await bridge.request(
                method: "coordinator.skill.pluginFactsSync",
                params: ["agentId": agentId]
            )
            _ = try await executeFeatureCommand(
                type: "workflow.list",
                requestId: "ios-plugin-skills-list-\(UUID().uuidString.lowercased())",
                fields: ["agentId": agentId]
            )
            let result = try await bridge.receiveFeatureEvent(
                deadlineMilliseconds: 5_120
            ) { event in
                event["type"] as? String == "workflow.listed"
                    && event["agentId"] as? String == agentId
            }
            guard serial == privateSkillRequestSerial,
                  scopeGeneration == privateSkillScopeGeneration,
                  privateSkillAgentId == agentId,
                  let event = result.value as? [String: Any],
                  let rows = event["workflows"] as? [[String: Any]]
            else {
                if serial == privateSkillRequestSerial,
                   scopeGeneration == privateSkillScopeGeneration,
                   privateSkillAgentId == agentId {
                    throw MahayanaCoordinator.CoordinatorError.invalidResponse
                }
                return
            }
            let projected = rows.compactMap(Self.marketplacePrivateSkill(from:))
                .sorted { lhs, rhs in
                    if lhs.name.localizedCaseInsensitiveCompare(rhs.name) == .orderedSame {
                        return lhs.id < rhs.id
                    }
                    return lhs.name.localizedCaseInsensitiveCompare(rhs.name) == .orderedAscending
                }
            privateSkills = projected
            privateSkillNameDrafts = Dictionary(uniqueKeysWithValues: projected.map { ($0.id, $0.name) })
            privateSkillDescriptionDrafts = Dictionary(uniqueKeysWithValues: projected.map { ($0.id, $0.description) })
            privateSkillBodyDrafts = Dictionary(uniqueKeysWithValues: projected.map { ($0.id, $0.body) })
        } catch {
            guard serial == privateSkillRequestSerial,
                  scopeGeneration == privateSkillScopeGeneration,
                  privateSkillAgentId == agentId
            else { return }
            let notice = error.localizedDescription
            privateSkillError = notice
            publishPluginsNotice(.privateSkillsLoad, kind: .error, message: notice, fence: noticeFence)
        }
        if serial == privateSkillRequestSerial,
           scopeGeneration == privateSkillScopeGeneration,
           privateSkillAgentId == agentId {
            privateSkillsLoading = false
        }
    }

    func refreshSkillPublishTargets() async {
        guard !skillPublishTargetsLoading else { return }
        skillPublishTargetsLoading = true
        privateSkillError = nil
        defer { skillPublishTargetsLoading = false }
        do {
            let response = try await bridge.request(method: "coordinator.skill.publishTargets")
            guard let object = response.value as? [String: Any],
                  let rows = object["teams"] as? [[String: Any]]
            else {
                throw MahayanaCoordinator.CoordinatorError.invalidResponse
            }
            let targets = rows.compactMap { row -> MarketplaceSkillPublishTarget? in
                guard let number = row["teamId"] as? NSNumber,
                      number.intValue > 0,
                      let rawName = row["name"] as? String
                else { return nil }
                let name = rawName.trimmingCharacters(in: .whitespacesAndNewlines)
                guard !name.isEmpty else { return nil }
                return .init(teamId: number.intValue, name: name)
            }
            skillPublishTargets = targets
            if let selectedSkillPublishTeamId,
               targets.contains(where: { $0.teamId == selectedSkillPublishTeamId }) {
                return
            }
            selectedSkillPublishTeamId = targets.first?.teamId
        } catch {
            privateSkillError = error.localizedDescription
        }
    }

    func publishPrivateSkill(_ skill: MarketplacePrivateSkill) async {
        guard skill.source == "workflow",
              privateSkillPublishingId == nil,
              privateSkillMutatingId == nil,
              let agentId = privateSkillAgentId,
              !agentId.isEmpty
        else { return }
        if skillPublishTargets.isEmpty {
            await refreshSkillPublishTargets()
        }
        guard let teamId = selectedSkillPublishTeamId else {
            privateSkillError = "没有可发布的团队目标。请确认 Cursor 账号属于至少一个可发布团队。"
            return
        }
        await runSkillPublishLifecycle(
            skillId: skill.id,
            method: "coordinator.skill.publish",
            params: [
                "agentId": agentId,
                "workflowId": skill.id,
                "teamId": NSNumber(value: teamId),
            ],
            requireConfirmed: true
        )
    }

    func resyncPublishedSkill(_ skill: MarketplacePrivateSkill) async {
        guard skill.source == "plugin",
              skill.publishedByCurrentUser,
              privateSkillPublishingId == nil,
              let agentId = privateSkillAgentId,
              !agentId.isEmpty
        else { return }
        await runSkillPublishLifecycle(
            skillId: skill.id,
            method: "coordinator.skill.resync",
            params: ["agentId": agentId, "workflowId": skill.id],
            requireConfirmed: true
        )
    }

    func unpublishPublishedSkill(_ skill: MarketplacePrivateSkill) async {
        guard skill.source == "plugin",
              skill.publishedByCurrentUser,
              privateSkillPublishingId == nil,
              let agentId = privateSkillAgentId,
              !agentId.isEmpty
        else { return }
        await runSkillPublishLifecycle(
            skillId: skill.id,
            method: "coordinator.skill.unpublish",
            params: ["agentId": agentId, "workflowId": skill.id],
            requireConfirmed: false
        )
    }

    private func runSkillPublishLifecycle(
        skillId: String,
        method: String,
        params: [String: Any],
        requireConfirmed: Bool
    ) async {
        guard let agentId = privateSkillAgentId else { return }
        let noticeFence = settingsNoticeController.makeFence()
        let scopeGeneration = privateSkillScopeGeneration
        privateSkillPublishingId = skillId
        privateSkillError = nil
        defer {
            if privateSkillPublishingId == skillId,
               scopeGeneration == privateSkillScopeGeneration,
               privateSkillAgentId == agentId {
                privateSkillPublishingId = nil
            }
        }
        do {
            let response = try await bridge.request(method: method, params: params)
            guard scopeGeneration == privateSkillScopeGeneration,
                  privateSkillAgentId == agentId,
                  let object = response.value as? [String: Any]
            else { return }
            if requireConfirmed, object["confirmed"] as? Bool != true {
                throw MahayanaCoordinator.CoordinatorError.requestFailed(
                    "发布已上传，但 authoritative plugin state 尚未确认；本地 private copy 已保留。"
                )
            }
            await refreshPrivateSkills()
            if privateSkillError == nil {
                publishPluginsNotice(
                    .privateSkillSync,
                    kind: .success,
                    message: "Skill 已与权威插件状态同步",
                    fence: noticeFence
                )
            }
        } catch {
            guard scopeGeneration == privateSkillScopeGeneration,
                  privateSkillAgentId == agentId
            else { return }
            let notice = error.localizedDescription
            privateSkillError = notice
            publishPluginsNotice(.privateSkillSync, kind: .error, message: notice, fence: noticeFence)
            await refreshPrivateSkills()
        }
    }

    func savePrivateSkill(_ skill: MarketplacePrivateSkill) async {
        guard skill.canEdit,
              privateSkillMutatingId == nil,
              let agentId = privateSkillAgentId,
              !agentId.isEmpty
        else { return }
        let name = (privateSkillNameDrafts[skill.id] ?? skill.name)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let description = (privateSkillDescriptionDrafts[skill.id] ?? skill.description)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let body = privateSkillBodyDrafts[skill.id] ?? skill.body
        guard !name.isEmpty, !body.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            let notice = "Skill 名称和 Instructions 不能为空。"
            privateSkillError = notice
            publishPluginsNotice(
                .privateSkillUpdate,
                kind: .error,
                message: notice,
                fence: settingsNoticeController.makeFence()
            )
            return
        }
        var fields: [String: Any] = [
            "agentId": agentId,
            "id": skill.id,
            "name": name,
            "description": description,
            "body": body,
        ]
        if let sourceRef = skill.sourceRef, !sourceRef.isEmpty {
            fields["sourceRef"] = sourceRef
        }
        if let schedule = skill.triggerSchedule, !schedule.isEmpty {
            fields["trigger"] = [
                "schedule": schedule,
                "isEnabled": skill.triggerEnabled ?? true,
            ]
        }
        await mutatePrivateSkill(
            skillId: skill.id,
            type: "workflow.upsert",
            action: "saved",
            fields: fields
        )
    }

    func setPrivateSkillEnabled(_ skill: MarketplacePrivateSkill, enabled: Bool) async {
        guard skill.canToggle,
              privateSkillMutatingId == nil,
              let agentId = privateSkillAgentId,
              !agentId.isEmpty
        else { return }
        await mutatePrivateSkill(
            skillId: skill.id,
            type: "workflow.setEnabled",
            action: "enabled",
            fields: [
                "agentId": agentId,
                "id": skill.id,
                "enabled": enabled,
            ]
        )
    }

    func deletePrivateSkill(_ skill: MarketplacePrivateSkill) async {
        guard skill.source == "workflow",
              privateSkillMutatingId == nil,
              let agentId = privateSkillAgentId,
              !agentId.isEmpty
        else { return }
        await mutatePrivateSkill(
            skillId: skill.id,
            type: "workflow.delete",
            action: "deleted",
            fields: [
                "agentId": agentId,
                "id": skill.id,
            ]
        )
    }

    private func mutatePrivateSkill(
        skillId: String,
        type: String,
        action: String,
        fields: [String: Any]
    ) async {
        guard let agentId = privateSkillAgentId, !agentId.isEmpty else { return }
        let noticeFence = settingsNoticeController.makeFence()
        let noticeOperation: PluginsNoticeOperation = switch type {
        case "workflow.delete": .privateSkillDelete
        case "workflow.setEnabled": .privateSkillToggle
        default: .privateSkillUpdate
        }
        let scopeGeneration = privateSkillScopeGeneration
        privateSkillMutatingId = skillId
        privateSkillError = nil
        defer {
            if privateSkillMutatingId == skillId,
               scopeGeneration == privateSkillScopeGeneration,
               privateSkillAgentId == agentId {
                privateSkillMutatingId = nil
            }
        }
        do {
            _ = try await executeFeatureCommand(
                type: type,
                requestId: "ios-plugin-skill-mutation-\(UUID().uuidString.lowercased())",
                fields: fields
            )
            _ = try await bridge.receiveFeatureEvent(
                deadlineMilliseconds: 5_120
            ) { event in
                guard event["type"] as? String == "workflow.changed",
                      event["agentId"] as? String == agentId,
                      event["action"] as? String == action
                else { return false }
                if event["id"] as? String == skillId { return true }
                return (event["workflow"] as? [String: Any])?["id"] as? String == skillId
            }
            guard scopeGeneration == privateSkillScopeGeneration,
                  privateSkillAgentId == agentId
            else { return }
            // Never trust the mutation echo as the long-lived UI owner. Read the
            // authoritative workflow directory + enablement state back through
            // the same Host before updating the visible Yours surface.
            await refreshPrivateSkills()
            if privateSkillError == nil {
                publishPluginsNotice(
                    noticeOperation,
                    kind: .success,
                    message: "Skill 已更新",
                    fence: noticeFence
                )
            }
        } catch {
            guard scopeGeneration == privateSkillScopeGeneration,
                  privateSkillAgentId == agentId
            else { return }
            let notice = error.localizedDescription
            privateSkillError = notice
            publishPluginsNotice(noticeOperation, kind: .error, message: notice, fence: noticeFence)
        }
    }

    func refresh() async {
        let noticeFence = settingsNoticeController.makeFence()
        loading = true
        defer { loading = false }
        do {
            let result = try await bridge.request(
                method: "feature.marketplace.browse",
                params: ["query": query.isEmpty ? NSNull() : query, "platform": "ios"]
            )
            let object = result.value as? [String: Any]
            let rows = object?["plugins"] as? [[String: Any]] ?? []
            plugins = rows.compactMap(Self.marketplacePlugin(from:))
            message = "原生 iOS · Rust Host 已连接"
        } catch {
            let notice = "市场加载失败：\(error.localizedDescription)"
            message = notice
            publishPluginsNotice(.load, kind: .error, message: notice, fence: noticeFence)
        }
    }

    func install(_ plugin: MarketplacePlugin) async {
        let noticeFence = settingsNoticeController.makeFence()
        guard let version = plugin.latestVersion, !version.isEmpty else {
            let notice = "\(plugin.pluginId) 没有可安装版本"
            message = notice
            publishPluginsNotice(.install, kind: .error, message: notice, fence: noticeFence)
            return
        }
        installingPluginId = plugin.pluginId
        message = "正在安装 \(plugin.pluginId)@\(version)…"
        do {
            let metadata = try await bridge.request(
                method: "feature.marketplace.release",
                params: ["pluginId": plugin.pluginId, "version": version]
            )
            guard let release = (metadata.value as? [String: Any])?["releaseManifest"] as? [String: Any] else {
                throw MahayanaCoordinator.CoordinatorError.invalidResponse
            }
            let install = (metadata.value as? [String: Any])?["install"] as? [String: Any]
                ?? release["install"] as? [String: Any]
            guard install?["protocol"] as? String == "fabushi.marketplace.install.v1",
                  install?["strategy"] as? String == "github-immutable",
                  let source = install?["source"] as? [String: Any],
                  let sourceRef = source["sourceRef"] as? String,
                  !sourceRef.isEmpty,
                  source["marketplaceHostsPackage"] as? Bool != true
            else { throw MahayanaCoordinator.CoordinatorError.invalidResponse }
            let installed = try await bridge.request(
                method: "feature.plugin.install",
                params: ["release": release, "platform": "ios"]
            )
            guard let object = installed.value as? [String: Any] else {
                throw MahayanaCoordinator.CoordinatorError.invalidResponse
            }
            let pluginId = object["pluginId"] as? String ?? plugin.pluginId
            let runtime = object["runtime"] as? String ?? "unknown"
            let permissions = object["requestedPermissions"] as? [String] ?? []
            let accountInstall = try await bridge.request(
                method: "feature.marketplace.add",
                params: ["pluginId": pluginId, "platform": "ios"]
            )
            guard let accountObject = accountInstall.value as? [String: Any],
                  accountObject["accountSynchronized"] as? Bool == true,
                  let bot = accountObject["bot"] as? [String: Any],
                  let botId = bot["id"] as? String,
                  !botId.isEmpty
            else {
                throw MahayanaCoordinator.CoordinatorError.requestFailed("Mini App 已本地安装，但 Fabushi 账号/Bot 同步未完成")
            }
            accountRosterRevision = accountRosterRevision == Int.max ? 1 : accountRosterRevision + 1
            installingPluginId = nil
            publishPluginsNotice(
                .install,
                kind: .success,
                message: "\(pluginId) 已安装",
                fence: noticeFence
            )
            if permissions.isEmpty {
                await startPortableRuntime(pluginId: pluginId, runtime: runtime)
            } else {
                permissionRequest = PluginPermissionRequest(
                    pluginId: pluginId,
                    runtime: runtime,
                    permissions: permissions
                )
                message = "\(pluginId) 请求 \(permissions.count) 项权限"
            }
        } catch {
            installingPluginId = nil
            let notice = "安装失败：\(error.localizedDescription)"
            message = notice
            publishPluginsNotice(.install, kind: .error, message: notice, fence: noticeFence)
        }
    }

    func approvePermissions() async {
        guard let request = permissionRequest else { return }
        let noticeFence = settingsNoticeController.makeFence()
        permissionRequest = nil
        installingPluginId = request.pluginId
        message = "正在授权 \(request.pluginId)…"
        do {
            for permission in request.permissions {
                _ = try await bridge.request(
                    method: "plugin.permission.grant",
                    params: ["pluginId": request.pluginId, "permission": permission]
                )
            }
            installingPluginId = nil
            publishPluginsNotice(
                .authenticate,
                kind: .success,
                message: "\(request.pluginId) 权限已授权",
                fence: noticeFence
            )
            await startPortableRuntime(pluginId: request.pluginId, runtime: request.runtime)
        } catch {
            installingPluginId = nil
            let notice = "授权失败：\(error.localizedDescription)"
            message = notice
            publishPluginsNotice(.authenticate, kind: .error, message: notice, fence: noticeFence)
        }
    }

    func denyPermissions() {
        guard let request = permissionRequest else { return }
        permissionRequest = nil
        installingPluginId = nil
        message = "\(request.pluginId) 已安装，但权限未授权"
    }

    private func startPortableRuntime(pluginId: String, runtime: String) async {
        guard ["deepseek-js", "javascript", "cordis-js"].contains(runtime) else {
            message = "\(pluginId) 已安装 · \(runtime)"
            return
        }
        installingPluginId = pluginId
        do {
            let compatibility = try await bridge.request(
                method: "plugin.compatibility",
                params: ["pluginId": pluginId]
            )
            guard let object = compatibility.value as? [String: Any], object["portableCompatible"] as? Bool == true else {
                throw MahayanaCoordinator.CoordinatorError.requestFailed("插件不满足移动端 portable runtime 约束")
            }
            _ = try await bridge.request(
                method: "runtime.start",
                params: ["pluginId": pluginId, "config": [String: Any]()]
            )
            message = "\(pluginId) 已安装并启动 · \(runtime)"
        } catch {
            message = "\(pluginId) 已安装但启动失败：\(error.localizedDescription)"
        }
        installingPluginId = nil
    }

    func webMcpPlugin(for plugin: MarketplacePlugin) async -> MarketplacePlugin {
        guard plugin.pluginId == GlobalDharmaMiniAppBridge.globalDharmaId else {
            return plugin
        }
        do {
            let advertised = try await globalDharmaBridge.listOfficialMcpTools(pluginId: plugin.pluginId)
            let tools = advertised.compactMap(Self.webMcpToolContract(from:))
            guard !tools.isEmpty else {
                throw MahayanaCoordinator.CoordinatorError.requestFailed(
                    "Global Dharma canonical MCP tools/list returned no usable tools"
                )
            }
            return plugin.replacingTools(tools)
        } catch {
            message = "Global Dharma WebMCP 工具合同不可用，仅保留只读 status 恢复：\(error.localizedDescription)"
            return plugin.replacingTools([Self.globalDharmaStatusFallbackTool])
        }
    }

    private func reconcileLocalMiniAppInstall(_ plugin: MarketplacePlugin) async throws {
        let active = try await bridge.request(
            method: "feature.plugin.active",
            params: ["pluginId": plugin.pluginId]
        )
        if Self.activeLocalInstallSatisfies(
            plugin: plugin,
            pointer: active.value as? [String: Any]
        ) {
            return
        }

        guard let version = plugin.latestVersion, !version.isEmpty else {
            throw MahayanaCoordinator.CoordinatorError.requestFailed(
                "\(plugin.pluginId) has no immutable Marketplace version for local reconciliation"
            )
        }
        let metadata = try await bridge.request(
            method: "feature.marketplace.release",
            params: ["pluginId": plugin.pluginId, "version": version]
        )
        guard let metadataObject = metadata.value as? [String: Any],
              let release = metadataObject["releaseManifest"] as? [String: Any]
        else {
            throw MahayanaCoordinator.CoordinatorError.invalidResponse
        }
        let install = metadataObject["install"] as? [String: Any]
            ?? release["install"] as? [String: Any]
        guard install?["protocol"] as? String == "fabushi.marketplace.install.v1",
              install?["strategy"] as? String == "github-immutable",
              let source = install?["source"] as? [String: Any],
              let sourceRef = source["sourceRef"] as? String,
              !sourceRef.isEmpty,
              source["marketplaceHostsPackage"] as? Bool != true
        else {
            throw MahayanaCoordinator.CoordinatorError.invalidResponse
        }
        let installed = try await bridge.request(
            method: "feature.plugin.install",
            params: ["release": release, "platform": "ios"]
        )
        guard let pointer = installed.value as? [String: Any],
              Self.activeLocalInstallSatisfies(plugin: plugin, pointer: pointer)
        else {
            throw MahayanaCoordinator.CoordinatorError.invalidResponse
        }
    }

    private func localMiniAppHtml(pluginId: String) async throws -> String {
        let result = try await bridge.request(
            method: "feature.plugin.uiDocument",
            params: ["pluginId": pluginId]
        )
        guard let html = (result.value as? [String: Any])?["html"] as? String,
              !html.isEmpty
        else {
            throw MahayanaCoordinator.CoordinatorError.invalidResponse
        }
        return html
    }

    func loadLocalMiniAppHtml(plugin: MarketplacePlugin) async -> String? {
        do {
            return try await localMiniAppHtml(pluginId: plugin.pluginId)
        } catch {
            do {
                try await reconcileLocalMiniAppInstall(plugin)
                return try await localMiniAppHtml(pluginId: plugin.pluginId)
            } catch {
                message = "本地 Mini App 包不可用，转 Hosted WebMCP：\(error.localizedDescription)"
                return nil
            }
        }
    }

    func callWebMcpTool(pluginId: String, name: String, arguments: [String: Any]) async throws -> Any {
        guard name.range(of: #"^[A-Za-z0-9_.-]{1,128}$"#, options: .regularExpression) != nil else {
            throw MahayanaCoordinator.CoordinatorError.requestFailed("Invalid WebMCP tool name")
        }
        if pluginId == GlobalDharmaMiniAppBridge.globalDharmaId {
            let result = try await globalDharmaBridge.callOfficialMcpTool(
                pluginId: pluginId,
                name: name,
                arguments: arguments
            )
            guard name == "status" else { return result }
            let runtime = try globalDharmaSharedRuntime()
            return Self.bridgeGlobalDharmaStatusResult(result, runtime: runtime)
        }
        return try await callRuntimeTool(pluginId: pluginId, name: name, arguments: arguments)
    }

    func callRuntimeTool(pluginId: String, name: String, arguments: [String: Any]) async throws -> Any {
        guard name.range(of: #"^[A-Za-z0-9_.-]{1,128}$"#, options: .regularExpression) != nil else {
            throw MahayanaCoordinator.CoordinatorError.requestFailed("Invalid WebMCP tool name")
        }
        let result = try await bridge.request(
            method: "runtime.call",
            params: [
                "pluginId": pluginId,
                "name": name,
                "arguments": arguments,
            ]
        )
        return result.value
    }

    private static func toolContract(from command: [String: Any]) -> MiniAppToolContract? {
        let name = ((command["tool"] as? String) ?? (command["name"] as? String) ?? "")
            .trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty,
              name.range(of: #"^[A-Za-z0-9_.-]{1,128}$"#, options: .regularExpression) != nil
        else { return nil }
        let description = ((command["description"] as? String) ?? name)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let title = (command["title"] as? String)?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return MiniAppToolContract(
            name: name,
            description: description.isEmpty ? name : description,
            approval: (command["approval"] as? String) ?? "none",
            title: title?.isEmpty == false ? title : nil,
            inputSchemaJSON: canonicalInputSchemaJSON(command["inputSchema"] ?? command["schema"])
        )
    }
}
