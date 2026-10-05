import SwiftUI

internal struct MobileBotSummary: Identifiable, Equatable, Sendable {
    let id: String
    let name: String
    let description: String
    let miniAppId: String?
    let menuButtonText: String?
    let isGroup: Bool
    let memberIds: [String]
    let isSharedRoom: Bool

    init(
        id: String,
        name: String,
        description: String,
        miniAppId: String? = nil,
        menuButtonText: String? = nil,
        isGroup: Bool = false,
        memberIds: [String] = [],
        isSharedRoom: Bool = false
    ) {
        self.id = id
        self.name = name
        self.description = description
        self.miniAppId = miniAppId
        self.menuButtonText = menuButtonText
        self.isGroup = isGroup
        self.memberIds = memberIds
        self.isSharedRoom = isSharedRoom
    }
}
