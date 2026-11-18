import Foundation
import Observation
import SwiftUI

/// Root application coordinator bridging `FabushiHostClient`, `TelegramThemeEngine`,
/// `LocalizationManager`, `AppRouter`, and `SemanticTreeExporter`.
@MainActor
@Observable
public final class FabushiAppCoordinator {
    public static let shared = FabushiAppCoordinator()

    public let hostClient: FabushiHostClient
    public let themeEngine: TelegramThemeEngine
    public let localization: LocalizationManager
    public let router: AppRouter
    public let semanticExporter: SemanticTreeExporter
    public let biometrics: BiometricsProvider

    public var connectionStatus: ConnectionStatus = .connecting
    public var isAuthenticated: Bool = false
    public var accounts: [AccountSessionSummary] = []
    public var unreadChatsCount: Int = 0
    public var isAccountSwitcherPresented: Bool = false
    public var isAppLocked: Bool = false
    public var passcodeInput: String = ""
    public var passcodeError: String?
    public var activeAlert: GlassAlertSpec?
    public var undoToastMessage: String?
    public var undoRemainingSeconds: Int = 5

    public let isUITestMode: Bool

    public init(
        hostClient: FabushiHostClient = .shared,
        themeEngine: TelegramThemeEngine = .shared,
        localization: LocalizationManager = .shared,
        router: AppRouter = .shared,
        semanticExporter: SemanticTreeExporter = .shared,
        biometrics: BiometricsProvider = BiometricsAuthenticator(),
        environment: [String: String] = ProcessInfo.processInfo.environment
    ) {
        self.hostClient = hostClient
        self.themeEngine = themeEngine
        self.localization = localization
        self.router = router
        self.semanticExporter = semanticExporter
        self.biometrics = biometrics
        self.isUITestMode = (environment["FABUSHI_UI_TEST_MODE"] == "1")
    }

    public func bootstrap() async {
        do {
            let boot = try await hostClient.initialize(
                keychainBridge: KeychainStore(),
                biometricsBridge: biometrics
            )
            self.connectionStatus = boot.connectionStatus
            self.isAuthenticated = boot.isAuthenticated
            self.accounts = boot.accounts
            refreshSemanticSnapshot()
        } catch {
            self.connectionStatus = .offline
            refreshSemanticSnapshot()
        }
    }

    public func handleScenePhase(_ phase: ScenePhase) async {
        switch phase {
        case .active:
            await hostClient.handleScenePhaseChange(.active)
        case .inactive:
            await hostClient.handleScenePhaseChange(.inactive)
        case .background:
            await hostClient.handleScenePhaseChange(.background)
        @unknown default:
            break
        }
        refreshSemanticSnapshot()
    }

    public func refreshSemanticSnapshot(viewportSize: CGSize = CGSize(width: 393, height: 852)) {
        let isSplit = viewportSize.width > 680.0
        let layoutName = isSplit ? "SplitView" : "CompactTab"
        let selectedChat = router.activeChatId.flatMap { $0 >= 0 ? UInt64($0) : nil }
        let tree = SemanticWorkspaceTree(
            windowLayout: layoutName,
            selectedChatId: selectedChat,
            composerText: "",
            activeModal: isAccountSwitcherPresented ? "accountSwitcher" : nil,
            isCallActive: false,
            window: SemanticWindowNode(
                layout: layoutName,
                widthPx: Float(viewportSize.width),
                heightPx: Float(viewportSize.height),
                activePanel: router.selectedTab.rawValue,
                isAuthenticated: isAuthenticated
            ),
            navRail: SemanticNavRailNode(
                activeDestination: router.selectedTab.rawValue,
                connected: connectionStatus == .connected
            ),
            sidebar: SemanticSidebarNode(
                dialogCount: UInt64(max(0, unreadChatsCount)),
                selectedChatId: selectedChat,
                searchQuery: ""
            ),
            stage: SemanticStageNode(
                activeChatId: selectedChat,
                chatTitle: "",
                messageCount: 0,
                isGroupChat: false,
                pinnedMessage: nil
            )
        )
        _ = try? semanticExporter.updateAndExport(tree)
    }
}

