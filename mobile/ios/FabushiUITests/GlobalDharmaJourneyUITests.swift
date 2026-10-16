import Foundation
import XCTest

final class GlobalDharmaJourneyUITests: XCTestCase {
    private var state: [String: Any] = [
        "schema": "fabushi.ios.global-dharma-journey.v1",
        "autoLogin": false,
        "searchedMiniApps": false,
        "globalDharmaVisible": false,
        "installTapped": false,
        "installedOrAlreadyInstalled": false,
        "botVisible": false,
        "naturalLanguageSent": false,
        "webMcpReady": false,
        "sharedRuntimeSynced": false,
        "purchaseTapped": false,
        "entitlementAllowed": false,
        "restoreTapped": false,
    ]

    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    @MainActor
    func testGlobalDharmaMarketplaceBotWebMcpCommerceJourney() throws {
        let app = XCUIApplication()
        configureRealCIEnvironment(for: app)
        app.launch()

        let skip = app.buttons["mobile-onboarding-skip"]
        if skip.waitForExistence(timeout: 4) {
            skip.tap()
        }

        let grokHome = app.descendants(matching: .any)["grok-mobile-home"]
        XCTAssertTrue(
            grokHome.waitForExistence(timeout: 25),
            "Expected the protected Fabushi CI account to auto-login without browser interaction"
        )
        XCTAssertFalse(
            app.buttons["mobile-login-browser"].exists,
            "The packaged journey must reuse the bounded Fabushi account session instead of asking for interactive login"
        )
        mark("autoLogin", true)
        checkpoint("010-auto-login")

        let legacy = app.buttons["grok-mobile-legacy"]
        XCTAssertTrue(legacy.waitForExistence(timeout: 8))
        legacy.tap()
        XCTAssertTrue(app.descendants(matching: .any)["app-shell"].waitForExistence(timeout: 15))

        openMarketplace(in: app)
        let search = app.textFields["marketplace-search"]
        XCTAssertTrue(search.waitForExistence(timeout: 8))
        search.tap()
        search.typeText("小程序")
        let submit = app.buttons["marketplace-search-submit"]
        XCTAssertTrue(submit.exists)
        submit.tap()
        mark("searchedMiniApps", true)

        let plugin = app.descendants(matching: .any)["plugin-global-dharma"]
        let install = app.buttons["install-global-dharma"]
        let open = app.buttons["open-global-dharma"]
        XCTAssertTrue(
            scrollUntilAnyExists([plugin, install, open], in: app, maxSwipes: 10),
            "Searching 小程序 must surface 全球法布施"
        )
        mark("globalDharmaVisible", true)
        checkpoint("020-search-global-dharma")

        if install.exists && install.isHittable {
            install.tap()
            mark("installTapped", true)
            let approve = app.buttons["permission-approve"]
            if approve.waitForExistence(timeout: 5), approve.isHittable {
                approve.tap()
            }
        }

        XCTAssertTrue(
            waitForAny([open, app.descendants(matching: .any)["plugin-global-dharma"]], timeout: 30),
            "全球法布施 must remain installed/available after the installation flow"
        )
        mark("installedOrAlreadyInstalled", true)
        checkpoint("030-installed-global-dharma")

        leaveMarketplaceForBotHome(in: app)
        XCTAssertTrue(grokHome.waitForExistence(timeout: 15))

        let botLabel = app.staticTexts["全球法布施"].firstMatch
        XCTAssertTrue(
            scrollToElement(botLabel, in: app, maxSwipes: 8),
            "Installing 全球法布施 must project its Bot into the message area"
        )
        botLabel.tap()

        let botChat = app.descendants(matching: .any)["mobile-bot-chat"]
        XCTAssertTrue(botChat.waitForExistence(timeout: 15))
        let openApp = app.buttons["mobile-bot-open-app"]
        XCTAssertTrue(openApp.waitForExistence(timeout: 8), "Global Dharma Bot must expose 打开应用")
        mark("botVisible", true)
        checkpoint("040-global-dharma-bot")

        let draft = app.textFields["mobile-bot-draft"]
        XCTAssertTrue(draft.waitForExistence(timeout: 8))
        draft.tap()
        draft.typeText("请通过 WebMCP 启动本地转经轮，并把当前操作状态同步到小程序界面。")
        let send = app.buttons["mobile-bot-send"]
        XCTAssertTrue(send.waitForExistence(timeout: 8))
        send.tap()
        mark("naturalLanguageSent", true)

        let stop = app.buttons["mobile-bot-stop"]
        if stop.waitForExistence(timeout: 4) {
            let finished = XCTNSPredicateExpectation(
                predicate: NSPredicate(format: "exists == false"),
                object: stop
            )
            XCTAssertEqual(
                XCTWaiter.wait(for: [finished], timeout: 90),
                .completed,
                "Global Dharma Bot WebMCP operation did not reach a terminal state"
            )
        }
        checkpoint("050-bot-webmcp-natural-language")

        openApp.tap()
        let surface = app.descendants(matching: .any)["miniapp-webmcp-surface"]
        XCTAssertTrue(surface.waitForExistence(timeout: 15), "打开应用 must present the Mini App Web UI")

        let connected = app.staticTexts["WebMCP 已连接"]
        let localConnected = app.staticTexts["本地 WebMCP 已连接"]
        let opened = app.staticTexts["WebMCP 页面已打开"]
        XCTAssertTrue(
            waitForAny([connected, localConnected, opened], timeout: 20),
            "Mini App Web UI must expose a ready WebMCP bridge"
        )
        mark("webMcpReady", true)
        checkpoint("060-miniapp-webmcp-open")

        let sharedRuntimePrefix = "Bot / Web UI 同一共享状态 · revision "
        let sharedRuntime = app.descendants(matching: .any)
            .matching(NSPredicate(format: "label BEGINSWITH %@", sharedRuntimePrefix))
            .firstMatch
        XCTAssertTrue(
            sharedRuntime.waitForExistence(timeout: 30),
            "Opening the Global Dharma Web UI must restore the canonical account-scoped runtime and complete the read-only WebMCP status bridge"
        )
        let revisionText = String(sharedRuntime.label.dropFirst(sharedRuntimePrefix.count))
        guard let revision = Int(revisionText), revision > 0 else {
            XCTFail("Bot/Web UI synchronization must expose a positive canonical runtime revision, got: \(sharedRuntime.label)")
            return
        }
        state["sharedRuntimeRevision"] = revision
        mark("sharedRuntimeSynced", true)
        checkpoint("061-bot-webui-shared-runtime")

        let allowed = app.descendants(matching: .any)["global-dharma-entitlement-allowed"]
        let buy = app.buttons["global-dharma-buy-lifetime"]
        if !allowed.waitForExistence(timeout: 6) {
            XCTAssertTrue(buy.waitForExistence(timeout: 12), "Expected either existing entitlement or the ¥1080 lifetime purchase action")
            XCTAssertTrue(
                buy.label.contains("1080"),
                "Lifetime local prayer-wheel purchase must visibly preserve the CNY 1080 contract"
            )
            buy.tap()
            mark("purchaseTapped", true)
            XCTAssertTrue(
                allowed.waitForExistence(timeout: 60),
                "Canonical test-mode purchase did not project an allowed server entitlement"
            )
            checkpoint("070-purchase-entitlement")
        } else {
            checkpoint("070-preexisting-entitlement")
        }

        mark("entitlementAllowed", true)
        let restore = app.buttons["global-dharma-restore-purchase"]
        XCTAssertTrue(restore.waitForExistence(timeout: 10))
        restore.tap()
        mark("restoreTapped", true)
        XCTAssertTrue(
            allowed.waitForExistence(timeout: 30),
            "Restore must leave the server-authoritative lifetime entitlement allowed"
        )
        checkpoint("080-restore-entitlement")

        let close = app.buttons["miniapp-webmcp-close"]
        XCTAssertTrue(close.waitForExistence(timeout: 8))
        close.tap()
        XCTAssertTrue(waitForNonExistence(surface, timeout: 12))
        checkpoint("090-journey-complete")
        persistState()
    }

