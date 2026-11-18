import Foundation
import Observation
import SwiftUI

/// Root tab bar destinations (`TelegramRootController` 4-tab bar).
public enum RootTab: String, CaseIterable, Codable, Sendable, Identifiable {
    case contacts
    case calls
    case chats
    case settings

    public var id: String { rawValue }

    public var accessibilityId: String {
        "fabushi.tabbar.tab.\(rawValue)"
    }

    public var sfSymbol: String {
        switch self {
        case .contacts: return "person.crop.circle.fill"
        case .calls: return "phone.fill"
        case .chats: return "bubble.left.and.bubble.right.fill"
        case .settings: return "gearshape.fill"
        }
    }

    public var localizationKey: String {
        "tab.\(rawValue)"
    }
}

/// All 26 `SettingsSectionType` destinations from `gpui_telegram` (`F39`).
public enum SettingsSectionID: String, CaseIterable, Codable, Sendable, Identifiable {
    case editProfile
    case myProfile
    case savedMessages
    case recentCalls
    case devices
    case chatFolders
    case notificationsAndSounds
    case privacyAndSecurity
    case passcodeAndBiometrics
    case twoStepVerification
    case passkeys
    case dataAndStorage
    case storageUsage
    case proxySettings
    case appearance
    case language
    case stickersAndEmoji
    case walletTon
    case telegramStars
    case alipayMembership
    case hermesStudio
    case magpieGateway
    case miniAppsCenter
    case remoteComputers
    case keyboardShortcuts
    case aboutTelegram

    public var id: String { rawValue }
}

/// All 7 tabs of Hermes Agent Studio (`F41`).
public enum HermesStudioTab: String, CaseIterable, Codable, Sendable, Identifiable {
    case tasks
    case skills
    case memory
    case mcp
    case cron
    case sessions
    case approvals

    public var id: String { rawValue }
}

/// Strongly-typed navigation destination across compact stack & regular split shell (`PROJECT.md § IC.3`).
public enum AppDestination: Hashable, Codable, Sendable {
    case contacts
    case calls
    case chats(folderId: Int?)
    case settings(section: SettingsSectionID?)
    case openChat(chatId: Int64, highlightMessageId: Int64?)
    case openPeerInfo(peerId: Int64)
    case openHermesStudio(tab: HermesStudioTab)
    case openMiniApp(appId: String, initUrl: String?)
    case openFabushiGlobalDharma
    case openComputerWorkbench(nodeId: String?)
    case openCallOverlay(callId: String)
    case openTalkMode
    case alipayAuthCallback(authCode: String, state: String?)
    case joinInvite(code: String)
    case resolvePeerDomain(domain: String)
    case tonTransfer(address: String, amountNano: String?)
    case magpieImport(payload: String)
}

@MainActor
public protocol AppRouterProtocol: AnyObject {
    var selectedTab: RootTab { get set }
    var activeChatId: Int64? { get set }
    var activeFolderId: Int? { get set }
    var activeModalDestination: AppDestination? { get set }
    var navigationPath: [AppDestination] { get set }

    func selectTab(_ tab: RootTab)
    func navigate(to destination: AppDestination)
    func dismissModal()
    @discardableResult
    func handleURL(_ url: URL) -> Bool
}

/// Central navigation & custom URL scheme router (`fabushi://`, `gpui-telegram://`, `tg://`, `ton://`, `magpie://`).
@MainActor
@Observable
public final class AppRouter: AppRouterProtocol {
    public static let shared = AppRouter()
    public static let supportedSchemes: Set<String> = [
        "fabushi",
        "gpui-telegram",
        "tg",
        "ton",
        "magpie",
    ]

    public var selectedTab: RootTab = .chats
    public var activeChatId: Int64?
    public var activeFolderId: Int?
    public var activeModalDestination: AppDestination?
    public var navigationPath: [AppDestination] = []
    public var chatsTabReselectCount: Int = 0
    public var lastHandledURL: URL?

    public init() {}

    public func selectTab(_ tab: RootTab) {
        if selectedTab == tab && tab == .chats {
            chatsTabReselectCount += 1
            activeFolderId = nil
        }
        selectedTab = tab
    }