/// Canonical Telegram-iOS Root Shell (`F11`):
/// - `UISplitViewController(style: .doubleColumn)` (`>680pt` side-by-side, `<=680pt` single column)
/// - `62.0pt` Liquid Glass floating capsule Tab Bar (`20.0pt` horizontal side inset + `LiquidLensView`)
///   or `49.0pt` classic docked bar
/// - Top & bottom `50.0pt` `EdgeEffectView` scroll feathering
/// - Complete `TEST_INFRA.md § 2.1` `fabushi.root.*`, `fabushi.tabbar.*`, `fabushi.chrome.*`,
///   `fabushi.overlay.*`, `fabushi.lockscreen.*`, and `fabushi.debug.*` accessibility identifiers.
public struct FabushiRootShellView: View {
    public static let liquidGlassBarHeightPt: CGFloat = 62.0
    public static let classicBarHeightPt: CGFloat = 49.0
    public static let liquidGlassHorizontalInsetPt: CGFloat = 20.0

    @Environment(\.scenePhase) private var scenePhase
    @State private var coordinator: FabushiAppCoordinator
    @State private var isTabLensLifted = false
    @Namespace private var tabLensNamespace

    public init(coordinator: FabushiAppCoordinator = .shared) {
        self._coordinator = State(initialValue: coordinator)
    }

    public var body: some View {
        let palette = coordinator.themeEngine.palette
        GeometryReader { geo in
            ZStack(alignment: .bottom) {
                palette.plainBackground.ignoresSafeArea()

                #if canImport(UIKit)
                TelegramRootSplitContainer(
                    primaryView: AnyView(primaryTabContent(palette: palette)),
                    secondaryView: AnyView(secondaryStageContent(palette: palette)),
                    showSecondaryInCompact: coordinator.router.activeChatId != nil,
                    onSelectTabShortcut: { tab in
                        coordinator.router.selectTab(tab)
                        coordinator.refreshSemanticSnapshot(viewportSize: geo.size)
                    },
                    onFocusSearchShortcut: {
                        coordinator.router.selectTab(.chats)
                    },
                    onCycleThemeShortcut: {
                        coordinator.themeEngine.cycleThemeMode()
                    },
                    onToggleLanguageShortcut: {
                        coordinator.localization.toggleLanguage()
                    },
                    onLockAppShortcut: {
                        coordinator.isAppLocked = true
                    }
                )
                .ignoresSafeArea()
                #else
                primaryTabContent(palette: palette)
                #endif

                VStack(spacing: 0) {
                    EdgeEffectView(position: .top, tintColor: palette.navOpaque)
                    Spacer()
                    EdgeEffectView(position: .bottom, tintColor: palette.navOpaque)
                }
                .ignoresSafeArea()

                tabBarView(palette: palette, viewportSize: geo.size)

                if let toast = coordinator.undoToastMessage {
                    UndoToastBanner(
                        message: toast,
                        undoTitle: coordinator.localization.tr("action.undo"),
                        remainingSeconds: coordinator.undoRemainingSeconds,
                        onUndo: { coordinator.undoToastMessage = nil }
                    )
                    .padding(.bottom, (coordinator.themeEngine.useLiquidGlassChrome ? Self.liquidGlassBarHeightPt : Self.classicBarHeightPt) + 18)
                }

                SheetContainer(isPresented: $coordinator.isAccountSwitcherPresented) {
                    accountSwitcherSheetContent(palette: palette)
                }

                GlassAlertPresenter(alert: coordinator.activeAlert) { _ in
                    Task { @MainActor in
                        coordinator.activeAlert = nil
                    }
                }

                if coordinator.isAppLocked {
                    lockScreenOverlay(palette: palette)
                }

                if coordinator.isUITestMode {
                    debugSemanticTreeOverlay(viewportSize: geo.size)
                }
            }
            .preferredColorScheme(coordinator.themeEngine.colorScheme)
            .environment(\.telegramThemeEngine, coordinator.themeEngine)
            .environment(\.localizationManager, coordinator.localization)
            .environment(\.appRouter, coordinator.router)
            .task {
                await coordinator.bootstrap()
                coordinator.refreshSemanticSnapshot(viewportSize: geo.size)
            }
            .onChange(of: scenePhase) { _, newPhase in
                Task {
                    await coordinator.handleScenePhase(newPhase)
                }
            }
            .onOpenURL { url in
                _ = coordinator.router.handleURL(url)
                coordinator.refreshSemanticSnapshot(viewportSize: geo.size)
            }
        }
    }

    // MARK: - Primary & Secondary Split Slots (Clean M2–M5 Feature Seams)

