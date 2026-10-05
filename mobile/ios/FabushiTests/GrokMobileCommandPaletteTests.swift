import XCTest
@testable import Fabushi

final class GrokMobileCommandPaletteTests: XCTestCase {
    func testSearchNormalizationAndFuzzyScoringMatchDesktopSemantics() {
        XCTAssertEqual(
            GrokMobileCommandPaletteModel.normalizeSearch(" HéLLo—World "),
            "hello world"
        )
        XCTAssertEqual(
            GrokMobileCommandPaletteModel.searchTokens("  settings / appearance "),
            ["settings", "appearance"]
        )
        XCTAssertNotNil(
            GrokMobileCommandPaletteModel.fuzzyScore(
                value: "open full messaging",
                query: "of"
            )
        )
        XCTAssertNil(
            GrokMobileCommandPaletteModel.fuzzyScore(
                value: "abcdefghi",
                query: "ai"
            )
        )
    }

    func testEmptyAllTabKeepsPrimaryNavigationAndActions() {
        let direct = conversation(id: "direct-1", title: "Alice", kind: .direct)
        let group = conversation(id: "group-1", title: "Engineering", kind: .group)
        let bot = MobileBotSummary(id: "bot-1", name: "Research", description: "Find sources")
        let action = MobileCommandPaletteAction(
            id: "create-bot",
            label: "New Bot",
            keywords: ["new", "create"],
            detail: "Actions",
            kind: .createBot
        )

        let all = GrokMobileCommandPaletteModel.entries(
            bots: [bot],
            conversations: [direct, group],
            messagesByConversation: [:],
            actions: [action],
            query: "",
            tab: .all
        )
        XCTAssertEqual(Set(all.map(\.id)), [
            "bot:bot-1",
            "conversation:direct-1",
            "conversation:group-1",
            "action:create-bot",
        ])

        let groups = GrokMobileCommandPaletteModel.entries(
            bots: [bot],
            conversations: [direct, group],
            messagesByConversation: [:],
            actions: [action],
            query: "",
            tab: .groups
        )
        XCTAssertEqual(groups.map(\.id), ["conversation:group-1"])
    }

    func testCanonicalMessagingProjectionFeedsMessagesFilesAndLinks() {
        let conversation = conversation(id: "chat-1", title: "Release", kind: .direct)
        let textMessage = message(
            id: "message-1",
            conversationId: conversation.id,
            text: "Review https://example.com/docs before launch"
        )
        let fileMessage = message(
            id: "message-2",
            conversationId: conversation.id,
            text: "Guide.pdf",
            contentType: "document",
            mediaFileName: "Guide.pdf",
            mediaMimeType: "application/pdf"
        )
        let messages = [conversation.id: [textMessage, fileMessage]]

        let messageResults = GrokMobileCommandPaletteModel.entries(
            bots: [],
            conversations: [conversation],
            messagesByConversation: messages,
            actions: [],
            query: "launch",
            tab: .messages
        )
        XCTAssertEqual(messageResults.map(\.id), ["message:chat-1:message-1"])

        let fileResults = GrokMobileCommandPaletteModel.entries(
            bots: [],
            conversations: [conversation],
            messagesByConversation: messages,
            actions: [],
            query: "guide",
            tab: .files
        )
        XCTAssertEqual(fileResults.map(\.id), ["file:chat-1:message-2"])

        let linkResults = GrokMobileCommandPaletteModel.entries(
            bots: [],
            conversations: [conversation],
            messagesByConversation: messages,
            actions: [],
            query: "example",
            tab: .links
        )
        XCTAssertEqual(linkResults.count, 1)
        guard case .link(let link) = linkResults[0] else {
            return XCTFail("Expected a canonical link search result")
        }
        XCTAssertEqual(link.url, "https://example.com/docs")
        XCTAssertEqual(link.conversationId, conversation.id)
    }


