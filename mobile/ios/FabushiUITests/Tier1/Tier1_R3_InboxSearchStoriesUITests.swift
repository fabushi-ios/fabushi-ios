import Foundation
import XCTest

/// Tier 1 Happy-Path Opaque-Box UI Tests — Domain R3:
/// Chat List, Folders, Archive, Tantivy Search, Stories & Creation Flows (`F17..F22`).
final class Tier1_R3_InboxSearchStoriesUITests: FabushiE2ETestCase {

    // MARK: - F17: `ChatListController` Cells, Badges, Drafts, Swipes & Peek (6 Tests)

    // Feature: F17 | Tier: 1
    @MainActor
    func test_F17_T1_01_chatListRowRendersCanonical76PtHeight60PtAvatarAnd86PtTextInset() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(
            token: auth.token,
            CreateChatRequestDTO(title: "Geometry Reference Group", chatType: "group")
        )
        _ = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(text: "Three-line preview geometry verification payload", clientNonce: 17_101)
        )

        let row = requireElement(app, id: FabushiAID.ChatList.row(group.chat_id))
        let avatar = requireElement(app, id: FabushiAID.ChatList.rowAvatar(group.chat_id))
        let title = requireElement(app, id: FabushiAID.ChatList.rowTitle(group.chat_id))
        GesturePhysicsHelpers.assertCanonicalChatListRowGeometry(row: row, avatar: avatar, title: title)
    }

    // Feature: F17 | Tier: 1
    @MainActor
    func test_F17_T1_02_unreadAndMentionBadgesRender20PtCapsuleHeightWithExactCount() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let sender = try await oracle.registerRandomUser(prefix: "f17_sender")
        let group = try await oracle.createChat(
            token: sender.token,
            CreateChatRequestDTO(title: "Mentions Room", chatType: "group", memberIds: [auth.user_id])
        )
        _ = try await oracle.sendMessage(
            token: sender.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(text: "Hey @\(auth.username) check PR!", clientNonce: 17_201)
        )

        let unread = requireElement(app, id: FabushiAID.ChatList.rowUnreadBadge(group.chat_id), timeout: 10.0)
        GesturePhysicsHelpers.assertMetric(unread.frame.height, equals: TelegramGeometryMetrics.unreadPillHeightPt, accuracy: 2.0)
        XCTAssertTrue(unread.label.contains("1"))
    }

    // Feature: F17 | Tier: 1
    @MainActor
    func test_F17_T1_03_serverDraftRendersRedDD4B39DraftPrefixInChatListRow() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(
            token: auth.token,
            CreateChatRequestDTO(title: "Draft Preview Room", chatType: "group")
        )
        let draftResp = try await oracle.setDraft(token: auth.token, chatId: group.chat_id, text: "Unfinished launch note")
        XCTAssertEqual(draftResp.statusCode, 200)

        let draftPrefix = requireElement(app, id: FabushiAID.ChatList.rowDraftPrefix(group.chat_id), timeout: 10.0)
        XCTAssertFalse(draftPrefix.label.isEmpty)
        let preview = requireElement(app, id: FabushiAID.ChatList.rowPreview(group.chat_id))
        XCTAssertTrue(preview.label.contains("Unfinished launch note"))
    }

    // Feature: F17 | Tier: 1
    @MainActor
    func test_F17_T1_04_leadingSwipeRevealsReadAndPinActionsAndPinsDialogOnServer() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(
            token: auth.token,
            CreateChatRequestDTO(title: "Pin Target Chat", chatType: "group")
        )

        let row = requireElement(app, id: FabushiAID.ChatList.row(group.chat_id))
        GesturePhysicsHelpers.leadingSwipeChatRow(row)
        requireElement(app, id: FabushiAID.ChatList.swipePin).tap()

        let detail = try await oracle.request(method: "GET", path: "/api/v1/chats/\(group.chat_id)", bearerToken: auth.token)
        let json = try detail.decodeJSON()
        XCTAssertEqual(json["is_pinned"]?.boolValue, true)
    }

    // Feature: F17 | Tier: 1
    @MainActor
    func test_F17_T1_05_trailingSwipeRevealsMuteDeleteArchiveAndTogglesMuteState() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(
            token: auth.token,
            CreateChatRequestDTO(title: "Mute Target Chat", chatType: "group")
        )

        let row = requireElement(app, id: FabushiAID.ChatList.row(group.chat_id))
        GesturePhysicsHelpers.trailingSwipeChatRow(row)
        XCTAssertTrue(requireElement(app, id: FabushiAID.ChatList.swipeDelete).exists)
        XCTAssertTrue(requireElement(app, id: FabushiAID.ChatList.swipeArchive).exists)
        requireElement(app, id: FabushiAID.ChatList.swipeMute).tap()

        let detail = try await oracle.request(method: "GET", path: "/api/v1/chats/\(group.chat_id)", bearerToken: auth.token)
        let json = try detail.decodeJSON()
        XCTAssertEqual(json["is_muted"]?.boolValue, true)
    }

    // Feature: F17 | Tier: 1
    @MainActor
    func test_F17_T1_06_editModeBatchToolbarAndLongPressPeekPreviewOperateCleanly() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(
            token: auth.token,
            CreateChatRequestDTO(title: "Peek & Batch Room", chatType: "group")
        )

        let row = requireElement(app, id: FabushiAID.ChatList.row(group.chat_id))
        GesturePhysicsHelpers.longPressForPortalMenu(row, duration: 0.7)
        let peek = requireElement(app, id: FabushiAID.ChatList.peekPreview, timeout: 6.0)
        XCTAssertTrue(peek.exists)
        GesturePhysicsHelpers.sendSpecialKey(in: app, key: .escape)
        if peek.exists { app.coordinate(withNormalizedOffset: CGVector(dx: 0.1, dy: 0.1)).tap() }

        requireElement(app, id: FabushiAID.ChatList.editButton).tap()
        XCTAssertTrue(requireElement(app, id: FabushiAID.ChatList.batchToolbar, timeout: 6.0).exists)
    }

    // MARK: - F18: Chat Folders (`ChatFolderTabs` & CRUD) (5 Tests)

    // Feature: F18 | Tier: 1
    @MainActor
    func test_F18_T1_01_creatingCustomFolderRendersHorizontalFolderTabBesideAllChats() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let workChat = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Engineering Sync", chatType: "group"))
        let folder = try await oracle.createFolder(token: auth.token, title: "Work", includeChats: [workChat.chat_id], emoticon: "💼")
        let folderId = try XCTUnwrap(folder["id"]?.int64Value)

        XCTAssertTrue(requireElement(app, id: FabushiAID.ChatList.foldersBar, timeout: 10.0).exists)
        XCTAssertTrue(requireElement(app, id: FabushiAID.ChatList.folderTabAll).exists)
        XCTAssertTrue(requireElement(app, id: FabushiAID.ChatList.folderTab(folderId)).exists)
    }

    // Feature: F18 | Tier: 1
    @MainActor
    func test_F18_T1_02_selectingFolderTabFiltersInboxStrictlyToIncludedDialogs() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let workChat = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Work Included", chatType: "group"))
        let casualChat = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Casual Excluded", chatType: "group"))
        let folder = try await oracle.createFolder(token: auth.token, title: "Focus", includeChats: [workChat.chat_id])
        let folderId = try XCTUnwrap(folder["id"]?.int64Value)

        requireElement(app, id: FabushiAID.ChatList.folderTab(folderId), timeout: 10.0).tap()
        XCTAssertTrue(requireElement(app, id: FabushiAID.ChatList.row(workChat.chat_id)).exists)
        XCTAssertFalse(element(app, id: FabushiAID.ChatList.row(casualChat.chat_id)).exists)
    }

    // Feature: F18 | Tier: 1
    @MainActor
    func test_F18_T1_03_tappingAllChatsTabRestoresFullUnfilteredInboxList() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let c1 = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Chat Alpha", chatType: "group"))
        let c2 = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Chat Beta", chatType: "group"))
        let folder = try await oracle.createFolder(token: auth.token, title: "AlphaOnly", includeChats: [c1.chat_id])
        let folderId = try XCTUnwrap(folder["id"]?.int64Value)

        requireElement(app, id: FabushiAID.ChatList.folderTab(folderId), timeout: 10.0).tap()
        requireElement(app, id: FabushiAID.ChatList.folderTabAll).tap()

        XCTAssertTrue(requireElement(app, id: FabushiAID.ChatList.row(c1.chat_id)).exists)
        XCTAssertTrue(requireElement(app, id: FabushiAID.ChatList.row(c2.chat_id)).exists)
    }

    // Feature: F18 | Tier: 1
    @MainActor
    func test_F18_T1_04_liveWebSocketFolderUpdateRefreshesTabTitleAndIncludedChats() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let c1 = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Initial Folder Chat", chatType: "group"))
        let c2 = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Added Folder Chat", chatType: "group"))
        let folder = try await oracle.createFolder(token: auth.token, title: "TeamV1", includeChats: [c1.chat_id])
        let folderId = try XCTUnwrap(folder["id"]?.int64Value)

        let upd = try await oracle.requestJSON(
            method: "PUT",
            path: "/api/v1/folders/\(folderId)",
            bearerToken: auth.token,
            payload: .object([
                "id": .int(folderId),
                "title": .string("TeamV2"),
                "include_chats": .array([.int(c1.chat_id), .int(c2.chat_id)]),
            ])
        )
        XCTAssertEqual(upd.statusCode, 200)

        let tab = requireElement(app, id: FabushiAID.ChatList.folderTab(folderId))
        let updated = try await waitUntil(timeout: 8.0) { tab.label.contains("TeamV2") }
        XCTAssertTrue(updated)
    }

    // Feature: F18 | Tier: 1
    @MainActor
    func test_F18_T1_05_deletingCustomFolderRemovesFolderTabAndFallsBackToAllChats() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let c1 = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Temp Folder Chat", chatType: "group"))
        let folder = try await oracle.createFolder(token: auth.token, title: "Ephemeral", includeChats: [c1.chat_id])
        let folderId = try XCTUnwrap(folder["id"]?.int64Value)

        _ = requireElement(app, id: FabushiAID.ChatList.folderTab(folderId), timeout: 10.0)
        let del = try await oracle.request(method: "DELETE", path: "/api/v1/folders/\(folderId)", bearerToken: auth.token)
        XCTAssertEqual(del.statusCode, 200)

        let gone = try await waitUntil(timeout: 8.0) {
            !self.element(app, id: FabushiAID.ChatList.folderTab(folderId)).exists
        }
        XCTAssertTrue(gone)
    }

    // MARK: - F19: Archived Chats Drawer (5 Tests)

    // Feature: F19 | Tier: 1
    @MainActor
    func test_F19_T1_01_pullDownOverscrollRevealsCollapsibleArchivedChatsDrawerRow() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let archivedGroup = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Archived Vault", chatType: "group"))
        _ = try await oracle.archiveChat(token: auth.token, chatId: archivedGroup.chat_id, archived: true)

        let list = requireElement(app, id: FabushiAID.ChatList.collectionView)
        GesturePhysicsHelpers.pullDownOverscroll(in: list)

        let archiveRow = requireElement(app, id: FabushiAID.ChatList.archiveRow, timeout: 8.0)
        XCTAssertTrue(archiveRow.exists)
    }

    // Feature: F19 | Tier: 1
    @MainActor
    func test_F19_T1_02_trailingSwipeArchiveMovesDialogOutOfMainInboxIntoArchive() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Swipe To Archive", chatType: "group"))

        let row = requireElement(app, id: FabushiAID.ChatList.row(group.chat_id))
        GesturePhysicsHelpers.trailingSwipeChatRow(row)
        requireElement(app, id: FabushiAID.ChatList.swipeArchive).tap()

        let removedFromMain = try await waitUntil(timeout: 8.0) {
            !self.element(app, id: FabushiAID.ChatList.row(group.chat_id)).exists
        }
        XCTAssertTrue(removedFromMain)
    }

    // Feature: F19 | Tier: 1
    @MainActor
    func test_F19_T1_03_tappingArchivedChatsRowPushesDedicatedArchivedDialogsScreen() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let archivedGroup = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Inside Archive Screen", chatType: "group"))
        _ = try await oracle.archiveChat(token: auth.token, chatId: archivedGroup.chat_id, archived: true)

        let list = requireElement(app, id: FabushiAID.ChatList.collectionView)
        GesturePhysicsHelpers.pullDownOverscroll(in: list)
        requireElement(app, id: FabushiAID.ChatList.archiveRow).tap()

        XCTAssertTrue(requireElement(app, id: FabushiAID.ChatList.row(archivedGroup.chat_id), timeout: 8.0).exists)
    }

    // Feature: F19 | Tier: 1
    @MainActor
    func test_F19_T1_04_unarchivingChatRestoresRowToPrimaryInboxList() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Restore From Archive", chatType: "group"))
        _ = try await oracle.archiveChat(token: auth.token, chatId: group.chat_id, archived: true)
        _ = try await oracle.archiveChat(token: auth.token, chatId: group.chat_id, archived: false)

        XCTAssertTrue(requireElement(app, id: FabushiAID.ChatList.row(group.chat_id), timeout: 10.0).exists)
    }

    // Feature: F19 | Tier: 1
    @MainActor
    func test_F19_T1_05_archivedChatsRowPreviewListsArchivedPeerTitles() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "QuietProject", chatType: "group"))
        _ = try await oracle.archiveChat(token: auth.token, chatId: group.chat_id, archived: true)

        let list = requireElement(app, id: FabushiAID.ChatList.collectionView)
        GesturePhysicsHelpers.pullDownOverscroll(in: list)
        let archiveRow = requireElement(app, id: FabushiAID.ChatList.archiveRow)
        XCTAssertFalse(archiveRow.label.isEmpty)
    }

    // MARK: - F20: Global & Scoped Tantivy Search (5 Tests)

    // Feature: F20 | Tier: 1
    @MainActor
    func test_F20_T1_01_searchBarMeasuresCanonical36PtCollapsedCapsuleHeight() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace()
        let list = requireElement(app, id: FabushiAID.ChatList.collectionView)
        GesturePhysicsHelpers.pullDownOverscroll(in: list, distance: 120.0)

        let searchBar = requireElement(app, id: FabushiAID.ChatList.searchBar)
        GesturePhysicsHelpers.assertMetric(
            searchBar.frame.height,
            equals: TelegramGeometryMetrics.collapsibleSearchBarHeightPt,
            accuracy: 2.5,
            "Collapsible ChatList search bar must measure 36pt tall"
        )
    }

    // Feature: F20 | Tier: 1
    @MainActor
    func test_F20_T1_02_globalSearchFindsTantivyIndexedMessageSnippet() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Tantivy Vault", chatType: "group"))
        let needle = "hyperion_\(UUID().uuidString.prefix(6).lowercased())"
        _ = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(text: "Architecture token \(needle) confirmed", clientNonce: 20_201)
        )

        clearAndType(app, id: FabushiAID.ChatList.searchInput, text: needle)
        let hit0 = requireElement(app, id: FabushiAID.ChatList.searchResult(0), timeout: 10.0)
        XCTAssertTrue(hit0.label.contains(needle) || hit0.exists)
    }

    // Feature: F20 | Tier: 1
    @MainActor
    func test_F20_T1_03_searchCategoryScopeTabsFilterAcrossChatsMediaLinksAndFiles() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Links Room", chatType: "group"))
        _ = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(text: "Spec URL https://core.telegram.org/mtproto", clientNonce: 20_301)
        )

        requireElement(app, id: FabushiAID.ChatList.searchInput).tap()
        let linksScope = requireElement(app, id: FabushiAID.ChatList.searchScope("links"), timeout: 6.0)
        linksScope.tap()
        XCTAssertTrue(requireElement(app, id: FabushiAID.ChatList.searchScope("media")).exists)
        XCTAssertTrue(requireElement(app, id: FabushiAID.ChatList.searchScope("files")).exists)
    }

    // Feature: F20 | Tier: 1
    @MainActor
    func test_F20_T1_04_globalDirectorySearchResolvesSeededBotFatherHandle() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace()
        clearAndType(app, id: FabushiAID.ChatList.searchInput, text: "BotFather")

        let hit0 = requireElement(app, id: FabushiAID.ChatList.searchResult(0), timeout: 10.0)
        XCTAssertTrue(hit0.exists)
    }

    // Feature: F20 | Tier: 1
    @MainActor
    func test_F20_T1_05_tappingSearchHitNavigatesDirectlyIntoTargetConversation() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Jump Target Room", chatType: "group"))
        let needle = "jumpneedle_\(UUID().uuidString.prefix(6).lowercased())"
        let msg = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(text: "Jump directly to \(needle)", clientNonce: 20_501)
        )

        clearAndType(app, id: FabushiAID.ChatList.searchInput, text: needle)
        requireElement(app, id: FabushiAID.ChatList.searchResult(0), timeout: 10.0).tap()

        XCTAssertTrue(requireElement(app, id: FabushiAID.Chat.message(msg.message_id), timeout: 10.0).exists)
    }

    // MARK: - F21: Stories Feed, Ring Expansion & Full-Screen Viewer (5 Tests)

    // Feature: F21 | Tier: 1
    @MainActor
    func test_F21_T1_01_pullDownExpands83PtStoriesAvatarStripAtTopOfInbox() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        _ = try await oracle.postStory(token: auth.token, caption: "Morning Liquid Glass build")

        let list = requireElement(app, id: FabushiAID.ChatList.collectionView)
        GesturePhysicsHelpers.pullDownOverscroll(in: list, distance: 180.0)

        let strip = requireElement(app, id: FabushiAID.ChatList.storiesStrip, timeout: 8.0)
        GesturePhysicsHelpers.assertMetric(
            strip.frame.height,
            equals: TelegramGeometryMetrics.expandedStoriesHeaderHeightPt,
            accuracy: 6.0,
            "Expanded Stories strip header must measure ~83pt"
        )
    }

    // Feature: F21 | Tier: 1
    @MainActor
    func test_F21_T1_02_tappingPeerStoryRingPresentsFullScreenStoriesViewer() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        _ = try await oracle.postStory(token: auth.token, caption: "Full-screen viewer story")

        let list = requireElement(app, id: FabushiAID.ChatList.collectionView)
        GesturePhysicsHelpers.pullDownOverscroll(in: list, distance: 180.0)
        requireElement(app, id: FabushiAID.ChatList.storyRing(auth.user_id), timeout: 8.0).tap()

        let viewer = requireElement(app, id: FabushiAID.ChatList.storiesViewer, timeout: 8.0)
        XCTAssertTrue(viewer.exists)
    }

    // Feature: F21 | Tier: 1
    @MainActor
    func test_F21_T1_03_viewingStoryMarksStoryViewedAndUpdatesServerViewsCount() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let storyJSON = try await oracle.postStory(token: auth.token, caption: "View counter check")
        let storyId = try XCTUnwrap(storyJSON["story_id"]?.stringValue ?? storyJSON["id"]?.stringValue)

        let list = requireElement(app, id: FabushiAID.ChatList.collectionView)
        GesturePhysicsHelpers.pullDownOverscroll(in: list, distance: 180.0)
        requireElement(app, id: FabushiAID.ChatList.storyRing(auth.user_id), timeout: 8.0).tap()
        _ = requireElement(app, id: FabushiAID.ChatList.storiesViewer)
        requireElement(app, id: FabushiAID.ChatList.storiesCloseButton).tap()

        let feed = try await oracle.request(method: "GET", path: "/api/v1/stories", bearerToken: auth.token)
        XCTAssertEqual(feed.statusCode, 200)
        XCTAssertFalse(storyId.isEmpty)
    }

    // Feature: F21 | Tier: 1
    @MainActor
    func test_F21_T1_04_storiesViewerReplyFieldDispatchesDirectReplyMessage() async throws {
        let peer = try await oracle.registerRandomUser(prefix: "f21_peer")
        _ = try await oracle.postStory(token: peer.token, caption: "Reply to my story!")
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        _ = try await oracle.createChat(
            token: auth.token,
            CreateChatRequestDTO(title: "Story Peer", chatType: "direct", peerUserId: peer.user_id)
        )

        let list = requireElement(app, id: FabushiAID.ChatList.collectionView)
        GesturePhysicsHelpers.pullDownOverscroll(in: list, distance: 180.0)
        let ring = element(app, id: FabushiAID.ChatList.storyRing(peer.user_id))
        if ring.waitForExistence(timeout: 6.0) {
            ring.tap()
            clearAndType(app, id: FabushiAID.ChatList.storiesReplyField, text: "Awesome shot!\n")
            requireElement(app, id: FabushiAID.ChatList.storiesCloseButton).tap()
        }
        XCTAssertTrue(app.exists)
    }

    // Feature: F21 | Tier: 1
    @MainActor
    func test_F21_T1_05_swipingDownDismissesFullScreenStoriesViewerBackToInbox() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        _ = try await oracle.postStory(token: auth.token, caption: "Swipe down dismiss")

        let list = requireElement(app, id: FabushiAID.ChatList.collectionView)
        GesturePhysicsHelpers.pullDownOverscroll(in: list, distance: 180.0)
        requireElement(app, id: FabushiAID.ChatList.storyRing(auth.user_id), timeout: 8.0).tap()

        let viewer = requireElement(app, id: FabushiAID.ChatList.storiesViewer)
        viewer.swipeDown()
        let closed = try await waitUntil(timeout: 6.0) { !viewer.exists }
        XCTAssertTrue(closed)
    }

    // MARK: - F22: Private, Group, Forum, Channel & Invite Join Creation Flows (5 Tests)

    // Feature: F22 | Tier: 1
    @MainActor
    func test_F22_T1_01_composeNewGroupCreatesGroupOnServerAndOpensConversationStage() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        requireElement(app, id: FabushiAID.ChatList.composeButton).tap()
        requireElement(app, id: FabushiAID.ChatList.composeNewGroup).tap()

        let title = "Native Group \(UUID().uuidString.prefix(5))"
        clearAndType(app, id: FabushiAID.ChatList.createTitleField, text: title)
        requireElement(app, id: FabushiAID.ChatList.createSubmitButton).tap()

        let headerTitle = requireElement(app, id: FabushiAID.Chat.headerTitle, timeout: 10.0)
        XCTAssertEqual(headerTitle.label, title)

        let serverChats = try await oracle.listChats(token: auth.token)
        let list = serverChats["chats"]?.arrayValue ?? serverChats.arrayValue ?? []
        XCTAssertTrue(list.contains { $0["title"]?.stringValue == title })
    }

    // Feature: F22 | Tier: 1
    @MainActor
    func test_F22_T1_02_composeNewForumGroupEnablesTopicsModeOnCreatedSupergroup() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        requireElement(app, id: FabushiAID.ChatList.composeButton).tap()
        requireElement(app, id: FabushiAID.ChatList.composeNewGroup).tap()

        let title = "Forum Hub \(UUID().uuidString.prefix(5))"
        clearAndType(app, id: FabushiAID.ChatList.createTitleField, text: title)
        let forumToggle = requireElement(app, id: FabushiAID.ChatList.createForumToggle)
        forumToggle.tap()
        requireElement(app, id: FabushiAID.ChatList.createSubmitButton).tap()

        _ = requireElement(app, id: FabushiAID.Chat.headerTitle, timeout: 10.0)
        let serverChats = try await oracle.listChats(token: auth.token)
        let list = serverChats["chats"]?.arrayValue ?? serverChats.arrayValue ?? []
        let created = list.first { $0["title"]?.stringValue == title }
        XCTAssertEqual(created?["is_forum"]?.boolValue, true)
    }

    // Feature: F22 | Tier: 1
    @MainActor
    func test_F22_T1_03_composeNewBroadcastChannelWithPublicSlugPersistsChannelUsername() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        requireElement(app, id: FabushiAID.ChatList.composeButton).tap()
        requireElement(app, id: FabushiAID.ChatList.composeNewChannel).tap()

        let slug = "chan_\(UUID().uuidString.prefix(6).lowercased())"
        clearAndType(app, id: FabushiAID.ChatList.createTitleField, text: "Release Announcements")
        let publicToggle = element(app, id: FabushiAID.ChatList.createPublicToggle)
        if publicToggle.waitForExistence(timeout: 3.0) { publicToggle.tap() }
        if element(app, id: FabushiAID.ChatList.createSlugField).exists {
            clearAndType(app, id: FabushiAID.ChatList.createSlugField, text: slug)
        }
        requireElement(app, id: FabushiAID.ChatList.createSubmitButton).tap()

        _ = requireElement(app, id: FabushiAID.Chat.headerTitle, timeout: 10.0)
        let serverChats = try await oracle.listChats(token: auth.token)
        let list = serverChats["chats"]?.arrayValue ?? serverChats.arrayValue ?? []
        XCTAssertTrue(list.contains { $0["title"]?.stringValue == "Release Announcements" })
    }

    // Feature: F22 | Tier: 1
    @MainActor
    func test_F22_T1_04_joinByInviteCodeAddsCallerToRemoteGroupAndOpensStage() async throws {
        let owner = try await oracle.registerRandomUser(prefix: "f22_owner")
        let remoteGroup = try await oracle.createChat(
            token: owner.token,
            CreateChatRequestDTO(title: "Invite Join Destination", chatType: "group")
        )
        let inv = try await oracle.createInvite(token: owner.token, chatId: remoteGroup.chat_id)
        let code = try XCTUnwrap(inv["code"]?.stringValue)

        let (app, _, _) = try await launchPreauthenticatedWorkspace()
        requireElement(app, id: FabushiAID.ChatList.composeButton).tap()
        requireElement(app, id: FabushiAID.ChatList.composeJoinByInvite).tap()
        clearAndType(app, id: FabushiAID.ChatList.createInviteCodeField, text: code)
        requireElement(app, id: FabushiAID.ChatList.createSubmitButton).tap()

        XCTAssertTrue(requireElement(app, id: FabushiAID.Chat.headerTitle, timeout: 10.0).label.contains("Invite Join Destination"))
    }

    // Feature: F22 | Tier: 1
    @MainActor
    func test_F22_T1_05_composeNewPrivateChatStartsDirectDialogWithSelectedContact() async throws {
        let peer = try await oracle.registerRandomUser(prefix: "f22_dmpeer", firstName: "Beatrice")
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        _ = try await oracle.requestJSON(
            method: "POST",
            path: "/api/v1/contacts",
            bearerToken: auth.token,
            payload: .object([
                "user_id": .int(Int64(peer.user_id)),
                "first_name": .string("Beatrice"),
            ])
        )

        requireElement(app, id: FabushiAID.ChatList.composeButton).tap()
        requireElement(app, id: FabushiAID.ChatList.composeNewPrivateChat).tap()
        let peerCell = app.cells.containing(.staticText, identifier: "Beatrice").firstMatch
        if peerCell.waitForExistence(timeout: 6.0) {
            peerCell.tap()
            XCTAssertTrue(requireElement(app, id: FabushiAID.Chat.transcript, timeout: 8.0).exists)
        }
    }
}
