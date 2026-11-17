import Foundation
import XCTest

/// Base `XCTestCase` for opaque-box parity suites (`Tier1..Tier4`).
/// Keeps `XCTestCase` lifecycle hooks `nonisolated` for Swift 6 `-strict-concurrency=complete`
/// compatibility while isolating all `XCUIApplication` / `XCUIElement` helpers to `@MainActor`.
open class ParityTestCase: XCTestCase {
    public nonisolated(unsafe) var serverURL: URL!
    public nonisolated(unsafe) var peerClient: LoopbackPeerClient!
    public nonisolated(unsafe) var storageDir: String!

    open override func setUpWithError() throws {
        try super.setUpWithError()
        continueAfterFailure = false
        let rawURL =
            ProcessInfo.processInfo.environment["GPUI_TELEGRAM_SERVER_URL"]
            ?? "http://127.0.0.1:18080"
        guard let parsed = URL(string: rawURL) else {
            XCTFail("Invalid GPUI_TELEGRAM_SERVER_URL: \(rawURL)")
            return
        }
        self.serverURL = parsed
        self.peerClient = LoopbackPeerClient(baseURL: parsed)

        let uniqueDir = FileManager.default.temporaryDirectory
            .appendingPathComponent("fabushi-parity-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: uniqueDir, withIntermediateDirectories: true)
        self.storageDir = uniqueDir.path
    }

    open override func tearDownWithError() throws {
        if let dir = self.storageDir {
            try? FileManager.default.removeItem(atPath: dir)
        }
        try super.tearDownWithError()
    }

    public func uniqueSuffix() -> String {
        String(UUID().uuidString.replacingOccurrences(of: "-", with: "").lowercased().prefix(8))
    }

    @MainActor
    @discardableResult
    public func launchApp(
        customServerURL: String? = nil,
        customStorageDir: String? = nil,
        simulateOffline: Bool = false,
        locale: String = "en",
        resetStorage: Bool = false,
        extraEnv: [String: String] = [:]
    ) -> XCUIApplication {
        let activeStorage = customStorageDir ?? self.storageDir ?? ""
        if resetStorage && !activeStorage.isEmpty {
            try? FileManager.default.removeItem(atPath: activeStorage)
            try? FileManager.default.createDirectory(
                atPath: activeStorage,
                withIntermediateDirectories: true
            )
        }
        let app = XCUIApplication()
        app.launchEnvironment["GPUI_TELEGRAM_SERVER_URL"] =
            customServerURL ?? self.serverURL.absoluteString
        app.launchEnvironment["GPUI_TELEGRAM_STORAGE_DIR"] = activeStorage
        app.launchEnvironment["GPUI_TELEGRAM_SIMULATE_OFFLINE"] = simulateOffline ? "1" : "0"
        app.launchEnvironment["GPUI_TELEGRAM_LOCALE"] = locale
        for (key, value) in extraEnv {
            app.launchEnvironment[key] = value
        }
        app.launch()
        return app
    }

    @MainActor
    public func element(in app: XCUIApplication, _ id: String) -> XCUIElement {
        app.descendants(matching: .any)[id].firstMatch
    }

    @MainActor
    @discardableResult
    public func waitForElement(
        in app: XCUIApplication,
        _ id: String,
        timeout: TimeInterval = 10,
        file: StaticString = #filePath,
        line: UInt = #line
    ) -> XCUIElement {
        let el = element(in: app, id)
        XCTAssertTrue(
            el.waitForExistence(timeout: timeout),
            "Expected element '\(id)' to exist within \(timeout)s",
            file: file,
            line: line
        )
        return el
    }

    @MainActor
    public func waitForElementDisappearance(
        in app: XCUIApplication,
        _ id: String,
        timeout: TimeInterval = 10,
        file: StaticString = #filePath,
        line: UInt = #line
    ) {
        let el = element(in: app, id)
        XCTAssertTrue(
            el.waitForNonExistence(timeout: timeout),
            "Expected element '\(id)' to disappear within \(timeout)s",
            file: file,
            line: line
        )
    }

    @MainActor
    public func clearAndTypeText(in target: XCUIElement, text: String) {
        target.tap()
        if let existing = target.value as? String, !existing.isEmpty {
            let deletes = String(repeating: XCUIKeyboardKey.delete.rawValue, count: existing.count)
            target.typeText(deletes)
        }
        target.typeText(text)
    }

    @MainActor
    public func launchAuthenticatedUser(
        usernamePrefix: String = "user_a",
        password: String = "StrongPass123!",
        firstName: String = "UserA",
        locale: String = "en"
    ) async throws -> (app: XCUIApplication, session: PeerAuthSession) {
        let client = self.peerClient!
        let uname = "\(usernamePrefix)_\(uniqueSuffix())"
        let session = try await client.registerUser(
            username: uname,
            password: password,
            firstName: firstName,
            deviceName: "SeedSession"
        )
        let app = launchApp(locale: locale)
        let startBtn = element(in: app, AccessibilityIDs.Auth.welcomeStartButton)
        if startBtn.waitForExistence(timeout: 2) {
            startBtn.tap()
        }
        let modePicker = element(in: app, AccessibilityIDs.Auth.modePicker)
        if modePicker.waitForExistence(timeout: 2), modePicker.buttons["Password"].exists {
            modePicker.buttons["Password"].tap()
        }
        let userField = waitForElement(in: app, AccessibilityIDs.Auth.usernameField)
        clearAndTypeText(in: userField, text: uname)
        let passField = waitForElement(in: app, AccessibilityIDs.Auth.passwordField)
        clearAndTypeText(in: passField, text: password)
        waitForElement(in: app, AccessibilityIDs.Auth.submitButton).tap()
        waitForElement(in: app, AccessibilityIDs.Shell.tabChats, timeout: 12)
        return (app, session)
    }

    // MARK: - Canonical Telegram iOS Gestures

    @MainActor
    public func swipeLeftToReply(in app: XCUIApplication, messageId: Int64) {
        let bubble = waitForElement(in: app, AccessibilityIDs.Stage.bubble(messageId))
        let start = bubble.coordinate(withNormalizedOffset: CGVector(dx: 0.82, dy: 0.5))
        let end = start.withOffset(CGVector(dx: -64, dy: 0))
        start.press(forDuration: 0.06, thenDragTo: end)
    }

    @MainActor
    public func tapMicToToggleMode(in app: XCUIApplication) {
        let btn = waitForElement(in: app, AccessibilityIDs.Composer.micOrSendButton)
        btn.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).press(forDuration: 0.12)
    }