    func testRoutineRosterProjectionAndSearchUsesCanonicalHostShape() {
        let raw: [[String: Any]] = [[
            "agentId": "research-bot",
            "automation": [
                "id": "daily",
                "name": "Daily research",
                "triggerDescription": "@daily",
                "createdAt": NSNumber(value: 10),
                "lastRunAt": NSNumber(value: 20),
            ],
        ]]
        let routines = GrokMobileCommandPaletteModel.routines(from: raw)
        XCTAssertEqual(routines.count, 1)
        XCTAssertEqual(routines[0].agentId, "research-bot")
        XCTAssertEqual(routines[0].automationId, "daily")
        XCTAssertEqual(routines[0].triggerDescription, "@daily")

        let rows = GrokMobileCommandPaletteModel.entries(
            bots: [],
            conversations: [],
            messagesByConversation: [:],
            actions: [],
            routines: routines,
            query: "daily",
            tab: .routines
        )
        XCTAssertEqual(rows.map(\.id), ["routine:research-bot:daily"])
    }

    func testLinkMetadataEnrichesCanonicalLinkWithoutOwningLinkStorage() {
        let conversation = conversation(id: "chat-1", title: "Release", kind: .direct)
        let textMessage = message(
            id: "message-1",
            conversationId: conversation.id,
            text: "Review https://example.com/docs before launch"
        )
        let rows = GrokMobileCommandPaletteModel.entries(
            bots: [],
            conversations: [conversation],
            messagesByConversation: [conversation.id: [textMessage]],
            actions: [],
            linkMetadata: [
                "https://example.com/docs": .init(
                    title: "Example Docs",
                    description: "Release guide",
                    hostname: "example.com"
                )
            ],
            query: "release guide",
            tab: .links
        )
        XCTAssertEqual(rows.count, 1)
        guard case .link(let link) = rows[0] else {
            return XCTFail("Expected an enriched link")
        }
        XCTAssertEqual(link.metadataTitle, "Example Docs")
        XCTAssertEqual(link.metadataDescription, "Release guide")
    }

    func testRoutinesRemainFailClosedWithoutCanonicalHostRoster() {
        let rows = GrokMobileCommandPaletteModel.entries(
            bots: [MobileBotSummary(id: "bot-1", name: "Research", description: "")],
            conversations: [conversation(id: "chat-1", title: "Chat", kind: .direct)],
            messagesByConversation: [:],
            actions: [],
            query: "",
            tab: .routines
        )
        XCTAssertTrue(rows.isEmpty)
    }

    private func conversation(
        id: String,
        title: String,
        kind: ConversationKind
    ) -> ConversationSummary {
        ConversationSummary(
            id: id,
            title: title,
            description: "",
            ownerId: nil,
            participants: [],
            preview: "",
            time: "",
            badge: "",
            kind: kind,
            unreadCount: 0,
            isPinned: false,
            isMuted: false,
            isArchived: false,
            lastMessageId: nil,
            pinnedMessageIds: [],
            folderIds: [],
            markedUnread: false
        )
    }

    private func message(
        id: String,
        conversationId: String,
        text: String,
        contentType: String = "text",
        mediaFileName: String? = nil,
        mediaMimeType: String? = nil
    ) -> ChatMessage {
        ChatMessage(
            id: id,
            conversationId: conversationId,
            text: text,
            contentType: contentType,
            mediaFileName: mediaFileName,
            mediaBlobId: mediaFileName == nil ? nil : "blob-1",
            mediaMimeType: mediaMimeType,
            mediaSizeBytes: mediaFileName == nil ? 0 : 42,
            contactName: nil,
            latitude: nil,
            longitude: nil,
            pollQuestion: nil,
            pollOptions: [],
            pollMultipleAnswers: false,
            isOutgoing: true,
            time: "",
            replyToMessageId: nil,
            forwardOrigin: nil,
            reactions: [],
            deliveryState: "sent",
            isEdited: false,
            isPinned: false
        )
    }
}
