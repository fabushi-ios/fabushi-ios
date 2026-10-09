import Foundation

enum IOSDevControlDisposition: Equatable, Sendable {
    case local
    case remoteRunner
    case unavailable(reason: String)
}

enum IOSDevControlsContract {
    static let contractName = "dev-controls"

    static func disposition(for method: String) -> IOSDevControlDisposition {
        switch method {
        case "restartOnboarding", "skipOnboarding", "themeStatus", "setThemePreference",
             "setWidgetGallery", "gatewayOfflineStatus", "setGatewayOffline",
             "networkLatencyStatus", "setNetworkLatency":
            return .local
        case "boxStatus", "boxHealth", "upgradeHost", "pokeHostUpgrade", "rebuildBox",
             "tailLogs", "startBox", "teardownBox", "nukeBox", "openDesktop",
             "boxStoreStatus", "boxStoreSnapshotNow", "boxStoreLogs", "boxStoreRecreateFresh",
             "boxStoreClear", "attachProdBoxStatus", "setAttachProdBoxEnabled":
            return .remoteRunner
        case "restartElectron", "reloadWindow":
            return .unavailable(reason: "Electron process/window controls do not exist on iOS")
        case "onePasswordCliStatus", "prepareOnePasswordCli", "cancelOnePasswordCliPrepare",
             "onePasswordAccounts", "onePasswordVaults", "onePasswordFindVault",
             "onePasswordSyntheticProvisioning":
            return .unavailable(reason: "Desktop 1Password CLI controls are replaced by native iOS credential/keychain flows")
        default:
            return .unavailable(reason: "Unknown or unsupported iOS developer control")
        }
    }
}
