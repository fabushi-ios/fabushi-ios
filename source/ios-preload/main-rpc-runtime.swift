import Foundation

enum IOSRPCArgumentShape: String, Equatable, Sendable {
    case none
    case object
}

enum IOSMainRPCEventDelivery: String, Equatable, Sendable {
    case nativeStateOwner = "native-state-owner"
    case nativeNavigationOwner = "native-navigation-owner"
    case platformMechanismReplacement = "platform-mechanism-replacement"
}

struct IOSMainRPCEventDisposition: Equatable, Sendable {
    let delivery: IOSMainRPCEventDelivery
    let ownerPath: String
    let ownerSymbol: String
    let productEffect: String
}

enum IOSMainRPCRuntime {
    static let contractName = "main"
    static let eventFamily = "events"
    static let eventNames: [String] = [
        "box-migration",
        "cursor-auth-changed",
        "deep-link",
        "dev-box-pull-progress",
        "dev-box-rebuild",
        "egress-tunnel-changed",
        "egress-tunnel-status-changed",
        "experiments-changed",
        "focus-agent",
        "force-onboarding",
        "open-about",
        "open-feedback",
        "skip-onboarding",
        "theme-changed",
        "update-status",
        "vnc-user-presence",
        "window-state",
        "webauthn-proxy-changed",
        "zoom-factor-changed",
    ]