    public func navigate(to destination: AppDestination) {
        switch destination {
        case .contacts:
            selectedTab = .contacts
        case .calls:
            selectedTab = .calls
        case .chats(let folderId):
            selectedTab = .chats
            activeFolderId = folderId
        case .settings(let section):
            selectedTab = .settings
            if section != nil {
                navigationPath.append(destination)
            }
        case .openChat(let chatId, _):
            selectedTab = .chats
            activeChatId = chatId
            navigationPath.append(destination)
        case .openPeerInfo:
            navigationPath.append(destination)
        case .openHermesStudio,
             .openMiniApp,
             .openFabushiGlobalDharma,
             .openComputerWorkbench,
             .openCallOverlay,
             .openTalkMode,
             .alipayAuthCallback,
             .joinInvite,
             .resolvePeerDomain,
             .tonTransfer,
             .magpieImport:
            activeModalDestination = destination
        }
    }

    public func dismissModal() {
        activeModalDestination = nil
    }

    @discardableResult
    public func handleURL(_ url: URL) -> Bool {
        guard let rawScheme = url.scheme?.lowercased(),
              Self.supportedSchemes.contains(rawScheme)
        else {
            return false
        }
        let components = URLComponents(url: url, resolvingAgainstBaseURL: false)
        let queryItems = components?.queryItems ?? []
        func query(_ name: String) -> String? {
            queryItems.first(where: { $0.name == name })?.value
        }

        let host = (url.host ?? "").lowercased()
        let path = url.path

        switch rawScheme {
        case "fabushi", "gpui-telegram":
            if (host == "auth" && path.contains("alipay")) || path.contains("auth/alipay/callback") {
                guard let code = query("auth_code"), !code.isEmpty else { return false }
                lastHandledURL = url
                navigate(to: .alipayAuthCallback(authCode: code, state: query("state")))
                return true
            }
            if host == "chat", let idStr = query("id"), let cid = Int64(idStr) {
                lastHandledURL = url
                let mid = query("msg_id").flatMap(Int64.init)
                navigate(to: .openChat(chatId: cid, highlightMessageId: mid))
                return true
            }
            if host == "global-dharma" || host == "dharma" {
                lastHandledURL = url
                navigate(to: .openFabushiGlobalDharma)
                return true
            }
            if host == "hermes" {
                let tab = query("tab").flatMap(HermesStudioTab.init(rawValue:)) ?? .tasks
                lastHandledURL = url
                navigate(to: .openHermesStudio(tab: tab))
                return true
            }
            if host == "miniapp", let appId = query("app_id"), !appId.isEmpty {
                lastHandledURL = url
                navigate(to: .openMiniApp(appId: appId, initUrl: query("url")))
                return true
            }
            if host == "computers" {
                lastHandledURL = url
                navigate(to: .openComputerWorkbench(nodeId: query("node")))
                return true
            }
            return false

        case "tg":
            if host == "join", let invite = query("invite"), !invite.isEmpty {
                lastHandledURL = url
                navigate(to: .joinInvite(code: invite))
                return true
            }
            if host == "resolve", let domain = query("domain"), !domain.isEmpty {
                lastHandledURL = url
                navigate(to: .resolvePeerDomain(domain: domain))
                return true
            }
            return false

        case "ton":
            if host == "transfer" {
                let address = path.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
                guard !address.isEmpty else { return false }
                lastHandledURL = url
                navigate(to: .tonTransfer(address: address, amountNano: query("amount")))
                return true
            }
            return false

        case "magpie":
            if host == "import", let payload = query("payload") ?? query("config"), !payload.isEmpty {
                lastHandledURL = url
                navigate(to: .magpieImport(payload: payload))
                return true
            }
            return false

        default:
            return false
        }
    }
}

private struct AppRouterKey: EnvironmentKey {
    @MainActor static let defaultValue = AppRouter.shared
}

public extension EnvironmentValues {
    var appRouter: AppRouter {
        get { self[AppRouterKey.self] }
        set { self[AppRouterKey.self] = newValue }
    }
}
