import SwiftUI

internal enum MobileBotLastEntry: Equatable, Sendable {
    case text(String)
    case attachment(count: Int, kinds: [String: Int])
    case link(String)
}

internal enum MobileAgentAvatarState: String, Equatable, Sendable {
    case idle
    case thinking
    case searching
    case working
    case loading
    case sending
    case orbit
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
    let avatarState: MobileAgentAvatarState
    let draftPrompt: String?
    let miniAppId: String?
    let menuButtonText: String?
    let isGroup: Bool
    let memberIds: [String]
    let conversationPartnerIds: [String]
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
        avatarState: MobileAgentAvatarState = .idle,
        draftPrompt: String? = nil,
        miniAppId: String? = nil,
        menuButtonText: String? = nil,
        isGroup: Bool = false,
        memberIds: [String] = [],
        conversationPartnerIds: [String] = [],
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
        self.avatarState = avatarState
        self.draftPrompt = draftPrompt
        self.miniAppId = miniAppId
        self.menuButtonText = menuButtonText
        self.isGroup = isGroup
        self.memberIds = memberIds
        self.conversationPartnerIds = conversationPartnerIds
        self.isSharedRoom = isSharedRoom
    }
}


extension MobileBotSummary {
    func replacingConversationPartnerIds(_ ids: [String]) -> MobileBotSummary {
        MobileBotSummary(
            id: id,
            name: name,
            description: description,
            title: title,
            avatarDataURL: avatarDataURL,
            avatarShape: avatarShape,
            avatarColor: avatarColor,
            notifyOnUpdatesEnabled: notifyOnUpdatesEnabled,
            hidden: hidden,
            unread: unread,
            conversationId: conversationId,
            lastEntry: lastEntry,
            lastMessageId: lastMessageId,
            lastMessagePreview: lastMessagePreview,
            updatedAtMs: updatedAtMs,
            isComposingMessage: isComposingMessage,
            waitingReason: waitingReason,
            isRunning: isRunning,
            avatarState: avatarState,
            draftPrompt: draftPrompt,
            miniAppId: miniAppId,
            menuButtonText: menuButtonText,
            isGroup: isGroup,
            memberIds: memberIds,
            conversationPartnerIds: ids,
            isSharedRoom: isSharedRoom
        )
    }
}
