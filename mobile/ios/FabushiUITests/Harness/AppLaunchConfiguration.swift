import Foundation
import XCTest

public enum NetworkFaultMode: String, Sendable, Codable, Equatable {
    case online = "online"
    case offline = "offline"
    case dropWebSocket = "drop_ws"
}

public enum MockBiometricsResult: String, Sendable, Codable, Equatable {
    case success = "success"
    case fail = "fail"
}

public struct PreseededSessionPayload: Codable, Sendable, Equatable {
    public let user_id: UInt64
    public let username: String
    public let session_id: String
    public let token: String
    public let server_url: String
    public let first_name: String?
    public let account_generation: UInt64

    public init(
        auth: AuthResponseDTO,
        serverURL: String,
        accountGeneration: UInt64 = 1
    ) {
        self.user_id = auth.user_id
        self.username = auth.username
        self.session_id = auth.session_id
        self.token = auth.token
        self.server_url = serverURL
        self.first_name = auth.first_name
        self.account_generation = auth.account_generation ?? accountGeneration
    }
}

public struct AppLaunchConfiguration: Sendable, Equatable {
    public static let canonicalBundleIdentifier = "com.ombhrum.fabushi"

    public var useExplicitBundleIdentifier: Bool
    public var serverURL: String
    public var sandboxDirectory: URL
    public var appSupportDirectory: URL
    public var semanticExportPath: URL
    public var initialLocale: String
    public var networkFaultMode: NetworkFaultMode
    public var mockBiometrics: MockBiometricsResult
    public var idleLockOverrideMs: Int?
    public var preseededAccounts: [PreseededSessionPayload]
    public var extraEnvironment: [String: String]
    public var launchArguments: [String]

    public init(
        serverURL: String,
        sandboxRoot: URL,
        testIdentifier: String,
        useExplicitBundleIdentifier: Bool = true,
        initialLocale: String = "en-US",
        networkFaultMode: NetworkFaultMode = .online,
        mockBiometrics: MockBiometricsResult = .success,
        idleLockOverrideMs: Int? = nil,
        preseededAccounts: [PreseededSessionPayload] = [],
        extraEnvironment: [String: String] = [:],
        launchArguments: [String] = []
    ) {
        let sanitized = testIdentifier.replacingOccurrences(
            of: "[^A-Za-z0-9_.-]",
            with: "_",
            options: .regularExpression
        )
        let dir = sandboxRoot.appendingPathComponent(
            "\(sanitized)-\(UUID().uuidString.prefix(8))",
            isDirectory: true
        )
        self.useExplicitBundleIdentifier = useExplicitBundleIdentifier
        self.serverURL = serverURL
        self.sandboxDirectory = dir
        self.appSupportDirectory = dir.appendingPathComponent("AppSupport", isDirectory: true)
        self.semanticExportPath = dir.appendingPathComponent("semantic_tree.json", isDirectory: false)
        self.initialLocale = initialLocale
        self.networkFaultMode = networkFaultMode
        self.mockBiometrics = mockBiometrics
        self.idleLockOverrideMs = idleLockOverrideMs
        self.preseededAccounts = preseededAccounts
        self.extraEnvironment = extraEnvironment
        self.launchArguments = launchArguments
    }

    public func prepareSandboxOnDisk() throws {
        let fm = FileManager.default
        try fm.createDirectory(at: appSupportDirectory, withIntermediateDirectories: true)
        if let primary = preseededAccounts.first {
            let encoder = JSONEncoder()
            encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
            let sessionData = try encoder.encode(primary)
            try sessionData.write(
                to: appSupportDirectory.appendingPathComponent("session.json"),
                options: .atomic
            )
        }
        if preseededAccounts.count > 1 {
            let encoder = JSONEncoder()
            encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
            let accountsData = try encoder.encode(preseededAccounts)
            try accountsData.write(
                to: appSupportDirectory.appendingPathComponent("accounts.json"),
                options: .atomic
            )
        }
    }

    public func makeEnvironment() throws -> [String: String] {
        var env: [String: String] = [
            "FABUSHI_UNIT_TEST_HOST": "0",
            "FABUSHI_UI_TEST_MODE": "1",
            "FABUSHI_SERVER_URL": serverURL,
            "GPUI_TELEGRAM_SERVER_URL": serverURL,
            "FABUSHI_APP_SUPPORT_DIR": appSupportDirectory.path,
            "GPUI_TELEGRAM_CONFIG_DIR": appSupportDirectory.path,
            "FABUSHI_SEMANTIC_EXPORT_PATH": semanticExportPath.path,
            "FABUSHI_INITIAL_LOCALE": initialLocale,
            "FABUSHI_NETWORK_FAULT_MODE": networkFaultMode.rawValue,
            "FABUSHI_MOCK_BIOMETRICS_RESULT": mockBiometrics.rawValue,
        ]
        if let idleMs = idleLockOverrideMs {
            env["FABUSHI_IDLE_LOCK_OVERRIDE_MS"] = String(idleMs)
        }
        if let primary = preseededAccounts.first {
            let data = try JSONEncoder().encode(primary)
            env["FABUSHI_PREAUTH_SESSION_JSON"] = String(decoding: data, as: UTF8.self)
        }
        if !preseededAccounts.isEmpty {
            let data = try JSONEncoder().encode(preseededAccounts)
            env["FABUSHI_PREAUTH_ACCOUNTS_JSON"] = String(decoding: data, as: UTF8.self)
        }
        for (key, value) in extraEnvironment {
            env[key] = value
        }
        return env
    }

    @MainActor
    public func instantiateApplication() -> XCUIApplication {
        if useExplicitBundleIdentifier {
            return XCUIApplication(bundleIdentifier: Self.canonicalBundleIdentifier)
        } else {
            return XCUIApplication()
        }
    }
}
