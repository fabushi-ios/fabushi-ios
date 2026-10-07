import SwiftUI

internal enum MobileBotLastEntry: Equatable, Sendable {
    case text(String)
    case attachment(count: Int, kinds: [String: Int])
    case link(String)
}

internal struct MobileBotSummary: Identifiable, Equatable, Sendable {
    let id: String
    let name: String
    let description: String
    let title: String?
    let avatarDataURL: String?
    let avatarShape: String?
    let avatarColor: String?
    let notifyOnUpdatesEnabled: Bool
    let hidden: Bool
    let unread: Bool
    let conversationId: String?
    let lastEntry: MobileBotLastEntry?
    let lastMessageId: String?
    let lastMessagePreview: String?
    let updatedAtMs: Int64?
    let isComposingMessage: Bool
    let waitingReason: String?
    let isRunning: Bool
    let draftPrompt: String?
    let miniAppId: String?
    let menuButtonText: String?
    let isGroup: Bool
    let memberIds: [String]
    let isSharedRoom: Bool

    init(
        id: String,
        name: String,
        description: String,
        title: String? = nil,
        avatarDataURL: String? = nil,
        avatarShape: String? = nil,
        avatarColor: String? = nil,
        notifyOnUpdatesEnabled: Bool = false,
        hidden: Bool = false,
        unread: Bool = false,
        conversationId: String? = nil,
        lastEntry: MobileBotLastEntry? = nil,
        lastMessageId: String? = nil,
        lastMessagePreview: String? = nil,
        updatedAtMs: Int64? = nil,
        isComposingMessage: Bool = false,
        waitingReason: String? = nil,
        isRunning: Bool = false,
        draftPrompt: String? = nil,
        miniAppId: String? = nil,
        menuButtonText: String? = nil,
        isGroup: Bool = false,
        memberIds: [String] = [],
        isSharedRoom: Bool = false
    ) {
        self.id = id
        self.name = name
        self.description = description
        self.title = title
        self.avatarDataURL = avatarDataURL
        self.avatarShape = avatarShape
        self.avatarColor = avatarColor
        self.notifyOnUpdatesEnabled = notifyOnUpdatesEnabled
        self.hidden = hidden
        self.unread = unread
        self.conversationId = conversationId
        self.lastEntry = lastEntry
        self.lastMessageId = lastMessageId
        self.lastMessagePreview = lastMessagePreview
        self.updatedAtMs = updatedAtMs
        self.isComposingMessage = isComposingMessage
        self.waitingReason = waitingReason
        self.isRunning = isRunning
        self.draftPrompt = draftPrompt
        self.miniAppId = miniAppId
        self.menuButtonText = menuButtonText
        self.isGroup = isGroup
        self.memberIds = memberIds
        self.isSharedRoom = isSharedRoom
    }
}
