import Foundation
import XCTest

@MainActor
public final class UITestSessionStore {
    public static let shared = UITestSessionStore()

    public var activeApp: XCUIApplication?
    public var activeConfig: AppLaunchConfiguration?

    private init() {}

    public func cleanupAfterTest() {
        if XCUIDevice.shared.orientation != .portrait {
            XCUIDevice.shared.orientation = .portrait
        }
        if let app = activeApp {
            if app.state != .notRunning {
                XCTContext.runActivity(named: "Final Screen State") { activity in
                    let attachment = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
                    attachment.name = "final-screen"
                    attachment.lifetime = .deleteOnSuccess
                    activity.add(attachment)
                }
                app.terminate()
            }
        }
        if let sandboxDir = activeConfig?.sandboxDirectory {
            try? FileManager.default.removeItem(at: sandboxDir)
        }
        activeApp = nil
        activeConfig = nil
    }
}

open class FabushiE2ETestCase: XCTestCase {
    public let oracle: LoopbackServerClient = LoopbackServerClient.fromEnvironment()

    override open func setUpWithError() throws {
        try super.setUpWithError()
        continueAfterFailure = false
    }

    override open func tearDown() async throws {
        await UITestSessionStore.shared.cleanupAfterTest()
        try await super.tearDown()
    }

    public func resolveSandboxRoot() -> URL {
        let env = ProcessInfo.processInfo.environment
        if let custom = env["TEST_RUNNER_FABUSHI_E2E_SANDBOX_ROOT"]
            ?? env["TEST_RUNNER_FABUSHI_SANDBOX_ROOT"]
            ?? env["FABUSHI_E2E_SANDBOX_ROOT"]
            ?? env["FABUSHI_SANDBOX_ROOT"],
           !custom.isEmpty
        {
            return URL(fileURLWithPath: custom, isDirectory: true)
        }
        return FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("fabushi_e2e_sandboxes", isDirectory: true)
    }

    @MainActor
    @discardableResult
    public func launchFabushi(_ config: AppLaunchConfiguration) throws -> XCUIApplication {
        try config.prepareSandboxOnDisk()
        let app = config.instantiateApplication()
        app.launchArguments = config.launchArguments
        let env = try config.makeEnvironment()
        for (k, v) in env {
            app.launchEnvironment[k] = v
        }
        UITestSessionStore.shared.activeConfig = config
        UITestSessionStore.shared.activeApp = app
        app.launch()
        return app
    }

    /// Launches a clean unauthenticated `Fabushi.app` instance targeting `oracle.baseURL`.
    @MainActor
    public func launchUnauthenticatedAuth(
        testName: String = #function,
        initialLocale: String = "en-US",
        networkFaultMode: NetworkFaultMode = .online,
        mockBiometrics: MockBiometricsResult = .success,
        extraEnvironment: [String: String] = [:]
    ) throws -> (app: XCUIApplication, config: AppLaunchConfiguration) {
        let config = AppLaunchConfiguration(
            serverURL: oracle.baseURL.absoluteString,
            sandboxRoot: resolveSandboxRoot(),
            testIdentifier: testName,
            initialLocale: initialLocale,
            networkFaultMode: networkFaultMode,
            mockBiometrics: mockBiometrics,
            extraEnvironment: extraEnvironment
        )
        let app = try launchFabushi(config)
        return (app, config)
    }

    /// Progressive-testability helper for R2/R3/R4: seeds a real user on loopback `gpui_telegram_server`
    /// and boots `Fabushi.app` directly into the authenticated workspace without touching R1 Auth UI.
    @MainActor
    public func launchPreauthenticatedWorkspace(
        testName: String = #function,
        initialLocale: String = "en-US",
        networkFaultMode: NetworkFaultMode = .online,
        mockBiometrics: MockBiometricsResult = .success,
        idleLockOverrideMs: Int? = nil,
        additionalAccounts: [AuthResponseDTO] = [],
        extraEnvironment: [String: String] = [:]
    ) async throws -> (app: XCUIApplication, auth: AuthResponseDTO, config: AppLaunchConfiguration) {
        let auth = try await oracle.registerRandomUser(prefix: "ws")
        var payloads = [PreseededSessionPayload(auth: auth, serverURL: oracle.baseURL.absoluteString)]
        for acct in additionalAccounts {
            payloads.append(PreseededSessionPayload(auth: acct, serverURL: oracle.baseURL.absoluteString))
        }
        let config = AppLaunchConfiguration(
            serverURL: oracle.baseURL.absoluteString,
            sandboxRoot: resolveSandboxRoot(),
            testIdentifier: testName,
            initialLocale: initialLocale,
            networkFaultMode: networkFaultMode,
            mockBiometrics: mockBiometrics,
            idleLockOverrideMs: idleLockOverrideMs,
            preseededAccounts: payloads,
            extraEnvironment: extraEnvironment
        )
        let app = try launchFabushi(config)
        let hydrated = app.descendants(matching: .any)[FabushiAID.Root.tabBarContainer].waitForExistence(timeout: 15)
            || app.descendants(matching: .any)[FabushiAID.Root.splitView].waitForExistence(timeout: 5)
        XCTAssertTrue(hydrated, "Pre-authenticated session must hydrate into root shell")
        return (app, auth, config)
    }

    @MainActor
    public func element(_ app: XCUIApplication, id: String) -> XCUIElement {
        app.descendants(matching: .any)[id].firstMatch
    }

    @MainActor
    @discardableResult
    public func requireElement(
        _ app: XCUIApplication,
        id: String,
        timeout: TimeInterval = 10.0,
        file: StaticString = #filePath,
        line: UInt = #line
    ) -> XCUIElement {
        let el = element(app, id: id)
        XCTAssertTrue(
            el.waitForExistence(timeout: timeout),
            "Expected accessibility element '\(id)' to exist within \(timeout)s",
            file: file,
            line: line
        )
        return el
    }

    @MainActor
    public func clearAndType(
        _ app: XCUIApplication,
        id: String,
        text: String,
        timeout: TimeInterval = 10.0,
        file: StaticString = #filePath,
        line: UInt = #line
    ) {
        let field = requireElement(app, id: id, timeout: timeout, file: file, line: line)
        field.tap()
        if let current = field.value as? String, !current.isEmpty {
            let deletes = String(repeating: XCUIKeyboardKey.delete.rawValue, count: current.count + 4)
            field.typeText(deletes)
        }
        if !text.isEmpty {
            field.typeText(text)
        }
    }

    @MainActor
    public func openChatFromInbox(
        _ app: XCUIApplication,
        chatId: Int64,
        timeout: TimeInterval = 12.0,
        file: StaticString = #filePath,
        line: UInt = #line
    ) {
        let row = requireElement(app, id: FabushiAID.ChatList.row(chatId), timeout: timeout, file: file, line: line)
        row.tap()
        _ = requireElement(app, id: FabushiAID.Chat.transcript, timeout: timeout, file: file, line: line)
    }

    @MainActor
    public func waitUntil(
        timeout: TimeInterval = 10.0,
        pollMs: UInt64 = 100,
        _ condition: @MainActor () -> Bool
    ) async throws -> Bool {
        let deadline = ContinuousClock.now + .milliseconds(Int(timeout * 1000))
        while ContinuousClock.now < deadline {
            if condition() { return true }
            try await Task.sleep(for: .milliseconds(pollMs))
        }
        return condition()
    }
}
