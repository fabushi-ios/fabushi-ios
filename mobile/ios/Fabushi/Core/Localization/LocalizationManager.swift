import Foundation
import Observation
import SwiftUI

/// Bilingual application language (`zh-CN` / `en-US`) matching `gpui_telegram` i18n (`F13`).
public enum AppLanguage: String, CaseIterable, Codable, Sendable, Identifiable {
    case zhCN = "zh-CN"
    case enUS = "en-US"

    public var id: String { rawValue }

    public static func normalize(_ raw: String?) -> AppLanguage {
        guard let trimmed = raw?.trimmingCharacters(in: .whitespacesAndNewlines).lowercased(),
              !trimmed.isEmpty
        else {
            return .enUS
        }
        if trimmed.hasPrefix("zh") {
            return .zhCN
        }
        return .enUS
    }

    public var toggled: AppLanguage {
        self == .zhCN ? .enUS : .zhCN
    }
}

/// Instant runtime localization engine (`Cmd+L`, Login, Settings) without app restart (`F13`).
@MainActor
@Observable
public final class LocalizationManager {
    public static let shared = LocalizationManager()

    private static let storageKey = "com.ombhrum.fabushi.app_language"

    public var language: AppLanguage {
        didSet {
            userDefaults.set(language.rawValue, forKey: Self.storageKey)
        }
    }

    private let userDefaults: UserDefaults

    public init(
        userDefaults: UserDefaults = .standard,
        environment: [String: String] = ProcessInfo.processInfo.environment
    ) {
        self.userDefaults = userDefaults
        if let envLang = environment["FABUSHI_INITIAL_LOCALE"] ?? environment["GPUI_TELEGRAM_LOCALE"],
           !envLang.isEmpty {
            self.language = AppLanguage.normalize(envLang)
        } else if let saved = userDefaults.string(forKey: Self.storageKey) {
            self.language = AppLanguage.normalize(saved)
        } else {
            self.language = .enUS
        }
    }

    @discardableResult
    public func toggleLanguage() -> AppLanguage {
        language = language.toggled
        return language
    }

    public func setLanguage(_ newLanguage: AppLanguage) {
        language = newLanguage
    }

    public func tr(_ key: String) -> String {
        switch language {
        case .zhCN:
            return Self.zhTable[key] ?? Self.enTable[key] ?? key
        case .enUS:
            return Self.enTable[key] ?? key
        }
    }

    private static let enTable: [String: String] = [
        "tab.contacts": "Contacts",
        "tab.calls": "Calls",
        "tab.chats": "Chats",
        "tab.settings": "Settings",
        "status.connecting": "Connecting...",
        "status.updating": "Updating...",
        "status.connected": "Chats",
        "status.offline": "Waiting for network...",
        "action.edit": "Edit",
        "action.done": "Done",
        "action.cancel": "Cancel",
        "action.undo": "Undo",
        "action.search": "Search",
        "action.add_account": "Add Account",
        "lock.title": "Enter Passcode",
        "lock.biometrics": "Unlock with Face ID",
        "lock.invalid": "Invalid Passcode",
        "stage.empty": "Select a chat to start messaging",
        "studio.hermes": "Hermes Studio",
        "miniapp.global_dharma": "Global Dharma",
        "computers.workbench": "Remote Computers",
    ]

    private static let zhTable: [String: String] = [
        "tab.contacts": "联系人",
        "tab.calls": "通话",
        "tab.chats": "对话",
        "tab.settings": "设置",
        "status.connecting": "连接中...",
        "status.updating": "更新中...",
        "status.connected": "对话",
        "status.offline": "等待网络...",
        "action.edit": "编辑",
        "action.done": "完成",
        "action.cancel": "取消",
        "action.undo": "撤销",
        "action.search": "搜索",
        "action.add_account": "添加账号",
        "lock.title": "输入锁定密码",
        "lock.biometrics": "使用面容 ID 解锁",
        "lock.invalid": "密码错误",
        "stage.empty": "选择一个会话开始聊天",
        "studio.hermes": "Hermes 工作台",
        "miniapp.global_dharma": "全球法布施",
        "computers.workbench": "远程电脑",
    ]
}

private struct LocalizationManagerKey: EnvironmentKey {
    @MainActor static let defaultValue = LocalizationManager.shared
}

public extension EnvironmentValues {
    var localizationManager: LocalizationManager {
        get { self[LocalizationManagerKey.self] }
        set { self[LocalizationManagerKey.self] = newValue }
    }
}
