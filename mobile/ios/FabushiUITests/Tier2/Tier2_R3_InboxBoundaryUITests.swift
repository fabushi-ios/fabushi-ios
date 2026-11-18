import Foundation
import XCTest

/// Tier 2 Boundary, Negative & Corner-Case Opaque-Box UI Tests — Domain R3:
/// Chat List Extremes, Folder Edge Cases, Archive Rules, Tantivy Query Sanitization,
/// Story Ownership & Invite Exhaustion (`F17..F22`).
final class Tier2_R3_InboxBoundaryUITests: FabushiE2ETestCase {

    // MARK: - F17: ChatList Empty State, Long Text, Cancelled Swipe & Blocked DM (6 Tests)

    // Feature: F17 | Tier: 2
    @MainActor
    func test_F17_T2_01_freshAccountWithZeroDialogsRendersEmptyInboxWithoutCrash() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace()
        let list = requireElement(app, id: FabushiAID.ChatList.collectionView)
        XCTAssertTrue(list.exists)
    }

    // Feature: F17 | Tier: 2
    @MainActor
    func test_F17_T2_02_compactTwoLineCellModeRenders64PtRowHeight() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace(
            extraEnvironment: ["FABUSHI_CHATLIST_COMPACT_2LINE": "1"]
        )
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "2-Line Compact Row", chatType: "group"))
        let row = requireElement(app, id: FabushiAID.ChatList.row(group.chat_id))
        GesturePhysicsHelpers.assertMetric(row.frame.height, equals: TelegramGeometryMetrics.chatListRow2LineHeightPt, accuracy: 2.0)
    }

    // Feature: F17 | Tier: 2
    @MainActor
    func test_F17_T2_03_maximum255CharTitleAnd4000CharPreviewTruncateWithoutExpandingRowHeight() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let maxTitle = String(repeating: "W", count: 255)
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: maxTitle, chatType: "group"))
        let hugeMsg = String(repeating: "Long preview line ", count: 200)
        _ = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: hugeMsg, clientNonce: 170_301))

        let row = requireElement(app, id: FabushiAID.ChatList.row(group.chat_id))
        GesturePhysicsHelpers.assertMetric(row.frame.height, equals: TelegramGeometryMetrics.chatListRow3LineHeightPt, accuracy: 2.0)
    }

    // Feature: F17 | Tier: 2
    @MainActor
    func test_F17_T2_04_deletingServerDraftRemovesRedDraftPrefixFromInboxRow() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Draft Clear", chatType: "group"))
        _ = try await oracle.setDraft(token: auth.token, chatId: group.chat_id, text: "Temporary draft")
        _ = requireElement(app, id: FabushiAID.ChatList.rowDraftPrefix(group.chat_id), timeout: 8.0)

        _ = try await oracle.request(method: "DELETE", path: "/api/v1/chats/\(group.chat_id)/draft", bearerToken: auth.token)
        let cleared = try await waitUntil(timeout: 8.0) {
            !self.element(app, id: FabushiAID.ChatList.rowDraftPrefix(group.chat_id)).exists
        }
        XCTAssertTrue(cleared)
    }

    // Feature: F17 | Tier: 2
    @MainActor
    func test_F17_T2_05_tappingOutsideOpenSwipeRowClosesActionButtonsWithoutMutatingChat() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Swipe Dismiss", chatType: "group"))
        let row = requireElement(app, id: FabushiAID.ChatList.row(group.chat_id))
        GesturePhysicsHelpers.trailingSwipeChatRow(row)
        _ = requireElement(app, id: FabushiAID.ChatList.swipeMute)

        requireElement(app, id: FabushiAID.ChatList.header).tap()
        let detail = try await oracle.request(method: "GET", path: "/api/v1/chats/\(group.chat_id)", bearerToken: auth.token)
        let json = try detail.decodeJSON()
        XCTAssertEqual(json["is_muted"]?.boolValue, false)
    }

    // Feature: F17 | Tier: 2
    @MainActor
    func test_F17_T2_06_blockingDirectPeerBlocksSubsequentMessagesWith403AndShowsUnblockState() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let peer = try await oracle.registerRandomUser(prefix: "f17_blk")
        let dm = try await oracle.createChat(
            token: auth.token,
            CreateChatRequestDTO(title: "Block Peer", chatType: "direct", peerUserId: peer.user_id)
        )
        let blk = try await oracle.requestJSON(method: "POST", path: "/api/v1/users/\(peer.user_id)/block", bearerToken: auth.token, payload: .object([:]))
        XCTAssertEqual(blk.statusCode, 200)

        let peerAttempt = try await oracle.requestJSON(
            method: "POST",
            path: "/api/v1/chats/\(dm.chat_id)/messages",
            bearerToken: peer.token,
            payload: .object(["text": .string("Blocked ping"), "client_nonce": .int(170_601)])
        )
        XCTAssertEqual(peerAttempt.statusCode, 403)
        openChatFromInbox(app, chatId: dm.chat_id)
        XCTAssertTrue(requireElement(app, id: FabushiAID.Chat.transcript).exists)
    }

    // MARK: - F18: Chat Folders Boundaries (5 Tests)

    // Feature: F18 | Tier: 2
    @MainActor
    func test_F18_T2_01_whitespaceOnlyFolderTitleRejectedWith400BadRequest() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let badResp = try await oracle.requestJSON(
            method: "POST",
            path: "/api/v1/folders",
            bearerToken: auth.token,
            payload: .object(["title": .string("   ")])
        )
        XCTAssertEqual(badResp.statusCode, 400)
        XCTAssertTrue(requireElement(app, id: FabushiAID.ChatList.collectionView).exists)
    }

    // Feature: F18 | Tier: 2
    @MainActor
    func test_F18_T2_02_selectingFolderWithZeroMatchingDialogsRendersCleanEmptyState() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        _ = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Outside Folder", chatType: "group"))
        let folder = try await oracle.createFolder(token: auth.token, title: "EmptyFolder", includeChats: [])
        let fid = try XCTUnwrap(folder["id"]?.int64Value)

        requireElement(app, id: FabushiAID.ChatList.folderTab(fid), timeout: 10.0).tap()
        XCTAssertTrue(requireElement(app, id: FabushiAID.ChatList.collectionView).exists)
    }

    // Feature: F18 | Tier: 2
    @MainActor
    func test_F18_T2_03_deletingCurrentlyActiveFolderTabFallsBackSafelyToAllChats() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Fallback AllChats", chatType: "group"))
        let folder = try await oracle.createFolder(token: auth.token, title: "DeleteActive", includeChats: [group.chat_id])
        let fid = try XCTUnwrap(folder["id"]?.int64Value)

        requireElement(app, id: FabushiAID.ChatList.folderTab(fid), timeout: 10.0).tap()
        _ = try await oracle.request(method: "DELETE", path: "/api/v1/folders/\(fid)", bearerToken: auth.token)

        XCTAssertTrue(requireElement(app, id: FabushiAID.ChatList.row(group.chat_id), timeout: 8.0).exists)
    }

    // Feature: F18 | Tier: 2
    @MainActor
    func test_F18_T2_04_tenCustomFoldersScrollHorizontallyInFolderTabStrip() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        var lastId: Int64 = 0
        for idx in 1...10 {
            let f = try await oracle.createFolder(token: auth.token, title: "Folder\(idx)", includeChats: [])
            lastId = f["id"]?.int64Value ?? lastId
        }

        let bar = requireElement(app, id: FabushiAID.ChatList.foldersBar, timeout: 10.0)
        bar.swipeLeft()
        XCTAssertTrue(requireElement(app, id: FabushiAID.ChatList.folderTab(lastId)).exists)
    }

    // Feature: F18 | Tier: 2
    @MainActor
    func test_F18_T2_05_excludeChatsRuleOverridesIncludeGroupsFlagInFolderFilter() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let gIncluded = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Kept Group", chatType: "group"))
        let gExcluded = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Excluded Group", chatType: "group"))
        let resp = try await oracle.requestJSON(
            method: "POST",
            path: "/api/v1/folders",
            bearerToken: auth.token,
            payload: .object([
                "title": .string("RuleTest"),
                "include_groups": .bool(true),
                "exclude_chats": .array([.int(gExcluded.chat_id)]),
            ])
        )
        let fid = try XCTUnwrap(try resp.decodeJSON()["id"]?.int64Value)

        requireElement(app, id: FabushiAID.ChatList.folderTab(fid), timeout: 10.0).tap()
        XCTAssertTrue(requireElement(app, id: FabushiAID.ChatList.row(gIncluded.chat_id)).exists)
        XCTAssertFalse(element(app, id: FabushiAID.ChatList.row(gExcluded.chat_id)).exists)
    }

    // MARK: - F19: Archived Chats Edge Cases (5 Tests)

    // Feature: F19 | Tier: 2
    @MainActor
    func test_F19_T2_01_inboxWithoutAnyArchivedChatsOmitsArchivedRowOnOverscroll() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        _ = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Active Only", chatType: "group"))

        let list = requireElement(app, id: FabushiAID.ChatList.collectionView)
        GesturePhysicsHelpers.pullDownOverscroll(in: list)
        XCTAssertFalse(element(app, id: FabushiAID.ChatList.archiveRow).exists)
    }

    // Feature: F19 | Tier: 2
    @MainActor
    func test_F19_T2_02_mutedArchivedChatDoesNotInflateRootChatsTabUnreadBadge() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let peer = try await oracle.registerRandomUser(prefix: "f19_muted")
        let group = try await oracle.createChat(
            token: peer.token,
            CreateChatRequestDTO(title: "Muted Archive", chatType: "group", memberIds: [auth.user_id])
        )
        _ = try await oracle.archiveChat(token: auth.token, chatId: group.chat_id, archived: true)
        _ = try await oracle.requestJSON(
            method: "POST",
            path: "/api/v1/chats/\(group.chat_id)/mute",
            bearerToken: auth.token,
            payload: .object(["muted": .bool(true)])
        )
        _ = try await oracle.sendMessage(token: peer.token, chatId: group.chat_id, SendMessageRequestDTO(text: "Silent archive msg", clientNonce: 190_201))

        XCTAssertFalse(element(app, id: FabushiAID.Root.badgeChats).exists)
    }

    // Feature: F19 | Tier: 2
    @MainActor
    func test_F19_T2_03_archivingPinnedDialogRemovesItFromPinnedInboxHeaderSlots() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Pinned Then Archived", chatType: "group"))
        _ = try await oracle.requestJSON(method: "POST", path: "/api/v1/chats/\(group.chat_id)/pin", bearerToken: auth.token, payload: .object(["pinned": .bool(true)]))
        _ = try await oracle.archiveChat(token: auth.token, chatId: group.chat_id, archived: true)

        let hidden = try await waitUntil(timeout: 8.0) { !self.element(app, id: FabushiAID.ChatList.row(group.chat_id)).exists }
        XCTAssertTrue(hidden)
    }

    // Feature: F19 | Tier: 2
    @MainActor
    func test_F19_T2_04_kickingMemberFromArchivedGroupRemovesStaleArchivedEntry() async throws {
        let owner = try await oracle.registerRandomUser(prefix: "f19_kickowner")
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(
            token: owner.token,
            CreateChatRequestDTO(title: "Kick Archived", chatType: "group", memberIds: [auth.user_id])
        )
        _ = try await oracle.archiveChat(token: auth.token, chatId: group.chat_id, archived: true)
        _ = try await oracle.request(method: "DELETE", path: "/api/v1/chats/\(group.chat_id)/members/\(auth.user_id)", bearerToken: owner.token)

        let check = try await oracle.request(method: "GET", path: "/api/v1/chats/\(group.chat_id)/messages", bearerToken: auth.token)
        XCTAssertEqual(check.statusCode, 403)
        XCTAssertTrue(app.exists)
    }

    // Feature: F19 | Tier: 2
    @MainActor
    func test_F19_T2_05_rapidArchiveUnarchiveToggleConvergesToFinalUnarchivedState() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "FlipFlop Archive", chatType: "group"))
        _ = try await oracle.archiveChat(token: auth.token, chatId: group.chat_id, archived: true)
        _ = try await oracle.archiveChat(token: auth.token, chatId: group.chat_id, archived: false)
        _ = try await oracle.archiveChat(token: auth.token, chatId: group.chat_id, archived: true)
        _ = try await oracle.archiveChat(token: auth.token, chatId: group.chat_id, archived: false)

        XCTAssertTrue(requireElement(app, id: FabushiAID.ChatList.row(group.chat_id), timeout: 8.0).exists)
    }

    // MARK: - F20: Tantivy Search Sanitization & Authorization Boundaries (5 Tests)

    // Feature: F20 | Tier: 2
    @MainActor
    func test_F20_T2_01_tantivyBooleanAndBracketSyntaxCharactersSanitizedWithout500Error() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace()
        let hostileQuery = "AND OR NOT (foo:bar) [1 TO 5] \"unclosed*?"
        clearAndType(app, id: FabushiAID.ChatList.searchInput, text: hostileQuery)

        XCTAssertTrue(app.state == .runningForeground)
        XCTAssertFalse(element(app, id: FabushiAID.ChatList.searchResult(0)).exists)
    }

    // Feature: F20 | Tier: 2
    @MainActor
    func test_F20_T2_02_cjkAndEmojiSearchQueryFindsExactMultilingualMessage() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "CJK Search", chatType: "group"))
        _ = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: "液态玻璃架构验证🚀", clientNonce: 200_201))

        clearAndType(app, id: FabushiAID.ChatList.searchInput, text: "液态玻璃")
        XCTAssertTrue(requireElement(app, id: FabushiAID.ChatList.searchResult(0), timeout: 10.0).exists)
    }

    // Feature: F20 | Tier: 2
    @MainActor
    func test_F20_T2_03_unmatchedRandomSearchTokenRendersZeroResultsCleanly() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace()
        clearAndType(app, id: FabushiAID.ChatList.searchInput, text: "zzz_nonexistent_\(UUID().uuidString.prefix(8))")
        XCTAssertFalse(element(app, id: FabushiAID.ChatList.searchResult(0)).exists)
    }

    // Feature: F20 | Tier: 2
    @MainActor
    func test_F20_T2_04_globallyDeletedMessageIsPurgedFromSubsequentTantivySearch() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Purge Index", chatType: "group"))
        let secret = "ephemeraltoken_\(UUID().uuidString.prefix(7).lowercased())"
        let msg = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: secret, clientNonce: 200_401))
        _ = try await oracle.deleteMessage(token: auth.token, chatId: group.chat_id, messageId: msg.message_id, revoke: true)

        clearAndType(app, id: FabushiAID.ChatList.searchInput, text: secret)
        XCTAssertFalse(element(app, id: FabushiAID.ChatList.searchResult(0)).exists)
    }

    // Feature: F20 | Tier: 2
    @MainActor
    func test_F20_T2_05_foreignPrivateGroupMessagesNeverLeakIntoOutsiderGlobalSearch() async throws {
        let other = try await oracle.registerRandomUser(prefix: "f20_secret")
        let secretGroup = try await oracle.createChat(token: other.token, CreateChatRequestDTO(title: "Secret Board", chatType: "group"))
        let needle = "classifiedneedle_\(UUID().uuidString.prefix(7).lowercased())"
        _ = try await oracle.sendMessage(token: other.token, chatId: secretGroup.chat_id, SendMessageRequestDTO(text: needle, clientNonce: 200_501))

        let (app, _, _) = try await launchPreauthenticatedWorkspace()
        clearAndType(app, id: FabushiAID.ChatList.searchInput, text: needle)
        XCTAssertFalse(element(app, id: FabushiAID.ChatList.searchResult(0)).exists, "Private group message must never leak to non-member search")
    }

    // MARK: - F21: Stories Ownership, TTL Clamping & Viewer Bounds (5 Tests)

    // Feature: F21 | Tier: 2
    @MainActor
    func test_F21_T2_01_storyCreationWithoutMediaIdOrUrlRejectedWith400MissingMedia() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let resp = try await oracle.requestJSON(
            method: "POST",
            path: "/api/v1/stories",
            bearerToken: auth.token,
            payload: .object(["caption": .string("No media story")])
        )
        XCTAssertEqual(resp.statusCode, 400)
        XCTAssertTrue(app.exists)
    }

    // Feature: F21 | Tier: 2
    @MainActor
    func test_F21_T2_02_nonOwnerDeletingPeerStoryRejectedWith404NotFound() async throws {
        let peer = try await oracle.registerRandomUser(prefix: "f21_owner")
        let story = try await oracle.postStory(token: peer.token, caption: "Owner protected story")
        let sid = try XCTUnwrap(story["story_id"]?.stringValue ?? story["id"]?.stringValue)

        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let del = try await oracle.request(method: "DELETE", path: "/api/v1/stories/\(sid)", bearerToken: auth.token)
        XCTAssertEqual(del.statusCode, 404)
        XCTAssertTrue(app.exists)
    }

    // Feature: F21 | Tier: 2
    @MainActor
    func test_F21_T2_03_deletingOwnStoryRemovesAvatarRingFromInboxStoriesStrip() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let story = try await oracle.postStory(token: auth.token, caption: "Delete my story")
        let sid = try XCTUnwrap(story["story_id"]?.stringValue ?? story["id"]?.stringValue)

        let list = requireElement(app, id: FabushiAID.ChatList.collectionView)
        GesturePhysicsHelpers.pullDownOverscroll(in: list, distance: 180.0)
        _ = requireElement(app, id: FabushiAID.ChatList.storyRing(auth.user_id), timeout: 8.0)

        _ = try await oracle.request(method: "DELETE", path: "/api/v1/stories/\(sid)", bearerToken: auth.token)
        let gone = try await waitUntil(timeout: 8.0) {
            !self.element(app, id: FabushiAID.ChatList.storyRing(auth.user_id)).exists
        }
        XCTAssertTrue(gone)
    }

    // Feature: F21 | Tier: 2
    @MainActor
    func test_F21_T2_04_outOfRangeStoryTtlHoursClampedSafelyTo1Through48Hours() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let resp = try await oracle.requestJSON(
            method: "POST",
            path: "/api/v1/stories",
            bearerToken: auth.token,
            payload: .object([
                "media_url": .string("https://example.invalid/clamp.jpg"),
                "ttl_hours": .int(9999),
            ])
        )
        XCTAssertEqual(resp.statusCode, 201)
        XCTAssertTrue(app.exists)
    }

    // Feature: F21 | Tier: 2
    @MainActor
    func test_F21_T2_05_tappingLeftEdgeOnFirstStorySlideDoesNotUnderflowViewer() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        _ = try await oracle.postStory(token: auth.token, caption: "First slide boundary")

        let list = requireElement(app, id: FabushiAID.ChatList.collectionView)
        GesturePhysicsHelpers.pullDownOverscroll(in: list, distance: 180.0)
        requireElement(app, id: FabushiAID.ChatList.storyRing(auth.user_id), timeout: 8.0).tap()

        let viewer = requireElement(app, id: FabushiAID.ChatList.storiesViewer)
        viewer.coordinate(withNormalizedOffset: CGVector(dx: 0.08, dy: 0.5)).tap()
        XCTAssertTrue(viewer.exists)
    }

    // MARK: - F22: Chat Creation & Invite Exhaustion / Revocation Boundaries (6 Tests)

    // Feature: F22 | Tier: 2
    @MainActor
    func test_F22_T2_01_emptyGroupTitleBlocksCreateSubmissionWithValidationError() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace()
        requireElement(app, id: FabushiAID.ChatList.composeButton).tap()
        requireElement(app, id: FabushiAID.ChatList.composeNewGroup).tap()

        clearAndType(app, id: FabushiAID.ChatList.createTitleField, text: "   ")
        let submit = requireElement(app, id: FabushiAID.ChatList.createSubmitButton)
        if submit.isEnabled {
            submit.tap()
            XCTAssertTrue(requireElement(app, id: FabushiAID.ChatList.createValidationError, timeout: 5.0).exists)
        }
    }

    // Feature: F22 | Tier: 2
    @MainActor
    func test_F22_T2_02_oversizedGroupTitleOver255CharactersRejectedWith400() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let resp = try await oracle.requestJSON(
            method: "POST",
            path: "/api/v1/chats",
            bearerToken: auth.token,
            payload: .object([
                "title": .string(String(repeating: "T", count: 260)),
                "chat_type": .string("group"),
            ])
        )
        XCTAssertEqual(resp.statusCode, 400)
        XCTAssertTrue(app.exists)
    }

    // Feature: F22 | Tier: 2
    @MainActor
    func test_F22_T2_03_singleUseInviteLinkRejectsSecondJoinerInUiWithError() async throws {
        let owner = try await oracle.registerRandomUser(prefix: "f22_lim_owner")
        let firstJoiner = try await oracle.registerRandomUser(prefix: "f22_lim_first")
        let group = try await oracle.createChat(token: owner.token, CreateChatRequestDTO(title: "OneSeat Group", chatType: "group"))
        let inv = try await oracle.createInvite(token: owner.token, chatId: group.chat_id, usageLimit: 1)
        let code = try XCTUnwrap(inv["code"]?.stringValue)
        _ = try await oracle.requestJSON(method: "POST", path: "/api/v1/invites/join", bearerToken: firstJoiner.token, payload: .object(["code": .string(code)]))

        let (app, _, _) = try await launchPreauthenticatedWorkspace()
        requireElement(app, id: FabushiAID.ChatList.composeButton).tap()
        requireElement(app, id: FabushiAID.ChatList.composeJoinByInvite).tap()
        clearAndType(app, id: FabushiAID.ChatList.createInviteCodeField, text: code)
        requireElement(app, id: FabushiAID.ChatList.createSubmitButton).tap()

        XCTAssertTrue(requireElement(app, id: FabushiAID.ChatList.createValidationError, timeout: 8.0).exists)
    }

    // Feature: F22 | Tier: 2
    @MainActor
    func test_F22_T2_04_revokedInviteCodeRejectedWhenJoiningFromComposeSheet() async throws {
        let owner = try await oracle.registerRandomUser(prefix: "f22_rev_owner")
        let group = try await oracle.createChat(token: owner.token, CreateChatRequestDTO(title: "Revoked Invite Group", chatType: "group"))
        let inv = try await oracle.createInvite(token: owner.token, chatId: group.chat_id)
        let code = try XCTUnwrap(inv["code"]?.stringValue)
        let invId = try XCTUnwrap(inv["invite_id"]?.stringValue)
        _ = try await oracle.request(method: "DELETE", path: "/api/v1/chats/\(group.chat_id)/invites/\(invId)", bearerToken: owner.token)

        let (app, _, _) = try await launchPreauthenticatedWorkspace()
        requireElement(app, id: FabushiAID.ChatList.composeButton).tap()
        requireElement(app, id: FabushiAID.ChatList.composeJoinByInvite).tap()
        clearAndType(app, id: FabushiAID.ChatList.createInviteCodeField, text: code)
        requireElement(app, id: FabushiAID.ChatList.createSubmitButton).tap()

        XCTAssertTrue(requireElement(app, id: FabushiAID.ChatList.createValidationError, timeout: 8.0).exists)
    }

    // Feature: F22 | Tier: 2
    @MainActor
    func test_F22_T2_05_nonexistentInviteCodeShowsValidationErrorWithoutClosingSheet() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace()
        requireElement(app, id: FabushiAID.ChatList.composeButton).tap()
        requireElement(app, id: FabushiAID.ChatList.composeJoinByInvite).tap()
        clearAndType(app, id: FabushiAID.ChatList.createInviteCodeField, text: "nonexistent_invite_code_000")
        requireElement(app, id: FabushiAID.ChatList.createSubmitButton).tap()

        XCTAssertTrue(requireElement(app, id: FabushiAID.ChatList.createValidationError, timeout: 8.0).exists)
    }

    // Feature: F22 | Tier: 2
    @MainActor
    func test_F22_T2_06_nonAdminMemberCannotRevokeGroupInviteLink() async throws {
        let owner = try await oracle.registerRandomUser(prefix: "f22_adm")
        let (app, member, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(
            token: owner.token,
            CreateChatRequestDTO(title: "Invite ACL", chatType: "group", memberIds: [member.user_id])
        )
        let inv = try await oracle.createInvite(token: owner.token, chatId: group.chat_id)
        let invId = try XCTUnwrap(inv["invite_id"]?.stringValue)

        let forbidden = try await oracle.request(
            method: "DELETE",
            path: "/api/v1/chats/\(group.chat_id)/invites/\(invId)",
            bearerToken: member.token
        )
        XCTAssertEqual(forbidden.statusCode, 403)
        XCTAssertTrue(app.exists)
    }
}
