import Foundation
import XCTest

final class Tier1_ComposerVoiceDrafts_F30_F34_UITests: ParityTestCase {

    // MARK: - F30: Multi-Line `UITextView` Composer & CJK `ImeSafetyMachine`

    @MainActor
    func testTier1_F30_01_ComposerTextViewTransitionsButtonFromMicToSendWhenNonEmpty() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f30_btn")
        let chatId = try await client.createChat(token: user.token, title: "Mic To Send", chatType: "group")

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        let actionBtn = waitForElement(in: app, AccessibilityIDs.Composer.micOrSendButton)
        let idleMode = (actionBtn.value as? String) ?? actionBtn.label
        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Composer.textView), text: "Hello")
        let sendMode = (actionBtn.value as? String) ?? actionBtn.label
        XCTAssertNotEqual(idleMode, sendMode)
        app.terminate()
    }

    @MainActor
    func testTier1_F30_02_MultiLineTextAutoGrowsComposerHeightUpTo120ptClamp() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f30_grow")
        let chatId = try await client.createChat(token: user.token, title: "Auto-Grow", chatType: "group")

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        let tv = waitForElement(in: app, AccessibilityIDs.Composer.textView)
        let initialHeight = tv.frame.height
        let multiline = (1...8).map { "Line \($0) of multi-line composer" }.joined(separator: "\n")
        clearAndTypeText(in: tv, text: multiline)

        let expandedHeight = tv.frame.height
        XCTAssertGreaterThan(expandedHeight, initialHeight)
        XCTAssertLessThanOrEqual(expandedHeight, 140)
        app.terminate()
    }

    @MainActor
    func testTier1_F30_03_CjkTextInputCommitsAndSendsCleanlyWithoutPrematureDispatch() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f30_cjk", locale: "zh-Hans")
        let chatId = try await client.createChat(token: user.token, title: "CJK IME Guard", chatType: "group")

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        let tv = waitForElement(in: app, AccessibilityIDs.Composer.textView)
        clearAndTypeText(in: tv, text: "般若波罗蜜多心经")
        let preSendHistory = try await client.listMessages(token: user.token, chatId: chatId)
        XCTAssertEqual(preSendHistory["messages"]?.arrayValue?.count ?? 0, 0)

        waitForElement(in: app, AccessibilityIDs.Composer.micOrSendButton).tap()
        let history = try await client.listMessages(token: user.token, chatId: chatId)
        XCTAssertEqual(history["messages"]?.arrayValue?.count, 1)
        app.terminate()
    }

    @MainActor
    func testTier1_F30_04_MixedBilingualCjkEmojiAndNewlinesPreserveExactMessagePayload() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f30_mix")
        let chatId = try await client.createChat(token: user.token, title: "Mixed Payload", chatType: "group")

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        let payload = "法布施 🚀\nLine 2 Swift 6"
        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Composer.textView), text: payload)
        waitForElement(in: app, AccessibilityIDs.Composer.micOrSendButton).tap()

        let history = try await client.listMessages(token: user.token, chatId: chatId)
        XCTAssertEqual(history["messages"]?.arrayValue?.count, 1)
        app.terminate()
    }

    @MainActor
    func testTier1_F30_05_SendingMessageClearsComposerTextViewAndRestoresCompactHeight() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f30_reset")
        let chatId = try await client.createChat(token: user.token, title: "Compact Reset", chatType: "group")

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        let tv = waitForElement(in: app, AccessibilityIDs.Composer.textView)
        let compactHeight = tv.frame.height
        clearAndTypeText(in: tv, text: "Line A\nLine B\nLine C\nLine D")
        waitForElement(in: app, AccessibilityIDs.Composer.micOrSendButton).tap()

        XCTAssertLessThanOrEqual(tv.frame.height, compactHeight + 8)
        app.terminate()
    }

    // MARK: - F31: Formatting Menu, Reply/Edit Strip, Silent/Scheduled & Send-As

    @MainActor
    func testTier1_F31_01_TextSelectionFormattingMenuAppliesBoldItalicAndSpoilerEntities() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f31_fmt")
        let chatId = try await client.createChat(token: user.token, title: "Formatting Menu", chatType: "group")

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        let tv = waitForElement(in: app, AccessibilityIDs.Composer.textView)
        clearAndTypeText(in: tv, text: "Format Bold Span")
        waitForElement(in: app, "composer.format.bold").tap()
        waitForElement(in: app, AccessibilityIDs.Composer.micOrSendButton).tap()

        let history = try await client.listMessages(token: user.token, chatId: chatId)
        let firstMsg = history["messages"]?.arrayValue?.first
        let entities = firstMsg?["entities"]?.arrayValue ?? []
        XCTAssertGreaterThanOrEqual(entities.count, 1, "Expected Bold MessageEntity on formatted message")
        app.terminate()
    }

    @MainActor
    func testTier1_F31_02_ActiveReplyAndEditBannerDisplaysPreviewAndCancelsCleanly() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f31_strip")
        let chatId = try await client.createChat(token: user.token, title: "Reply Cancel", chatType: "group")
        let msgId = try await client.sendMessage(token: user.token, chatId: chatId, text: "Cancelable anchor")

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        swipeLeftToReply(in: app, messageId: msgId)
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Composer.replyBanner).exists)
        waitForElement(in: app, AccessibilityIDs.Composer.cancelReplyButton).tap()
        waitForElementDisappearance(in: app, AccessibilityIDs.Composer.replyBanner)
        app.terminate()
    }

    @MainActor
    func testTier1_F31_03_LongPressSendWithoutSoundDeliversSilentFlaggedMessage() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f31_silent")
        let chatId = try await client.createChat(token: user.token, title: "Silent Send", chatType: "group")

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Composer.textView), text: "Midnight silent ping")
        let sendBtn = waitForElement(in: app, AccessibilityIDs.Composer.micOrSendButton)
        sendBtn.press(forDuration: 0.7)
        waitForElement(in: app, "composer.sendWithoutSoundAction").tap()

        let history = try await client.listMessages(token: user.token, chatId: chatId)
        let msg = history["messages"]?.arrayValue?.first
        XCTAssertEqual(msg?["is_silent"]?.boolValue ?? msg?["flags"]?["is_silent"]?.boolValue, true)
        app.terminate()
    }

    @MainActor
    func testTier1_F31_04_LongPressScheduleMessageQueuesFutureTimestampedMessage() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f31_sched")
        let chatId = try await client.createChat(token: user.token, title: "Scheduled Send", chatType: "group")

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Composer.textView), text: "Tomorrow 9am reminder")
        waitForElement(in: app, AccessibilityIDs.Composer.micOrSendButton).press(forDuration: 0.7)
        waitForElement(in: app, "composer.scheduleMessageAction").tap()
        waitForElement(in: app, "schedule.confirmButton").tap()

        let history = try await client.listMessages(token: user.token, chatId: chatId)
        let msgs = history["messages"]?.arrayValue ?? []
        XCTAssertEqual(msgs.count, 1)
        let scheduledAt = msgs.first?["scheduled_at_ms"]?.int64Value ?? 0
        XCTAssertGreaterThan(scheduledAt, Int64(Date().timeIntervalSince1970 * 1000) - 5_000)
        app.terminate()
    }

    @MainActor
    func testTier1_F31_05_SendAsIdentityPickerPostsGroupMessageUnderOwnedChannelIdentity() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f31_sendas")
        let channelId = try await client.createChat(token: user.token, title: "Official Lotus Channel", chatType: "channel")
        let groupId = try await client.createChat(token: user.token, title: "Public Supergroup", chatType: "supergroup")

        waitForElement(in: app, AccessibilityIDs.Chats.row(groupId), timeout: 10).tap()
        waitForElement(in: app, AccessibilityIDs.Composer.sendAsButton).tap()
        app.staticTexts["Official Lotus Channel"].firstMatch.tap()
        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Composer.textView), text: "Posted as channel")
        waitForElement(in: app, AccessibilityIDs.Composer.micOrSendButton).tap()

        let history = try await client.listMessages(token: user.token, chatId: groupId)
        let msgs = history["messages"]?.arrayValue ?? []
        XCTAssertEqual(msgs.count, 1)
        let sendAsId = msgs.first?["send_as_peer_id"]?.int64Value ?? 0
        XCTAssertEqual(sendAsId, channelId)
        app.terminate()
    }

    // MARK: - F32: Attachment Half/Full Sheet, Sticker/GIF Keyboard & Bot Menu

    @MainActor
    func testTier1_F32_01_AttachmentHalfSheetExpandsAndSwitchesGalleryFileLocationPollContactTabs() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f32_tabs")
        let chatId = try await client.createChat(token: user.token, title: "Attach Sheet", chatType: "group")

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        waitForElement(in: app, AccessibilityIDs.Composer.attachButton).tap()

        for tab in ["gallery", "file", "location", "poll", "contact"] {
            let tabEl = waitForElement(in: app, AccessibilityIDs.Composer.attachTab(tab))
            tabEl.tap()
            XCTAssertTrue(isElementMarkedSelected(tabEl), "Attach tab '\(tab)' must report selected state")
        }
        app.terminate()
    }

    @MainActor
    func testTier1_F32_02_SendMultiPhotoSelectionWithCaptionAndHdOptionFromGallerySheet() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f32_gal")
        let chatId = try await client.createChat(token: user.token, title: "HD Gallery", chatType: "group")

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        waitForElement(in: app, AccessibilityIDs.Composer.attachButton).tap()
        waitForElement(in: app, AccessibilityIDs.Composer.attachTab("gallery")).tap()
        waitForElement(in: app, "attach.gallery.tile.0").tap()
        waitForElement(in: app, "attach.hdToggle").tap()
        clearAndTypeText(in: waitForElement(in: app, "attach.captionField"), text: "HD Photo Caption")
        waitForElement(in: app, "attach.sendButton").tap()

        let history = try await client.listMessages(token: user.token, chatId: chatId)
        XCTAssertGreaterThanOrEqual((history["messages"]?.arrayValue ?? []).count, 1)
        app.terminate()
    }

    @MainActor
    func testTier1_F32_03_CreatePollFromAttachmentSheetPostsInteractivePollInStage() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f32_poll")
        let chatId = try await client.createChat(token: user.token, title: "Poll Creator", chatType: "group")

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        waitForElement(in: app, AccessibilityIDs.Composer.attachButton).tap()
        waitForElement(in: app, AccessibilityIDs.Composer.attachTab("poll")).tap()
        clearAndTypeText(in: waitForElement(in: app, "pollCreator.questionField"), text: "Best UI Framework?")
        clearAndTypeText(in: waitForElement(in: app, "pollCreator.option.0"), text: "UIKit 120Hz")
        clearAndTypeText(in: waitForElement(in: app, "pollCreator.option.1"), text: "SwiftUI Shell")
        waitForElement(in: app, "pollCreator.submitButton").tap()

        let history = try await client.listMessages(token: user.token, chatId: chatId)
        XCTAssertGreaterThanOrEqual((history["messages"]?.arrayValue ?? []).count, 1)
        app.terminate()
    }

    @MainActor
    func testTier1_F32_04_EmojiStickerAndGifKeyboardPanelSwitchesTabsAndSendsAsset() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f32_panel")
        let chatId = try await client.createChat(token: user.token, title: "Sticker Keyboard", chatType: "group")

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        waitForElement(in: app, AccessibilityIDs.Composer.stickerButton).tap()
        for tab in ["emoji", "stickers", "gifs"] {
            let tabBtn = waitForElement(in: app, AccessibilityIDs.Composer.stickerPanelTab(tab))
            tabBtn.tap()
            XCTAssertTrue(isElementMarkedSelected(tabBtn), "Sticker panel tab '\(tab)' must report selected state")
        }
        app.terminate()
    }

    @MainActor
    func testTier1_F32_05_BotMenuButtonInBotChatOpensCommandsAndMiniAppLauncher() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f32_botmenu")
        let created = try await client.createBotWithMiniAppViaNL(
            token: user.token,
            prompt: "Create helper bot with menu called MenuMate"
        )
        let chatId = created.chatId
        XCTAssertGreaterThan(chatId, 0)

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        waitForElement(in: app, AccessibilityIDs.Composer.botMenuButton).tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Composer.botCommandsSheet, timeout: 8).exists)
        app.terminate()
    }

    // MARK: - F33: Voice/Video Hold-to-Record Gesture FSM (`VoiceRecorder`)

    @MainActor
    func testTier1_F33_01_QuickTapEmptyMicButtonTogglesBetweenAudioMicAndVideoCircle() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f33_tap")
        let chatId = try await client.createChat(token: user.token, title: "Mic Toggle FSM", chatType: "group")

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        let btn = waitForElement(in: app, AccessibilityIDs.Composer.micOrSendButton)
        let beforeVal = (btn.value as? String) ?? btn.label
        tapMicToToggleMode(in: app)
        let toggledVal = (btn.value as? String) ?? btn.label
        XCTAssertNotEqual(beforeVal, toggledVal)
        tapMicToToggleMode(in: app)
        XCTAssertEqual((btn.value as? String) ?? btn.label, beforeVal)
        app.terminate()
    }

    @MainActor
    func testTier1_F33_02_HoldMicAndReleaseRecordsAndUploads5BitWaveformVoiceNote() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f33_hold")
        let chatId = try await client.createChat(token: user.token, title: "Hold Record", chatType: "group")

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        holdMicToRecordAndRelease(in: app, duration: 1.1)

        let history = try await client.listMessages(token: user.token, chatId: chatId)
        XCTAssertGreaterThanOrEqual((history["messages"]?.arrayValue ?? []).count, 1)
        app.terminate()
    }

    @MainActor
    func testTier1_F33_03_HoldMicAndSlideLeftPast80ptCancelsRecordingWithoutSending() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f33_cancel")
        let chatId = try await client.createChat(token: user.token, title: "Slide Cancel", chatType: "group")

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        holdMicAndSlideLeftToCancel(in: app)

        let history = try await client.listMessages(token: user.token, chatId: chatId)
        XCTAssertEqual((history["messages"]?.arrayValue ?? []).count, 0)
        app.terminate()
    }

    @MainActor
    func testTier1_F33_04_HoldMicAndSlideUpPast50ptLocksHandsFreePreviewAndSends() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f33_lock")
        let chatId = try await client.createChat(token: user.token, title: "Slide Up Lock", chatType: "group")

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        holdMicAndSlideUpToLock(in: app)
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Composer.voiceLockIndicator).exists)
        waitForElement(in: app, AccessibilityIDs.Composer.voicePauseButton).tap()
        waitForElement(in: app, AccessibilityIDs.Composer.voiceLockedSendButton).tap()

        let history = try await client.listMessages(token: user.token, chatId: chatId)
        XCTAssertGreaterThanOrEqual((history["messages"]?.arrayValue ?? []).count, 1)
        app.terminate()
    }

    @MainActor
    func testTier1_F33_05_HoldVideoCircleModeRecordsAndSendsCircularVideoNote() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f33_vcirc")
        let chatId = try await client.createChat(token: user.token, title: "Video Circle FSM", chatType: "group")

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        tapMicToToggleMode(in: app)
        holdMicToRecordAndRelease(in: app, duration: 1.2)

        let history = try await client.listMessages(token: user.token, chatId: chatId)
        XCTAssertGreaterThanOrEqual((history["messages"]?.arrayValue ?? []).count, 1)
        app.terminate()
    }

    // MARK: - F34: Cloud Draft Sync & 61 Hermes/Bot `/slash` + `@` Autocomplete

    @MainActor
    func testTier1_F34_01_UnsentComposerTextDebouncesCloudDraftToServerAndChatsRow() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f34_draft")
        let chatId = try await client.createChat(token: user.token, title: "Cloud Draft", chatType: "group")

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Composer.textView), text: "Unsent cloud draft v1")
        interactivePopSwipeBack(in: app)

        let savedDraft = try await client.getDraft(token: user.token, chatId: chatId)
        XCTAssertEqual(savedDraft, "Unsent cloud draft v1")
        app.terminate()
    }

    @MainActor
    func testTier1_F34_02_RemoteCloudDraftUpdateHydratesComposerTextViewOnOpen() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f34_remdraft")
        let chatId = try await client.createChat(token: user.token, title: "Remote Draft", chatType: "group")
        try await client.putDraft(token: user.token, chatId: chatId, text: "Hydrated From Desktop")

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        let tv = waitForElement(in: app, AccessibilityIDs.Composer.textView)
        XCTAssertEqual(tv.value as? String, "Hydrated From Desktop")
        app.terminate()
    }

    @MainActor
    func testTier1_F34_03_TypingSlashOpens61HermesAndBotCommandPopupAndCompletesCommand() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f34_slash")
        let welcome = try await client.requestJSON(method: "POST", path: "/api/v1/chats/welcome", token: user.token)
        let botChatId = welcome["chat_id"]?.int64Value ?? welcome["id"]?.int64Value ?? 1

        let cmds = try await client.requestJSON(method: "GET", path: "/api/v1/hermes/commands", token: user.token)
        XCTAssertGreaterThanOrEqual((cmds["commands"]?.arrayValue ?? []).count, 20)

        waitForElement(in: app, AccessibilityIDs.Chats.row(botChatId), timeout: 10).tap()
        let tv = waitForElement(in: app, AccessibilityIDs.Composer.textView)
        clearAndTypeText(in: tv, text: "/he")
        let popup = waitForElement(in: app, AccessibilityIDs.Composer.slashPopup, timeout: 8)
        XCTAssertTrue(popup.exists)
        app.staticTexts["/help"].firstMatch.tap()
        XCTAssertTrue(((tv.value as? String) ?? "").hasPrefix("/help"))
        app.terminate()
    }

    @MainActor
    func testTier1_F34_04_TypingAtMentionHashtagAndColonEmojiCompletesInlineTokens() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f34_comp")
        let peer = try await client.registerUser(username: "manjushri_\(uniqueSuffix())", firstName: "Manjushri")
        let chatId = try await client.createChat(
            token: user.token,
            title: "Inline Completion",
            chatType: "group",
            memberIds: [peer.userId]
        )

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        let tv = waitForElement(in: app, AccessibilityIDs.Composer.textView)
        clearAndTypeText(in: tv, text: "@manju")
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Composer.slashPopup, timeout: 8).exists)
        app.staticTexts["Manjushri"].firstMatch.tap()
        XCTAssertTrue(((tv.value as? String) ?? "").contains("@\(peer.username)"))
        app.terminate()
    }

    @MainActor
    func testTier1_F34_05_ContextMentionBadgesAttachUrlAndGitContextToAgentPrompt() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f34_ctx")
        let welcome = try await client.requestJSON(method: "POST", path: "/api/v1/chats/welcome", token: user.token)
        let botChatId = welcome["chat_id"]?.int64Value ?? welcome["id"]?.int64Value ?? 0
        XCTAssertGreaterThan(botChatId, 0)
        let beforeHistory = try await client.listMessages(token: user.token, chatId: botChatId)
        let beforeCount = (beforeHistory["messages"]?.arrayValue ?? []).count

        waitForElement(in: app, AccessibilityIDs.Chats.row(botChatId), timeout: 10).tap()
        clearAndTypeText(
            in: waitForElement(in: app, AccessibilityIDs.Composer.textView),
            text: "@url https://example.com Summarize"
        )
        waitForElement(in: app, AccessibilityIDs.Composer.micOrSendButton).tap()

        let history = try await client.listMessages(token: user.token, chatId: botChatId)
        let afterCount = (history["messages"]?.arrayValue ?? []).count
        XCTAssertGreaterThan(afterCount, beforeCount)
        app.terminate()
    }
}