    @MainActor
    public func holdMicToRecordAndRelease(in app: XCUIApplication, duration: TimeInterval = 1.1) {
        let btn = waitForElement(in: app, AccessibilityIDs.Composer.micOrSendButton)
        btn.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).press(forDuration: max(0.65, duration))
    }

    @MainActor
    public func holdMicAndSlideLeftToCancel(in app: XCUIApplication) {
        let btn = waitForElement(in: app, AccessibilityIDs.Composer.micOrSendButton)
        let start = btn.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5))
        let cancelTarget = start.withOffset(CGVector(dx: -110, dy: 0))
        start.press(forDuration: 0.65, thenDragTo: cancelTarget)
    }

    @MainActor
    public func holdMicAndSlideUpToLock(in app: XCUIApplication) {
        let btn = waitForElement(in: app, AccessibilityIDs.Composer.micOrSendButton)
        let start = btn.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5))
        let lockTarget = start.withOffset(CGVector(dx: 0, dy: -72))
        start.press(forDuration: 0.65, thenDragTo: lockTarget)
    }

    @MainActor
    public func longPressBubbleForPortal(in app: XCUIApplication, messageId: Int64) {
        let bubble = waitForElement(in: app, AccessibilityIDs.Stage.bubble(messageId))
        bubble.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).press(forDuration: 0.7)
    }

    @MainActor
    public func longPressSettingsTabForAccountSwitcher(in app: XCUIApplication) {
        let tab = waitForElement(in: app, AccessibilityIDs.Shell.tabSettings)
        tab.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).press(forDuration: 0.75)
    }

    @MainActor
    public func pullDownChatsToRevealArchiveAndStories(in app: XCUIApplication) {
        let anchor = waitForElement(in: app, AccessibilityIDs.Chats.searchBar)
        let start = anchor.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 1.5))
        let end = start.withOffset(CGVector(dx: 0, dy: 180))
        start.press(forDuration: 0.08, thenDragTo: end)
    }

    @MainActor
    public func interactivePopSwipeBack(in app: XCUIApplication) {
        let root = app.windows.firstMatch
        let start = root.coordinate(withNormalizedOffset: CGVector(dx: 0.03, dy: 0.5))
        let end = root.coordinate(withNormalizedOffset: CGVector(dx: 0.82, dy: 0.5))
        start.press(forDuration: 0.06, thenDragTo: end)
    }
}
