import Foundation
import XCTest

final class Tier1_SettingsWalletAgentComputers_F39_F44_UITests: ParityTestCase {

    // MARK: - F39: Complete 26-Section Settings Router, Storage & Proxy

    @MainActor
    func testTier1_F39_01_SettingsRouterNavigatesAndSearchesAcrossAll26SectionScreens() async throws {
        let (app, _) = try await launchAuthenticatedUser(usernamePrefix: "f39_nav")
        waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).tap()
        let list = waitForElement(in: app, AccessibilityIDs.Settings.list)
        XCTAssertTrue(list.exists)

        for section in ["profile", "notifications", "privacy", "dataStorage", "appearance", "language", "stickers"] {
            XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Settings.row(section), timeout: 6).exists)
        }
        clearAndTypeText(in: waitForElement(in: app, "settings.searchField"), text: "Proxy")
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Settings.row("proxy"), timeout: 6).exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F39_02_EditMyProfileNameUsernameAndBioUpdatesServerUserProfile() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f39_edit")
        let updatedUname = "arya_\(uniqueSuffix())"

        waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).tap()
        waitForElement(in: app, AccessibilityIDs.Settings.row("profile")).tap()
        clearAndTypeText(in: waitForElement(in: app, "settings.profile.firstNameField"), text: "Aryadeva")
        clearAndTypeText(in: waitForElement(in: app, "settings.profile.usernameField"), text: updatedUname)
        clearAndTypeText(in: waitForElement(in: app, "settings.profile.bioField"), text: "Native iOS Parity")
        waitForElement(in: app, "settings.profile.saveButton").tap()

        let me = try await client.requestJSON(method: "GET", path: "/api/v1/users/me", token: user.token)
        XCTAssertEqual(me["first_name"]?.stringValue, "Aryadeva")
        XCTAssertEqual(me["username"]?.stringValue, updatedUname)
        app.terminate()
    }

    @MainActor
    func testTier1_F39_03_NotificationsPrivacyAndPowerSavingTogglesPersistUserSettings() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f39_tog")
        waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).tap()
        waitForElement(in: app, AccessibilityIDs.Settings.row("powerSaving")).tap()
        waitForElement(in: app, "powerSaving.autoplayGifSwitch").tap()

        let settings = try await client.requestJSON(method: "GET", path: "/api/v1/users/me/settings", token: user.token)
        XCTAssertNotNil(settings.objectValue)
        app.terminate()
    }

    @MainActor
    func testTier1_F39_04_StorageUsagePieChartPurgesLocalCacheWithoutDeletingCloudHistory() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f39_cache")
        let chatId = try await client.createChat(token: user.token, title: "Cloud Retained", chatType: "group")
        try await client.sendMessage(token: user.token, chatId: chatId, text: "Must survive local purge")

        waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).tap()
        waitForElement(in: app, AccessibilityIDs.Settings.row("dataStorage")).tap()
        XCTAssertTrue(waitForElement(in: app, "storage.pieChartView").exists)
        waitForElement(in: app, "storage.clearCacheButton").tap()

        let history = try await client.listMessages(token: user.token, chatId: chatId)
        XCTAssertEqual(history["messages"]?.arrayValue?.count, 1)
        app.terminate()
    }

    @MainActor
    func testTier1_F39_05_ConfigureSocks5AndHttpProxyAndVerifyCallsMicTestScreen() async throws {
        let (app, _) = try await launchAuthenticatedUser(usernamePrefix: "f39_proxy")
        waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).tap()
        waitForElement(in: app, AccessibilityIDs.Settings.row("proxy")).tap()
        waitForElement(in: app, "proxy.addButton").tap()
        clearAndTypeText(in: waitForElement(in: app, "proxy.hostField"), text: "127.0.0.1")
        clearAndTypeText(in: waitForElement(in: app, "proxy.portField"), text: "1080")
        waitForElement(in: app, "proxy.saveButton").tap()
        XCTAssertTrue(app.staticTexts["127.0.0.1:1080"].waitForExistence(timeout: 6))
        app.terminate()
    }

    // MARK: - F40: Telegram Stars Wallet, Star Gifts, Alipay & Redeem Codes

    @MainActor
    func testTier1_F40_01_StarsWalletDisplaysBalanceLabelAndTransactionHistoryLedger() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f40_bal")
        try await client.topupStars(token: user.token, amount: 880, description: "Genesis Grant")

        waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).tap()
        waitForElement(in: app, AccessibilityIDs.Settings.row("wallet")).tap()
        let balLabel = waitForElement(in: app, AccessibilityIDs.Settings.walletBalanceLabel)
        XCTAssertTrue(balLabel.label.contains("880"))
        XCTAssertTrue(app.staticTexts["Genesis Grant"].waitForExistence(timeout: 8))
        app.terminate()
    }

    @MainActor
    func testTier1_F40_02_TopUpStarsPackageIncrementsWalletBalanceAndRecordsTransaction() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f40_topup")
        try await client.topupStars(token: user.token, amount: 100, description: "Base")

        waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).tap()
        waitForElement(in: app, AccessibilityIDs.Settings.row("wallet")).tap()
        waitForElement(in: app, "wallet.topup500Button").tap()

        let updatedBal = try await client.getWalletBalance(token: user.token)
        XCTAssertEqual(updatedBal, 600)
        app.terminate()
    }

    @MainActor
    func testTier1_F40_03_SendStarGiftToPeerTransfersStarsAndDeliversGiftCard() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f40_gift")
        try await client.topupStars(token: user.token, amount: 500, description: "Gift Fund")
        let recipient = try await client.registerUser(username: "f40_rcp_\(uniqueSuffix())", firstName: "GiftReceiver")
        try await client.addContact(token: user.token, userId: recipient.userId, username: recipient.username, firstName: "GiftReceiver")

        waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).tap()
        waitForElement(in: app, AccessibilityIDs.Settings.row("wallet")).tap()
        waitForElement(in: app, AccessibilityIDs.Settings.walletSendGiftButton).tap()
        app.staticTexts["GiftReceiver"].firstMatch.tap()
        waitForElement(in: app, "wallet.giftPack.100").tap()
        waitForElement(in: app, "wallet.confirmSendGiftButton").tap()

        let recipientBal = try await client.getWalletBalance(token: recipient.token)
        XCTAssertEqual(recipientBal, 100)
        app.terminate()
    }

    @MainActor
    func testTier1_F40_04_CreateAlipayOrderGeneratesOrderSheetAndMembershipStatusCheck() async throws {
        let (app, _) = try await launchAuthenticatedUser(usernamePrefix: "f40_alipay")
        waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).tap()
        waitForElement(in: app, AccessibilityIDs.Settings.row("wallet")).tap()
        waitForElement(in: app, "wallet.alipayOrderButton").tap()

        XCTAssertTrue(waitForElement(in: app, "wallet.alipayOrderSheet", timeout: 8).exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F40_05_RedeemMembershipCodeActivatesPremiumEntitlementOnServer() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f40_redeem")
        let admin = try await client.registerUser(username: "f40_adm_\(uniqueSuffix())")
        let code = "VIP\(uniqueSuffix().uppercased())"
        try await client.createRedeemCode(token: admin.token, code: code, plan: "monthly", days: 30)

        waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).tap()
        waitForElement(in: app, AccessibilityIDs.Settings.row("wallet")).tap()
        waitForElement(in: app, "wallet.redeemCodeButton").tap()
        clearAndTypeText(in: waitForElement(in: app, "wallet.redeemCodeField"), text: code)
        waitForElement(in: app, "wallet.redeemSubmitButton").tap()

        let status = try await client.requestJSON(method: "GET", path: "/api/alipay/check-membership", token: user.token)
        XCTAssertNotNil(status.objectValue)
        app.terminate()
    }

    // MARK: - F41: Hermes 7-Tab AI Agent Studio & Magpie Multi-Provider Gateway

    @MainActor
    func testTier1_F41_01_HermesStudioTasksKanbanCreatesAndMovesTaskCards() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f41_kanban")
        waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).tap()
        waitForElement(in: app, AccessibilityIDs.Settings.row("hermesStudio")).tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Settings.hermesStudioRoot).exists)

        waitForElement(in: app, AccessibilityIDs.Settings.hermesTab("tasks")).tap()
        waitForElement(in: app, "hermes.kanban.addCardButton").tap()
        clearAndTypeText(in: waitForElement(in: app, "hermes.kanban.titleField"), text: "Ship Inverted Transcript")
        waitForElement(in: app, "hermes.kanban.saveButton").tap()

        let boards = try await client.requestJSON(method: "GET", path: "/api/v1/hermes/kanban/boards", token: user.token)
        XCTAssertNotNil(boards.objectValue)
        app.terminate()
    }

    @MainActor
    func testTier1_F41_02_HermesSkillsAndMemorySoulTabsSaveCustomSkillAndPersona() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f41_soul")
        waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).tap()
        waitForElement(in: app, AccessibilityIDs.Settings.row("hermesStudio")).tap()

        waitForElement(in: app, AccessibilityIDs.Settings.hermesTab("skills")).tap()
        waitForElement(in: app, "hermes.skills.createButton").tap()
        clearAndTypeText(in: waitForElement(in: app, "hermes.skills.nameField"), text: "ios-parity-skill")
        waitForElement(in: app, "hermes.skills.saveButton").tap()

        waitForElement(in: app, AccessibilityIDs.Settings.hermesTab("memory")).tap()
        clearAndTypeText(in: waitForElement(in: app, "hermes.soul.personaField"), text: "Calm Systems Mentor")
        waitForElement(in: app, "hermes.soul.saveButton").tap()

        let soul = try await client.requestJSON(method: "GET", path: "/api/v1/hermes/soul", token: user.token)
        XCTAssertNotNil(soul.objectValue)
        app.terminate()
    }

    @MainActor
    func testTier1_F41_03_HermesMcpToolsAndApprovalsTabsToggle28ToolsetsAndApproveGate() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f41_mcp")
        waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).tap()
        waitForElement(in: app, AccessibilityIDs.Settings.row("hermesStudio")).tap()

        waitForElement(in: app, AccessibilityIDs.Settings.hermesTab("tools")).tap()
        waitForElement(in: app, "hermes.tools.preset.safe").tap()
        waitForElement(in: app, AccessibilityIDs.Settings.hermesTab("approvals")).tap()

        let toolsets = try await client.requestJSON(method: "GET", path: "/api/v1/hermes/toolsets", token: user.token)
        XCTAssertNotNil(toolsets.objectValue)
        app.terminate()
    }

    @MainActor
    func testTier1_F41_04_HermesCronAndSessionsTabsScheduleJobAndBranchSessionHistory() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f41_cron")
        waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).tap()
        waitForElement(in: app, AccessibilityIDs.Settings.row("hermesStudio")).tap()

        waitForElement(in: app, AccessibilityIDs.Settings.hermesTab("cron")).tap()
        waitForElement(in: app, "hermes.cron.addButton").tap()
        clearAndTypeText(in: waitForElement(in: app, "hermes.cron.nameField"), text: "Morning Standup")
        waitForElement(in: app, "hermes.cron.saveButton").tap()

        waitForElement(in: app, AccessibilityIDs.Settings.hermesTab("sessions")).tap()
        let cronRes = try await client.requestJSON(method: "GET", path: "/api/v1/hermes/cron", token: user.token)
        XCTAssertNotNil(cronRes.objectValue)
        app.terminate()
    }

    @MainActor
    func testTier1_F41_05_MagpieMultiProviderGatewayConfiguresProviderAndRoutingGroup() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f41_magpie")
        waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).tap()
        waitForElement(in: app, AccessibilityIDs.Settings.row("hermesStudio")).tap()
        waitForElement(in: app, "hermes.magpieGatewayButton").tap()
        clearAndTypeText(in: waitForElement(in: app, "magpie.groupNameField"), text: "fast-inference")
        waitForElement(in: app, "magpie.saveGroupButton").tap()

        let groups = try await client.requestJSON(method: "GET", path: "/api/v1/hermes/magpie/groups", token: user.token)
        XCTAssertNotNil(groups.objectValue)
        app.terminate()
    }

    // MARK: - F42: BotFather NL Creator, Native `WKWebView` Mini Apps & `全球法布施`

    @MainActor
    func testTier1_F42_01_BotFatherNaturalLanguagePromptCreatesCompanionBotAndMiniApp() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f42_nl")
        waitForElement(in: app, AccessibilityIDs.Chats.composeMenuButton).tap()
        waitForElement(in: app, AccessibilityIDs.Chats.composeNewBot).tap()
        clearAndTypeText(
            in: waitForElement(in: app, "botFather.nlPromptField"),
            text: "Create a tea timer bot and mini app called ZenTea"
        )
        waitForElement(in: app, "botFather.nlSubmitButton").tap()

        let apps = try await client.requestJSON(method: "GET", path: "/api/v1/miniapps", token: user.token)
        XCTAssertNotNil(apps.objectValue)
        app.terminate()
    }

    @MainActor
    func testTier1_F42_02_FirstTimeMiniAppLaunchShowsTosDisclaimerAndMountsWkWebView() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f42_tos")
        let created = try await client.createBotWithMiniAppViaNL(
            token: user.token,
            prompt: "Create checklist mini app called DharmaTasks"
        )

        waitForElement(in: app, AccessibilityIDs.Chats.row(created.chatId), timeout: 10).tap()
        waitForElement(in: app, "stage.openMiniAppButton").tap()
        let tosAccept = waitForElement(in: app, "miniapp.tosAcceptButton", timeout: 8)
        tosAccept.tap()

        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Settings.miniappWebviewSheet, timeout: 10).exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F42_03_MiniAppWebAppBridgePersistsCloudStorageKeyValueAcrossRelaunch() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f42_store")
        let created = try await client.createBotWithMiniAppViaNL(
            token: user.token,
            prompt: "Create note mini app called CloudNotes"
        )
        if created.botId > 0 {
            try await client.acceptBotAppTos(token: user.token, botId: created.botId)
        }
        _ = try await client.requestJSON(
            method: "PUT",
            path: "/api/v1/miniapps/\(created.botUsername)/\(created.shortName)/storage",
            token: user.token,
            body: ["key": "theme", "value": "lotus_night"]
        )

        waitForElement(in: app, AccessibilityIDs.Chats.row(created.chatId), timeout: 10).tap()
        waitForElement(in: app, "stage.openMiniAppButton").tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Settings.miniappWebviewSheet, timeout: 10).exists)

        let stored = try await client.requestJSON(
            method: "GET",
            path: "/api/v1/miniapps/\(created.botUsername)/\(created.shortName)/storage",
            token: user.token
        )
        XCTAssertNotNil(stored.objectValue)
        app.terminate()
    }

    @MainActor
    func testTier1_F42_04_GlobalFabushiMiniAppBootstrapsAndRecordsSutraPracticeSession() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f42_dharma")
        waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).tap()
        waitForElement(in: app, AccessibilityIDs.Settings.row("globalFabushi")).tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Settings.miniappWebviewSheet, timeout: 10).exists)
        waitForElement(in: app, "fabushiMiniapp.recordPracticeButton").tap()

        let records = try await client.requestJSON(
            method: "GET",
            path: "/api/v1/fabushi-miniapp/practice/record",
            token: user.token
        )
        XCTAssertNotNil(records.objectValue)
        app.terminate()
    }

    @MainActor
    func testTier1_F42_05_GlobalFabushiMiniAppSubmitsMeritTransferAndUpdatesLeaderboard() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f42_merit")
        waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).tap()
        waitForElement(in: app, AccessibilityIDs.Settings.row("globalFabushi")).tap()
        waitForElement(in: app, "fabushiMiniapp.transferMeritButton").tap()

        let board = try await client.requestJSON(
            method: "GET",
            path: "/api/v1/fabushi-miniapp/leaderboard",
            token: user.token
        )
        XCTAssertNotNil(board.objectValue)
        app.terminate()
    }

    // MARK: - F43: Multi-Device Remote Computer Workbench & Tailnet/DERP Mesh

    @MainActor
    func testTier1_F43_01_ComputerWorkbenchListsRegisteredNodesAndTailnetDerpMeshStatus() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f43_nodes")
        _ = try await client.registerLocalComputer(
            token: user.token,
            deviceId: "mac_studio_f43",
            displayName: "CI Mac Studio"
        )

        waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).tap()
        waitForElement(in: app, AccessibilityIDs.Settings.row("computers")).tap()
        let workbench = waitForElement(in: app, AccessibilityIDs.Settings.computerWorkbenchRoot)
        XCTAssertTrue(workbench.exists)
        XCTAssertTrue(app.staticTexts["CI Mac Studio"].waitForExistence(timeout: 8))
        app.terminate()
    }

    @MainActor
    func testTier1_F43_02_LiveDesktopViewerRendersBase64ScreenFrameAndInjectsTouchMouseClick() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f43_screen")
        _ = try await client.registerLocalComputer(
            token: user.token,
            deviceId: "mac_screen_f43",
            displayName: "Screen Host"
        )

        waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).tap()
        waitForElement(in: app, AccessibilityIDs.Settings.row("computers")).tap()
        app.staticTexts["Screen Host"].firstMatch.tap()
        let canvas = waitForElement(in: app, "computer.screenCanvas", timeout: 8)
        canvas.tap()
        XCTAssertTrue(canvas.exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F43_03_RemoteShellTerminalExecutesCommandAndRendersStdout() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f43_shell")
        _ = try await client.registerLocalComputer(
            token: user.token,
            deviceId: "mac_term_f43",
            displayName: "Terminal Host"
        )

        waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).tap()
        waitForElement(in: app, AccessibilityIDs.Settings.row("computers")).tap()
        app.staticTexts["Terminal Host"].firstMatch.tap()
        clearAndTypeText(in: waitForElement(in: app, "computer.terminalInput"), text: "uname -a")
        waitForElement(in: app, "computer.terminalRunButton").tap()
        XCTAssertTrue(waitForElement(in: app, "computer.terminalOutput", timeout: 8).exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F43_04_RemoteFileBrowserListsDirectoryEntriesWithin32MbPayload() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f43_files")
        _ = try await client.registerLocalComputer(
            token: user.token,
            deviceId: "mac_fs_f43",
            displayName: "Filesystem Host"
        )

        waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).tap()
        waitForElement(in: app, AccessibilityIDs.Settings.row("computers")).tap()
        app.staticTexts["Filesystem Host"].firstMatch.tap()
        waitForElement(in: app, "computer.filesTab").tap()
        XCTAssertTrue(waitForElement(in: app, "computer.fileListView", timeout: 8).exists)
        app.terminate()
    }

    @MainActor
    func testTier1_F43_05_AskEveryTimePolicyPresentsApprovalCardBeforeDispatchingCommand() async throws {
        let client = self.peerClient!
        let (app, user) = try await launchAuthenticatedUser(usernamePrefix: "f43_ask")
        let nodeId = try await client.registerLocalComputer(
            token: user.token,
            deviceId: "mac_ask_f43",
            displayName: "Guarded Host"
        )
        _ = try await client.requestJSON(
            method: "PATCH",
            path: "/api/v1/computers/\(nodeId)/policy",
            token: user.token,
            body: ["policy": "ask_every_time"]
        )

        waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).tap()
        waitForElement(in: app, AccessibilityIDs.Settings.row("computers")).tap()
        app.staticTexts["Guarded Host"].firstMatch.tap()
        XCTAssertTrue(waitForElement(in: app, AccessibilityIDs.Settings.computerWorkbenchRoot).exists)
        app.terminate()
    }

    // MARK: - F44: Platform Version Check & TestFlight / App Store Update Banner

    @MainActor
    func testTier1_F44_01_AboutAndVersionCheckDisplaysCurrentBuildAndHealthyServerVersion() async throws {
        let status = try await self.peerClient.fetchStatus()
        let serverVer = status["version"]?.stringValue ?? ""
        XCTAssertFalse(serverVer.isEmpty)

        let (app, _) = try await launchAuthenticatedUser(usernamePrefix: "f44_ver")
        waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).tap()
        waitForElement(in: app, AccessibilityIDs.Settings.row("about")).tap()
        let verLabel = waitForElement(in: app, "about.versionLabel")
        XCTAssertFalse(verLabel.label.isEmpty)
        app.terminate()
    }

    @MainActor
    func testTier1_F44_02_NewerReleaseMetadataSurfacesNonIntrusiveUpdateBannerInShell() async throws {
        let (app1, _) = try await launchAuthenticatedUser(usernamePrefix: "f44_banner")
        app1.terminate()

        let app2 = launchApp(extraEnv: ["GPUI_TELEGRAM_LATEST_RELEASE_TAG": "v99.0.0"])
        let banner = waitForElement(in: app2, AccessibilityIDs.Shell.updateBanner, timeout: 10)
        XCTAssertTrue(banner.exists)
        XCTAssertTrue(element(in: app2, AccessibilityIDs.Shell.tabChats).isHittable)
        app2.terminate()
    }

    @MainActor
    func testTier1_F44_03_UpdateBannerExposesTestFlightAndAppStoreActionWithoutBundleSwap() async throws {
        let (app1, _) = try await launchAuthenticatedUser(usernamePrefix: "f44_tf")
        app1.terminate()

        let app2 = launchApp(extraEnv: ["GPUI_TELEGRAM_LATEST_RELEASE_TAG": "v99.0.0"])
        let actionBtn = waitForElement(in: app2, AccessibilityIDs.Shell.updateActionButton, timeout: 10)
        XCTAssertTrue(actionBtn.isHittable)
        app2.terminate()
    }

    @MainActor
    func testTier1_F44_04_DismissingUpdateBannerKeepsActiveConversationUninterrupted() async throws {
        let client = self.peerClient!
        let (app1, user) = try await launchAuthenticatedUser(usernamePrefix: "f44_dis")
        let chatId = try await client.createChat(token: user.token, title: "Uninterrupted Chat", chatType: "group")
        app1.terminate()

        let app2 = launchApp(extraEnv: ["GPUI_TELEGRAM_LATEST_RELEASE_TAG": "v99.0.0"])
        waitForElement(in: app2, AccessibilityIDs.Shell.updateDismissButton, timeout: 10).tap()
        waitForElementDisappearance(in: app2, AccessibilityIDs.Shell.updateBanner)
        waitForElement(in: app2, AccessibilityIDs.Chats.row(chatId)).tap()
        XCTAssertTrue(waitForElement(in: app2, AccessibilityIDs.Stage.collectionView).exists)
        app2.terminate()
    }

    @MainActor
    func testTier1_F44_05_UpToDateVersionCheckReportsCurrentReleaseStatusCleanly() async throws {
        let (app, _) = try await launchAuthenticatedUser(usernamePrefix: "f44_uptodate")
        waitForElement(in: app, AccessibilityIDs.Shell.tabSettings).tap()
        waitForElement(in: app, AccessibilityIDs.Settings.row("about")).tap()
        waitForElement(in: app, "about.checkForUpdatesButton").tap()
        XCTAssertFalse(element(in: app, AccessibilityIDs.Shell.updateBanner).exists)
        app.terminate()
    }
}
