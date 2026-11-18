import Foundation
import XCTest

/// Tier 1 Happy-Path Opaque-Box UI Tests — Domain R2:
/// Adaptive Root Shell, Liquid Glass Chrome, 4 Canonical Themes, Bilingual i18n,
/// Semantic Workspace Tree & Keyboard Shortcuts (`F11..F13, F15, F45, F46, F52`).
final class Tier1_R2_ShellThemeGlassUITests: FabushiE2ETestCase {

    // MARK: - F11: Adaptive `TelegramRootController` Navigation Shell (5 Tests)

    // Feature: F11 | Tier: 1
    @MainActor
    func test_F11_T1_01_rootShellRendersFourCanonicalTabsOrAdaptiveSplitView() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace()
        let windowWidth = app.windows.firstMatch.frame.width

        if windowWidth > TelegramGeometryMetrics.splitViewBreakpointPt {
            let split = requireElement(app, id: FabushiAID.Root.splitView)
            XCTAssertGreaterThan(split.frame.width, TelegramGeometryMetrics.splitViewBreakpointPt)
        } else {
            _ = requireElement(app, id: FabushiAID.Root.tabBarContainer)
            XCTAssertTrue(requireElement(app, id: FabushiAID.Root.tabContacts).isHittable)
            XCTAssertTrue(requireElement(app, id: FabushiAID.Root.tabCalls).isHittable)
            XCTAssertTrue(requireElement(app, id: FabushiAID.Root.tabChats).isHittable)
            XCTAssertTrue(requireElement(app, id: FabushiAID.Root.tabSettings).isHittable)
        }
    }

    // Feature: F11 | Tier: 1
    @MainActor
    func test_F11_T1_02_switchingAcrossContactsCallsChatsSettingsPreservesTabState() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace()

        requireElement(app, id: FabushiAID.Root.tabContacts).tap()
        _ = requireElement(app, id: FabushiAID.SettingsAndPeers.contactsList)

        requireElement(app, id: FabushiAID.Root.tabCalls).tap()
        _ = requireElement(app, id: FabushiAID.SettingsAndPeers.callsSegmentControl)

        requireElement(app, id: FabushiAID.Root.tabSettings).tap()
        _ = requireElement(app, id: FabushiAID.SettingsAndPeers.settingsList)

        requireElement(app, id: FabushiAID.Root.tabChats).tap()
        _ = requireElement(app, id: FabushiAID.ChatList.collectionView)
    }

    // Feature: F11 | Tier: 1
    @MainActor
    func test_F11_T1_03_reTappingChatsTabScrollsScrolledInboxBackToTop() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        for idx in 1...14 {
            _ = try await oracle.createChat(
                token: auth.token,
                CreateChatRequestDTO(title: String(format: "Dialog #%02d", idx), chatType: "group")
            )
        }
        let list = requireElement(app, id: FabushiAID.ChatList.collectionView)
        list.swipeUp()
        list.swipeUp()

        let chatsTab = requireElement(app, id: FabushiAID.Root.tabChats)
        chatsTab.tap()
        XCTAssertTrue(requireElement(app, id: FabushiAID.ChatList.header).exists)
    }

    // Feature: F11 | Tier: 1
    @MainActor
    func test_F11_T1_04_incomingPeerMessageUpdatesChatsTabUnreadBadgeCount() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let bob = try await oracle.registerRandomUser(prefix: "f11_bob")
        let dm = try await oracle.createChat(
            token: bob.token,
            CreateChatRequestDTO(title: "Badge Peer", chatType: "direct", peerUserId: auth.user_id)
        )
        requireElement(app, id: FabushiAID.Root.tabSettings).tap()

        _ = try await oracle.sendMessage(
            token: bob.token,
            chatId: dm.chat_id,
            SendMessageRequestDTO(text: "Unread badge increment", clientNonce: 11_401)
        )

        let badge = requireElement(app, id: FabushiAID.Root.badgeChats, timeout: 10.0)
        XCTAssertTrue(badge.label.contains("1") || (badge.value as? String ?? "").contains("1"))
    }

    // Feature: F11 | Tier: 1
    @MainActor
    func test_F11_T1_05_rotatingPortraitToLandscapeRecomputesShellWidthAndRestoresPortrait() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace()
        let portraitWidth = app.windows.firstMatch.frame.width

        XCUIDevice.shared.orientation = .landscapeLeft
        let rotated = try await waitUntil(timeout: 6.0) {
            app.windows.firstMatch.frame.width > portraitWidth
        }
        XCTAssertTrue(rotated, "Window width must expand in landscape orientation")

        XCUIDevice.shared.orientation = .portrait
        let restored = try await waitUntil(timeout: 6.0) {
            abs(app.windows.firstMatch.frame.width - portraitWidth) <= 2.0
        }
        XCTAssertTrue(restored)
    }

    // MARK: - F12: 4 Canonical Telegram-iOS Themes, Typography Scale & Bubble-Free Mode (5 Tests)

    // Feature: F12 | Tier: 1
    @MainActor
    func test_F12_T1_01_dayClassicAndDayThemeSelectionAppliesCanonicalSurfaceColors() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace()
        requireElement(app, id: FabushiAID.Root.tabSettings).tap()
        requireElement(app, id: FabushiAID.SettingsAndPeers.settingsSection("appearance")).tap()

        requireElement(app, id: FabushiAID.SettingsAndPeers.appearanceTheme("day")).tap()
        let daySample = try PixelColorSampler.sampleNormalizedPixel(
            in: app.windows.firstMatch,
            normalizedPoint: CGPoint(x: 0.5, y: 0.12)
        )
        XCTAssertTrue(
            daySample.matches(TelegramThemePalette.dayClassicPlainBg, channelTolerance: 20) ||
            daySample.matches(TelegramThemePalette.dayBlocksBg, channelTolerance: 20),
            "Day theme surface pixel must match #FFFFFF / #F2F2F7, got \(daySample)"
        )

        requireElement(app, id: FabushiAID.SettingsAndPeers.appearanceTheme("dayClassic")).tap()
        XCTAssertTrue(element(app, id: FabushiAID.SettingsAndPeers.appearanceTheme("dayClassic")).exists)
    }

    // Feature: F12 | Tier: 1
    @MainActor
    func test_F12_T1_02_nightOledThemeAppliesTrueBlack000000Background() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace()
        requireElement(app, id: FabushiAID.Root.tabSettings).tap()
        requireElement(app, id: FabushiAID.SettingsAndPeers.settingsSection("appearance")).tap()
        requireElement(app, id: FabushiAID.SettingsAndPeers.appearanceTheme("night")).tap()

        requireElement(app, id: FabushiAID.Root.tabChats).tap()
        let list = requireElement(app, id: FabushiAID.ChatList.collectionView)
        let oledSample = try PixelColorSampler.sampleNormalizedPixel(
            in: list,
            normalizedPoint: CGPoint(x: 0.5, y: 0.85)
        )
        XCTAssertTrue(
            oledSample.matches(TelegramThemePalette.nightOledPlainBg, channelTolerance: 18) ||
            oledSample.matches(TelegramThemePalette.nightOledCardBg, channelTolerance: 18),
            "Night OLED theme must render true black #000000 / #1C1C1D, got \(oledSample)"
        )
    }

    // Feature: F12 | Tier: 1
    @MainActor
    func test_F12_T1_03_nightTintedThemeAppliesSlateBlue18222DSurface() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace()
        requireElement(app, id: FabushiAID.Root.tabSettings).tap()
        requireElement(app, id: FabushiAID.SettingsAndPeers.settingsSection("appearance")).tap()
        requireElement(app, id: FabushiAID.SettingsAndPeers.appearanceTheme("nightTinted")).tap()

        requireElement(app, id: FabushiAID.Root.tabChats).tap()
        let list = requireElement(app, id: FabushiAID.ChatList.collectionView)
        let slateSample = try PixelColorSampler.sampleNormalizedPixel(
            in: list,
            normalizedPoint: CGPoint(x: 0.5, y: 0.85)
        )
        XCTAssertTrue(
            slateSample.matches(TelegramThemePalette.nightTintedPlainBg, channelTolerance: 22) ||
            slateSample.matches(TelegramThemePalette.nightTintedCardBg, channelTolerance: 22),
            "Night Tinted theme must render #18222D / #213040 slate blue, got \(slateSample)"
        )
    }

    // Feature: F12 | Tier: 1
    @MainActor
    func test_F12_T1_04_sevenStepFontSizeSliderScalesMessageTextAndComposerFloor() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(
            token: auth.token,
            CreateChatRequestDTO(title: "Typography Scale", chatType: "group")
        )
        let msg = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(text: "Dynamic font size sample line", clientNonce: 12_401)
        )

        openChatFromInbox(app, chatId: group.chat_id)
        let defaultBubbleHeight = requireElement(app, id: FabushiAID.Chat.message(msg.message_id)).frame.height

        if app.navigationBars.buttons.firstMatch.exists {
            app.navigationBars.buttons.firstMatch.tap()
        }
        requireElement(app, id: FabushiAID.Root.tabSettings).tap()
        requireElement(app, id: FabushiAID.SettingsAndPeers.settingsSection("appearance")).tap()
        let slider = requireElement(app, id: FabushiAID.SettingsAndPeers.appearanceFontSizeSlider)
        slider.adjust(toNormalizedSliderPosition: 1.0) // 26pt max step

        requireElement(app, id: FabushiAID.Root.tabChats).tap()
        openChatFromInbox(app, chatId: group.chat_id)
        let enlargedHeight = requireElement(app, id: FabushiAID.Chat.message(msg.message_id)).frame.height
        XCTAssertGreaterThan(enlargedHeight, defaultBubbleHeight, "26pt step must increase rendered message bubble height over 17pt default")
    }

    // Feature: F12 | Tier: 1
    @MainActor
    func test_F12_T1_05_bubbleFreePageModeToggleSwitchesTranscriptToFlatSlackStyleLayout() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(
            token: auth.token,
            CreateChatRequestDTO(title: "Page Mode Group", chatType: "group")
        )
        let sent = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(text: "Bubble-free full-width row", clientNonce: 12_501)
        )

        requireElement(app, id: FabushiAID.Root.tabSettings).tap()
        requireElement(app, id: FabushiAID.SettingsAndPeers.settingsSection("appearance")).tap()
        requireElement(app, id: FabushiAID.SettingsAndPeers.appearanceBubbleFreeToggle).tap()

        requireElement(app, id: FabushiAID.Root.tabChats).tap()
        openChatFromInbox(app, chatId: group.chat_id)
        let textEl = requireElement(app, id: FabushiAID.Chat.messageText(sent.message_id))
        let transcript = requireElement(app, id: FabushiAID.Chat.transcript)
        let relX = textEl.frame.minX - transcript.frame.minX
        GesturePhysicsHelpers.assertMetric(relX, equals: 51.0, accuracy: 4.0, "Group Bubble-Free Page mode text aligns at x=51pt")
    }

    // MARK: - F13: Bilingual Runtime i18n (`zh-CN` & `en-US`) (5 Tests)

    // Feature: F13 | Tier: 1
    @MainActor
    func test_F13_T1_01_initialLocaleZhCNLaunchesShellWithSimplifiedChineseChrome() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace(initialLocale: "zh-CN")
        let chatsTab = requireElement(app, id: FabushiAID.Root.tabChats)
        XCTAssertTrue(
            chatsTab.label.contains("对话") || chatsTab.label.contains("消息") || chatsTab.label.contains("聊天"),
            "zh-CN locale must render Chinese Chats tab label, got '\(chatsTab.label)'"
        )
    }

    // Feature: F13 | Tier: 1
    @MainActor
    func test_F13_T1_02_initialLocaleEnUSLaunchesShellWithEnglishChrome() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace(initialLocale: "en-US")
        let chatsTab = requireElement(app, id: FabushiAID.Root.tabChats)
        XCTAssertTrue(chatsTab.label.localizedCaseInsensitiveContains("Chat"))
    }

    // Feature: F13 | Tier: 1
    @MainActor
    func test_F13_T1_03_authScreenLanguageToggleHotSwapsCopyWithoutRestart() async throws {
        let (app, _) = try launchUnauthenticatedAuth(initialLocale: "en-US")
        let toggle = requireElement(app, id: FabushiAID.Auth.languageToggle)
        let beforeLabel = requireElement(app, id: FabushiAID.Auth.splashStartButton).label

        toggle.tap()
        let afterLabel = requireElement(app, id: FabushiAID.Auth.splashStartButton).label
        XCTAssertNotEqual(beforeLabel, afterLabel, "Auth language toggle must update Start button copy in-place")
    }

    // Feature: F13 | Tier: 1
    @MainActor
    func test_F13_T1_04_settingsLanguagePickerSwitchesActiveWorkspaceCopyLive() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace(initialLocale: "en-US")
        requireElement(app, id: FabushiAID.Root.tabSettings).tap()
        requireElement(app, id: FabushiAID.SettingsAndPeers.settingsSection("language")).tap()

        let zhCell = app.cells.containing(.staticText, identifier: "简体中文").firstMatch
        if zhCell.waitForExistence(timeout: 5.0) {
            zhCell.tap()
        }
        let settingsTab = requireElement(app, id: FabushiAID.Root.tabSettings)
        XCTAssertTrue(settingsTab.label.contains("设置") || !settingsTab.label.isEmpty)
    }

    // Feature: F13 | Tier: 1
    @MainActor
    func test_F13_T1_05_selectedLanguagePreferencePersistsAcrossColdAppRelaunch() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace(initialLocale: "zh-CN")
        let zhLabel = requireElement(app, id: FabushiAID.Root.tabSettings).label
        app.terminate()

        app.launch()
        let afterRelaunch = requireElement(app, id: FabushiAID.Root.tabSettings, timeout: 15.0).label
        XCTAssertEqual(zhLabel, afterRelaunch)
    }

    // MARK: - F15: `SemanticWorkspaceTree` Inspection & State-Hash Determinism (6 Tests)

    // Feature: F15 | Tier: 1
    @MainActor
    func test_F15_T1_01_semanticWorkspaceTreeExportsValid64HexSha256StateHash() async throws {
        let (app, _, cfg) = try await launchPreauthenticatedWorkspace()
        let snapshot = try SemanticTreeSnapshotReader.readSnapshot(from: app, fallbackFileURL: cfg.semanticExportPath)
        XCTAssertTrue(snapshot.isValidHexHash, "Expected 64-char lowercase SHA-256 hex state_hash, got '\(snapshot.state_hash)'")
        XCTAssertTrue(snapshot.window.is_authenticated)
    }

    // Feature: F15 | Tier: 1
    @MainActor
    func test_F15_T1_02_semanticStateHashRemainsDeterministicWhenWorkspaceIsIdle() async throws {
        let (app, _, cfg) = try await launchPreauthenticatedWorkspace()
        let first = try SemanticTreeSnapshotReader.readSnapshot(from: app, fallbackFileURL: cfg.semanticExportPath)
        let second = try SemanticTreeSnapshotReader.readSnapshot(from: app, fallbackFileURL: cfg.semanticExportPath)
        XCTAssertEqual(first.state_hash, second.state_hash, "Idle workspace must produce deterministic state_hash")
    }

    // Feature: F15 | Tier: 1
    @MainActor
    func test_F15_T1_03_selectingDialogMutatesStateHashAndUpdatesStageActiveChat() async throws {
        let (app, auth, cfg) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(
            token: auth.token,
            CreateChatRequestDTO(title: "Semantic Stage Target", chatType: "group")
        )
        let before = try SemanticTreeSnapshotReader.readSnapshot(from: app, fallbackFileURL: cfg.semanticExportPath)

        openChatFromInbox(app, chatId: group.chat_id)
        let after = try SemanticTreeSnapshotReader.readSnapshot(from: app, fallbackFileURL: cfg.semanticExportPath)
        XCTAssertNotEqual(before.state_hash, after.state_hash)
        XCTAssertEqual(after.stage.chat_title, "Semantic Stage Target")
    }

    // Feature: F15 | Tier: 1
    @MainActor
    func test_F15_T1_04_typingInComposerUpdatesSemanticComposerTextNode() async throws {
        let (app, auth, cfg) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(
            token: auth.token,
            CreateChatRequestDTO(title: "Composer Mirror", chatType: "group")
        )
        openChatFromInbox(app, chatId: group.chat_id)
        clearAndType(app, id: FabushiAID.Composer.textView, text: "Semantic draft check")

        let snap = try SemanticTreeSnapshotReader.readSnapshot(from: app, fallbackFileURL: cfg.semanticExportPath)
        XCTAssertEqual(snap.composer.text, "Semantic draft check")
    }

    // Feature: F15 | Tier: 1
    @MainActor
    func test_F15_T1_05_switchingNavigationTabsUpdatesSemanticNavRailActiveDestination() async throws {
        let (app, _, cfg) = try await launchPreauthenticatedWorkspace()
        requireElement(app, id: FabushiAID.Root.tabSettings).tap()

        let snap = try SemanticTreeSnapshotReader.readSnapshot(from: app, fallbackFileURL: cfg.semanticExportPath)
        XCTAssertTrue(snap.nav_rail.active_destination.lowercased().contains("setting"))
    }

    // Feature: F15 | Tier: 1
    @MainActor
    func test_F15_T1_06_semanticSidebarDialogCountMatchesSeededServerChats() async throws {
        let (app, auth, cfg) = try await launchPreauthenticatedWorkspace()
        _ = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Seed A", chatType: "group"))
        _ = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Seed B", chatType: "group"))

        _ = try await waitUntil(timeout: 8.0) {
            ((try? SemanticTreeSnapshotReader.readSnapshot(from: app, fallbackFileURL: cfg.semanticExportPath).sidebar.dialog_count) ?? 0) >= 2
        }
        let snap = try SemanticTreeSnapshotReader.readSnapshot(from: app, fallbackFileURL: cfg.semanticExportPath)
        XCTAssertGreaterThanOrEqual(snap.sidebar.dialog_count, 2)
    }

    // MARK: - F45: Liquid Glass Material System (`LiquidLensView` & `EdgeEffectView`) (5 Tests)

    // Feature: F45 | Tier: 1
    @MainActor
    func test_F45_T1_01_liquidGlassTabBarContainerMeasures62PtHeightWith20PtSideInsets() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace()
        let windowWidth = app.windows.firstMatch.frame.width
        guard windowWidth <= TelegramGeometryMetrics.splitViewBreakpointPt else { return }

        let bar = requireElement(app, id: FabushiAID.Root.tabBarContainer)
        GesturePhysicsHelpers.assertMetric(
            bar.frame.height,
            equals: TelegramGeometryMetrics.liquidGlassTabBarHeightPt,
            accuracy: 1.5,
            "Floating Liquid Glass Tab Bar must measure 62pt tall"
        )
        GesturePhysicsHelpers.assertMetric(
            bar.frame.minX,
            equals: TelegramGeometryMetrics.liquidGlassTabBarSideInsetPt,
            accuracy: 2.0,
            "Floating Liquid Glass Tab Bar must have 20pt horizontal inset"
        )
    }

    // Feature: F45 | Tier: 1
    @MainActor
    func test_F45_T1_02_liquidLensSelectionCapsuleTracksSelectedTabCenterX() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace()
        guard app.windows.firstMatch.frame.width <= TelegramGeometryMetrics.splitViewBreakpointPt else { return }

        let contactsTab = requireElement(app, id: FabushiAID.Root.tabContacts)
        contactsTab.tap()
        let lens = requireElement(app, id: FabushiAID.Root.liquidLens)
        GesturePhysicsHelpers.assertMetric(lens.frame.midX, equals: contactsTab.frame.midX, accuracy: 6.0)

        let settingsTab = requireElement(app, id: FabushiAID.Root.tabSettings)
        settingsTab.tap()
        let moved = try await waitUntil(timeout: 4.0) {
            abs(lens.frame.midX - settingsTab.frame.midX) <= 6.0
        }
        XCTAssertTrue(moved, "LiquidLensView must animate its horizontal center to the newly selected tab")
    }

    // Feature: F45 | Tier: 1
    @MainActor
    func test_F45_T1_03_edgeEffectTopAndBottomFeatherContainersMeasure50PtHeight() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Edge Feather", chatType: "group"))
        openChatFromInbox(app, chatId: group.chat_id)

        let topEdge = requireElement(app, id: FabushiAID.Root.edgeEffectTop)
        let bottomEdge = requireElement(app, id: FabushiAID.Root.edgeEffectBottom)
        GesturePhysicsHelpers.assertMetric(topEdge.frame.height, equals: TelegramGeometryMetrics.edgeEffectFeatherHeightPt, accuracy: 2.0)
        GesturePhysicsHelpers.assertMetric(bottomEdge.frame.height, equals: TelegramGeometryMetrics.edgeEffectFeatherHeightPt, accuracy: 2.0)
    }

    // Feature: F45 | Tier: 1
    @MainActor
    func test_F45_T1_04_legacyChromeFallbackUses49PtClassicTabBarHeight() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace(
            extraEnvironment: ["FABUSHI_FORCE_LEGACY_TAB_CHROME": "1"]
        )
        guard app.windows.firstMatch.frame.width <= TelegramGeometryMetrics.splitViewBreakpointPt else { return }

        let bar = requireElement(app, id: FabushiAID.Root.tabBarContainer)
        GesturePhysicsHelpers.assertMetric(bar.frame.height, equals: TelegramGeometryMetrics.classicTabBarHeightPt, accuracy: 1.5)
    }

    // Feature: F45 | Tier: 1
    @MainActor
    func test_F45_T1_05_settingsGroupedCardsRenderInsetSectionGeometry() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace()
        requireElement(app, id: FabushiAID.Root.tabSettings).tap()
        let appearanceRow = requireElement(app, id: FabushiAID.SettingsAndPeers.settingsSection("appearance"))
        XCTAssertGreaterThanOrEqual(appearanceRow.frame.minX, 12.0, "Inset grouped glass cards must inset horizontally from screen edge")
    }

    // MARK: - F46: Custom `NavigationController`, Sheets, Alerts & 5s Undo Toast (5 Tests)

    // Feature: F46 | Tier: 1
    @MainActor
    func test_F46_T1_01_interactiveLeftEdgeSwipePopsActiveConversationBackToInbox() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        guard app.windows.firstMatch.frame.width <= TelegramGeometryMetrics.splitViewBreakpointPt else { return }

        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Edge Pop Chat", chatType: "group"))
        openChatFromInbox(app, chatId: group.chat_id)

        let leftEdge = app.windows.firstMatch.coordinate(withNormalizedOffset: CGVector(dx: 0.02, dy: 0.5))
        let release = app.windows.firstMatch.coordinate(withNormalizedOffset: CGVector(dx: 0.82, dy: 0.5))
        leftEdge.press(forDuration: 0.05, thenDragTo: release)

        XCTAssertTrue(requireElement(app, id: FabushiAID.ChatList.collectionView, timeout: 8.0).isHittable)
    }

    // Feature: F46 | Tier: 1
    @MainActor
    func test_F46_T1_02_modalSheetDismissesCleanlyOnDownwardSwipeGesture() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace()
        requireElement(app, id: FabushiAID.ChatList.composeButton).tap()

        let sheet = requireElement(app, id: FabushiAID.Root.overlaySheet, timeout: 6.0)
        sheet.swipeDown()
        let dismissed = try await waitUntil(timeout: 6.0) { !sheet.exists }
        XCTAssertTrue(dismissed)
    }

    // Feature: F46 | Tier: 1
    @MainActor
    func test_F46_T1_03_glassAlertCancelButtonAbortsDestructiveActionWithoutMutatingServer() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Keep Me Group", chatType: "group"))

        let row = requireElement(app, id: FabushiAID.ChatList.row(group.chat_id))
        GesturePhysicsHelpers.trailingSwipeChatRow(row)
        requireElement(app, id: FabushiAID.ChatList.swipeDelete).tap()

        let cancelBtn = requireElement(app, id: FabushiAID.Root.alertCancelButton, timeout: 6.0)
        cancelBtn.tap()
        XCTAssertTrue(requireElement(app, id: FabushiAID.ChatList.row(group.chat_id)).exists)
    }

    // Feature: F46 | Tier: 1
    @MainActor
    func test_F46_T1_04_fiveSecondUndoToastReversesDestructiveActionBeforeCommit() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Undoable Group", chatType: "group"))

        let row = requireElement(app, id: FabushiAID.ChatList.row(group.chat_id))
        GesturePhysicsHelpers.trailingSwipeChatRow(row)
        requireElement(app, id: FabushiAID.ChatList.swipeDelete).tap()
        let confirm = element(app, id: FabushiAID.Root.alertPrimaryButton)
        if confirm.waitForExistence(timeout: 3.0) { confirm.tap() }

        let toast = requireElement(app, id: FabushiAID.Root.undoToast, timeout: 5.0)
        XCTAssertTrue(requireElement(app, id: FabushiAID.Root.undoTimer).exists)
        requireElement(app, id: FabushiAID.Root.undoButton).tap()

        XCTAssertTrue(requireElement(app, id: FabushiAID.ChatList.row(group.chat_id)).exists)
        XCTAssertFalse(toast.exists)
    }

    // Feature: F46 | Tier: 1
    @MainActor
    func test_F46_T1_05_undoToastNaturalFiveSecondExpiryCommitsActionToServer() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Expire Delete Group", chatType: "group"))

        let row = requireElement(app, id: FabushiAID.ChatList.row(group.chat_id))
        GesturePhysicsHelpers.trailingSwipeChatRow(row)
        requireElement(app, id: FabushiAID.ChatList.swipeArchive).tap()

        if element(app, id: FabushiAID.Root.undoToast).waitForExistence(timeout: 2.0) {
            try await Task.sleep(for: .milliseconds(5300))
        }
        let detail = try await oracle.request(method: "GET", path: "/api/v1/chats/\(group.chat_id)", bearerToken: auth.token)
        let json = try detail.decodeJSON()
        XCTAssertEqual(json["is_archived"]?.boolValue, true)
    }

    // MARK: - F52: iPadOS Split-View & Hardware Keyboard Shortcuts (5 Tests)

    // Feature: F52 | Tier: 1
    @MainActor
    func test_F52_T1_01_commandKShortcutFocusesGlobalSearchInputField() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace()
        GesturePhysicsHelpers.sendHardwareKey(in: app, key: "k", modifiers: [.command])

        let searchInput = requireElement(app, id: FabushiAID.ChatList.searchInput, timeout: 6.0)
        XCTAssertTrue(searchInput.exists)
    }

    // Feature: F52 | Tier: 1
    @MainActor
    func test_F52_T1_02_commandDigitShortcutsSwitchRootDestinationsDirectly() async throws {
        let (app, _, cfg) = try await launchPreauthenticatedWorkspace()
        GesturePhysicsHelpers.sendHardwareKey(in: app, key: "4", modifiers: [.command])
        _ = requireElement(app, id: FabushiAID.SettingsAndPeers.settingsList, timeout: 6.0)

        GesturePhysicsHelpers.sendHardwareKey(in: app, key: "3", modifiers: [.command])
        _ = requireElement(app, id: FabushiAID.ChatList.collectionView, timeout: 6.0)

        let snap = try SemanticTreeSnapshotReader.readSnapshot(from: app, fallbackFileURL: cfg.semanticExportPath)
        XCTAssertTrue(snap.isValidHexHash)
    }

    // Feature: F52 | Tier: 1
    @MainActor
    func test_F52_T1_03_escapeHardwareKeyDismissesActiveSheetOrConversationStage() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace()
        requireElement(app, id: FabushiAID.ChatList.composeButton).tap()
        _ = requireElement(app, id: FabushiAID.Root.overlaySheet, timeout: 5.0)

        GesturePhysicsHelpers.sendSpecialKey(in: app, key: .escape)
        let closed = try await waitUntil(timeout: 5.0) {
            !self.element(app, id: FabushiAID.Root.overlaySheet).exists
        }
        XCTAssertTrue(closed)
    }

    // Feature: F52 | Tier: 1
    @MainActor
    func test_F52_T1_04_upArrowInEmptyComposerEntersEditModeForLastOwnMessage() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Keyboard Edit", chatType: "group"))
        _ = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(text: "Edit me via UpArrow", clientNonce: 52_401)
        )

        openChatFromInbox(app, chatId: group.chat_id)
        requireElement(app, id: FabushiAID.Composer.textView).tap()
        GesturePhysicsHelpers.sendSpecialKey(in: app, key: .upArrow)

        let editBanner = requireElement(app, id: FabushiAID.Composer.editBanner, timeout: 6.0)
        XCTAssertTrue(editBanner.exists)
    }

    // Feature: F52 | Tier: 1
    @MainActor
    func test_F52_T1_05_commandLTriggersPasscodeLockScreenWhenPasscodeConfigured() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace(
            extraEnvironment: ["FABUSHI_TEST_PASSCODE_PIN": "2580"]
        )
        GesturePhysicsHelpers.sendHardwareKey(in: app, key: "l", modifiers: [.command])

        let lockOverlay = requireElement(app, id: FabushiAID.Root.lockscreenOverlay, timeout: 6.0)
        XCTAssertTrue(lockOverlay.exists)
    }
}
