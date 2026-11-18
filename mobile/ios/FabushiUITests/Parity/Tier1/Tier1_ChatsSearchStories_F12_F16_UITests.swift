import Foundation
import XCTest

final class Tier1_ChatsSearchStories_F12_F16_UITests: ParityTestCase {

    // MARK: - F12: Virtualized Dialog Rows, Swipe & Blurred Context Menu

    @MainActor
    func testTier1_F12_01_DialogRowsRenderPreviewUnreadBadgeAndRedDraftIndicator() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f12_rows")
        let peer = try await client.registerUser(username: "f12_peer_\(uniqueSuffix())")

        let unreadChat = try await client.createChat(
            token: peer.token,
            title: "Unread Dialog",
            chatType: "group",
            memberIds: [user.userId]
        )
        try await client.sendMessage(token: peer.token, chatId: unreadChat, text: "Msg 1")
        try await client.sendMessage(token: peer.token, chatId: unreadChat, text: "Msg 2")

        let draftChat = try await client.createChat(
            token: user.token,
            title: "Draft Dialog",
            chatType: "group",
            memberIds: [peer.userId]
        )
        try await client.putDraft(token: user.token, chatId: draftChat, text: "WIP architecture note")

        let badge = waitForElement(in: app, AccessibilityIDs.Chats.rowUnreadBadge(unreadChat), timeout: 10)
        XCTAssertEqual(badge.label, "2")
        let draftBadge = waitForElement(in: app, AccessibilityIDs.Chats.rowDraftBadge(draftChat), timeout: 10)
        XCTAssertFalse(draftBadge.label.isEmpty)
        let draftRow = waitForElement(in: app, AccessibilityIDs.Chats.row(draftChat))
        XCTAssertTrue(draftRow.label.contains("WIP architecture note"))
        app.terminate()
    }

    @MainActor
    func testTier1_F12_02_LeadingSwipeMarksUnreadDialogReadAndClearsServerUnreadCount() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f12_read")
        let peer = try await client.registerUser(username: "f12_rpeer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "Swipe Read Room",
            chatType: "group",
            memberIds: [user.userId]
        )
        try await client.sendMessage(token: peer.token, chatId: chatId, text: "Mark me read")

        let row = waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10)
        row.swipeRight()
        app.buttons["Read"].firstMatch.tap()

        waitForElementDisappearance(in: app, AccessibilityIDs.Chats.rowUnreadBadge(chatId))
        let detail = try await client.getChat(token: user.token, chatId: chatId)
        XCTAssertEqual(detail["unread_count"]?.intValue ?? 0, 0)
        app.terminate()
    }

    @MainActor
    func testTier1_F12_03_LeadingSwipePinsDialogToTopOfChatsList() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f12_pin")
        let peer = try await client.registerUser(username: "f12_ppeer_\(uniqueSuffix())")
        let olderChat = try await client.createChat(
            token: peer.token,
            title: "Older Pin Target",
            chatType: "group",
            memberIds: [user.userId]
        )
        let newerChat = try await client.createChat(
            token: peer.token,
            title: "Newer Unpinned",
            chatType: "group",
            memberIds: [user.userId]
        )
        try await client.sendMessage(token: peer.token, chatId: newerChat, text: "Latest")

        let olderRow = waitForElement(in: app, AccessibilityIDs.Chats.row(olderChat), timeout: 10)
        olderRow.swipeRight()
        app.buttons["Pin"].firstMatch.tap()

        let detail = try await client.getChat(token: user.token, chatId: olderChat)
        XCTAssertEqual(detail["is_pinned"]?.boolValue, true)
        XCTAssertLessThan(
            waitForElement(in: app, AccessibilityIDs.Chats.row(olderChat)).frame.minY,
            waitForElement(in: app, AccessibilityIDs.Chats.row(newerChat)).frame.minY
        )
        app.terminate()
    }

    @MainActor
    func testTier1_F12_04_TrailingSwipeMutesDialogAndPersistsMuteFlagOnServer() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f12_mute")
        let peer = try await client.registerUser(username: "f12_mpeer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "Quiet Room",
            chatType: "group",
            memberIds: [user.userId]
        )

        let row = waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10)
        row.swipeLeft()
        app.buttons["Mute"].firstMatch.tap()

        let detail = try await client.getChat(token: user.token, chatId: chatId)
        XCTAssertEqual(detail["is_muted"]?.boolValue, true)
        app.terminate()
    }

    @MainActor
    func testTier1_F12_05_LongPressContextMenuAndEditMultiSelectManageDialogs() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f12_ctx")
        let peer = try await client.registerUser(username: "f12_cpeer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "Context Preview Dialog",
            chatType: "group",
            memberIds: [user.userId]
        )
        try await client.sendMessage(token: peer.token, chatId: chatId, text: "Context preview line")

        let row = waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10)
        row.press(forDuration: 0.9)
        let pinAction = app.buttons["Pin"].firstMatch
        XCTAssertTrue(pinAction.waitForExistence(timeout: 5))
        pinAction.tap()

        waitForElement(in: app, AccessibilityIDs.Chats.editButton).tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Chats.multiSelectBar, timeout: 6).exists)
        app.terminate()
    }

    // MARK: - F13: Chat Folders, Collapsible Archive & Forum Topics

    @MainActor
    func testTier1_F13_01_SyncedCustomChatFolderChipFiltersDialogsAndShowsUnreadPill() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f13_fld")
        let peer = try await client.registerUser(username: "f13_fpeer_\(uniqueSuffix())")
        let workChat = try await client.createChat(
            token: peer.token,
            title: "Work Sprint",
            chatType: "group",
            memberIds: [user.userId]
        )
        let personalChat = try await client.createChat(
            token: peer.token,
            title: "Weekend Hike",
            chatType: "group",
            memberIds: [user.userId]
        )
        let folderId = try await client.createFolder(
            token: user.token,
            title: "Work",
            includeChats: [workChat]
        )

        waitForElement(in: app, AccessibilityIDs.Chats.folder("\(folderId)"), timeout: 10).tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Chats.row(workChat)).exists)
        XCTAssertFalse(element(in: app, AccessibilityIDs.Chats.row(personalChat)).exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F13_02_CreateCustomFolderInSettingsSyncsLiveToChatsFolderStrip() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f13_mkfld")
        waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).tap()
        waitForElement(in: app, AccessibilityIDs.Settings.row("folders")).tap()
        waitForElement(in: app, "folders.createButton").tap()
        clearAndTypeText(in: waitForElement(in: app, "folders.titleField"), text: "Engineering")
        waitForElement(in: app, "folders.saveButton").tap()

        let folders = try await client.listFolders(token: user.token)
        let items = folders["folders"]?.arrayValue ?? []
        XCTAssertTrue(items.contains { $0["title"]?.stringValue == "Engineering" })
        app.terminate()
    }

    @MainActor
    func testTier1_F13_03_PullDownRevealsArchiveRowAndTrailingSwipeArchivesAndUnarchivesChat() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f13_arch")
        let peer = try await client.registerUser(username: "f13_apeer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "Archive Candidate",
            chatType: "group",
            memberIds: [user.userId]
        )

        let row = waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10)
        row.swipeLeft()
        app.buttons["Archive"].firstMatch.tap()

        let archivedCheck = try await client.getChat(token: user.token, chatId: chatId)
        XCTAssertEqual(archivedCheck["is_archived"]?.boolValue, true)

        pullDownChatsToRevealArchiveAndStories(in: app)
        waitForElement(in: app, AccessibilityIDs.Chats.archiveRow).tap()
        let archivedRow = waitForElement(in: app, AccessibilityIDs.Chats.row(chatId))
        archivedRow.swipeLeft()
        app.buttons["Unarchive"].firstMatch.tap()

        let unarchivedCheck = try await client.getChat(token: user.token, chatId: chatId)
        XCTAssertEqual(unarchivedCheck["is_archived"]?.boolValue, false)
        app.terminate()
    }

    @MainActor
    func testTier1_F13_04_ForumSupergroupOpensTopicsListAndScopesMessagesByTopicId() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f13_forum")
        let peer = try await client.registerUser(username: "f13_toppeer_\(uniqueSuffix())")
        let forumId = try await client.createChat(
            token: user.token,
            title: "iOS Architecture Forum",
            chatType: "supergroup",
            memberIds: [peer.userId],
            isForum: true
        )
        let topic1 = try await client.createForumTopic(token: user.token, chatId: forumId, title: "Swift 6 FFI")
        let topic2 = try await client.createForumTopic(token: user.token, chatId: forumId, title: "SQLite WAL")
        try await client.sendMessage(token: peer.token, chatId: forumId, text: "Only in Topic 2", topicId: topic2)

        waitForElement(in: app, AccessibilityIDs.Chats.row(forumId), timeout: 10).tap()
        let topicRow = app.staticTexts["Swift 6 FFI"].firstMatch
        XCTAssertTrue(topicRow.waitForExistence(timeout: 8))
        topicRow.tap()

        clearAndTypeText(
            in: waitForElement(in: app, AccessibilityIDs.Composer.textView),
            text: "Scoped Topic 1 Reply"
        )
        waitForElement(in: app, AccessibilityIDs.Composer.micOrSendButton).tap()

        let t1History = try await client.listMessages(token: user.token, chatId: forumId, topicId: topic1)
        let msgs = t1History["messages"]?.arrayValue ?? []
        XCTAssertGreaterThanOrEqual(msgs.count, 1)
        app.terminate()
    }

    @MainActor
    func testTier1_F13_05_CreateNewForumTopicFromToolbarPersistsTopicOnServer() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f13_newtop")
        let forumId = try await client.createChat(
            token: user.token,
            title: "Product Roadmap Forum",
            chatType: "supergroup",
            isForum: true
        )

        waitForElement(in: app, AccessibilityIDs.Chats.row(forumId), timeout: 10).tap()
        waitForElement(in: app, "forum.createTopicButton").tap()
        clearAndTypeText(in: waitForElement(in: app, "forum.topicTitleField"), text: "Milestone Q4")
        waitForElement(in: app, "forum.topicSaveButton").tap()

        let topicsRes = try await client.listForumTopics(token: user.token, chatId: forumId)
        let topics = topicsRes["topics"]?.arrayValue ?? []
        XCTAssertTrue(topics.contains { $0["title"]?.stringValue == "Milestone Q4" })
        app.terminate()
    }

    // MARK: - F14: Universal 10-Category Tantivy Search & Filter Chips

    @MainActor
    func testTier1_F14_01_FocusingSearchBarDisplaysTenCategoryScopesAndSixFilterTags() async throws {
        let (app, _) = try await launchAuthenticatedUser(usernamePrefix: "f14_ui")
        waitForElement(in: app, AccessibilityIDs.Chats.searchBar).tap()

        for scope in ["chats", "channels", "apps", "posts", "media", "downloads", "links", "files", "music", "voice"] {
            XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Chats.searchScope(scope), timeout: 6).exists)
        }
        for tag in ["all", "unread", "bot", "pinned", "media", "links"] {
            XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Chats.searchTag(tag), timeout: 6).exists)
        }
        app.terminate()
    }

    @MainActor
    func testTier1_F14_02_FullTextQueryFindsTantivyHitAndJumpsDirectlyToTranscriptBubble() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f14_fts")
        let peer = try await client.registerUser(username: "f14_peer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "Searchable Stage",
            chatType: "direct",
            peerUserId: user.userId,
            memberIds: [user.userId]
        )
        let msgId = try await client.sendMessage(
            token: peer.token,
            chatId: chatId,
            text: "vajracchedika_prajnaparamita_unique_hit"
        )

        let searchBar = waitForElement(in: app, AccessibilityIDs.Chats.searchBar)
        clearAndTypeText(in: searchBar, text: "vajracchedika")
        let hitRow = app.staticTexts["vajracchedika_prajnaparamita_unique_hit"].firstMatch
        XCTAssertTrue(hitRow.waitForExistence(timeout: 8))
        hitRow.tap()

        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Stage.bubble(msgId), timeout: 8).exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F14_03_MediaLinksAndFilesSearchScopesFilterMatchingAttachments() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f14_scopes")
        let peer = try await client.registerUser(username: "f14_speer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "Assets Chat",
            chatType: "direct",
            peerUserId: user.userId,
            memberIds: [user.userId]
        )
        try await client.sendMessage(token: peer.token, chatId: chatId, text: "Spec https://rust-lang.org/spec")
        let docId = try await client.uploadMediaDirect(
            token: peer.token,
            chatId: chatId,
            filename: "design_audit.pdf",
            mimeType: "application/pdf",
            data: Data("PDF_PAYLOAD".utf8)
        )
        try await client.sendMessage(token: peer.token, chatId: chatId, text: "design_audit.pdf", mediaId: docId)

        waitForElement(in: app, AccessibilityIDs.Chats.searchBar).tap()
        waitForElement(in: app, AccessibilityIDs.Chats.searchScope("links")).tap()
        XCTAssertTrue(app.staticTexts.containing(NSPredicate(format: "label CONTAINS 'rust-lang.org'")).firstMatch.waitForExistence(timeout: 8))

        waitForElement(in: app, AccessibilityIDs.Chats.searchScope("files")).tap()
        XCTAssertTrue(app.staticTexts.containing(NSPredicate(format: "label CONTAINS 'design_audit.pdf'")).firstMatch.waitForExistence(timeout: 8))
        app.terminate()
    }

    @MainActor
    func testTier1_F14_04_ChannelsAppsPostsDownloadsMusicAndVoiceScopesSwitchCleanly() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f14_allsc")
        _ = try await client.createChat(token: user.token, title: "Public Dharma Broadcast", chatType: "channel")

        waitForElement(in: app, AccessibilityIDs.Chats.searchBar).tap()
        waitForElement(in: app, AccessibilityIDs.Chats.searchScope("channels")).tap()
        XCTAssertTrue(app.staticTexts["Public Dharma Broadcast"].waitForExistence(timeout: 8))
        for scope in ["channels", "apps", "posts", "downloads", "music", "voice"] {
            let tab = waitForElement(in: app, AccessibilityIDs.Chats.searchScope(scope))
            tab.tap()
            XCTAssertTrue(isElementMarkedSelected(tab), "Search scope '\(scope)' must become selected")
        }
        app.terminate()
    }

    @MainActor
    func testTier1_F14_05_UnreadAndBotFilterTagsNarrowSearchResultsDeterministically() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f14_tags")
        let peer = try await client.registerUser(username: "f14_tpeer_\(uniqueSuffix())")
        let unreadChat = try await client.createChat(
            token: peer.token,
            title: "Unread Human Dialog",
            chatType: "group",
            memberIds: [user.userId]
        )
        try await client.sendMessage(token: peer.token, chatId: unreadChat, text: "Unread ping")

        waitForElement(in: app, AccessibilityIDs.Chats.searchBar).tap()
        waitForElement(in: app, AccessibilityIDs.Chats.searchTag("unread")).tap()
        XCTAssertTrue(app.staticTexts["Unread Human Dialog"].waitForExistence(timeout: 8))
        waitForElement(in: app, AccessibilityIDs.Chats.searchTag("bot")).tap()
        XCTAssertFalse(app.staticTexts["Unread Human Dialog"].exists)
        app.terminate()
    }

    // MARK: - F15: Stories Strip & Fullscreen Story Viewer

    @MainActor
    func testTier1_F15_01_PullDownChatsRevealsStoriesStripAndOpensFullscreenStoryViewer() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f15_open")
        let peer = try await client.registerUser(username: "f15_peer_\(uniqueSuffix())", firstName: "StoryPeer")
        try await client.addContact(token: user.token, userId: peer.userId, username: peer.username, firstName: "StoryPeer")
        let chatId = try await client.createChat(token: peer.token, title: "Story DM", chatType: "direct", peerUserId: user.userId, memberIds: [user.userId])
        let mediaId = try await client.uploadMediaDirect(
            token: peer.token,
            chatId: chatId,
            filename: "story.png",
            mimeType: "image/png",
            data: LoopbackPeerClient.sampleMinimalPngData()
        )
        _ = try await client.createStory(token: peer.token, mediaId: mediaId, caption: "Morning Lotus")

        pullDownChatsToRevealArchiveAndStories(in: app)
        let strip = waitForElement(in: app, AccessibilityIDs.Chats.storiesStrip)
        XCTAssertTrue(strip.exists)
        strip.tap()
        XCTAssertTrue(app.staticTexts["Morning Lotus"].waitForExistence(timeout: 8))
        app.terminate()
    }

    @MainActor
    func testTier1_F15_02_ViewingPeerStoryIncrementsServerStoryViewCount() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f15_views")
        let peer = try await client.registerUser(username: "f15_vpeer_\(uniqueSuffix())")
        let chatId = try await client.createChat(token: peer.token, title: "Views DM", chatType: "direct", peerUserId: user.userId, memberIds: [user.userId])
        let mediaId = try await client.uploadMediaDirect(
            token: peer.token,
            chatId: chatId,
            filename: "view.png",
            mimeType: "image/png",
            data: LoopbackPeerClient.sampleMinimalPngData()
        )
        let storyId = try await client.createStory(token: peer.token, mediaId: mediaId, caption: "View Counter Check")
        XCTAssertFalse(storyId.isEmpty)

        pullDownChatsToRevealArchiveAndStories(in: app)
        waitForElement(in: app, AccessibilityIDs.Chats.storiesStrip).tap()
        XCTAssertTrue(app.staticTexts["View Counter Check"].waitForExistence(timeout: 8))

        let stories = try await client.listStories(token: peer.token)
        let items = stories["stories"]?.arrayValue ?? []
        let target = items.first { $0["story_id"]?.stringValue == storyId }
        XCTAssertNotNil(target, "Expected story '\(storyId)' in /api/v1/stories")
        XCTAssertGreaterThanOrEqual(target?["views_count"]?.intValue ?? 0, 1)
        app.terminate()
    }

    @MainActor
    func testTier1_F15_03_CreatePhotoStoryWithCaptionPublishesRingToStoriesStrip() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f15_create")
        pullDownChatsToRevealArchiveAndStories(in: app)
        waitForElement(in: app, "stories.addStoryButton").tap()
        clearAndTypeText(in: waitForElement(in: app, "stories.captionField"), text: "Published From iOS")
        waitForElement(in: app, "stories.publishButton").tap()

        let stories = try await client.listStories(token: user.token)
        let list = stories["stories"]?.arrayValue ?? []
        XCTAssertTrue(list.contains { $0["caption"]?.stringValue == "Published From iOS" })
        app.terminate()
    }

    @MainActor
    func testTier1_F15_04_ReplyingToPeerStoryDeliversDirectMessageInCompanionChat() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f15_reply")
        let peer = try await client.registerUser(username: "f15_rpeer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "Story Reply DM",
            chatType: "direct",
            peerUserId: user.userId,
            memberIds: [user.userId]
        )
        let mediaId = try await client.uploadMediaDirect(
            token: peer.token,
            chatId: chatId,
            filename: "reply_story.png",
            mimeType: "image/png",
            data: LoopbackPeerClient.sampleMinimalPngData()
        )
        _ = try await client.createStory(token: peer.token, mediaId: mediaId, caption: "Reply To Me")

        pullDownChatsToRevealArchiveAndStories(in: app)
        waitForElement(in: app, AccessibilityIDs.Chats.storiesStrip).tap()
        clearAndTypeText(in: waitForElement(in: app, "stories.replyField"), text: "Great mountain view!")
        waitForElement(in: app, "stories.replySendButton").tap()

        let history = try await client.listMessages(token: peer.token, chatId: chatId)
        let msgs = history["messages"]?.arrayValue ?? []
        XCTAssertGreaterThanOrEqual(msgs.count, 1)
        app.terminate()
    }

    @MainActor
    func testTier1_F15_05_DeleteOwnActiveStoryRemovesStoryFromStripAndServer() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f15_del")
        let savedChat = try await client.createChat(token: user.token, title: "Saved", chatType: "saved")
        let mediaId = try await client.uploadMediaDirect(
            token: user.token,
            chatId: savedChat,
            filename: "own_story.png",
            mimeType: "image/png",
            data: LoopbackPeerClient.sampleMinimalPngData()
        )
        let storyId = try await client.createStory(token: user.token, mediaId: mediaId, caption: "Delete Me Story")
        XCTAssertFalse(storyId.isEmpty)
        let preDelete = try await client.listStories(token: user.token)
        XCTAssertTrue((preDelete["stories"]?.arrayValue ?? []).contains { $0["story_id"]?.stringValue == storyId })

        pullDownChatsToRevealArchiveAndStories(in: app)
        waitForElement(in: app, AccessibilityIDs.Chats.storiesStrip).tap()
        waitForElement(in: app, "stories.deleteButton").tap()

        let stories = try await client.listStories(token: user.token)
        let remaining = stories["stories"]?.arrayValue ?? []
        XCTAssertFalse(remaining.contains { $0["story_id"]?.stringValue == storyId })
        app.terminate()
    }

    // MARK: - F16: Quick `+` Compose Menu (DM / AI Bot / Group / Channel)

    @MainActor
    func testTier1_F16_01_TapComposeMenuButtonPresentsFourCreationActions() async throws {
        let (app, _) = try await launchAuthenticatedUser(usernamePrefix: "f16_menu")
        waitForElement(in: app, AccessibilityIDs.Chats.composeMenuButton).tap()

        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Chats.composeNewMessage).exists)
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Chats.composeNewBot).exists)
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Chats.composeNewGroup).exists)
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Chats.composeNewChannel).exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F16_02_ComposeNewDirectMessageOpensOneToOneStageWithContact() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f16_dm")
        let friend = try await client.registerUser(username: "f16_friend_\(uniqueSuffix())", firstName: "Maitreya")
        try await client.addContact(token: user.token, userId: friend.userId, username: friend.username, firstName: "Maitreya")

        waitForElement(in: app, AccessibilityIDs.Chats.composeMenuButton).tap()
        waitForElement(in: app, AccessibilityIDs.Chats.composeNewMessage).tap()
        app.staticTexts["Maitreya"].firstMatch.tap()

        XCTAssertEqual(waitForElement(in: app, AccessibilityIDs.Stage.headerTitle, timeout: 8).label, "Maitreya")
        app.terminate()
    }

    @MainActor
    func testTier1_F16_03_ComposeNewFabushiAiBotCreatesCompanionBotWithPersona() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f16_bot")
        waitForElement(in: app, AccessibilityIDs.Chats.composeMenuButton).tap()
        waitForElement(in: app, AccessibilityIDs.Chats.composeNewBot).tap()
        clearAndTypeText(in: waitForElement(in: app, "composeBot.nameField"), text: "Sutra Scholar Bot")
        clearAndTypeText(in: waitForElement(in: app, "composeBot.personaField"), text: "Explain sutras concisely")
        waitForElement(in: app, "composeBot.createButton").tap()

        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Stage.collectionView, timeout: 10).exists)
        let bots = try await client.listFabushiBots(token: user.token)
        let botArr = bots["bots"]?.arrayValue ?? []
        XCTAssertTrue(
            botArr.contains {
                $0["display_name"]?.stringValue == "Sutra Scholar Bot"
                    || $0["name"]?.stringValue == "Sutra Scholar Bot"
            },
            "Expected 'Sutra Scholar Bot' in /api/v1/fabushi-bots"
        )
        app.terminate()
    }

    @MainActor
    func testTier1_F16_04_ComposeNewGroupWithMembersCreatesGroupConversation() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f16_grp")
        let m1 = try await client.registerUser(username: "f16_m1_\(uniqueSuffix())", firstName: "MemberOne")
        try await client.addContact(token: user.token, userId: m1.userId, username: m1.username, firstName: "MemberOne")

        waitForElement(in: app, AccessibilityIDs.Chats.composeMenuButton).tap()
        waitForElement(in: app, AccessibilityIDs.Chats.composeNewGroup).tap()
        app.staticTexts["MemberOne"].firstMatch.tap()
        clearAndTypeText(in: waitForElement(in: app, "composeGroup.titleField"), text: "Core Parity Crew")
        waitForElement(in: app, "composeGroup.createButton").tap()

        XCTAssertEqual(waitForElement(in: app, AccessibilityIDs.Stage.headerTitle, timeout: 10).label, "Core Parity Crew")
        let chats = try await client.listChats(token: user.token)
        let arr = chats["chats"]?.arrayValue ?? []
        XCTAssertTrue(arr.contains { $0["title"]?.stringValue == "Core Parity Crew" })
        app.terminate()
    }

    @MainActor
    func testTier1_F16_05_ComposeNewChannelCreatesBroadcastChannelConversation() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f16_chan")
        waitForElement(in: app, AccessibilityIDs.Chats.composeMenuButton).tap()
        waitForElement(in: app, AccessibilityIDs.Chats.composeNewChannel).tap()
        clearAndTypeText(in: waitForElement(in: app, "composeChannel.titleField"), text: "Fabushi Release Feed")
        waitForElement(in: app, "composeChannel.createButton").tap()

        XCTAssertEqual(waitForElement(in: app, AccessibilityIDs.Stage.headerTitle, timeout: 10).label, "Fabushi Release Feed")
        let chats = try await client.listChats(token: user.token)
        let arr = chats["chats"]?.arrayValue ?? []
        XCTAssertTrue(arr.contains { $0["title"]?.stringValue == "Fabushi Release Feed" })
        app.terminate()
    }
}
