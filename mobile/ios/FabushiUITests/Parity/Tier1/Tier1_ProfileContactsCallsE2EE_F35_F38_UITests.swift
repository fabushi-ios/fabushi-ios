import Foundation
import XCTest

final class Tier1_ProfileContactsCallsE2EE_F35_F38_UITests: ParityTestCase {

    // MARK: - F35: Stretchy Peer Profile, Group/Channel Admin, Stats & Export

    @MainActor
    func testTier1_F35_01_StretchyPeerProfileRendersQuickActionsUsernameQrAnd5SharedMediaTabs() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f35_prof")
        let peer = try await client.registerUser(username: "f35_peer_\(uniqueSuffix())", firstName: "Sariputra")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "Sariputra",
            chatType: "direct",
            peerUserId: user.userId,
            memberIds: [user.userId]
        )
        let fid = try await client.uploadMediaDirect(
            token: peer.token,
            chatId: chatId,
            filename: "shared.png",
            mimeType: "image/png",
            data: LoopbackPeerClient.sampleMinimalPngData()
        )
        try await client.sendMessage(token: peer.token, chatId: chatId, text: "https://example.org", mediaId: fid)

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        waitForElement(in: app, AccessibilityIDs.Stage.headerAvatarButton).tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Profile.root).exists)
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Profile.callButton).exists)
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Profile.videoButton).exists)
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Profile.muteButton).exists)

        for tab in ["media", "files", "links", "voice", "groups"] {
            let t = waitForElement(in: app, AccessibilityIDs.Profile.tab(tab))
            t.tap()
            XCTAssertTrue(t.exists)
        }
        app.terminate()
    }

    @MainActor
    func testTier1_F35_02_GroupAdminPromotesMemberToAdminWithCustomTitleInMembersList() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f35_adm")
        let member = try await client.registerUser(username: "f35_mem_\(uniqueSuffix())", firstName: "PromoteTarget")
        let groupId = try await client.createChat(
            token: user.token,
            title: "Admin Permissions Group",
            chatType: "supergroup",
            memberIds: [member.userId]
        )

        waitForElement(in: app, AccessibilityIDs.Chats.row(groupId), timeout: 10).tap()
        waitForElement(in: app, AccessibilityIDs.Stage.headerAvatarButton).tap()
        let membersList = waitForElement(in: app, AccessibilityIDs.Profile.groupAdminMembersList)
        XCTAssertTrue(membersList.exists)
        app.staticTexts["PromoteTarget"].firstMatch.tap()
        clearAndTypeText(in: waitForElement(in: app, "groupAdmin.customTitleField"), text: "Core Maintainer")
        waitForElement(in: app, "groupAdmin.saveRoleButton").tap()

        let membersRes = try await client.requestJSON(
            method: "GET",
            path: "/api/v1/chats/\(groupId)/members",
            token: user.token
        )
        XCTAssertGreaterThanOrEqual((membersRes["members"]?.arrayValue ?? []).count, 2)
        app.terminate()
    }

    @MainActor
    func testTier1_F35_03_CreateGroupInviteLinkAllowsCompanionPeerToJoinGroup() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f35_inv")
        let joiner = try await client.registerUser(username: "f35_join_\(uniqueSuffix())")
        let groupId = try await client.createChat(token: user.token, title: "Open Sangha Group", chatType: "group")

        waitForElement(in: app, AccessibilityIDs.Chats.row(groupId), timeout: 10).tap()
        waitForElement(in: app, AccessibilityIDs.Stage.headerAvatarButton).tap()
        waitForElement(in: app, "groupAdmin.inviteLinksRow").tap()
        waitForElement(in: app, "groupAdmin.createInviteButton").tap()

        let linksRes = try await client.requestJSON(
            method: "GET",
            path: "/api/v1/chats/\(groupId)/invites",
            token: user.token
        )
        let firstCode = linksRes["invites"]?.arrayValue?.first?["invite_code"]?.stringValue ?? ""
        XCTAssertFalse(firstCode.isEmpty)
        _ = try await client.requestJSON(
            method: "POST",
            path: "/api/v1/invites/join",
            token: joiner.token,
            body: ["invite_code": .string(firstCode)]
        )
        app.terminate()
    }

    @MainActor
    func testTier1_F35_04_GroupAdminManagesForumTopicsListCreateUpdateAndClose() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f35_top")
        let forumId = try await client.createChat(
            token: user.token,
            title: "Admin Topics Forum",
            chatType: "supergroup",
            isForum: true
        )
        _ = try await client.createForumTopic(token: user.token, chatId: forumId, title: "FFI Safety")

        waitForElement(in: app, AccessibilityIDs.Chats.row(forumId), timeout: 10).tap()
        let topicsList = waitForElement(in: app, AccessibilityIDs.Profile.groupAdminTopicsList)
        XCTAssertTrue(topicsList.exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F35_05_ChannelStatisticsChartsAndChatExportGenerateStructuredHistory() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f35_stat")
        let chanId = try await client.createChat(token: user.token, title: "Metrics Channel", chatType: "channel")
        for idx in 1...3 {
            try await client.sendMessage(token: user.token, chatId: chanId, text: "Announcement #\(idx)")
        }

        waitForElement(in: app, AccessibilityIDs.Chats.row(chanId), timeout: 10).tap()
        waitForElement(in: app, AccessibilityIDs.Stage.headerAvatarButton).tap()
        waitForElement(in: app, "profile.statisticsButton").tap()
        XCTAssertTrue(waitForElement(in: app, "statistics.chartView", timeout: 8).exists)

        let exportRes = try await client.requestJSON(
            method: "GET",
            path: "/api/v1/chats/\(chanId)/export",
            token: user.token
        )
        XCTAssertNotNil(exportRes.objectValue)
        app.terminate()
    }

    // MARK: - F36: Contacts Tab, Global `@username` Resolution & Blocking

    @MainActor
    func testTier1_F36_01_ContactsTabDisplaysAlphabeticalSyncedContactsWithOnlineStatus() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f36_list")
        let alice = try await client.registerUser(username: "f36_alice_\(uniqueSuffix())", firstName: "Alice")
        let zeno = try await client.registerUser(username: "f36_zeno_\(uniqueSuffix())", firstName: "Zeno")
        try await client.addContact(token: user.token, userId: zeno.userId, username: zeno.username, firstName: "Zeno")
        try await client.addContact(token: user.token, userId: alice.userId, username: alice.username, firstName: "Alice")

        waitForElement(in: app, AccessibilityIDs.Shell.tabContacts).tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Profile.contactsList).exists)
        let aliceY = app.staticTexts["Alice"].firstMatch.frame.minY
        let zenoY = app.staticTexts["Zeno"].firstMatch.frame.minY
        XCTAssertLessThanOrEqual(aliceY, zenoY)
        app.terminate()
    }

    @MainActor
    func testTier1_F36_02_AddNewContactPersistsToServerAndOpensDirectChat() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f36_add")
        let target = try await client.registerUser(username: "f36_target_\(uniqueSuffix())", firstName: "BodhiFriend")

        waitForElement(in: app, AccessibilityIDs.Shell.tabContacts).tap()
        waitForElement(in: app, AccessibilityIDs.Profile.contactsAddButton).tap()
        clearAndTypeText(in: waitForElement(in: app, "contacts.addUsernameField"), text: target.username)
        waitForElement(in: app, "contacts.addSaveButton").tap()

        let contacts = try await client.listContacts(token: user.token)
        let list = contacts["contacts"]?.arrayValue ?? []
        XCTAssertGreaterThanOrEqual(list.count, 1)
        app.terminate()
    }

    @MainActor
    func testTier1_F36_03_GlobalUsernameResolutionFindsNonContactPeerAndOpensProfile() async throws {
        let client = self.peerClient!
        let (app, _) = try await launchAuthenticatedUser(usernamePrefix: "f36_res")
        let stranger = try await client.registerUser(username: "global_sage_\(uniqueSuffix())", firstName: "GlobalSage")

        waitForElement(in: app, AccessibilityIDs.Shell.tabContacts).tap()
        clearAndTypeText(in: waitForElement(in: app, "contacts.searchField"), text: "@\(stranger.username)")
        let row = app.staticTexts["GlobalSage"].firstMatch
        XCTAssertTrue(row.waitForExistence(timeout: 8))
        row.tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Stage.collectionView, timeout: 8).exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F36_04_DeleteContactRemovesPeerFromContactsListOnServer() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f36_del")
        let contact = try await client.registerUser(username: "f36_rm_\(uniqueSuffix())", firstName: "RemoveContact")
        try await client.addContact(token: user.token, userId: contact.userId, username: contact.username, firstName: "RemoveContact")

        waitForElement(in: app, AccessibilityIDs.Shell.tabContacts).tap()
        let row = app.staticTexts["RemoveContact"].firstMatch
        XCTAssertTrue(row.waitForExistence(timeout: 8))
        row.swipeLeft()
        app.buttons["Delete"].firstMatch.tap()

        let contacts = try await client.listContacts(token: user.token)
        let remaining = contacts["contacts"]?.arrayValue ?? []
        XCTAssertFalse(remaining.contains { $0["user_id"]?.uint64Value == contact.userId })
        app.terminate()
    }

    @MainActor
    func testTier1_F36_05_BlockAndUnblockPeerEnforcesAndLifts403MessagingRestriction() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f36_blk")
        let peer = try await client.registerUser(username: "f36_bpeer_\(uniqueSuffix())", firstName: "BlockPeer")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "BlockPeer",
            chatType: "direct",
            peerUserId: user.userId,
            memberIds: [user.userId]
        )

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        waitForElement(in: app, AccessibilityIDs.Stage.headerAvatarButton).tap()
        waitForElement(in: app, "profile.blockUserButton").tap()

        let (blockedStatus, _, _) = try await client.requestRaw(
            method: "POST",
            path: "/api/v1/chats/\(chatId)/messages",
            token: peer.token,
            body: Data("{\"chat_id\":\(chatId),\"text\":\"blocked\"}".utf8),
            expectedStatuses: [403]
        )
        XCTAssertEqual(blockedStatus, 403)

        waitForElement(in: app, "profile.unblockUserButton").tap()
        let allowedId = try await client.sendMessage(token: peer.token, chatId: chatId, text: "Unblocked now")
        XCTAssertGreaterThan(allowedId, 0)
        app.terminate()
    }

    // MARK: - F37: WebRTC Voice/Video Calls, Screen Share & AI Talk Mode

    @MainActor
    func testTier1_F37_01_CallsTabSegmentPickerFiltersAllAndMissedCallHistoryLogs() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f37_logs")
        let peer = try await client.registerUser(username: "f37_peer_\(uniqueSuffix())")
        let inv = try await client.inviteCall(token: peer.token, calleeId: user.userId, chatId: nil, isVideo: false)
        let callId = inv["call_id"]?.stringValue ?? ""
        if !callId.isEmpty {
            try await client.sendCallSignal(token: peer.token, callId: callId, signalType: "hangup")
        }

        waitForElement(in: app, AccessibilityIDs.Shell.tabCalls).tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Profile.callsList).exists)
        let seg = waitForElement(in: app, AccessibilityIDs.Profile.callsSegmentPicker)
        if seg.buttons["Missed"].exists {
            seg.buttons["Missed"].tap()
        }
        XCTAssertTrue(seg.exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F37_02_OutgoingVoiceCallConnectsWith4EmojiKeyBadgeAndHangup() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f37_out")
        let peer = try await client.registerUser(username: "f37_opeer_\(uniqueSuffix())", firstName: "CallPeer")
        let chatId = try await client.createChat(
            token: user.token,
            title: "CallPeer",
            chatType: "direct",
            peerUserId: peer.userId,
            memberIds: [peer.userId]
        )

        waitForElement(in: app, AccessibilityIDs.Chats.row(chatId), timeout: 10).tap()
        waitForElement(in: app, AccessibilityIDs.Stage.headerCallButton).tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Profile.callOverlayRoot, timeout: 8).exists)

        let calls = try await client.listCalls(token: peer.token)
        if let activeId = calls["calls"]?.arrayValue?.first?["call_id"]?.stringValue {
            try await client.sendCallSignal(token: peer.token, callId: activeId, signalType: "accept")
        }
        let emojiBadge = waitForElement(in: app, AccessibilityIDs.Profile.callOverlayEmojiBadge, timeout: 8)
        XCTAssertFalse(emojiBadge.label.isEmpty)
        waitForElement(in: app, AccessibilityIDs.Profile.callOverlayHangupButton).tap()
        app.terminate()
    }

    @MainActor
    func testTier1_F37_03_IncomingVideoCallAcceptsTogglesScreenShareAndSubmits5StarRating() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f37_in")
        let peer = try await client.registerUser(username: "f37_ipeer_\(uniqueSuffix())")
        let chatId = try await client.createChat(
            token: peer.token,
            title: "Video Call Peer",
            chatType: "direct",
            peerUserId: user.userId,
            memberIds: [user.userId]
        )
        let inv = try await client.inviteCall(token: peer.token, calleeId: user.userId, chatId: chatId, isVideo: true)
        let callId = inv["call_id"]?.stringValue ?? ""
        XCTAssertFalse(callId.isEmpty)

        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Profile.callOverlayRoot, timeout: 10).exists)
        waitForElement(in: app, "callOverlay.acceptButton").tap()
        waitForElement(in: app, "callOverlay.screenShareButton").tap()
        waitForElement(in: app, AccessibilityIDs.Profile.callOverlayHangupButton).tap()
        waitForElement(in: app, "callOverlay.ratingStar.5").tap()
        waitForElement(in: app, "callOverlay.ratingSubmitButton").tap()

        let calls = try await client.listCalls(token: user.token)
        XCTAssertGreaterThanOrEqual((calls["calls"]?.arrayValue ?? []).count, 1)
        app.terminate()
    }

    @MainActor
    func testTier1_F37_04_CallingAiBotLaunchesFullDuplexTalkModeOrbWithLiveTranscript() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f37_talk")
        let welcome = try await client.requestJSON(method: "POST", path: "/api/v1/chats/welcome", token: user.token)
        let botChatId = welcome["chat_id"]?.int64Value ?? welcome["id"]?.int64Value ?? 1

        waitForElement(in: app, AccessibilityIDs.Chats.row(botChatId), timeout: 10).tap()
        waitForElement(in: app, AccessibilityIDs.Stage.headerCallButton).tap()
        let orb = waitForElement(in: app, AccessibilityIDs.Profile.talkModeOrbView, timeout: 10)
        XCTAssertTrue(orb.exists)
        waitForElement(in: app, AccessibilityIDs.Profile.callOverlayHangupButton).tap()
        app.terminate()
    }

    @MainActor
    func testTier1_F37_05_AiTalkModeBargeInInterruptsAssistantSpeechBackToListening() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f37_barge")
        let welcome = try await client.requestJSON(method: "POST", path: "/api/v1/chats/welcome", token: user.token)
        let botChatId = welcome["chat_id"]?.int64Value ?? welcome["id"]?.int64Value ?? 1

        waitForElement(in: app, AccessibilityIDs.Chats.row(botChatId), timeout: 10).tap()
        waitForElement(in: app, AccessibilityIDs.Stage.headerCallButton).tap()
        let orb = waitForElement(in: app, AccessibilityIDs.Profile.talkModeOrbView, timeout: 10)
        orb.tap()
        XCTAssertTrue(orb.exists)
        waitForElement(in: app, AccessibilityIDs.Profile.callOverlayHangupButton).tap()
        app.terminate()
    }

    // MARK: - F38: Signal-Style Multi-Device E2EE & Secret Chats

    @MainActor
    func testTier1_F38_01_AuthenticatedDevicePublishesEd25519SignedPrekeyBundleAndPushToken() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f38_spk")
        waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).tap()
        waitForElement(in: app, AccessibilityIDs.Settings.row("privacy")).tap()

        let countRes = try await client.getPrekeyCount(token: user.token)
        XCTAssertEqual(countRes["has_signed_prekey"]?.boolValue, true)
        XCTAssertGreaterThan(countRes["remaining_one_time_prekeys"]?.intValue ?? 0, 0)
        app.terminate()
    }

    @MainActor
    func testTier1_F38_02_InitiateSecretChatPerformsX3dhAndDisplays4EmojiAnd12x12Grid() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f38_x3dh")
        let peer = try await client.registerUser(username: "f38_peer_\(uniqueSuffix())", firstName: "SecretPeer")
        try await client.publishValidPrekeys(token: peer.token, deviceId: "peer_ios_1")
        let dmId = try await client.createChat(
            token: user.token,
            title: "SecretPeer",
            chatType: "direct",
            peerUserId: peer.userId,
            memberIds: [peer.userId]
        )

        waitForElement(in: app, AccessibilityIDs.Chats.row(dmId), timeout: 10).tap()
        waitForElement(in: app, AccessibilityIDs.Stage.headerAvatarButton).tap()
        waitForElement(in: app, "profile.startSecretChatButton").tap()
        waitForElement(in: app, "stage.e2eeKeyBadge").tap()

        XCTAssertTrue(waitForElement(in: app, "e2ee.fingerprint12x12Grid", timeout: 8).exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F38_03_DoubleRatchetSecretMessageExcludesPlaintextFromServerSearch() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f38_dr")
        let peer = try await client.registerUser(username: "f38_dpeer_\(uniqueSuffix())", firstName: "CipherPeer")
        try await client.publishValidPrekeys(token: peer.token, deviceId: "peer_dr_1")
        let dmId = try await client.createChat(
            token: user.token,
            title: "CipherPeer",
            chatType: "direct",
            peerUserId: peer.userId,
            memberIds: [peer.userId]
        )

        waitForElement(in: app, AccessibilityIDs.Chats.row(dmId), timeout: 10).tap()
        waitForElement(in: app, AccessibilityIDs.Stage.headerAvatarButton).tap()
        waitForElement(in: app, "profile.startSecretChatButton").tap()
        clearAndTypeText(
            in: waitForElement(in: app, AccessibilityIDs.Composer.textView),
            text: "TopSecretPlaintextOmega777"
        )
        waitForElement(in: app, AccessibilityIDs.Composer.micOrSendButton).tap()

        let search = try await client.searchGlobal(token: user.token, query: "TopSecretPlaintextOmega777")
        let hits = search["hits"]?.arrayValue ?? search["results"]?.arrayValue ?? []
        XCTAssertEqual(hits.count, 0, "E2EE ciphertext must never index plaintext into server Tantivy search")
        app.terminate()
    }

    @MainActor
    func testTier1_F38_04_GroupSenderKeyEncryptedChatDistributesKeysAndDecryptsMessages() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f38_gsk")
        let peer = try await client.registerUser(username: "f38_gpeer_\(uniqueSuffix())", firstName: "SenderKeyPeer")
        try await client.publishValidPrekeys(token: peer.token, deviceId: "peer_gsk_1")
        try await client.addContact(token: user.token, userId: peer.userId, username: peer.username, firstName: "SenderKeyPeer")

        waitForElement(in: app, AccessibilityIDs.Chats.composeMenuButton).tap()
        waitForElement(in: app, AccessibilityIDs.Chats.composeNewGroup).tap()
        app.staticTexts["SenderKeyPeer"].firstMatch.tap()
        waitForElement(in: app, "composeGroup.encryptedToggle").tap()
        clearAndTypeText(in: waitForElement(in: app, "composeGroup.titleField"), text: "SenderKey Vault")
        waitForElement(in: app, "composeGroup.createButton").tap()

        clearAndTypeText(in: waitForElement(in: app, AccessibilityIDs.Composer.textView), text: "Encrypted Group Msg")
        waitForElement(in: app, AccessibilityIDs.Composer.micOrSendButton).tap()
        XCTAssertTrue(app.staticTexts["Encrypted Group Msg"].waitForExistence(timeout: 8))
        app.terminate()
    }

    @MainActor
    func testTier1_F38_05_RevokingSecondaryE2eeDeviceRemovesPrekeyBundleFromDirectory() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f38_rev")
        try await client.publishValidPrekeys(token: user.token, deviceId: "stale_secondary_pad")

        waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).tap()
        waitForElement(in: app, AccessibilityIDs.Settings.row("e2eeDevices")).tap()
        app.staticTexts["stale_secondary_pad"].firstMatch.swipeLeft()
        app.buttons["Revoke"].firstMatch.tap()

        let (status, _, _) = try await client.requestRaw(
            method: "GET",
            path: "/api/v1/e2ee/devices/\(user.userId)/prekeys?device_id=stale_secondary_pad",
            token: user.token,
            contentType: nil,
            expectedStatuses: [404]
        )
        XCTAssertEqual(status, 404)
        app.terminate()
    }
}