    @MainActor
    private func configureRealCIEnvironment(for app: XCUIApplication) {
        let environment = ProcessInfo.processInfo.environment
        if let session = environment["FABUSHI_CI_APP_SESSION_IN_SIMULATOR"], !session.isEmpty {
            app.launchEnvironment["FABUSHI_CI_ACCOUNT_SESSION_FILE"] = session
        }
        if let sourceSHA = environment["FABUSHI_E2E_SOURCE_SHA"], !sourceSHA.isEmpty {
            app.launchEnvironment["GITHUB_SHA"] = sourceSHA
        }
        for key in [
            "GITHUB_ACTIONS", "GITHUB_REPOSITORY", "GITHUB_WORKFLOW", "GITHUB_JOB",
            "GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT", "RUNNER_NAME", "RUNNER_OS", "RUNNER_ARCH",
            "FABUSHI_API_BASE_URL", "FABUSHI_DEVICE_NAME"
        ] {
            if let value = environment[key], !value.isEmpty {
                app.launchEnvironment[key] = value
            }
        }
    }

    @MainActor
    private func openMarketplace(in app: XCUIApplication) {
        let profile = app.buttons["profile-avatar"]
        XCTAssertTrue(profile.waitForExistence(timeout: 10))
        profile.tap()
        let marketplace = app.buttons["marketplace-entry"]
        XCTAssertTrue(marketplace.waitForExistence(timeout: 8))
        marketplace.tap()
        XCTAssertTrue(app.textFields["marketplace-search"].waitForExistence(timeout: 12))
    }

