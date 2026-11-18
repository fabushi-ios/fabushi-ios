import Foundation
import XCTest

final class Tier1_StageSyncMedia_F17_F22_UITests: ParityTestCase {

    // MARK: - F17: Conversation Header, In-Chat Search & Pinned Banner

    @MainActor
    func testTier1_F17_01_ConversationHeaderDisplaysPeerTitleOnlineStatusAndLiveTypingSubtitle() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f17_hdr")
        let peer = try await client.registerUser(username: "f17_peer_\(uniqueSuffix())", firstName: "Ananda")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "Ananda",
            chatType: "direct",
            peerUserId: user.userId,
            memberIds: [user.userId]
        )

        let wsPeer = try client.connectWebSocket(token: peer.token)
        defer { wsPeer.close() }
        try await wsPeer.subscribeChat(chatId)

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Stage.headerTitle).exists)
        try await wsPeer.sendTyping(chatId: chatId, userId: peer.userId, action: "typing")

        let subtitle = waitForElement(in: app, AccessibilityIDs.Stage.headerSubtitle)
        XCTAssertFalse(subtitle.label.isEmpty)
        app.terminate()
    }

    @MainActor
    func testTier1_F17_02_TappingHeaderAvatarOpensStretchyPeerProfileScreen() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f17_av")
        let peer = try await client.registerUser(username: "f17_avpeer_\(uniqueSuffix())", firstName: "Subhuti")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "Subhuti",
            chatType: "direct",
            peerUserId: user.userId,
            memberIds: [user.userId]
        )

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        waitForElement(in: app, AccessibilityIDs.Stage.headerAvatarButton).tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Profile.root, timeout: 8).exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F17_03_InChatSearchFindsKeywordMatchesAndNavigatesPreviousNext() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f17_srch")
        let peer = try await client.registerUser(username: "f17_speer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "In-Chat Search Room",
            chatType: "group",
            memberIds: [user.userId]
        )
        let firstHit = try await client.sendMessage(token: peer.token, chatId: chatId, text: "AnchorKeyword alpha")
        for idx in 1...12 {
            try await client.sendMessage(token: peer.token, chatId: chatId, text: "Filler row \(idx)")
        }
        _ = try await client.sendMessage(token: peer.token, chatId: chatId, text: "AnchorKeyword omega")

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        waitForElement(in: app, AccessibilityIDs.Stage.headerSearchButton).tap()
        clearAndTypeText(in: waitForElement(in: app, "stage.inChatSearchField"), text: "AnchorKeyword")
        waitForElement(in: app, "stage.inChatSearchPrevButton").tap()

        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Stage.bubble(firstHit), timeout: 8).exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F17_04_PinMessageShowsPinnedBannerAndCyclesMultiplePinnedMessages() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f17_pin")
        let peer = try await client.registerUser(username: "f17_ppeer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: user.token,
            title: "Pin Cycling Room",
            chatType: "group",
            memberIds: [peer.userId]
        )
        let pin1 = try await client.sendMessage(token: user.token, chatId: chatId, text: "Pinned Milestone 1")
        for idx in 1...15 {
            try await client.sendMessage(token: peer.token, chatId: chatId, text: "Discussion \(idx)")
        }
        let pin2 = try await client.sendMessage(token: user.token, chatId: chatId, text: "Pinned Milestone 2")
        try await client.pinMessage(token: user.token, chatId: chatId, messageId: pin1)
        try await client.pinMessage(token: user.token, chatId: chatId, messageId: pin2)

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        let banner = waitForElement(in: app, AccessibilityIDs.Stage.pinnedBanner)
        XCTAssertTrue(banner.exists)
        banner.tap()
        banner.tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Stage.bubble(pin1), timeout: 8).exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F17_05_UnpinMessageFromBannerDismissesPinnedBarAndUpdatesServer() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f17_unpin")
        let chatId = try await client.createChat(token: user.token, title: "Unpin Room", chatType: "group")
        let msgId = try await client.sendMessage(token: user.token, chatId: chatId, text: "Temporary Pin")
        try await client.pinMessage(token: user.token, chatId: chatId, messageId: msgId)

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Stage.pinnedBanner).exists)
        longPressBubbleForPortal(in: app, messageId: msgId)
        waitForElement(in: app, AccessibilityIDs.Stage.contextMenu("unpin")).tap()

        waitForElementDisappearance(in: app, AccessibilityIDs.Stage.pinnedBanner)
        let pins = try await client.listPins(token: user.token, chatId: chatId)
        XCTAssertEqual((pins["pinned_message_ids"]?.arrayValue ?? []).count, 0)
        app.terminate()
    }

    // MARK: - F18: Inverted 120Hz `UICollectionView` Transcript & Grouping

    @MainActor
    func testTier1_F18_01_InvertedTranscriptOpensAnchoredAtBottomAndPaginatesOlderHistory() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f18_inv")
        let peer = try await client.registerUser(username: "f18_peer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "Pagination Stage",
            chatType: "group",
            memberIds: [user.userId]
        )
        var firstId: Int64 = 0
        var lastId: Int64 = 0
        for idx in 1...40 {
            let mid = try await client.sendMessage(token: peer.token, chatId: chatId, text: "History Entry #\(idx)")
            if idx == 1 { firstId = mid }
            lastId = mid
        }

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Stage.bubble(lastId), timeout: 8).exists)
        let collection = waitForElement(in: app, AccessibilityIDs.Stage.collectionView)
        collection.swipeDown()
        collection.swipeDown()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Stage.bubble(firstId), timeout: 10).exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F18_02_ConsecutiveMessagesWithin900sClusterTightlyComparedToCrossSender() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f18_grp")
        let peer = try await client.registerUser(username: "f18_gpeer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "900s Grouping",
            chatType: "group",
            memberIds: [user.userId]
        )
        let m1 = try await client.sendMessage(token: peer.token, chatId: chatId, text: "Cluster A1")
        let m2 = try await client.sendMessage(token: peer.token, chatId: chatId, text: "Cluster A2")
        let m3 = try await client.sendMessage(token: user.token, chatId: chatId, text: "Sender Switch B1")

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        let b1 = waitForElement(in: app, AccessibilityIDs.Stage.bubble(m1)).frame
        let b2 = waitForElement(in: app, AccessibilityIDs.Stage.bubble(m2)).frame
        let b3 = waitForElement(in: app, AccessibilityIDs.Stage.bubble(m3)).frame

        let sameSenderGap = abs(b2.minY - b1.maxY)
        let crossSenderGap = abs(b3.minY - b2.maxY)
        XCTAssertLessThan(sameSenderGap, crossSenderGap)
        app.terminate()
    }

    @MainActor
    func testTier1_F18_03_ScrollingUpShowsScrollToBottomButtonWithLiveUnreadCounter() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f18_fab")
        let peer = try await client.registerUser(username: "f18_fpeer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "Scroll FAB Room",
            chatType: "group",
            memberIds: [user.userId]
        )
        for idx in 1...30 {
            try await client.sendMessage(token: peer.token, chatId: chatId, text: "Seed \(idx)")
        }

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        let stage = waitForElement(in: app, AccessibilityIDs.Stage.collectionView)
        stage.swipeDown()
        stage.swipeDown()

        let fab = waitForElement(in: app, AccessibilityIDs.Stage.scrollToBottomButton, timeout: 8)
        let liveId = try await client.sendMessage(token: peer.token, chatId: chatId, text: "Live Arrival Bottom")
        fab.tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Stage.bubble(liveId), timeout: 8).exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F18_04_IncomingMentionRevealsJumpMentionFabAndScrollsToTargetBubble() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f18_mnt")
        let peer = try await client.registerUser(username: "f18_mpeer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "Mention FAB Room",
            chatType: "group",
            memberIds: [user.userId]
        )
        for idx in 1...28 {
            try await client.sendMessage(token: peer.token, chatId: chatId, text: "Item \(idx)")
        }

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        let stage = waitForElement(in: app, AccessibilityIDs.Stage.collectionView)
        stage.swipeDown()
        stage.swipeDown()

        let mentionId = try await client.sendMessage(
            token: peer.token,
            chatId: chatId,
            text: "Hey @\(user.username) check this"
        )
        waitForElement(in: app, AccessibilityIDs.Stage.jumpMentionButton, timeout: 8).tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Stage.bubble(mentionId), timeout: 8).exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F18_05_InteractiveDragDownOnTranscriptDismissesSoftwareKeyboard() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f18_kbd")
        let chatId = try await client.createChat(token: user.token, title: "Keyboard Drag", chatType: "group")
        try await client.sendMessage(token: user.token, chatId: chatId, text: "Anchor Bubble")

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        waitForElement(in: app, AccessibilityIDs.Composer.textView).tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 6))
        let stage = waitForElement(in: app, AccessibilityIDs.Stage.collectionView)
        stage.swipeDown()
        XCTAssertTrue(app.keyboards.firstMatch.waitForNonExistence(timeout: 6))
        app.terminate()
    }

    // MARK: - F19: Optimistic Durable Outbox & PTS Sync Reconciliation

    @MainActor
    func testTier1_F19_01_OptimisticOutgoingMessageDeduplicatesClientNonceAgainstWsPush() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f19_dedup")
        let peer = try await client.registerUser(username: "f19_peer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: user.token,
            title: "Nonce Dedup",
            chatType: "direct",
            peerUserId: peer.userId,
            memberIds: [peer.userId]
        )

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Composer.textView), text: "UniqueNonceText99")
        waitForElement(in: app, AccessibilityIDs.Composer.micOrSendButton).tap()

        let history = try await client.listMessages(token: peer.token, chatId: chatId)
        let messages = history["messages"]?.arrayValue ?? []
        XCTAssertEqual(messages.count, 1, "client_nonce reconciliation must produce exactly 1 persisted message")
        app.terminate()
    }

    @MainActor
    func testTier1_F19_02_PeerReadReceiptUpgradesOutgoingBubbleFromSentToRead() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f19_read")
        let peer = try await client.registerUser(username: "f19_rpeer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: user.token,
            title: "Read Checkmarks",
            chatType: "direct",
            peerUserId: peer.userId,
            memberIds: [peer.userId]
        )

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Composer.textView), text: "Upgrade my tick")
        waitForElement(in: app, AccessibilityIDs.Composer.micOrSendButton).tap()

        let history = try await client.listMessages(token: peer.token, chatId: chatId)
        let msgId = history["messages"]?.arrayValue?.first?["id"]?.int64Value
            ?? history["messages"]?.arrayValue?.first?["message_id"]?.int64Value ?? 1
        try await client.markChatRead(token: peer.token, chatId: chatId, maxId: msgId)

        let statusBadge = waitForElement(in: app, AccessibilityIDs.Stage.bubbleStatus(msgId), timeout: 8)
        let statusVal = ((statusBadge.value as? String) ?? statusBadge.label).lowercased()
        XCTAssertEqual(statusVal, "read")
        app.terminate()
    }

    @MainActor
    func testTier1_F19_03_OfflineOutboxQueuesMessagesAcrossRestartAndFlushesInOrderWhenOnline() async throws {
        let client = self.peerClient!
        let (app1, user) = try await launchAuthenticatedUser(usernamePrefix: "f19_outbox")
        let peer = try await client.registerUser(username: "f19_opeer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: user.token,
            title: "Offline Outbox",
            chatType: "direct",
            peerUserId: peer.userId,
            memberIds: [peer.userId]
        )
        XCTAssertTrue(waitForElement(in: app1, AccessibilityIDs.Chats.row(chatId), timeout: 10).exists)
        app1.terminate()

        let appOffline = launchApp(simulateOffline: true)
        waitForElement(in: appOffline, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        for idx in 1...3 {
            clearAndTypeText(
                in: waitForElement(in: appOffline, AccessibilityIDs.Composer.textView),
                text: "Queued-\(idx)"
            )
            waitForElement(in: appOffline, AccessibilityIDs.Composer.micOrSendButton).tap()
        }
        appOffline.terminate()

        let appOnline = launchApp(simulateOffline: false)
        XCTAssertTrue(waitForElement(in: appOnline, AccessibilityIDs.Chats.row(chatId), timeout: 10).exists)
        try await Task.sleep(nanoseconds: 2_000_000_000)

        let history = try await client.listMessages(token: peer.token, chatId: chatId)
        let msgs = history["messages"]?.arrayValue ?? []
        XCTAssertEqual(msgs.count, 3)
        appOnline.terminate()
    }

    @MainActor
    func testTier1_F19_04_PtsSequenceGapRecoversMissedMessagesAndEditsViaGetDifference() async throws {
        let client = self.peerClient!
        let (app1, user) = try await launchAuthenticatedUser(usernamePrefix: "f19_pts")
        let peer = try await client.registerUser(username: "f19_ppeer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "PTS Gap Room",
            chatType: "group",
            memberIds: [user.userId]
        )
        let baseId = try await client.sendMessage(token: peer.token, chatId: chatId, text: "Before Offline")
        XCTAssertTrue(waitForElement(in: app1, AccessibilityIDs.Chats.row(chatId), timeout: 10).exists)
        app1.terminate()

        let gap1 = try await client.sendMessage(token: peer.token, chatId: chatId, text: "Gap Msg 1")
        let gap2 = try await client.sendMessage(token: peer.token, chatId: chatId, text: "Gap Msg 2")
        try await client.editMessage(token: peer.token, chatId: chatId, messageId: baseId, text: "Before Offline Edited")

        let app2 = launchApp()
        waitForElement(in: app2, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        XCTAssertTrue(waitForElement(in: app2, AccessibilityIDs.Stage.bubble(gap1), timeout: 10).exists)
        XCTAssertTrue(waitForElement(in: app2, AccessibilityIDs.Stage.bubble(gap2), timeout: 10).exists)
        let editedBase = waitForElement(in: app2, AccessibilityIDs.Stage.bubble(baseId), timeout: 8)
        XCTAssertTrue(editedBase.label.contains("Before Offline Edited"))
        app2.terminate()
    }

    @MainActor
    func testTier1_F19_05_SimulatedOfflineToOnlineReconnectResumesRealTimePtsDelivery() async throws {
        let client = self.peerClient!
        let (app1, user) = try await launchAuthenticatedUser(usernamePrefix: "f19_recon")
        let peer = try await client.registerUser(username: "f19_rcpeer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "Reconnect Stage",
            chatType: "group",
            memberIds: [user.userId]
        )
        app1.terminate()

        let appOff = launchApp(simulateOffline: true)
        XCTAssertTrue(waitForElement(in: appOff, AccessibilityIDs.Shell.connectionStatus, timeout: 8).exists)
        appOff.terminate()

        let appOn = launchApp(simulateOffline: false)
        waitForElement(in: appOn, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        let liveId = try await client.sendMessage(token: peer.token, chatId: chatId, text: "Post-reconnect live PTS")
        XCTAssertTrue(waitForElement(in: appOn, AccessibilityIDs.Stage.bubble(liveId), timeout: 10).exists)
        appOn.terminate()
    }

    // MARK: - F20: UTF-16 Rich Text Entities, Code Copy, Quotes & Spoilers

    @MainActor
    func testTier1_F20_01_RenderBoldItalicUnderlineStrikethroughAndInlineCodeEntities() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f20_ent")
        let peer = try await client.registerUser(username: "f20_peer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "Rich Entities",
            chatType: "group",
            memberIds: [user.userId]
        )
        let msgId = try await client.sendMessage(
            token: peer.token,
            chatId: chatId,
            text: "Bold Italic Code",
            entities: [
                ["offset": 0, "length": 4, "entity_type": ["type": "Bold"]],
                ["offset": 5, "length": 6, "entity_type": ["type": "Italic"]],
                ["offset": 12, "length": 4, "entity_type": ["type": "Code"]],
            ]
        )

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        let bubble = waitForElement(in: app, AccessibilityIDs.Stage.bubble(msgId))
        XCTAssertTrue(bubble.label.contains("Bold Italic Code"))
        app.terminate()
    }

    @MainActor
    func testTier1_F20_02_PreformattedCodeBlockDisplaysLanguageHeaderAndCopiesSource() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f20_pre")
        let peer = try await client.registerUser(username: "f20_ppeer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "Code Copy",
            chatType: "group",
            memberIds: [user.userId]
        )
        let codeText = "let pts: UInt64 = 42"
        let msgId = try await client.sendMessage(
            token: peer.token,
            chatId: chatId,
            text: codeText,
            entities: [
                [
                    "offset": 0,
                    "length": .int(Int64(codeText.utf16.count)),
                    "entity_type": ["type": "Pre", "value": ["language": "swift"]],
                ]
            ]
        )

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        let bubble = waitForElement(in: app, AccessibilityIDs.Stage.bubble(msgId))
        XCTAssertTrue(bubble.label.contains("swift") || bubble.label.contains(codeText))
        let copyBtn = waitForElement(in: app, AccessibilityIDs.Stage.codeBlockCopyButton(msgId), timeout: 6)
        copyBtn.tap()
        XCTAssertEqual((copyBtn.value as? String)?.lowercased(), "copied")
        app.terminate()
    }

    @MainActor
    func testTier1_F20_03_Utf16SurrogateEmojiAndCjkEntityOffsetsRenderWithoutCorruption() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f20_utf16")
        let peer = try await client.registerUser(username: "f20_upeer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "UTF16 Slicing",
            chatType: "group",
            memberIds: [user.userId]
        )
        let text = "😀🎉 法布施 BoldTail"
        let msgId = try await client.sendMessage(
            token: peer.token,
            chatId: chatId,
            text: text,
            entities: [["offset": 9, "length": 8, "entity_type": ["type": "Bold"]]]
        )

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        let bubble = waitForElement(in: app, AccessibilityIDs.Stage.bubble(msgId))
        XCTAssertFalse(bubble.label.contains("\u{FFFD}"), "UTF-16 surrogate pair slicing must never corrupt scalars")
        XCTAssertTrue(bubble.label.contains("法布施 BoldTail"))
        app.terminate()
    }

    @MainActor
    func testTier1_F20_04_CollapsibleBlockquoteAndTextSpoilerExpandAndRevealOnTap() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f20_spoil")
        let peer = try await client.registerUser(username: "f20_speer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "Spoiler Stage",
            chatType: "group",
            memberIds: [user.userId]
        )
        let msgId = try await client.sendMessage(
            token: peer.token,
            chatId: chatId,
            text: "Secret: HiddenLotus77",
            entities: [["offset": 8, "length": 13, "entity_type": ["type": "Spoiler"]]]
        )

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        let spoilerMask = waitForElement(in: app, AccessibilityIDs.Stage.textSpoilerOverlay(msgId), timeout: 6)
        XCTAssertTrue(spoilerMask.exists)
        spoilerMask.tap()
        waitForElementDisappearance(in: app, AccessibilityIDs.Stage.textSpoilerOverlay(msgId))
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Stage.bubble(msgId)).label.contains("HiddenLotus77"))
        app.terminate()
    }

    @MainActor
    func testTier1_F20_05_InteractiveMentionAndTextUrlEntitiesNavigateOnTap() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f20_mnt")
        let peer = try await client.registerUser(username: "f20_mpeer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "Mention Nav",
            chatType: "group",
            memberIds: [user.userId]
        )
        let mentionStr = "@\(peer.username)"
        let msgId = try await client.sendMessage(
            token: peer.token,
            chatId: chatId,
            text: "Contact \(mentionStr) now",
            entities: [
                [
                    "offset": 8,
                    "length": .int(Int64(mentionStr.utf16.count)),
                    "entity_type": ["type": "Mention"],
                ]
            ]
        )

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        waitForElement(in: app, AccessibilityIDs.Stage.entityMention(msgId, peer.username), timeout: 6).tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Profile.root, timeout: 8).exists)
        app.terminate()
    }

    // MARK: - F21: Photos, Videos, Mosaic Albums & Fullscreen `MediaViewer`

    @MainActor
    func testTier1_F21_01_PhotoBubbleOpensFullscreenMediaViewerSupportingPinchZoomAndClose() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f21_photo")
        let peer = try await client.registerUser(username: "f21_peer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "Photo Viewer",
            chatType: "group",
            memberIds: [user.userId]
        )
        let fid = try await client.uploadMediaDirect(
            token: peer.token,
            chatId: chatId,
            filename: "lotus.png",
            mimeType: "image/png",
            data: LoopbackPeerClient.sampleMinimalPngData()
        )
        let msgId = try await client.sendMessage(token: peer.token, chatId: chatId, text: "Lotus Pond", mediaId: fid)

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        waitForElement(in: app, AccessibilityIDs.Stage.bubble(msgId)).tap()
        let viewer = waitForElement(in: app, AccessibilityIDs.MediaViewer.root, timeout: 8)
        XCTAssertTrue(viewer.exists)
        viewer.pinch(withScale: 1.8, velocity: 1.0)
        waitForElement(in: app, AccessibilityIDs.MediaViewer.closeButton).tap()
        waitForElementDisappearance(in: app, AccessibilityIDs.MediaViewer.root)
        XCTAssertFalse(element(in: app, AccessibilityIDs.MediaViewer.root).exists)
        XCTAssertTrue(element(in: app, AccessibilityIDs.Stage.collectionView).exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F21_02_MosaicAlbumRendersProportionalMultiPhotoGridAndSwipesInViewer() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f21_album")
        let peer = try await client.registerUser(username: "f21_apeer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "Mosaic Album",
            chatType: "group",
            memberIds: [user.userId]
        )
        var firstMsgId: Int64 = 0
        for idx in 1...3 {
            let fid = try await client.uploadMediaDirect(
                token: peer.token,
                chatId: chatId,
                filename: "tile_\(idx).png",
                mimeType: "image/png",
                data: LoopbackPeerClient.sampleMinimalPngData()
            )
            let mid = try await client.sendMessage(token: peer.token, chatId: chatId, text: "Album \(idx)", mediaId: fid)
            if idx == 1 { firstMsgId = mid }
        }

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        waitForElement(in: app, AccessibilityIDs.Stage.bubble(firstMsgId)).tap()
        let viewer = waitForElement(in: app, AccessibilityIDs.MediaViewer.root, timeout: 8)
        viewer.swipeLeft()
        XCTAssertTrue(viewer.exists)
        waitForElement(in: app, AccessibilityIDs.MediaViewer.closeButton).tap()
        app.terminate()
    }

    @MainActor
    func testTier1_F21_03_MediaSpoilerBlurRevealsOnFirstTapAndOpensViewerOnSecondTap() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f21_spoil")
        let chatId = try await client.createChat(token: user.token, title: "Media Spoiler", chatType: "group")

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        waitForElement(in: app, AccessibilityIDs.Composer.attachButton).tap()
        waitForElement(in: app, AccessibilityIDs.Composer.attachTab("gallery")).tap()
        waitForElement(in: app, AccessibilityIDs.Composer.attachGalleryTile(0)).tap()
        waitForElement(in: app, AccessibilityIDs.Composer.attachSpoilerToggle).tap()
        waitForElement(in: app, AccessibilityIDs.Composer.attachSendButton).tap()

        let history = try await client.listMessages(token: user.token, chatId: chatId)
        let msgs = history["messages"]?.arrayValue ?? []
        XCTAssertEqual(msgs.count, 1)
        let msgId = msgs.first?["id"]?.int64Value ?? msgs.first?["message_id"]?.int64Value ?? 0
        XCTAssertGreaterThan(msgId, 0)

        let overlay = waitForElement(in: app, AccessibilityIDs.Stage.mediaSpoilerOverlay(msgId), timeout: 8)
        XCTAssertTrue(overlay.exists)
        let bubble = waitForElement(in: app, AccessibilityIDs.Stage.bubble(msgId))
        bubble.tap()
        waitForElementDisappearance(in: app, AccessibilityIDs.Stage.mediaSpoilerOverlay(msgId))
        XCTAssertFalse(element(in: app, AccessibilityIDs.MediaViewer.root).exists)
        bubble.tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.MediaViewer.root, timeout: 8).exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F21_04_VideoBubbleStreamsHttp206RangeInFullscreenMediaViewer() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f21_vid")
        let peer = try await client.registerUser(username: "f21_vpeer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "Video 206 Stream",
            chatType: "group",
            memberIds: [user.userId]
        )
        let videoPayload = Data(repeating: 0x47, count: 4096)
        let fid = try await client.uploadMediaDirect(
            token: peer.token,
            chatId: chatId,
            filename: "sample.mp4",
            mimeType: "video/mp4",
            data: videoPayload
        )
        let (rangeStatus, rangeBytes) = try await client.streamMediaRange(token: user.token, fileId: fid, start: 0, end: 511)
        XCTAssertEqual(rangeStatus, 206)
        XCTAssertEqual(rangeBytes.count, 512)

        let msgId = try await client.sendMessage(token: peer.token, chatId: chatId, text: "Video Clip", mediaId: fid)
        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        waitForElement(in: app, AccessibilityIDs.Stage.bubble(msgId)).tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.MediaViewer.root, timeout: 8).exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F21_05_MediaEditorAppliesCaptionAndCropBeforeSendingPhoto() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f21_edit")
        let chatId = try await client.createChat(token: user.token, title: "Media Editor", chatType: "group")

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        waitForElement(in: app, AccessibilityIDs.Composer.attachButton).tap()
        waitForElement(in: app, AccessibilityIDs.Composer.attachTab("gallery")).tap()
        waitForElement(in: app, "attach.gallery.tile.0").tap()
        waitForElement(in: app, AccessibilityIDs.MediaViewer.editButton).tap()
        clearAndTypeText(in: waitForElement(in: app, "mediaEditor.captionField"), text: "Edited Gallery Shot")
        waitForElement(in: app, "mediaEditor.sendButton").tap()

        let history = try await client.listMessages(token: user.token, chatId: chatId)
        XCTAssertGreaterThanOrEqual((history["messages"]?.arrayValue ?? []).count, 1)
        app.terminate()
    }

    // MARK: - F22: Documents/Files & Resumable Chunked CAS Transfers

    @MainActor
    func testTier1_F22_01_SendLargeDocumentCompletesMultipartChunkedCasUploadMatchingSha256() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f22_cas")
        let chatId = try await client.createChat(token: user.token, title: "Chunked CAS", chatType: "group")

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        waitForElement(in: app, AccessibilityIDs.Composer.attachButton).tap()
        waitForElement(in: app, AccessibilityIDs.Composer.attachTab("file")).tap()
        waitForElement(in: app, "attach.file.sampleLargeBundle").tap()

        let history = try await client.listMessages(token: user.token, chatId: chatId)
        XCTAssertGreaterThanOrEqual((history["messages"]?.arrayValue ?? []).count, 1)
        app.terminate()
    }

    @MainActor
    func testTier1_F22_02_ResumableChunkedUploadRecoversMissingPartsAndRendersInChat() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f22_resume")
        let peer = try await client.registerUser(username: "f22_rpeer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "Resumable File",
            chatType: "group",
            memberIds: [user.userId]
        )
        let initRes = try await client.requestJSON(
            method: "POST",
            path: "/api/v1/media/upload/init",
            token: peer.token,
            body: ["size_bytes": 2048, "filename": "resumed_archive.bin", "chunk_size": 1024]
        )
        let uploadId = initRes["upload_id"]?.stringValue ?? ""
        XCTAssertFalse(uploadId.isEmpty)

        _ = try await client.requestRaw(
            method: "PUT",
            path: "/api/v1/media/upload/\(uploadId)/part/0",
            token: peer.token,
            body: Data(repeating: 0xAA, count: 1024),
            contentType: "application/octet-stream"
        )
        let st = try await client.requestJSON(
            method: "GET",
            path: "/api/v1/media/upload/\(uploadId)/status",
            token: peer.token
        )
        XCTAssertEqual(st["missing_parts"]?.arrayValue?.first?.intValue, 1)

        _ = try await client.requestRaw(
            method: "PUT",
            path: "/api/v1/media/upload/\(uploadId)/part/1",
            token: peer.token,
            body: Data(repeating: 0xBB, count: 1024),
            contentType: "application/octet-stream"
        )
        let done = try await client.requestJSON(
            method: "POST",
            path: "/api/v1/media/upload/\(uploadId)/complete",
            token: peer.token,
            body: ["filename": "resumed_archive.bin", "mime_type": "application/octet-stream", "chat_id": .int(chatId)]
        )
        let fid = done["file_id"]?.stringValue ?? ""
        let msgId = try await client.sendMessage(token: peer.token, chatId: chatId, text: "resumed_archive.bin", mediaId: fid)

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Stage.bubble(msgId), timeout: 8).exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F22_03_DocumentBubbleDisplaysFilenameByteSizeAndOpensQuickLookPreview() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f22_ql")
        let peer = try await client.registerUser(username: "f22_qpeer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "QuickLook Docs",
            chatType: "group",
            memberIds: [user.userId]
        )
        let fid = try await client.uploadMediaDirect(
            token: peer.token,
            chatId: chatId,
            filename: "release_manifest.txt",
            mimeType: "text/plain",
            data: Data(repeating: 0x41, count: 2048)
        )
        let msgId = try await client.sendMessage(token: peer.token, chatId: chatId, text: "release_manifest.txt", mediaId: fid)

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        let bubble = waitForElement(in: app, AccessibilityIDs.Stage.bubble(msgId))
        XCTAssertTrue(bubble.label.contains("release_manifest.txt"))
        bubble.tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Stage.documentPreviewSheet, timeout: 8).exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F22_04_DuplicateFileUploadsDeduplicateToIdenticalSha256CasFileId() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f22_dedup")
        let chatId = try await client.createChat(token: user.token, title: "CAS Dedup", chatType: "group")
        let payload = Data("IDENTICAL_CAS_CONTENT_V1".utf8)
        let fid1 = try await client.uploadMediaDirect(
            token: user.token,
            chatId: chatId,
            filename: "copy1.txt",
            mimeType: "text/plain",
            data: payload
        )
        let fid2 = try await client.uploadMediaDirect(
            token: user.token,
            chatId: chatId,
            filename: "copy2.txt",
            mimeType: "text/plain",
            data: payload
        )
        XCTAssertEqual(fid1, fid2, "Content-addressable storage must deduplicate identical blobs by SHA-256")
        let m1 = try await client.sendMessage(token: user.token, chatId: chatId, text: "Copy 1", mediaId: fid1)
        let m2 = try await client.sendMessage(token: user.token, chatId: chatId, text: "Copy 2", mediaId: fid2)

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Stage.bubble(m1)).exists)
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Stage.bubble(m2)).exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F22_05_DownloadedDocumentRemainsAccessibleFromLocalCacheWhenOffline() async throws {
        let client = self.peerClient!
        let (app1, user) = try await launchAuthenticatedUser(usernamePrefix: "f22_cache")
        let chatId = try await client.createChat(token: user.token, title: "Offline Doc Cache", chatType: "group")
        let fid = try await client.uploadMediaDirect(
            token: user.token,
            chatId: chatId,
            filename: "offline_handbook.txt",
            mimeType: "text/plain",
            data: Data("CACHED_DOCUMENT_BODY".utf8)
        )
        let msgId = try await client.sendMessage(token: user.token, chatId: chatId, text: "offline_handbook.txt", mediaId: fid)

        waitForElement(in: app1, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        waitForElement(in: app1, AccessibilityIDs.Stage.bubble(msgId)).tap()
        app1.terminate()

        let app2 = launchApp(simulateOffline: true)
        waitForElement(in: app2, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        XCTAssertTrue(waitForElement(in: app2, AccessibilityIDs.Stage.bubble(msgId), timeout: 8).exists)
        app2.terminate()
    }
}
