import XCTest

final class FabushiUITests: XCTestCase {
    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    @MainActor
    func testCompleteFeatureHostUserJourneyInAppProcess() throws {
        let app = XCUIApplication()
        app.launchEnvironment["FABUSHI_FEATURE_HOST_SMOKE"] = "1"
        app.launch()

        let smoke = app.descendants(matching: .any)["feature-host-smoke"]
        XCTAssertTrue(smoke.waitForExistence(timeout: 15))
        let finished = XCTNSPredicateExpectation(
            predicate: NSPredicate(format: "label == %@ OR label BEGINSWITH %@", "passed", "failed:"),
            object: smoke
        )
        XCTAssertEqual(XCTWaiter.wait(for: [finished], timeout: 90), .completed)
        XCTAssertEqual(smoke.label, "passed", "FeatureHost smoke result: \(smoke.label)")
    }

    @MainActor
    func testHomeMatchesConversationLayoutAndMarketplaceRemainsReachable() throws {
        let app = launchAuthenticatedApp()

        XCTAssertTrue(app.descendants(matching: .any)["app-shell"].waitForExistence(timeout: 10))
        XCTAssertTrue(app.buttons["profile-avatar"].exists)
        XCTAssertTrue(app.buttons["home-search-button"].exists)
        XCTAssertTrue(app.buttons["home-add-button"].exists)
        XCTAssertTrue(app.descendants(matching: .any)["conversation-list"].exists)
        XCTAssertFalse(app.staticTexts["Chief of Staff"].exists)
        app.buttons["home-add-button"].tap()
        let newMessage = app.buttons["新消息"]
        XCTAssertTrue(newMessage.waitForExistence(timeout: 5))
        newMessage.tap()
        XCTAssertTrue(app.navigationBars["联系人"].waitForExistence(timeout: 5))
        app.buttons["完成"].tap()

        openRemoteComputer(in: app)
        let remoteComputer = app.descendants(matching: .any)["remote-computer-surface"]
        XCTAssertTrue(remoteComputer.waitForExistence(timeout: 10))
        tapSurfaceClose(identifier: "remote-computer-close", in: app)
        XCTAssertTrue(remoteComputer.waitForNonExistence(timeout: 10))
        XCTAssertTrue(
            app.descendants(matching: .any)["app-shell"].waitForExistence(timeout: 10),
            "Expected Home app shell after closing the remote-computer surface"
        )
        XCTAssertTrue(
            app.buttons["profile-avatar"].waitForExistence(timeout: 10),
            "Expected Home profile control after closing the remote-computer surface"
        )

        openMarketplace(in: app)
        XCTAssertTrue(app.descendants(matching: .any)["runtime-badge"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.descendants(matching: .any)["host-status"].exists)

        let search = app.textFields["marketplace-search"]
        XCTAssertTrue(search.exists)
        search.tap()
        search.typeText("telegram")
        let submit = app.buttons["marketplace-search-submit"]
        XCTAssertTrue(submit.exists)
        submit.tap()
    }

    @MainActor
    func testMiniAppOpensAndClosesDedicatedWebMcpSurface() throws {
        let app = launchAuthenticatedApp()
        openMarketplace(in: app)

        let open = app.buttons["open-global-dharma"]
        XCTAssertTrue(
            scrollToElement(open, in: app),
            "Expected global-dharma WebMCP open button after scrolling Marketplace"
        )
        open.tap()

        let surface = app.descendants(matching: .any)["miniapp-webmcp-surface"]
        XCTAssertTrue(surface.waitForExistence(timeout: 10))

        // WKWebView's internal DOM accessibility projection is not stable across
        // simulator/WebKit versions. The product already runs a JavaScript WebMCP
        // probe in WKNavigationDelegate.didFinish and projects that result into the
        // native SwiftUI status label. Assert that native readiness projection.
        let connected = app.staticTexts["WebMCP 已连接"]
        let localConnected = app.staticTexts["本地 WebMCP 已连接"]
        let opened = app.staticTexts["WebMCP 页面已打开"]
        let ready = XCTNSPredicateExpectation(
            predicate: NSPredicate { _, _ in
                connected.exists || localConnected.exists || opened.exists
            },
            object: nil
        )
        XCTAssertEqual(
            XCTWaiter.wait(for: [ready], timeout: 15),
            .completed,
            "Expected the native WebMCP readiness status after WKNavigationDelegate.didFinish"
        )

        tapSurfaceClose(identifier: "miniapp-webmcp-close", in: app)
        XCTAssertTrue(surface.waitForNonExistence(timeout: 10))
    }

    @MainActor
    private func tapSurfaceClose(identifier: String, in app: XCUIApplication) {
        let byIdentifier = app.buttons[identifier]
        if byIdentifier.waitForExistence(timeout: 5), byIdentifier.isHittable {
            byIdentifier.tap()
            return
        }

        // SwiftUI can omit a Button's identifier from the simulator accessibility
        // projection while preserving its visible label and tap action. Only tap the
        // labelled control when XCTest confirms it is actually hittable.
        let byLabel = app.buttons["返回"]
        if byLabel.waitForExistence(timeout: 2), byLabel.isHittable {
            byLabel.tap()
            return
        }

        let attachment = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        attachment.name = "\(identifier)-not-hittable"
        attachment.lifetime = .keepAlways
        add(attachment)

        // GrokMobileShell overlays its own legacy-workbench back button above the
        // ContentView stack. On affected simulator layouts that overlay owns the hit
        // region above RemoteComputerSurface's visible close label. Never tap the
        // non-hittable child element by coordinate because that can tunnel through to
        // the overlay. Exit through the explicit shell control, then immediately
        // restore the legacy workbench before the helper returns.
        guard identifier == "remote-computer-close" else {
            XCTFail("Expected close control \(identifier) to become hittable")
            return
        }
        let shellBack = app.buttons["grok-mobile-back"]
        XCTAssertTrue(
            shellBack.waitForExistence(timeout: 5),
            "Expected the legacy shell back control when remote-computer-close is obscured"
        )
        XCTAssertTrue(shellBack.isHittable, "Expected grok-mobile-back to be interactable")
        shellBack.tap()
        ensureLegacyWorkbench(in: app)
    }

    @MainActor
    private func openMarketplace(in app: XCUIApplication) {
        ensureLegacyWorkbench(in: app)
        let profile = app.buttons["profile-avatar"]
        XCTAssertTrue(profile.waitForExistence(timeout: 10))
        profile.tap()
        let marketplace = app.buttons["marketplace-entry"]
        XCTAssertTrue(marketplace.waitForExistence(timeout: 5))
        marketplace.tap()
    }

    @MainActor
    private func ensureLegacyWorkbench(in app: XCUIApplication) {
        let appShell = app.descendants(matching: .any)["app-shell"]
        if appShell.waitForExistence(timeout: 2) {
            return
        }

        let grokHome = app.descendants(matching: .any)["grok-mobile-home"]
        XCTAssertTrue(
            grokHome.waitForExistence(timeout: 10),
            "Expected either the legacy app-shell or authenticated Grok mobile home before Marketplace"
        )
        let legacy = app.buttons["grok-mobile-legacy"]
        XCTAssertTrue(legacy.waitForExistence(timeout: 5))
        XCTAssertTrue(legacy.isHittable, "Expected grok-mobile-legacy to be interactable")
        legacy.tap()
        XCTAssertTrue(
            appShell.waitForExistence(timeout: 10),
            "Expected legacy app-shell after recovering from Grok mobile home"
        )
    }

    @MainActor
    private func launchAuthenticatedApp() -> XCUIApplication {
        let app = XCUIApplication()
        app.launchEnvironment["FABUSHI_FEATURE_HOST_TEST"] = "1"
        app.launch()
        let skip = app.buttons["mobile-onboarding-skip"]
        if skip.waitForExistence(timeout: 5) { skip.tap() }

        let grokHome = app.descendants(matching: .any)["grok-mobile-home"]
        if !grokHome.waitForExistence(timeout: 2) {
            let login = app.buttons["mobile-login-browser"]
            if login.waitForExistence(timeout: 15) {
                login.tap()
            }
        }

        XCTAssertTrue(
            grokHome.waitForExistence(timeout: 20),
            "Expected authenticated Grok mobile home before opening the legacy message workbench"
        )
        let legacy = app.buttons["grok-mobile-legacy"]
        XCTAssertTrue(legacy.waitForExistence(timeout: 5))
        legacy.tap()
        XCTAssertTrue(app.descendants(matching: .any)["app-shell"].waitForExistence(timeout: 20))
        return app
    }

    @MainActor
    private func openRemoteComputer(in app: XCUIApplication) {
        let profile = app.buttons["profile-avatar"]
        XCTAssertTrue(profile.waitForExistence(timeout: 10))
        profile.tap()
        let remoteComputer = app.buttons["remote-computer-entry"]
        XCTAssertTrue(remoteComputer.waitForExistence(timeout: 5))
        remoteComputer.tap()
    }

    @MainActor
    private func scrollToElement(_ element: XCUIElement, in app: XCUIApplication, maxSwipes: Int = 8) -> Bool {
        if element.exists {
            return true
        }

        for _ in 0..<maxSwipes {
            app.swipeUp()
            if element.waitForExistence(timeout: 1) {
                return true
            }
        }

        return element.exists
    }
}