    /// Desktop main uses renderer event transport. iOS preserves the named
    /// contract for source accounting, but each event resolves to one existing
    /// native owner or an explicit platform-mechanism replacement. isEvent is
    /// backed by this table so a newly named Desktop event fails closed until
    /// its iOS disposition is reviewed.
    static let eventDispositions: [String: IOSMainRPCEventDisposition] = [
        "box-migration": .init(delivery: .nativeStateOwner, ownerPath: "mobile/ios/Fabushi/RemoteComputerSurface.swift", ownerSymbol: "RemoteComputerRebuildOwner", productEffect: "Hydrates, fences, and presents the durable Computer migration lifecycle without renderer event fanout."),
        "cursor-auth-changed": .init(delivery: .nativeStateOwner, ownerPath: "source/ios-main/account/cursor-auth.swift", ownerSymbol: "IOSCursorAuthService", productEffect: "Publishes canonical Cursor authentication status to native account and MCP consumers."),
        "deep-link": .init(delivery: .nativeNavigationOwner, ownerPath: "source/ios-main/deep-link/deep-link-controller.swift", ownerSymbol: "IOSDeepLinkController", productEffect: "Validates, deduplicates, bounds, and dispatches native deep-link navigation."),
        "dev-box-pull-progress": .init(delivery: .nativeStateOwner, ownerPath: "mobile/ios/Fabushi/RemoteComputerSurface.swift", ownerSymbol: "RemoteComputerShellModel", productEffect: "Projects Computer image-pull progress from the canonical status snapshot into the native Computer surface."),
        "dev-box-rebuild": .init(delivery: .nativeStateOwner, ownerPath: "mobile/ios/Fabushi/RemoteComputerSurface.swift", ownerSymbol: "RemoteComputerRebuildOwner", productEffect: "Owns update, reset, recover, reconnect state, operation fencing, and terminal settlement."),
        "egress-tunnel-changed": .init(delivery: .nativeStateOwner, ownerPath: "source/shared/node/egress-tunnel/egress-tunnel-controller.swift", ownerSymbol: "EgressTunnelController", productEffect: "Reconciles enabled and configuration state through the single native egress tunnel owner."),
        "egress-tunnel-status-changed": .init(delivery: .nativeStateOwner, ownerPath: "source/shared/node/egress-tunnel/egress-tunnel-controller.swift", ownerSymbol: "EgressTunnelController", productEffect: "Publishes native egress tunnel status from the canonical controller callback."),
        "experiments-changed": .init(delivery: .nativeStateOwner, ownerPath: "source/shared/node/experiments/cursor-experiments.swift", ownerSymbol: "SandExperimentService", productEffect: "Delivers bounded experiment snapshots through the service subscription owner."),
        "focus-agent": .init(delivery: .nativeNavigationOwner, ownerPath: "frontend/src/production/GrokMobileShell.swift", ownerSymbol: "GrokMobileShell.selectBotForConversation(_:)", productEffect: "Moves the canonical mobile roster selection to the requested Agent conversation."),
        "force-onboarding": .init(delivery: .nativeNavigationOwner, ownerPath: "mobile/ios/Fabushi/MarketplaceModel.swift", ownerSymbol: "MarketplaceModel.signedInOnboardingStep", productEffect: "Uses account-scoped native onboarding route state instead of an Electron navigation event."),
        "open-about": .init(delivery: .nativeNavigationOwner, ownerPath: "frontend/src/recovered/features/account/session/menu.view.swift", ownerSymbol: "AccountMenuView", productEffect: "Presents the canonical FabushiAboutOverlayView from native account and settings navigation."),
        "open-feedback": .init(delivery: .nativeNavigationOwner, ownerPath: "frontend/src/recovered/features/account/session/menu.view.swift", ownerSymbol: "AccountFeedbackView", productEffect: "Presents the canonical feedback surface and submits through MarketplaceModel.submitAccountFeedback."),
        "skip-onboarding": .init(delivery: .nativeNavigationOwner, ownerPath: "mobile/ios/Fabushi/MarketplaceModel.swift", ownerSymbol: "MarketplaceModel.skipSignedInOnboarding()", productEffect: "Settles account-scoped onboarding completion through the canonical native settings bridge."),
        "theme-changed": .init(delivery: .nativeStateOwner, ownerPath: "frontend/src/production/MobileUiPreferences.swift", ownerSymbol: "MobileUiPreferencesStore", productEffect: "Applies system and native presentation preferences through SwiftUI environment state without renderer theme events."),
        "update-status": .init(delivery: .nativeStateOwner, ownerPath: "source/ios-main/update/update-wiring.swift", ownerSymbol: "IOSUpdateServiceWiring", productEffect: "Projects App Store update status while rejecting non-portable Desktop self-install actions."),
        "vnc-user-presence": .init(delivery: .nativeStateOwner, ownerPath: "source/shared/vnc-viewer-visibility.swift", ownerSymbol: "VNCViewerVisibilityContract", productEffect: "Derives native viewer visibility and presence for the single Remote Computer surface."),
        "window-state": .init(delivery: .platformMechanismReplacement, ownerPath: "frontend/src/production/FabushiSceneRoot.swift", ownerSymbol: "FabushiProductionSceneRoot", productEffect: "SwiftUI Scene lifecycle replaces desktop minimize, maximize, and window-chrome state; no synthetic Electron window event is emitted."),
        "webauthn-proxy-changed": .init(delivery: .nativeStateOwner, ownerPath: "source/shared/webauthn-proxy-availability.swift", ownerSymbol: "sandWebauthnProxyMirroredEnablement", productEffect: "Projects native iOS and iPadOS WebAuthn availability into the passkey and security owner."),
        "zoom-factor-changed": .init(delivery: .platformMechanismReplacement, ownerPath: "frontend/src/production/MobileUiPreferences.swift", ownerSymbol: "MobileUiPreferencesStore", productEffect: "Native Dynamic Type, text scale, and view gestures replace Electron renderer zoom-factor events."),
    ]
    static let methodTable: [String: IOSRPCArgumentShape] = [
        "openExternal": .object,
        "submitFeedback": .object,
        "getDesktopEnvironment": .none,
        "getWindowState": .none,
        "minimizeWindow": .none,
        "toggleMaximizeWindow": .none,
        "closeWindow": .none,
        "resizeWindowWidth": .object,
        "setTitleBarOverlayTone": .object,
        "getThemeState": .none,
        "setThemePreference": .object,
        "getEgressTunnelEnabled": .none,
        "setEgressTunnelEnabled": .object,
        "getEgressTunnelStatus": .none,
        "getWebauthnProxyEnabled": .none,
        "setWebauthnProxyEnabled": .object,
        "getUpdateStatus": .none,
        "checkForUpdates": .none,
        "setUpdateTrack": .object,
        "quitAndInstallUpdate": .none,
        "setAutoUpdateWhenIdleOptIn": .object,
        "getBoxMigrationStatus": .none,
        "markDeepLinksReady": .none,
        "getOnboardingSeen": .none,
        "setOnboardingSeen": .object,
        "getTimeZone": .none,
        "setTimeZoneOverride": .object,
        "getAutoReviewInstructions": .none,
        "setAutoReviewInstructions": .object,
        "getLocalToolPermission": .none,
        "getLocalToolPermissionCeiling": .none,
        "setLocalToolPermission": .object,
        "recordLocalToolApproval": .object,
        "clearLocalToolApprovals": .none,
        "getSidebarCollapsed": .none,
        "setSidebarCollapsed": .object,
        "pickAvatarSource": .none,
        "pickAvatarFile": .none,
        "generateAgentAvatarImage": .object,
        "resolveAttachmentMedia": .object,
        "readAttachmentText": .object,
        "readAttachmentBytes": .object,
        "stageAttachmentBytes": .object,
        "downloadAttachment": .object,
        "commitStagedAttachments": .object,
        "discardStagedAttachment": .object,
        "forceRecreateComputer": .none,
        "updateComputer": .object,
        "forceReconnectGateway": .none,
        "getExperimentsSnapshot": .none,
        "applyFeatureFlagOverride": .object,
        "refreshFeatureFlags": .none,
        "startRpcTraceWindow": .none,
        "getAgentDefaultModel": .none,
        "setAgentDefaultModel": .object,
        "getComputerUseModel": .none,
        "setComputerUseModel": .object,
        "getHostPinnedAgents": .none,
        "setHostPinnedAgents": .object,
        "getHostSidebarSections": .none,
        "setHostSidebarSections": .object,
        "getAvailableModels": .none,
        "getInferenceRouter": .none,
        "setInferenceRouter": .object,
        "getBoxRuntime": .none,
        "setBoxRuntime": .object,
        "transcribeAudio": .object,
        "getCursorAuthStatus": .none,
        "loginCursor": .none,
        "cancelCursorLogin": .none,
        "logoutCursor": .none,
        "updateCursorAccountName": .object,
        "getCursorAvatar": .none,
        "getCursorWeeklyUsage": .none,
        "getCursorUsageSummary": .none,
        "getCursorPrReviewPreferences": .none,
        "getCursorPrivacyModeEnabled": .none,
        "getSandAccess": .none,
        "getSandAccessFresh": .none,
        "invokeCursorDashboardAction": .object,
        "cancelCursorSandTrial": .none,
        "reportAgentLoad": .object,
        "reportAccessBlocked": .object,
        "reportAgentsUnreachable": .object,
        "reportRecoveryAction": .object,
        "reportRebuildLifecycle": .object,
        "reportReconciliation": .object,
        "reportBoxVisibility": .object,
        "reportSendLatency": .object,
        "reportSendAck": .object,
        "reportReactionAck": .object,
        "reportRenderTtfr": .object,
        "reportRenderStream": .object,
        "reportVncSession": .object,
        "reportVncLiveness": .object,
        "reportOpenComputer": .object,
        "reportUpdatePrompt": .object,
        "reportSigninGate": .object,
        "reportOnboardingStep": .object,
        "reportClientFailure": .object,
        "openCloudAgent": .object,
        "getLinkMetadata": .object,
        "listAllAutomations": .none,
        "listSecrets": .none,
        "revealSecret": .object,
        "upsertSecrets": .object,
        "removeSecrets": .object,
        "getMcpState": .none,
        "getEffectivePlugins": .none,
        "getMcpCatalog": .none,
        "getMcpTeamPopularity": .none,
        "getMcpPluginLogo": .object,
        "installEntry": .object,
        "updatePluginInstall": .object,
        "removeMcpServer": .object,
        "uninstallPlugin": .object,
        "authenticateMcpServer": .object,
        "renameMcpAccount": .object,
        "removeMcpAccount": .object,
        "setMcpCustomInstructions": .object,
        "listMcpServerTools": .object,
        "toggleMcpToolDisabled": .object,
    ]

    static func isMethod(_ name: String) -> Bool {
        methodTable[name] != nil
    }

    static func isEvent(_ name: String) -> Bool {
        eventDispositions[name] != nil
    }

    static func eventDisposition(_ name: String) -> IOSMainRPCEventDisposition? {
        eventDispositions[name]
    }

    static func methodChannel(_ method: String) -> String {
        IOSRPCEdgeRuntime.methodChannel(edge: contractName, method: method)
    }

    static func eventChannel(_ event: String) -> String {
        IOSRPCEdgeRuntime.eventChannel(edge: contractName, event: event)
    }
}