    @ViewBuilder
    private func primaryTabContent(palette: TelegramThemePalette) -> some View {
        switch coordinator.router.selectedTab {
        case .contacts:
            VStack(spacing: 12) {
                Text(coordinator.localization.tr("tab.contacts"))
                    .font(.system(size: 20, weight: .semibold))
                    .foregroundStyle(palette.primaryText)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(palette.plainBackground)
            .accessibilityIdentifier("fabushi.contacts.list")

        case .calls:
            VStack(spacing: 12) {
                Text(coordinator.localization.tr("tab.calls"))
                    .font(.system(size: 20, weight: .semibold))
                    .foregroundStyle(palette.primaryText)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(palette.plainBackground)
            .accessibilityIdentifier("fabushi.calls.segmentControl")

        case .chats:
            ChatListView(
                viewModel: ChatListViewModel(),
                coordinator: TelegramTabNavigationCoordinator(),
                onSelectChat: { summary in
                    coordinator.router.navigate(to: .openChat(chatId: summary.id, highlightMessageId: nil))
                }
            )
            .accessibilityIdentifier("fabushi.chatlist.collectionView")

        case .settings:
            VStack(spacing: 16) {
                Text(coordinator.localization.tr("tab.settings"))
                    .font(.system(size: 20, weight: .semibold))
                    .foregroundStyle(palette.primaryText)

                HStack(spacing: 12) {
                    ForEach(TelegramThemeMode.allCases) { mode in
                        Button(mode.rawValue) {
                            coordinator.themeEngine.currentMode = mode
                        }
                        .buttonStyle(.bordered)
                        .accessibilityIdentifier("fabushi.settings.appearance.theme.\(mode.rawValue)")
                    }
                }
                .padding(16)
                .telegramGroupedCard(
                    useLiquidGlass: coordinator.themeEngine.useLiquidGlassChrome,
                    cardColor: palette.cardBackground
                )
            }
            .padding()
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(palette.blocksBackground)
            .accessibilityIdentifier("fabushi.settings.list")
        }
    }

    @ViewBuilder
    private func secondaryStageContent(palette: TelegramThemePalette) -> some View {
        if let activeChatId = coordinator.router.activeChatId {
            ConversationStageView(
                viewModel: ConversationStageViewModel(chatId: activeChatId)
            )
            .accessibilityIdentifier("fabushi.chat.transcript")
        } else {
            VStack(spacing: 10) {
                Image(systemName: "bubble.left.and.bubble.right")
                    .font(.system(size: 36))
                    .foregroundStyle(palette.secondaryText)
                Text(coordinator.localization.tr("stage.empty"))
                    .font(TelegramTypography.body(coordinator.themeEngine.fontSizeStep))
                    .foregroundStyle(palette.secondaryText)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(palette.plainBackground)
        }
    }

    // MARK: - 62pt Liquid Glass Floating Capsule / 49pt Classic Tab Bar

    @ViewBuilder
    private func tabBarView(palette: TelegramThemePalette, viewportSize: CGSize) -> some View {
        let isGlass = coordinator.themeEngine.useLiquidGlassChrome
        HStack(spacing: 0) {
            ForEach(RootTab.allCases) { tab in
                let isSelected = (coordinator.router.selectedTab == tab)
                Button {
                    withAnimation(.spring(response: 0.28, dampingFraction: 0.78)) {
                        coordinator.router.selectTab(tab)
                    }
                    coordinator.refreshSemanticSnapshot(viewportSize: viewportSize)
                } label: {
                    ZStack {
                        if isGlass && isSelected {
                            LiquidLensView(
                                accentColor: coordinator.themeEngine.effectiveAccentColor,
                                isLifted: isTabLensLifted,
                                isDark: palette.overallDarkAppearance
                            )
                            .matchedGeometryEffect(id: "activeTabLens", in: tabLensNamespace)
                            .padding(.horizontal, 6)
                            .padding(.vertical, 5)
                            .accessibilityIdentifier("fabushi.tabbar.liquidLens")
                        }

                        VStack(spacing: 2) {
                            ZStack(alignment: .topTrailing) {
                                Image(systemName: tab.sfSymbol)
                                    .font(.system(size: 22, weight: isSelected ? .semibold : .regular))

                                if tab == .chats && coordinator.unreadChatsCount > 0 {
                                    Text("\(coordinator.unreadChatsCount)")
                                        .font(.system(size: 11, weight: .semibold).monospacedDigit())
                                        .foregroundStyle(.white)
                                        .padding(.horizontal, 5)
                                        .frame(minWidth: 18, minHeight: 18)
                                        .background(Capsule().fill(Color(tgHex: 0xFF3B30)))
                                        .offset(x: 10, y: -6)
                                        .accessibilityIdentifier("fabushi.tabbar.badge.chats")
                                }
                            }

                            Text(coordinator.localization.tr(tab.localizationKey))
                                .font(.system(size: 10, weight: isSelected ? .semibold : .medium))
                        }
                        .foregroundStyle(
                            isSelected ? coordinator.themeEngine.effectiveAccentColor : palette.secondaryText
                        )
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                    }
                }
                .buttonStyle(.plain)
                .simultaneousGesture(
                    LongPressGesture(minimumDuration: 0.45).onEnded { _ in
                        if tab == .settings {
                            coordinator.isAccountSwitcherPresented = true
                            coordinator.refreshSemanticSnapshot(viewportSize: viewportSize)
                        }
                    }
                )
                .accessibilityIdentifier(tab.accessibilityId)
            }
        }
        .frame(height: isGlass ? Self.liquidGlassBarHeightPt : Self.classicBarHeightPt)
        .background {
            if isGlass {
                GlassBackgroundView(
                    cornerRadius: Self.liquidGlassBarHeightPt / 2.0,
                    isDark: palette.overallDarkAppearance,
                    isInteractivePressed: isTabLensLifted,
                    fallbackColor: palette.navOpaque
                )
            } else {
                palette.navOpaque
                    .overlay(alignment: .top) {
                        Rectangle()
                            .fill(palette.separator)
                            .frame(height: 0.5)
                    }
            }
        }
        .padding(.horizontal, isGlass ? Self.liquidGlassHorizontalInsetPt : 0)
        .padding(.bottom, isGlass ? 6 : 0)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("fabushi.tabbar.container")
    }

    // MARK: - Multi-Account Switcher Sheet (`fabushi.accountSwitcher.*`)

    @ViewBuilder
    private func accountSwitcherSheetContent(palette: TelegramThemePalette) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            ForEach(Array(coordinator.accounts.enumerated()), id: \.element.userId) { index, acct in
                Button {
                    coordinator.isAccountSwitcherPresented = false
                } label: {
                    HStack(spacing: 12) {
                        PeerAvatarView(peerId: Int64(acct.userId), title: acct.username, diameter: 40)
                        Text(acct.username)
                            .font(.system(size: 16, weight: .medium))
                            .foregroundStyle(palette.primaryText)
                        Spacer()
                    }
                    .padding(.vertical, 6)
                }
                .accessibilityIdentifier("fabushi.accountSwitcher.slot.\(index)")
            }

            Button {
                coordinator.isAccountSwitcherPresented = false
            } label: {
                Label(coordinator.localization.tr("action.add_account"), systemImage: "plus.circle.fill")
                    .font(.system(size: 16, weight: .semibold))
            }
            .accessibilityIdentifier("fabushi.accountSwitcher.addAccount")

            Spacer()
        }
        .padding(20)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("fabushi.accountSwitcher.sheet")
    }

    // MARK: - Passcode / Face ID Lock Screen (`fabushi.lockscreen.*`)

    @ViewBuilder
    private func lockScreenOverlay(palette: TelegramThemePalette) -> some View {
        VStack(spacing: 20) {
            Text(coordinator.localization.tr("lock.title"))
                .font(.system(size: 20, weight: .semibold))
                .foregroundStyle(palette.primaryText)

            SecureField("PIN", text: $coordinator.passcodeInput)
                .textFieldStyle(.roundedBorder)
                .frame(maxWidth: 220)
                .accessibilityIdentifier("fabushi.lockscreen.pinInput")

            if let err = coordinator.passcodeError {
                Text(err)
                    .font(.system(size: 13))
                    .foregroundStyle(palette.destructive)
                    .accessibilityIdentifier("fabushi.lockscreen.errorLabel")
            }

            Button(coordinator.localization.tr("lock.biometrics")) {
                Task {
                    if await coordinator.biometrics.authenticate(reason: "Unlock Fabushi") {
                        coordinator.isAppLocked = false
                    }
                }
            }
            .buttonStyle(.borderedProminent)
            .accessibilityIdentifier("fabushi.lockscreen.biometricButton")
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(.ultraThinMaterial)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("fabushi.lockscreen.overlay")
    }

    // MARK: - Opaque-Box Semantic Tree Export Hook (`fabushi.debug.*`)

    @ViewBuilder
    private func debugSemanticTreeOverlay(viewportSize: CGSize) -> some View {
        HStack(spacing: 1) {
            Text(coordinator.semanticExporter.latestTree.stateHash)
                .font(.system(size: 1))
                .opacity(0.01)
                .accessibilityIdentifier("fabushi.debug.semanticTree")
                .accessibilityValue(coordinator.semanticExporter.latestJSON)

            Button("") {
                coordinator.refreshSemanticSnapshot(viewportSize: viewportSize)
            }
            .frame(width: 1, height: 1)
            .opacity(0.01)
            .accessibilityIdentifier("fabushi.debug.exportSemanticTree")
        }
        .frame(width: 2, height: 2)
    }
}
