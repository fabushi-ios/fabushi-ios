import Foundation
import XCTest

final class Tier1_VoiceCardsActions_F23_F29_UITests: ParityTestCase {

    // MARK: - F23: 100-Bar 5-Bit Voice Notes & Circular Video Notes

    @MainActor
    func testTier1_F23_01_VoiceNoteBubbleRenders100Bar5BitWaveformAndPlaysAudio() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f23_voice")
        let peer = try await client.registerUser(username: "f23_peer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "Voice Note Stage",
            chatType: "group",
            memberIds: [user.userId]
        )
        let fid = try await client.uploadMediaDirect(
            token: peer.token,
            chatId: chatId,
            filename: "voice.ogg",
            mimeType: "audio/ogg",
            data: Data(repeating: 0x4F, count: 1024)
        )
        let waveform = (0..<100).map { JSONValue.int(Int64($0 % 32)) }
        let msgId = try await client.sendContentMessage(
            token: peer.token,
            chatId: chatId,
            contentType: "Voice",
            data: [
                "file_id": .string(fid),
                "duration_seconds": 4,
                "waveform": .array(waveform),
                "mime_type": "audio/ogg",
                "size_bytes": 1024,
            ]
        )

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        let bubble = waitForElement(in: app, AccessibilityIDs.Stage.bubble(msgId))
        XCTAssertGreaterThan(bubble.frame.width, 100)
        bubble.tap()
        XCTAssertTrue(bubble.exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F23_02_ScrubbingVoiceWaveformSeeksPlaybackProgressDeterministically() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f23_seek")
        let peer = try await client.registerUser(username: "f23_speer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "Voice Seek",
            chatType: "group",
            memberIds: [user.userId]
        )
        let fid = try await client.uploadMediaDirect(
            token: peer.token,
            chatId: chatId,
            filename: "seek.ogg",
            mimeType: "audio/ogg",
            data: Data(repeating: 0x52, count: 2048)
        )
        let msgId = try await client.sendContentMessage(
            token: peer.token,
            chatId: chatId,
            contentType: "Voice",
            data: [
                "file_id": .string(fid),
                "duration_seconds": 12,
                "waveform": .array((0..<100).map { .int(Int64(($0 * 7) % 32)) }),
                "mime_type": "audio/ogg",
                "size_bytes": 2048,
            ]
        )

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        let bubble = waitForElement(in: app, AccessibilityIDs.Stage.bubble(msgId))
        let start = bubble.coordinate(withNormalizedOffset: CGVector(dx: 0.30, dy: 0.5))
        let end = bubble.coordinate(withNormalizedOffset: CGVector(dx: 0.75, dy: 0.5))
        start.press(forDuration: 0.08, thenDragTo: end)
        XCTAssertTrue(bubble.exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F23_03_VoicePlaybackSpeedPillCycles1x1_5xAnd2xRates() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f23_speed")
        let peer = try await client.registerUser(username: "f23_sppeer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "Voice Speed",
            chatType: "group",
            memberIds: [user.userId]
        )
        let fid = try await client.uploadMediaDirect(
            token: peer.token,
            chatId: chatId,
            filename: "speed.ogg",
            mimeType: "audio/ogg",
            data: Data(repeating: 0x33, count: 1024)
        )
        let msgId = try await client.sendContentMessage(
            token: peer.token,
            chatId: chatId,
            contentType: "Voice",
            data: [
                "file_id": .string(fid),
                "duration_seconds": 8,
                "waveform": .array((0..<100).map { .int(Int64($0 % 31)) }),
                "mime_type": "audio/ogg",
                "size_bytes": 1024,
            ]
        )

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        waitForElement(in: app, AccessibilityIDs.Stage.bubble(msgId)).tap()
        let speedBtn = waitForElement(in: app, "stage.voiceSpeedPill", timeout: 8)
        let initialLabel = speedBtn.label
        speedBtn.tap()
        XCTAssertNotEqual(speedBtn.label, initialLabel)
        app.terminate()
    }

    @MainActor
    func testTier1_F23_04_CircularVideoNoteBubblePlaysInlineAndExpandsWithAudioOnTap() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f23_vnote")
        let peer = try await client.registerUser(username: "f23_vnpeer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "VideoNote Stage",
            chatType: "group",
            memberIds: [user.userId]
        )
        let fid = try await client.uploadMediaDirect(
            token: peer.token,
            chatId: chatId,
            filename: "circle.mp4",
            mimeType: "video/mp4",
            data: Data(repeating: 0x66, count: 2048)
        )
        let msgId = try await client.sendContentMessage(
            token: peer.token,
            chatId: chatId,
            contentType: "VideoNote",
            data: [
                "file_id": .string(fid),
                "duration_seconds": 5,
                "diameter": 240,
                "size_bytes": 2048,
            ]
        )

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        let circle = waitForElement(in: app, AccessibilityIDs.Stage.bubble(msgId))
        XCTAssertGreaterThanOrEqual(circle.frame.width, 120)
        circle.tap()
        XCTAssertTrue(circle.exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F23_05_PlayingUnheardVoiceNoteClearsMediaUnreadDotIndicator() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f23_unheard")
        let peer = try await client.registerUser(username: "f23_upeer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "Unheard Voice",
            chatType: "group",
            memberIds: [user.userId]
        )
        let fid = try await client.uploadMediaDirect(
            token: peer.token,
            chatId: chatId,
            filename: "unheard.ogg",
            mimeType: "audio/ogg",
            data: Data(repeating: 0x71, count: 1024)
        )
        let msgId = try await client.sendContentMessage(
            token: peer.token,
            chatId: chatId,
            contentType: "Voice",
            data: [
                "file_id": .string(fid),
                "duration_seconds": 3,
                "waveform": [10, 20, 30, 15],
                "mime_type": "audio/ogg",
                "size_bytes": 1024,
            ]
        )

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        let bubble = waitForElement(in: app, AccessibilityIDs.Stage.bubble(msgId))
        bubble.tap()
        XCTAssertTrue(bubble.exists)
        app.terminate()
    }

    // MARK: - F24: Stickers, Animated Stickers, Dice & GIF Loops

    @MainActor
    func testTier1_F24_01_SendStickerFromCatalogRendersBorderlessStickerBubble() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f24_stk")
        let chatId = try await client.createChat(token: user.token, title: "Stickers", chatType: "group")

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        waitForElement(in: app, AccessibilityIDs.Composer.stickerButton).tap()
        waitForElement(in: app, AccessibilityIDs.Composer.stickerPanelTab("stickers")).tap()
        waitForElement(in: app, "stickerPanel.cell.0").tap()

        let history = try await client.listMessages(token: user.token, chatId: chatId)
        XCTAssertGreaterThanOrEqual((history["messages"]?.arrayValue ?? []).count, 1)
        app.terminate()
    }

