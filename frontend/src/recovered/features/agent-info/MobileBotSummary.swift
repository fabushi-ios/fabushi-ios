import SwiftUI

internal struct MobileBotSummary: Identifiable, Equatable, Sendable {
    let id: String
    let name: String
    let description: String
    let title: String?
    let notifyOnUpdatesEnabled: Bool
    let hidden: Bool
    let unread: Bool
    let conversationId: String?
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
        notifyOnUpdatesEnabled: Bool = false,
        hidden: Bool = false,
        unread: Bool = false,
        conversationId: String? = nil,
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
        self.notifyOnUpdatesEnabled = notifyOnUpdatesEnabled
        self.hidden = hidden
        self.unread = unread
        self.conversationId = conversationId
        self.miniAppId = miniAppId
        self.menuButtonText = menuButtonText
        self.isGroup = isGroup
        self.memberIds = memberIds
        self.isSharedRoom = isSharedRoom
    }
}
