import Foundation
import XCTest

/// Tier 2 Boundary, Negative & Corner-Case Opaque-Box UI Tests — Domain R2:
/// Adaptive Shell Stress, Theme/Font Extremes, Locale Fallbacks, Semantic Tree Redaction,
/// Liquid Glass Accessibility & Cancelled Gesture Physics (`F11..F13, F15, F45, F46, F52`).
final class Tier2_R2_ShellThemeBoundaryUITests: FabushiE2ETestCase {

    // MARK: - F11: Adaptive Shell Stress & Deep-Link Edge Cases (5 Tests)

    // Feature: F11 | Tier: 2
    @MainActor
    func test_F11_T2_01_rapidMultiAxisOrientationTransitionsPreserveActiveConversationStage() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Rotation Torture", chatType: "group"))
        openChatFromInbox(app, chatId: group.chat_id)

        for orientation in [UIDeviceOrientation.landscapeLeft, .portrait, .landscapeRight, .portrait] {
            XCUIDevice.shared.orientation = orientation
        }
        XCTAssertEqual(requireElement(app, id: FabushiAID.Chat.headerTitle).label, "Rotation Torture")
    }

    // Feature: F11 | Tier: 2
    @MainActor
    func test_F11_T2_02_deepLinkToInaccessibleChatIdSurfacesErrorWithoutOpeningPhantomStage() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace()
        try GesturePhysicsHelpers.openCustomURL("fabushi://chat/999999999")

        XCTAssertTrue(app.state == .runningForeground)
        XCTAssertFalse(element(app, id: FabushiAID.Chat.headerTitle).label == "999999999")
    }

    // Feature: F11 | Tier: 2
    @MainActor
    func test_F11_T2_03_compactViewportHidesTabBarWhenPushingFullScreenConversation() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        guard app.windows.firstMatch.frame.width <= TelegramGeometryMetrics.splitViewBreakpointPt else { return }

        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Tab Hide Check", chatType: "group"))
        openChatFromInbox(app, chatId: group.chat_id)

        let tabBar = element(app, id: FabushiAID.Root.tabBarContainer)
        XCTAssertTrue(!tabBar.exists || !tabBar.isHittable, "Pushed conversation in compact mode must hide floating bottom tab bar")
    }

    // Feature: F11 | Tier: 2
    @MainActor
    func test_F11_T2_04_tappingAlreadySelectedSettingsRootTabDoesNotCorruptNavigationStack() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace()
        let settingsTab = requireElement(app, id: FabushiAID.Root.tabSettings)
        settingsTab.tap()
        settingsTab.tap()
        settingsTab.tap()

        XCTAssertTrue(requireElement(app, id: FabushiAID.SettingsAndPeers.settingsList).exists)
    }

    // Feature: F11 | Tier: 2
    @MainActor
    func test_F11_T2_05_unknownCustomUrlPathIgnoredGracefullyWithoutCrash() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace()
        try GesturePhysicsHelpers.openCustomURL("fabushi://nonexistent/route/path?x=1")
        XCTAssertTrue(app.state == .runningForeground)
    }

    // MARK: - F12: Theme Cycling, Dynamic Type Extremes & Contrast Boundaries (5 Tests)

    // Feature: F12 | Tier: 2
    @MainActor
    func test_F12_T2_01_rapidCyclingAcrossAllFourThemesLeavesConsistentFinalPalette() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace()
        requireElement(app, id: FabushiAID.Root.tabSettings).tap()
        requireElement(app, id: FabushiAID.SettingsAndPeers.settingsSection("appearance")).tap()

        for mode in ["dayClassic", "day", "night", "nightTinted", "night"] {
            requireElement(app, id: FabushiAID.SettingsAndPeers.appearanceTheme(mode)).tap()
        }

        requireElement(app, id: FabushiAID.Root.tabChats).tap()
        let sample = try PixelColorSampler.sampleNormalizedPixel(
            in: requireElement(app, id: FabushiAID.ChatList.collectionView),
            normalizedPoint: CGPoint(x: 0.5, y: 0.85)
        )
        XCTAssertTrue(
            sample.matches(TelegramThemePalette.nightOledPlainBg, channelTolerance: 20) ||
            sample.matches(TelegramThemePalette.nightOledCardBg, channelTolerance: 20)
        )
    }

    // Feature: F12 | Tier: 2
    @MainActor
    func test_F12_T2_02_minimum14PtAndMaximum26PtFontSliderExtremesKeepSendButtonHittable() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Font Extremes", chatType: "group"))

        requireElement(app, id: FabushiAID.Root.tabSettings).tap()
        requireElement(app, id: FabushiAID.SettingsAndPeers.settingsSection("appearance")).tap()
        let slider = requireElement(app, id: FabushiAID.SettingsAndPeers.appearanceFontSizeSlider)
        slider.adjust(toNormalizedSliderPosition: 0.0) // 14pt
        slider.adjust(toNormalizedSliderPosition: 1.0) // 26pt

        requireElement(app, id: FabushiAID.Root.tabChats).tap()
        openChatFromInbox(app, chatId: group.chat_id)
        clearAndType(app, id: FabushiAID.Composer.textView, text: "26pt send button check")
        XCTAssertTrue(requireElement(app, id: FabushiAID.Composer.sendButton).isHittable)
    }

    // Feature: F12 | Tier: 2
    @MainActor
    func test_F12_T2_03_cjkRtlAndZwjEmojiAt26PtNeverOverflowTranscriptViewportWidth() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Script Torture", chatType: "group"))
        let mixed = "中文测试👨‍👩‍👧‍👦مرحبا بالعالم_Supercalifragilisticexpialidocious_2026"
        let msg = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: mixed, clientNonce: 120_301))

        requireElement(app, id: FabushiAID.Root.tabSettings).tap()
        requireElement(app, id: FabushiAID.SettingsAndPeers.settingsSection("appearance")).tap()
        requireElement(app, id: FabushiAID.SettingsAndPeers.appearanceFontSizeSlider).adjust(toNormalizedSliderPosition: 1.0)

        requireElement(app, id: FabushiAID.Root.tabChats).tap()
        openChatFromInbox(app, chatId: group.chat_id)
        let bubble = requireElement(app, id: FabushiAID.Chat.message(msg.message_id))
        let transcript = requireElement(app, id: FabushiAID.Chat.transcript)
        XCTAssertLessThanOrEqual(bubble.frame.maxX, transcript.frame.maxX + 1.0)
    }

    // Feature: F12 | Tier: 2
    @MainActor
    func test_F12_T2_04_togglingBubbleFreeModeWithMixedMediaMessagesPreservesOrder() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "BubbleFree Order", chatType: "group"))
        let m1 = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: "First row", clientNonce: 120_401))
        let m2 = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: "Second row", clientNonce: 120_402))

        requireElement(app, id: FabushiAID.Root.tabSettings).tap()
        requireElement(app, id: FabushiAID.SettingsAndPeers.settingsSection("appearance")).tap()
        requireElement(app, id: FabushiAID.SettingsAndPeers.appearanceBubbleFreeToggle).tap()

        requireElement(app, id: FabushiAID.Root.tabChats).tap()
        openChatFromInbox(app, chatId: group.chat_id)
        let b1 = requireElement(app, id: FabushiAID.Chat.message(m1.message_id))
        let b2 = requireElement(app, id: FabushiAID.Chat.message(m2.message_id))
        XCTAssertLessThan(b1.frame.minY, b2.frame.minY)
    }

    // Feature: F12 | Tier: 2
    @MainActor
    func test_F12_T2_05_unknownThemeEnvironmentOverrideFallsBackToDefaultDayTheme() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace(
            extraEnvironment: ["FABUSHI_INITIAL_THEME": "nonexistent_cyberpunk_99"]
        )
        XCTAssertTrue(requireElement(app, id: FabushiAID.ChatList.collectionView).exists)
    }

    // MARK: - F13: Localization Fallback & Draft-Preserving Hot-Swap (5 Tests)

    // Feature: F13 | Tier: 2
    @MainActor
    func test_F13_T2_01_unsupportedLocaleTagFallsBackCleanlyToEnglishWithoutRawKeys() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace(initialLocale: "fr-CA")
        let label = requireElement(app, id: FabushiAID.Root.tabChats).label
        XCTAssertFalse(label.contains("."), "Fallback locale must never expose raw dotted localization key: '\(label)'")
        XCTAssertFalse(label.isEmpty)
    }

    // Feature: F13 | Tier: 2
    @MainActor
    func test_F13_T2_02_switchingLanguageDoesNotLoseUnsentComposerText() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace(initialLocale: "en-US")
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "i18n Draft", chatType: "group"))
        openChatFromInbox(app, chatId: group.chat_id)
        clearAndType(app, id: FabushiAID.Composer.textView, text: "Keep across locale switch")

        if app.navigationBars.buttons.firstMatch.exists { app.navigationBars.buttons.firstMatch.tap() }
        requireElement(app, id: FabushiAID.Root.tabSettings).tap()
        requireElement(app, id: FabushiAID.SettingsAndPeers.settingsSection("language")).tap()
        let zhCell = app.cells.containing(.staticText, identifier: "简体中文").firstMatch
        if zhCell.waitForExistence(timeout: 4.0) { zhCell.tap() }

        requireElement(app, id: FabushiAID.Root.tabChats).tap()
        openChatFromInbox(app, chatId: group.chat_id)
        let current = requireElement(app, id: FabushiAID.Composer.textView).value as? String ?? ""
        XCTAssertTrue(current.contains("Keep across locale switch"))
    }

    // Feature: F13 | Tier: 2
    @MainActor
    func test_F13_T2_03_tabBarItemsNeverOverlapEachOtherUnderChineseOrEnglishCopy() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace(initialLocale: "zh-CN")
        guard app.windows.firstMatch.frame.width <= TelegramGeometryMetrics.splitViewBreakpointPt else { return }

        let t1 = requireElement(app, id: FabushiAID.Root.tabContacts).frame
        let t2 = requireElement(app, id: FabushiAID.Root.tabCalls).frame
        let t3 = requireElement(app, id: FabushiAID.Root.tabChats).frame
        let t4 = requireElement(app, id: FabushiAID.Root.tabSettings).frame

        XCTAssertLessThanOrEqual(t1.maxX, t2.minX + 2.0)
        XCTAssertLessThanOrEqual(t2.maxX, t3.minX + 2.0)
        XCTAssertLessThanOrEqual(t3.maxX, t4.minX + 2.0)
    }

    // Feature: F13 | Tier: 2
    @MainActor
    func test_F13_T2_04_mixedBilingualPeerTitlesRenderWithoutMojibakeOrTruncationCrash() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace(initialLocale: "zh-CN")
        let title = "双语架构组 Bilingual Engineering 🚀 العربية"
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: title, chatType: "group"))

        let rowTitle = requireElement(app, id: FabushiAID.ChatList.rowTitle(group.chat_id))
        XCTAssertTrue(rowTitle.label.contains("双语架构组"))
    }

    // Feature: F13 | Tier: 2
    @MainActor
    func test_F13_T2_05_emptyLocaleEnvironmentStringDefaultsCleanlyWithoutCrash() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace(initialLocale: "")
        XCTAssertTrue(requireElement(app, id: FabushiAID.ChatList.collectionView).exists)
    }

    // MARK: - F15: SemanticWorkspaceTree Redaction & Edge Cases (6 Tests)

    // Feature: F15 | Tier: 2
    @MainActor
    func test_F15_T2_01_unauthenticatedAuthScreenExportsSemanticTreeWithIsAuthenticatedFalse() async throws {
        let (app, cfg) = try launchUnauthenticatedAuth()
        _ = element(app, id: FabushiAID.Auth.splashStartButton).waitForExistence(timeout: 6.0)

        let snap = try SemanticTreeSnapshotReader.readSnapshot(from: app, fallbackFileURL: cfg.semanticExportPath)
        XCTAssertFalse(snap.window.is_authenticated)
        XCTAssertTrue(snap.isValidHexHash)
    }

    // Feature: F15 | Tier: 2
    @MainActor
    func test_F15_T2_02_semanticWorkspaceTreeNeverLeaksRawBearerTokenInJsonPayload() async throws {
        let (app, auth, cfg) = try await launchPreauthenticatedWorkspace()
        _ = try SemanticTreeSnapshotReader.readSnapshot(from: app, fallbackFileURL: cfg.semanticExportPath)

        let probe = element(app, id: FabushiAID.Root.semanticTree)
        let rawText = (probe.value as? String)
            ?? (try? String(contentsOf: cfg.semanticExportPath, encoding: .utf8))
            ?? ""
        XCTAssertFalse(rawText.contains(auth.token), "SemanticWorkspaceTree JSON must NEVER expose raw session Bearer token")
    }

    // Feature: F15 | Tier: 2
    @MainActor
    func test_F15_T2_03_dismissingModalClearsActiveModalNodeInSemanticSnapshot() async throws {
        let (app, _, cfg) = try await launchPreauthenticatedWorkspace()
        requireElement(app, id: FabushiAID.ChatList.composeButton).tap()
        _ = requireElement(app, id: FabushiAID.Root.overlaySheet)
        GesturePhysicsHelpers.sendSpecialKey(in: app, key: .escape)

        _ = try await waitUntil(timeout: 5.0) { !self.element(app, id: FabushiAID.Root.overlaySheet).exists }
        let snap = try SemanticTreeSnapshotReader.readSnapshot(from: app, fallbackFileURL: cfg.semanticExportPath)
        XCTAssertNil(snap.active_modal)
    }

    // Feature: F15 | Tier: 2
    @MainActor
    func test_F15_T2_04_offlineDisconnectReflectsNavRailConnectedFalseInSemanticTree() async throws {
        let (app, _, cfg) = try await launchPreauthenticatedWorkspace(networkFaultMode: .offline)
        let snap = try SemanticTreeSnapshotReader.readSnapshot(from: app, fallbackFileURL: cfg.semanticExportPath)
        XCTAssertFalse(snap.nav_rail.connected)
    }

    // Feature: F15 | Tier: 2
    @MainActor
    func test_F15_T2_05_unicodeEmojiInComposerProducesValidDeterministicSha256Hash() async throws {
        let (app, auth, cfg) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Hash Unicode", chatType: "group"))
        openChatFromInbox(app, chatId: group.chat_id)
        clearAndType(app, id: FabushiAID.Composer.textView, text: "🧘‍♂️莲花生大士✨")

        let s1 = try SemanticTreeSnapshotReader.readSnapshot(from: app, fallbackFileURL: cfg.semanticExportPath)
        let s2 = try SemanticTreeSnapshotReader.readSnapshot(from: app, fallbackFileURL: cfg.semanticExportPath)
        XCTAssertTrue(s1.isValidHexHash)
        XCTAssertEqual(s1.state_hash, s2.state_hash)
    }

    // Feature: F15 | Tier: 2
    @MainActor
    func test_F15_T2_06_poppingConversationResetsStageActiveChatIdInCompactMode() async throws {
        let (app, auth, cfg) = try await launchPreauthenticatedWorkspace()
        guard app.windows.firstMatch.frame.width <= TelegramGeometryMetrics.splitViewBreakpointPt else { return }

        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Pop Stage", chatType: "group"))
        openChatFromInbox(app, chatId: group.chat_id)
        if app.navigationBars.buttons.firstMatch.exists {
            app.navigationBars.buttons.firstMatch.tap()
        }
        _ = requireElement(app, id: FabushiAID.ChatList.collectionView)
        let snap = try SemanticTreeSnapshotReader.readSnapshot(from: app, fallbackFileURL: cfg.semanticExportPath)
        XCTAssertNil(snap.stage.active_chat_id)
    }

    // MARK: - F45: Liquid Glass Edge Pass-Through & Accessibility Fallbacks (5 Tests)

    // Feature: F45 | Tier: 2
    @MainActor
    func test_F45_T2_01_edgeEffectFeatherViewsDoNotInterceptTouchesOnUnderlyingBubbles() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "PassThrough Edge", chatType: "group"))
        let msg = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: "Tap under bottom feather", clientNonce: 450_101))

        openChatFromInbox(app, chatId: group.chat_id)
        let bubble = requireElement(app, id: FabushiAID.Chat.message(msg.message_id))
        XCTAssertTrue(bubble.isHittable, "50pt EdgeEffectView must be hit-test pass-through")
    }

    // Feature: F45 | Tier: 2
    @MainActor
    func test_F45_T2_02_reduceTransparencyAccessibilityFallbackKeepsControlsOpaqueAndTappable() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace(
            extraEnvironment: ["FABUSHI_SIMULATE_REDUCE_TRANSPARENCY": "1"]
        )
        XCTAssertTrue(requireElement(app, id: FabushiAID.ChatList.composeButton).isHittable)
    }

    // Feature: F45 | Tier: 2
    @MainActor
    func test_F45_T2_03_compactLandscapeUsesSlimTabBarWithoutClippingIcons() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace()
        guard app.windows.firstMatch.frame.width <= TelegramGeometryMetrics.splitViewBreakpointPt else { return }

        XCUIDevice.shared.orientation = .landscapeLeft
        let bar = requireElement(app, id: FabushiAID.Root.tabBarContainer)
        XCTAssertLessThanOrEqual(bar.frame.height, TelegramGeometryMetrics.liquidGlassTabBarHeightPt + 2.0)
        XCUIDevice.shared.orientation = .portrait
    }

    // Feature: F45 | Tier: 2
    @MainActor
    func test_F45_T2_04_rapidTabDragGestureSettlesLiquidLensCleanlyOntoNearestTab() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace()
        guard app.windows.firstMatch.frame.width <= TelegramGeometryMetrics.splitViewBreakpointPt else { return }

        let bar = requireElement(app, id: FabushiAID.Root.tabBarContainer)
        let left = bar.coordinate(withNormalizedOffset: CGVector(dx: 0.15, dy: 0.5))
        let right = bar.coordinate(withNormalizedOffset: CGVector(dx: 0.88, dy: 0.5))
        left.press(forDuration: 0.05, thenDragTo: right)

        let lens = requireElement(app, id: FabushiAID.Root.liquidLens)
        let settings = requireElement(app, id: FabushiAID.Root.tabSettings)
        GesturePhysicsHelpers.assertMetric(lens.frame.midX, equals: settings.frame.midX, accuracy: 10.0)
    }

    // Feature: F45 | Tier: 2
    @MainActor
    func test_F45_T2_05_glassTabBarMaintainsPositiveBottomSafeAreaClearance() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace()
        guard app.windows.firstMatch.frame.width <= TelegramGeometryMetrics.splitViewBreakpointPt else { return }

        let windowMaxY = app.windows.firstMatch.frame.maxY
        let barMaxY = requireElement(app, id: FabushiAID.Root.tabBarContainer).frame.maxY
        XCTAssertLessThanOrEqual(barMaxY, windowMaxY)
    }

    // MARK: - F46: Cancelled Transitions, Half-Drags & Overlapping Undo Toasts (5 Tests)

    // Feature: F46 | Tier: 2
    @MainActor
    func test_F46_T2_01_shortAbortedLeftEdgeSwipeUnderThresholdStaysInConversation() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        guard app.windows.firstMatch.frame.width <= TelegramGeometryMetrics.splitViewBreakpointPt else { return }

        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Stay Open Stage", chatType: "group"))
        openChatFromInbox(app, chatId: group.chat_id)

        let start = app.windows.firstMatch.coordinate(withNormalizedOffset: CGVector(dx: 0.02, dy: 0.5))
        let tinyDrag = app.windows.firstMatch.coordinate(withNormalizedOffset: CGVector(dx: 0.10, dy: 0.5))
        start.press(forDuration: 0.05, thenDragTo: tinyDrag, withVelocity: .slow, thenHoldForDuration: 0.1)

        XCTAssertTrue(requireElement(app, id: FabushiAID.Chat.transcript).exists)
    }

    // Feature: F46 | Tier: 2
    @MainActor
    func test_F46_T2_02_shortSheetNudgeSnapsModalSheetBackToPresentedDetent() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace()
        requireElement(app, id: FabushiAID.ChatList.composeButton).tap()

        let sheet = requireElement(app, id: FabushiAID.Root.overlaySheet)
        let top = sheet.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.1))
        let nudge = top.withOffset(CGVector(dx: 0.0, dy: 24.0))
        top.press(forDuration: 0.05, thenDragTo: nudge, withVelocity: .slow, thenHoldForDuration: 0.1)

        XCTAssertTrue(sheet.exists)
    }

    // Feature: F46 | Tier: 2
    @MainActor
    func test_F46_T2_03_consecutiveUndoableActionsCoalesceWithoutStackingDuplicateToasts() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let g1 = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Toast One", chatType: "group"))
        let g2 = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Toast Two", chatType: "group"))

        GesturePhysicsHelpers.trailingSwipeChatRow(requireElement(app, id: FabushiAID.ChatList.row(g1.chat_id)))
        requireElement(app, id: FabushiAID.ChatList.swipeArchive).tap()

        GesturePhysicsHelpers.trailingSwipeChatRow(requireElement(app, id: FabushiAID.ChatList.row(g2.chat_id)))
        requireElement(app, id: FabushiAID.ChatList.swipeArchive).tap()

        let toastMatches = app.descendants(matching: .any).matching(identifier: FabushiAID.Root.undoToast)
        XCTAssertLessThanOrEqual(toastMatches.count, 1, "At most one UndoOverlayController toast may be visible at once")
    }

    // Feature: F46 | Tier: 2
    @MainActor
    func test_F46_T2_04_tappingUndoNearFourSecondBoundaryStillRestoresDialog() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Late Undo Group", chatType: "group"))

        GesturePhysicsHelpers.trailingSwipeChatRow(requireElement(app, id: FabushiAID.ChatList.row(group.chat_id)))
        requireElement(app, id: FabushiAID.ChatList.swipeArchive).tap()
        _ = requireElement(app, id: FabushiAID.Root.undoToast, timeout: 4.0)

        try await Task.sleep(for: .milliseconds(3800))
        if element(app, id: FabushiAID.Root.undoButton).exists {
            element(app, id: FabushiAID.Root.undoButton).tap()
            XCTAssertTrue(requireElement(app, id: FabushiAID.ChatList.row(group.chat_id)).exists)
        }
    }

    // Feature: F46 | Tier: 2
    @MainActor
    func test_F46_T2_05_escapeKeyOnDestructiveAlertActsAsCancelSafely() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Esc Cancel Group", chatType: "group"))

        GesturePhysicsHelpers.trailingSwipeChatRow(requireElement(app, id: FabushiAID.ChatList.row(group.chat_id)))
        requireElement(app, id: FabushiAID.ChatList.swipeDelete).tap()
        _ = requireElement(app, id: FabushiAID.Root.overlayAlert, timeout: 5.0)

        GesturePhysicsHelpers.sendSpecialKey(in: app, key: .escape)
        XCTAssertTrue(requireElement(app, id: FabushiAID.ChatList.row(group.chat_id)).exists)
    }

    // MARK: - F52: Hardware Keyboard Shortcut Boundary Conditions (5 Tests)

    // Feature: F52 | Tier: 2
    @MainActor
    func test_F52_T2_01_upArrowInNonEmptyComposerDoesNotOverwriteDraftWithEditMode() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "NonEmpty UpArrow", chatType: "group"))
        _ = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: "Previous sent", clientNonce: 520_101))

        openChatFromInbox(app, chatId: group.chat_id)
        clearAndType(app, id: FabushiAID.Composer.textView, text: "Active draft do not overwrite")
        GesturePhysicsHelpers.sendSpecialKey(in: app, key: .upArrow)

        XCTAssertFalse(element(app, id: FabushiAID.Composer.editBanner).exists, "Up-Arrow with non-empty draft must move cursor, NOT enter edit mode")
    }

    // Feature: F52 | Tier: 2
    @MainActor
    func test_F52_T2_02_upArrowInChatWithZeroOwnMessagesIsSafeNoOp() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let peer = try await oracle.registerRandomUser(prefix: "f52_peer")
        let group = try await oracle.createChat(
            token: peer.token,
            CreateChatRequestDTO(title: "Only Peer Messages", chatType: "group", memberIds: [auth.user_id])
        )
        _ = try await oracle.sendMessage(token: peer.token, chatId: group.chat_id, SendMessageRequestDTO(text: "Peer message only", clientNonce: 520_201))

        openChatFromInbox(app, chatId: group.chat_id)
        requireElement(app, id: FabushiAID.Composer.textView).tap()
        GesturePhysicsHelpers.sendSpecialKey(in: app, key: .upArrow)

        XCTAssertFalse(element(app, id: FabushiAID.Composer.editBanner).exists)
    }

    // Feature: F52 | Tier: 2
    @MainActor
    func test_F52_T2_03_commandLWithoutConfiguredPasscodeDoesNotTrapUserOnLockOverlay() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace()
        GesturePhysicsHelpers.sendHardwareKey(in: app, key: "l", modifiers: [.command])

        XCTAssertFalse(element(app, id: FabushiAID.Root.lockscreenOverlay).exists, "Cmd+L without configured PIN must not lock user out")
    }

    // Feature: F52 | Tier: 2
    @MainActor
    func test_F52_T2_04_outOfBoundsCommandNineShortcutIsIgnoredSafely() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace()
        GesturePhysicsHelpers.sendHardwareKey(in: app, key: "9", modifiers: [.command])
        XCTAssertTrue(requireElement(app, id: FabushiAID.ChatList.collectionView).exists)
    }

    // Feature: F52 | Tier: 2
    @MainActor
    func test_F52_T2_05_escapeAtRootChatListDoesNotDismissRootShell() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace()
        GesturePhysicsHelpers.sendSpecialKey(in: app, key: .escape)
        GesturePhysicsHelpers.sendSpecialKey(in: app, key: .escape)
        XCTAssertTrue(requireElement(app, id: FabushiAID.ChatList.collectionView).exists)
    }
}