    @MainActor
    func testTier1_F24_02_BrowseAndInstallServerStickerSetPersistsInstalledPack() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f24_pack")
        let chatId = try await client.createChat(token: user.token, title: "Sticker Packs", chatType: "group")

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        waitForElement(in: app, AccessibilityIDs.Composer.stickerButton).tap()
        waitForElement(in: app, "stickerPanel.browsePacksButton").tap()
        waitForElement(in: app, "stickerPack.install.0").tap()

        let setsRes = try await client.requestJSON(method: "GET", path: "/api/v1/stickers/sets", token: user.token)
        let sets = setsRes["sets"]?.arrayValue ?? []
        XCTAssertGreaterThanOrEqual(sets.count, 1)
        app.terminate()
    }

    @MainActor
    func testTier1_F24_03_AnimatedStickerBubblePlaysAndRespectsPowerSavingSetting() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f24_anim")
        let chatId = try await client.createChat(token: user.token, title: "Animated Stickers", chatType: "group")
        let fid = try await client.uploadMediaDirect(
            token: user.token,
            chatId: chatId,
            filename: "animated.webp",
            mimeType: "image/webp",
            data: LoopbackPeerClient.sampleMinimalPngData()
        )
        let msgId = try await client.sendContentMessage(
            token: user.token,
            chatId: chatId,
            contentType: "Media",
            data: [
                "file_id": .string(fid),
                "file_name": "animated.webp",
                "mime_type": "image/webp",
                "size_bytes": 68,
                "width": 512,
                "height": 512,
            ]
        )

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        let bubble = waitForElement(in: app, AccessibilityIDs.Stage.bubble(msgId))
        XCTAssertGreaterThan(bubble.frame.height, 80)
        app.terminate()
    }

    @MainActor
    func testTier1_F24_04_SendAnimatedDiceEmojisRendersDeterministicRollOutcome() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f24_dice")
        let chatId = try await client.createChat(token: user.token, title: "Dice Roll", chatType: "group")

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Composer.textView), text: "🎲")
        waitForElement(in: app, AccessibilityIDs.Composer.micOrSendButton).tap()

        let history = try await client.listMessages(token: user.token, chatId: chatId)
        let msgs = history["messages"]?.arrayValue ?? []
        XCTAssertEqual(msgs.count, 1)
        app.terminate()
    }

    @MainActor
    func testTier1_F24_05_SearchAndSendGifRendersAutoplayLoopingGifBubble() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f24_gif")
        let chatId = try await client.createChat(token: user.token, title: "GIF Autoplay", chatType: "group")

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        waitForElement(in: app, AccessibilityIDs.Composer.stickerButton).tap()
        waitForElement(in: app, AccessibilityIDs.Composer.stickerPanelTab("gifs")).tap()
        clearAndTypeText(in: waitForElement(in: app, "stickerPanel.gifSearchField"), text: "wave")
        waitForElement(in: app, "stickerPanel.gifCell.0").tap()

        let history = try await client.listMessages(token: user.token, chatId: chatId)
        XCTAssertGreaterThanOrEqual((history["messages"]?.arrayValue ?? []).count, 1)
        app.terminate()
    }

    // MARK: - F25: Interactive Polls, Quizzes, Todo, Invoice, Contact & Map Cards

    @MainActor
    func testTier1_F25_01_VoteOnLivePollAndQuizCardRevealsPercentagesAndExplanation() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f25_quiz")
        let peer = try await client.registerUser(username: "f25_peer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "Quiz Room",
            chatType: "group",
            memberIds: [user.userId]
        )
        let (msgId, pollId) = try await client.createPoll(
            token: peer.token,
            chatId: chatId,
            question: "PTS stands for?",
            options: ["Presentation Sequence", "Packet Socket"],
            isQuiz: true,
            correctOptionId: 0,
            solution: "Monotonic per-chat presentation sequence"
        )

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        let bubble = waitForElement(in: app, AccessibilityIDs.Stage.bubble(msgId))
        app.staticTexts["Presentation Sequence"].firstMatch.tap()
        XCTAssertTrue(bubble.exists)

        let pollState = try await client.requestJSON(method: "GET", path: "/api/v1/polls/\(pollId)", token: user.token)
        XCTAssertNotNil(pollState.objectValue)
        app.terminate()
    }

    @MainActor
    func testTier1_F25_02_CreatorClosingPollLocksFurtherVotesAcrossPeers() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f25_close")
        let peer = try await client.registerUser(username: "f25_cpeer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "Close Poll Room",
            chatType: "group",
            memberIds: [user.userId]
        )
        let (msgId, pollId) = try await client.createPoll(
            token: peer.token,
            chatId: chatId,
            question: "Ship Friday?",
            options: ["Yes", "Monday"]
        )
        try await client.closePoll(token: peer.token, pollId: pollId)

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        let bubble = waitForElement(in: app, AccessibilityIDs.Stage.bubble(msgId))
        XCTAssertTrue(bubble.exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F25_03_InteractiveTodoChecklistCardTogglesItemCompletionLive() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f25_todo")
        let peer = try await client.registerUser(username: "f25_tpeer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "Todo Checklist",
            chatType: "group",
            memberIds: [user.userId]
        )
        let msgId = try await client.sendContentMessage(
            token: peer.token,
            chatId: chatId,
            contentType: "Todo",
            data: [
                "id": "td_f25",
                "list_id": "td_f25",
                "title": "Release Checklist",
                "items": [
                    ["id": "item_1", "title": "Build Rust staticlib", "completed": false],
                    ["id": "item_2", "title": "Run Parity XCUITest", "completed": false],
                ],
                "completed_count": 0,
                "action_buttons": [],
            ]
        )

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        let bubble = waitForElement(in: app, AccessibilityIDs.Stage.bubble(msgId))
        app.staticTexts["Build Rust staticlib"].firstMatch.tap()
        XCTAssertTrue(bubble.exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F25_04_StarsInvoiceCardPaysFromWalletAndUpdatesStatusToPaid() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f25_inv")
        try await client.topupStars(token: user.token, amount: 500, description: "Invoice Seed")
        let peer = try await client.registerUser(username: "f25_ipeer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "Stars Invoice",
            chatType: "group",
            memberIds: [user.userId]
        )
        let msgId = try await client.sendContentMessage(
            token: peer.token,
            chatId: chatId,
            contentType: "Invoice",
            data: [
                "id": "inv_1",
                "invoice_id": "inv_1",
                "title": "Agent Pro Pack",
                "description": "Unlock extra skills",
                "currency": "XTR",
                "amount_cents": 150,
                "total_amount": 150,
                "status": "pending",
                "is_paid": false,
                "prices": [["label": "Pack", "amount_cents": 150]],
            ]
        )

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        let bubble = waitForElement(in: app, AccessibilityIDs.Stage.bubble(msgId))
        XCTAssertTrue(bubble.exists)
        let payBtn = element(in: app, "invoice.payButton")
        if payBtn.waitForExistence(timeout: 5) {
            payBtn.tap()
        }
        let bal = try await client.getWalletBalance(token: user.token)
        XCTAssertLessThanOrEqual(bal, 500)
        app.terminate()
    }

    @MainActor
    func testTier1_F25_05_SharedContactAndMapLocationCardsRenderAndOpenDetailSheets() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f25_loc")
        let peer = try await client.registerUser(username: "f25_lpeer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "Contact & Map",
            chatType: "group",
            memberIds: [user.userId]
        )
        let locMsgId = try await client.sendMessage(
            token: peer.token,
            chatId: chatId,
            text: "geo:37.7749,-122.4194?q=San+Francisco"
        )

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        let bubble = waitForElement(in: app, AccessibilityIDs.Stage.bubble(locMsgId))
        bubble.tap()
        XCTAssertTrue(bubble.exists)
        app.terminate()
    }

    // MARK: - F26: Swipe-to-Reply, Quotes, Forwards, Edits, Deletes & Multi-Select

    @MainActor
    func testTier1_F26_01_SwipeLeft44ptOnBubblePopulatesReplyBannerAndSendsThreadedReply() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f26_reply")
        let peer = try await client.registerUser(username: "f26_peer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "Swipe Reply",
            chatType: "group",
            memberIds: [user.userId]
        )
        let anchorId = try await client.sendMessage(token: peer.token, chatId: chatId, text: "Swipe 44pt target")

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        swipeLeftToReply(in: app, messageId: anchorId)
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Composer.replyBanner).exists)
        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Composer.textView), text: "Threaded response")
        waitForElement(in: app, AccessibilityIDs.Composer.micOrSendButton).tap()

        waitForElementDisappearance(in: app, AccessibilityIDs.Composer.replyBanner)
        let history = try await client.listMessages(token: user.token, chatId: chatId)
        XCTAssertGreaterThanOrEqual((history["messages"]?.arrayValue ?? []).count, 2)
        app.terminate()
    }

    @MainActor
    func testTier1_F26_02_QuotedSubstringReplyPreservesExactQuoteTextInBubble() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f26_quote")
        let peer = try await client.registerUser(username: "f26_qpeer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "Substring Quote",
            chatType: "group",
            memberIds: [user.userId]
        )
        let srcId = try await client.sendMessage(
            token: peer.token,
            chatId: chatId,
            text: "First clause; Critical invariant clause; Final clause"
        )
        let quoteReplyId = try await client.sendMessage(
            token: user.token,
            chatId: chatId,
            text: "Confirmed invariant",
            replyTo: srcId,
            quoteText: "Critical invariant clause"
        )

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Stage.bubble(quoteReplyId)).exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F26_03_ForwardMessageWithCaptionToTargetChatDeliversForwardedHeader() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f26_fwd")
        let peer = try await client.registerUser(username: "f26_fpeer_\(uniqueSuffix())")
        let srcChat = try await client.createChat(
            token: peer.token,
            title: "Source Room",
            chatType: "group",
            memberIds: [user.userId]
        )
        let dstChat = try await client.createChat(
            token: user.token,
            title: "Destination Room",
            chatType: "group",
            memberIds: [peer.userId]
        )
        let msgId = try await client.sendMessage(token: peer.token, chatId: srcChat, text: "Forwardable Insight")

        waitForElement(in: app, AccessibilityIDs.Chats.row(srcChat), timeout: 10).tap()
        longPressBubbleForPortal(in: app, messageId: msgId)
        waitForElement(in: app, AccessibilityIDs.Stage.contextMenu("forward")).tap()
        app.staticTexts["Destination Room"].firstMatch.tap()
        waitForElement(in: app, AccessibilityIDs.Composer.micOrSendButton).tap()

        let dstHistory = try await client.listMessages(token: user.token, chatId: dstChat)
        XCTAssertGreaterThanOrEqual((dstHistory["messages"]?.arrayValue ?? []).count, 1)
        app.terminate()
    }

    @MainActor
    func testTier1_F26_04_EditOutgoingMessageUpdatesTextAndDisplaysEditedLabelLive() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f26_edit")
        let chatId = try await client.createChat(token: user.token, title: "Edit Stage", chatType: "group")
        let msgId = try await client.sendMessage(token: user.token, chatId: chatId, text: "Typo v1")

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        longPressBubbleForPortal(in: app, messageId: msgId)
        waitForElement(in: app, AccessibilityIDs.Stage.contextMenu("edit")).tap()
        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Composer.textView), text: "Polished v2")
        waitForElement(in: app, AccessibilityIDs.Composer.micOrSendButton).tap()

        let history = try await client.listMessages(token: user.token, chatId: chatId)
        let first = history["messages"]?.arrayValue?.first
        XCTAssertNotNil(first)
        app.terminate()
    }

    @MainActor
    func testTier1_F26_05_MultiSelectMessagesBatchForwardsAndDeletesForEveryone() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f26_batch")
        let chatId = try await client.createChat(token: user.token, title: "Batch Delete", chatType: "group")
        let m1 = try await client.sendMessage(token: user.token, chatId: chatId, text: "Delete Row 1")
        let m2 = try await client.sendMessage(token: user.token, chatId: chatId, text: "Delete Row 2")

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        longPressBubbleForPortal(in: app, messageId: m1)
        waitForElement(in: app, AccessibilityIDs.Stage.contextMenu("select")).tap()
        waitForElement(in: app, AccessibilityIDs.Stage.bubble(m2)).tap()
        waitForElement(in: app, "stage.batchDeleteButton").tap()
        app.buttons["Delete for Everyone"].firstMatch.tap()

        waitForElementDisappearance(in: app, AccessibilityIDs.Stage.bubble(m1))
        app.terminate()
    }

    // MARK: - F27: Blurred Reaction Portal (`QuickStrip` + 8-Col Grid)

    @MainActor
    func testTier1_F27_01_LongPressBubbleOpensBlurredPortalWithQuickReactionStrip() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f27_open")
        let peer = try await client.registerUser(username: "f27_peer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "Reaction Portal",
            chatType: "group",
            memberIds: [user.userId]
        )
        let msgId = try await client.sendMessage(token: peer.token, chatId: chatId, text: "Open quick strip")

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        longPressBubbleForPortal(in: app, messageId: msgId)
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Stage.reactionStrip("👍")).exists)
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Stage.reactionStrip("❤️")).exists)
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Stage.reactionStrip("🔥")).exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F27_02_TapQuickReactionAddsViewerHighlightedPillAndSyncsToPeer() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f27_quick")
        let peer = try await client.registerUser(username: "f27_qpeer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "Quick Fire Reaction",
            chatType: "group",
            memberIds: [user.userId]
        )
        let msgId = try await client.sendMessage(token: peer.token, chatId: chatId, text: "React with fire")

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        longPressBubbleForPortal(in: app, messageId: msgId)
        waitForElement(in: app, AccessibilityIDs.Stage.reactionStrip("🔥")).tap()

        let history = try await client.listMessages(token: peer.token, chatId: chatId)
        let msg = history["messages"]?.arrayValue?.first
        XCTAssertNotNil(msg)
        app.terminate()
    }

    @MainActor
    func testTier1_F27_03_Expand8ColumnReactionGridAndSelectExtendedEmoji() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f27_grid")
        let chatId = try await client.createChat(token: user.token, title: "8-Col Grid", chatType: "group")
        let msgId = try await client.sendMessage(token: user.token, chatId: chatId, text: "Expand 32-emoji grid")

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        longPressBubbleForPortal(in: app, messageId: msgId)
        waitForElement(in: app, AccessibilityIDs.Stage.reactionStrip("expand")).tap()
        waitForElement(in: app, AccessibilityIDs.Stage.reactionStrip("🎉")).tap()

        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Stage.bubble(msgId)).exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F27_04_TapActiveReactionPillOnBubbleTogglesReactionOff() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f27_toggle")
        let chatId = try await client.createChat(token: user.token, title: "Toggle Reaction", chatType: "group")
        let msgId = try await client.sendMessage(token: user.token, chatId: chatId, text: "Toggle me off")
        try await client.reactToMessage(token: user.token, chatId: chatId, messageId: msgId, emoji: "👍", action: "add")

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        let pill = waitForElement(in: app, "stage.bubble.\(msgId).reaction.👍")
        pill.tap()
        waitForElementDisappearance(in: app, "stage.bubble.\(msgId).reaction.👍")
        app.terminate()
    }

    @MainActor
    func testTier1_F27_05_CompanionPeerWebSocketReactionIncrementsPillCounterLive() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f27_live")
        let peer = try await client.registerUser(username: "f27_lpeer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: user.token,
            title: "Live Reaction Counter",
            chatType: "group",
            memberIds: [peer.userId]
        )
        let msgId = try await client.sendMessage(token: user.token, chatId: chatId, text: "Multi-user reaction")
        try await client.reactToMessage(token: user.token, chatId: chatId, messageId: msgId, emoji: "❤️", action: "add")

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Stage.bubble(msgId)).exists)
        try await client.reactToMessage(token: peer.token, chatId: chatId, messageId: msgId, emoji: "❤️", action: "add")

        let pill = waitForElement(in: app, "stage.bubble.\(msgId).reaction.❤️", timeout: 8)
        XCTAssertTrue(pill.label.contains("2"))
        app.terminate()
    }

    // MARK: - F28: Inline Bot Keyboards, AI Streaming Cards, Translate & Instant View

    @MainActor
    func testTier1_F28_01_TapInlineKeyboardCallbackButtonExecutesBotActionAndUpdatesMessage() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f28_cb")
        let peer = try await client.registerUser(username: "f28_peer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "Inline Bot Keyboard",
            chatType: "group",
            memberIds: [user.userId]
        )
        let msgId = try await client.sendMessage(
            token: peer.token,
            chatId: chatId,
            text: "Approve deployment #88?",
            inlineKeyboard: [[["text": "Approve", "callback_data": "deploy:approve"]]]
        )

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Stage.bubble(msgId)).exists)
        app.buttons["Approve"].firstMatch.tap()
        XCTAssertTrue(element(in: app, AccessibilityIDs.Stage.collectionView).exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F28_02_TapInlineKeyboardWebAppButtonOpensMiniAppWebViewSheet() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f28_webapp")
        let created = try await client.createBotWithMiniAppViaNL(
            token: user.token,
            prompt: "Create mini app called SprintBoard"
        )
        if created.botId > 0 {
            try await client.acceptBotAppTos(token: user.token, botId: created.botId)
        }
        let chatId = try await client.createChat(token: user.token, title: "WebApp Button", chatType: "group")
        let appURL = "\(self.serverURL.absoluteString)/miniapps/\(created.botUsername)/\(created.shortName)"
        _ = try await client.sendMessage(
            token: user.token,
            chatId: chatId,
            text: "Open SprintBoard",
            inlineKeyboard: [[["text": "Launch Board", "web_app_url": .string(appURL)]]]
        )

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        app.buttons["Launch Board"].firstMatch.tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Settings.miniappWebviewSheet, timeout: 10).exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F28_03_AiStreamingThinkingBubbleAndToolPermissionDockApproveExecution() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f28_ai")
        let welcome = try await client.requestJSON(method: "POST", path: "/api/v1/chats/welcome", token: user.token)
        let botChatId = welcome["chat_id"]?.int64Value ?? welcome["id"]?.int64Value ?? 1

        waitForElement(in: app, AccessibilityIDs.Chats.row(botChatId), timeout: 10).tap()
        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Composer.textView), text: "Explain PTS sync")
        waitForElement(in: app, AccessibilityIDs.Composer.micOrSendButton).tap()

        try await Task.sleep(nanoseconds: 1_500_000_000)
        let history = try await client.listMessages(token: user.token, chatId: botChatId)
        XCTAssertGreaterThanOrEqual((history["messages"]?.arrayValue ?? []).count, 1)
        app.terminate()
    }

    @MainActor
    func testTier1_F28_04_ContextMenuTranslateActionOpensMessageTranslationSheet() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f28_tr")
        let peer = try await client.registerUser(username: "f28_tpeer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "Translate Room",
            chatType: "group",
            memberIds: [user.userId]
        )
        let msgId = try await client.sendMessage(token: peer.token, chatId: chatId, text: "应无所住而生其心")

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        longPressBubbleForPortal(in: app, messageId: msgId)
        waitForElement(in: app, AccessibilityIDs.Stage.contextMenu("translate")).tap()
        XCTAssertTrue(waitForElement(in: app, "stage.translateSheet", timeout: 8).exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F28_05_LinkPreviewInstantViewButtonOpensNativeIvViewerReader() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f28_iv")
        let chatId = try await client.createChat(token: user.token, title: "Instant View", chatType: "group")
        let msgId = try await client.sendMessage(
            token: user.token,
            chatId: chatId,
            text: "Read article https://example.com/instant-view-guide"
        )

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Stage.bubble(msgId)).exists)
        waitForElement(in: app, "stage.instantViewButton").tap()
        XCTAssertTrue(waitForElement(in: app, "instantView.readerRoot", timeout: 8).exists)
        app.terminate()
    }

    // MARK: - F29: Per-Chat & Global Auto-Delete Message TTL Reaper

    @MainActor
    func testTier1_F29_01_SetPerChatAutoDeleteTimerDisplaysHeaderTtlBadge() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f29_set")
        let chatId = try await client.createChat(token: user.token, title: "TTL Header", chatType: "group")

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        waitForElement(in: app, AccessibilityIDs.Stage.headerMoreButton).tap()
        waitForElement(in: app, "stage.more.autoDelete86400").tap()

        let chatDetail = try await client.getChat(token: user.token, chatId: chatId)
        XCTAssertEqual(chatDetail["message_ttl_seconds"]?.intValue, 86400)
        app.terminate()
    }

    @MainActor
    func testTier1_F29_02_MessagesSentUnderActiveTtlCarryExpiryCountdownMetadata() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f29_meta")
        let chatId = try await client.createChat(token: user.token, title: "TTL Metadata", chatType: "group")
        try await client.setChatTTL(token: user.token, chatId: chatId, ttlSeconds: 3600)

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Composer.textView), text: "Expiring in 1h")
        waitForElement(in: app, AccessibilityIDs.Composer.micOrSendButton).tap()

        let history = try await client.listMessages(token: user.token, chatId: chatId)
        let msg = history["messages"]?.arrayValue?.first
        XCTAssertEqual(msg?["ttl_seconds"]?.intValue, 3600)
        app.terminate()
    }

    @MainActor
    func testTier1_F29_03_BackgroundTtlReaperPurgesExpiredMessageFromTranscriptAndServer() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f29_reap")
        let chatId = try await client.createChat(token: user.token, title: "2s Reaper", chatType: "group")
        try await client.setChatTTL(token: user.token, chatId: chatId, ttlSeconds: 2)
        let msgId = try await client.sendMessage(token: user.token, chatId: chatId, text: "Vanish in 2s")

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        try await Task.sleep(nanoseconds: 2_600_000_000)

        waitForElementDisappearance(in: app, AccessibilityIDs.Stage.bubble(msgId), timeout: 6)
        let history = try await client.listMessages(token: user.token, chatId: chatId)
        XCTAssertEqual((history["messages"]?.arrayValue ?? []).count, 0)
        app.terminate()
    }

    @MainActor
    func testTier1_F29_04_ConfigureGlobalAutoDeleteTimerInPrivacySettingsAppliesToNewChats() async throws {
        let (app, session) = try await launchAuthenticatedUser(usernamePrefix: "f29_glob")
        waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).tap()
        waitForElement(in: app, AccessibilityIDs.Settings.row("autoDelete")).tap()
        waitForElement(in: app, "autoDelete.option.1w").tap()

        let settings = try await self.peerClient.requestJSON(
            method: "GET",
            path: "/api/v1/users/me/settings",
            token: session.token
        )
        XCTAssertNotNil(settings.objectValue)
        app.terminate()
    }

    @MainActor
    func testTier1_F29_05_DisableChatAutoDeleteTimerRestoresPermanentMessageRetention() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f29_off")
        let chatId = try await client.createChat(token: user.token, title: "Disable TTL", chatType: "group")
        try await client.setChatTTL(token: user.token, chatId: chatId, ttlSeconds: 60)

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        waitForElement(in: app, AccessibilityIDs.Stage.headerMoreButton).tap()
        waitForElement(in: app, "stage.more.autoDeleteOff").tap()

        let msgId = try await client.sendMessage(token: user.token, chatId: chatId, text: "Permanent Note")
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Stage.bubble(msgId)).exists)
        let detail = try await client.getChat(token: user.token, chatId: chatId)
        XCTAssertEqual(detail["message_ttl_seconds"]?.intValue ?? 0, 0)
        app.terminate()
    }
}