    @MainActor
    private func leaveMarketplaceForBotHome(in app: XCUIApplication) {
        let semanticBack = app.buttons["marketplace-back"]
        let labelledBack = app.buttons["消息"]
        if semanticBack.waitForExistence(timeout: 3), semanticBack.isHittable {
            semanticBack.tap()
        } else {
            XCTAssertTrue(labelledBack.waitForExistence(timeout: 5), "Expected Marketplace back-to-messages control")
            labelledBack.tap()
        }
        XCTAssertTrue(app.descendants(matching: .any)["app-shell"].waitForExistence(timeout: 12))

        let shellBack = app.buttons["grok-mobile-back"]
        XCTAssertTrue(shellBack.waitForExistence(timeout: 8), "Expected shell back control to return to the Bot list")
        shellBack.tap()
    }

    @MainActor
    private func scrollUntilAnyExists(_ elements: [XCUIElement], in app: XCUIApplication, maxSwipes: Int) -> Bool {
        if elements.contains(where: { $0.exists }) { return true }
        for _ in 0..<maxSwipes {
            app.swipeUp()
            if elements.contains(where: { $0.waitForExistence(timeout: 1) }) { return true }
        }
        return elements.contains(where: { $0.exists })
    }

    @MainActor
    private func scrollToElement(_ element: XCUIElement, in app: XCUIApplication, maxSwipes: Int) -> Bool {
        if element.exists { return true }
        for _ in 0..<maxSwipes {
            app.swipeUp()
            if element.waitForExistence(timeout: 1) { return true }
        }
        return element.exists
    }

    @MainActor
    private func waitForAny(_ elements: [XCUIElement], timeout: TimeInterval) -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if elements.contains(where: { $0.exists }) { return true }
            RunLoop.current.run(until: Date().addingTimeInterval(0.25))
        }
        return elements.contains(where: { $0.exists })
    }

    @MainActor
    private func waitForNonExistence(_ element: XCUIElement, timeout: TimeInterval) -> Bool {
        let expectation = XCTNSPredicateExpectation(
            predicate: NSPredicate(format: "exists == false"),
            object: element
        )
        return XCTWaiter.wait(for: [expectation], timeout: timeout) == .completed
    }

    private func mark(_ key: String, _ value: Bool) {
        state[key] = value
        persistState()
    }

    private func checkpoint(_ name: String) {
        let screenshot = XCUIScreen.main.screenshot()
        let attachment = XCTAttachment(screenshot: screenshot)
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)

        guard let directory = ProcessInfo.processInfo.environment["FABUSHI_E2E_SCREENSHOT_DIR"], !directory.isEmpty else {
            return
        }
        let url = URL(fileURLWithPath: directory, isDirectory: true)
        try? FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        try? screenshot.pngRepresentation.write(to: url.appendingPathComponent("\(name).png"), options: .atomic)
    }

    private func persistState() {
        guard let path = ProcessInfo.processInfo.environment["FABUSHI_E2E_STATE_FILE"], !path.isEmpty,
              JSONSerialization.isValidJSONObject(state),
              let data = try? JSONSerialization.data(withJSONObject: state, options: [.prettyPrinted, .sortedKeys])
        else { return }
        let url = URL(fileURLWithPath: path)
        try? FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try? data.write(to: url, options: .atomic)
    }
}
