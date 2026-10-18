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
        "permissionDeniedObserved": false,
        "livePageMutationObserved": false,
        "bidirectionalStateObserved": false,
        "purchaseTapped": false,
        "entitlementAllowed": false,
        "restoreTapped": false,
        "commerceExternalBlock": false,
        "removedAtEnd": false,
    ]

    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    @MainActor
    func testGlobalDharmaMarketplaceBotWebMcpCommerceJourney() throws {
        let environment = ProcessInfo.processInfo.environment
        guard environment["GITHUB_ACTIONS"] == "true" else {
            XCTFail("Protected Fabushi CI UI tests must run under GitHub Actions")
            return
        }
        guard let protectedSession = environment["FABUSHI_CI_ACCOUNT_SESSION_BASE64"],
              !protectedSession.isEmpty
        else {
            throw XCTSkip(
                "Requires the protected, bounded Fabushi CI account session injected into the UI test runner"
            )
        }

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

        let botEntry = app.buttons["grok-mobile-miniapp-bot-global-dharma"]
        XCTAssertTrue(
            scrollToElement(botEntry, in: app, maxSwipes: 8),
            "Installing 全球法布施 must project its canonical Mini App Bot into the message area"
        )
        botEntry.tap()

        let botChat = app.descendants(matching: .any)["mobile-bot-chat"]
        XCTAssertTrue(botChat.waitForExistence(timeout: 15))
        let openApp = app.buttons["mobile-bot-open-miniapp"]
        XCTAssertTrue(openApp.waitForExistence(timeout: 8), "Global Dharma Bot must expose 打开应用")
        mark("botVisible", true)
        checkpoint("040-global-dharma-bot")

        openApp.tap()
        let panel = app.descendants(matching: .any)["mobile-bot-live-miniapp-panel"]
        XCTAssertTrue(
            panel.waitForExistence(timeout: 15),
            "打开应用必须在同一 Bot 会话内保持 live Mini App panel，而不是卸载当前 WKWebView"
        )
        let surface = app.descendants(matching: .any)["miniapp-webmcp-surface"]
        XCTAssertTrue(surface.waitForExistence(timeout: 15), "打开应用 must present the Mini App Web UI")

        let connected = app.staticTexts.matching(
            NSPredicate(format: "label BEGINSWITH %@", "本地 WebMCP 已连接 · 页面注册 ")
        ).firstMatch
        XCTAssertTrue(
            connected.waitForExistence(timeout: 30),
            "Host must report tools registered by the current page session"
        )
        XCTAssertTrue(
            connected.label.contains("4 个实时 Tool"),
            "Global Dharma 1.1.0 must register the four reviewed local WebMCP tools"
        )
        mark("webMcpReady", true)
        checkpoint("050-live-miniapp-webmcp-ready")

        let revisionPrefix = "rev "
        let revisionLabel = app.descendants(matching: .any)
            .matching(NSPredicate(format: "label BEGINSWITH %@", revisionPrefix))
            .firstMatch
        XCTAssertTrue(
            revisionLabel.waitForExistence(timeout: 20),
            "The visible page must expose its real persisted revision"
        )
        guard let initialRevision = parseRevision(revisionLabel.label, prefix: revisionPrefix) else {
            XCTFail("Unable to parse initial page revision: \(revisionLabel.label)")
            return
        }
        state["pageToolCount"] = 4
        state["initialRevision"] = initialRevision
        mark("pageProvidedToolsObserved", true)

        let draft = app.textFields["mobile-bot-draft"]
        XCTAssertTrue(
            draft.waitForExistence(timeout: 8),
            "Bot composer must remain usable while the exact same Mini App WebView stays mounted"
        )
        draft.tap()
        draft.typeText("在全球法布施里面执行一项免费的本地操作，并让我看到结果。")
        let send = app.buttons["mobile-bot-send"]
        XCTAssertTrue(send.waitForExistence(timeout: 8))
        send.tap()
        mark("naturalLanguageSent", true)

        let denial = app.alerts.buttons["取消"]
        XCTAssertTrue(
            denial.waitForExistence(timeout: 30),
            "Bot mutation must expose a real Host-owned denial path"
        )
        denial.tap()

        let stop = app.buttons["mobile-bot-stop"]
        if stop.waitForExistence(timeout: 4) {
            let deniedFinished = XCTNSPredicateExpectation(
                predicate: NSPredicate(format: "exists == false"),
                object: stop
            )
            XCTAssertEqual(
                XCTWaiter.wait(for: [deniedFinished], timeout: 60),
                .completed,
                "Denied Global Dharma WebMCP operation did not reach a terminal state"
            )
        }
        XCTAssertEqual(
            parseRevision(revisionLabel.label, prefix: revisionPrefix),
            initialRevision,
            "Denying the Bot mutation must leave the current page-owned state unchanged"
        )
        mark("permissionDeniedObserved", true)
        checkpoint("055-bot-mutation-denied")

        draft.tap()
        draft.typeText("在全球法布施里面执行一项免费的本地操作，并让我看到结果。")
        send.tap()

        let approval = app.alerts.buttons["允许"]
        XCTAssertTrue(
            approval.waitForExistence(timeout: 30),
            "Retrying the Bot mutation must require Host-owned per-tool user approval"
        )
        approval.tap()
        mark("permissionApprovedObserved", true)

        if stop.waitForExistence(timeout: 4) {
            let finished = XCTNSPredicateExpectation(
                predicate: NSPredicate(format: "exists == false"),
                object: stop
            )
            XCTAssertEqual(
                XCTWaiter.wait(for: [finished], timeout: 90),
                .completed,
                "Global Dharma Bot local WebMCP operation did not reach a terminal state"
            )
        }

        let botMutatedRevision = waitForRevisionGreaterThan(
            initialRevision,
            element: revisionLabel,
            prefix: revisionPrefix,
            timeout: 30
        )
        XCTAssertNotNil(
            botMutatedRevision,
            "Bot -> Host -> active WKWebView must mutate the visible page revision without reload"
        )
        state["botMutationRevision"] = botMutatedRevision
        mark("livePageMutationObserved", true)
        checkpoint("060-bot-mutated-live-page")

        let manualFreeAction = app.buttons["执行免费本地操作"]
        XCTAssertTrue(
            manualFreeAction.waitForExistence(timeout: 15),
            "The current offline page must expose its real free local action"
        )
        manualFreeAction.tap()
        guard let afterBot = botMutatedRevision,
              let manualRevision = waitForRevisionGreaterThan(
                  afterBot,
                  element: revisionLabel,
                  prefix: revisionPrefix,
                  timeout: 15
              )
        else {
            XCTFail("Manual page mutation must advance the same persisted revision")
            return
        }
        state["manualMutationRevision"] = manualRevision

        draft.tap()
        draft.typeText("读取刚才操作后的状态")
        send.tap()
        let stateReadReply = app.staticTexts["已读取全球法布施本地状态。"]
        XCTAssertTrue(
            stateReadReply.waitForExistence(timeout: 60),
            "Bot must read the latest state from the active page-owned WebMCP tool after manual mutation"
        )
        XCTAssertEqual(
            parseRevision(revisionLabel.label, prefix: revisionPrefix),
            manualRevision,
            "Read-only Bot access must not replace or regress the page-owned revision"
        )
        mark("bidirectionalStateObserved", true)
        checkpoint("061-bidirectional-live-state")

        if environment["FABUSHI_E2E_REQUIRE_REAL_APPLE_COMMERCE"] == "1" {
            let allowed = app.descendants(matching: .any)["global-dharma-entitlement-allowed"]
            let buyMonthly = app.buttons["global-dharma-buy-monthly"]
            let buy = app.buttons["global-dharma-buy-lifetime"]
            if !allowed.waitForExistence(timeout: 6) {
                XCTAssertTrue(
                    buyMonthly.waitForExistence(timeout: 12),
                    "Expected either existing entitlement or the canonical ¥30/month purchase action"
                )
                XCTAssertTrue(
                    buyMonthly.label.contains("30"),
                    "Monthly local prayer-wheel purchase must visibly preserve the server CNY 30 / 30-day contract"
                )
                XCTAssertTrue(buy.waitForExistence(timeout: 12), "Expected either existing entitlement or the ¥1080 lifetime purchase action")
                XCTAssertTrue(
                    buy.label.contains("1080"),
                    "Lifetime local prayer-wheel purchase must visibly preserve the CNY 1080 contract"
                )
                buy.tap()
                mark("purchaseTapped", true)
                XCTAssertTrue(
                    buy.isEnabled,
                    "Protected acceptance requires a real purchase/authorization path. If StoreKit/provider checkout is unavailable in this CI environment, commerce remains blocked rather than using synthetic entitlement state."
                )
                XCTAssertTrue(
                    allowed.waitForExistence(timeout: 60),
                    "Real purchase/authorization did not project an allowed service entitlement"
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
        } else {
            // Apple product/provider eligibility is an external release prerequisite.
            // Keep the live Marketplace/WebMCP journey hard-gated while recording
            // commerce as blocked instead of substituting synthetic entitlement.
            mark("commerceExternalBlock", true)
            checkpoint("070-commerce-external-block")
        }

        let close = app.buttons["miniapp-webmcp-close"]
        XCTAssertTrue(close.waitForExistence(timeout: 8))
        close.tap()
        XCTAssertTrue(waitForNonExistence(surface, timeout: 12))
        checkpoint("090-webmcp-closed")

        let closeBot = app.buttons["mobile-bot-close"]
        XCTAssertTrue(closeBot.waitForExistence(timeout: 8))
        closeBot.tap()
        XCTAssertTrue(grokHome.waitForExistence(timeout: 12))

        XCTAssertTrue(legacy.waitForExistence(timeout: 8))
        legacy.tap()
        XCTAssertTrue(app.descendants(matching: .any)["app-shell"].waitForExistence(timeout: 15))
        openMarketplace(in: app)

        let cleanupSearch = app.textFields["marketplace-search"]
        XCTAssertTrue(cleanupSearch.waitForExistence(timeout: 8))
        cleanupSearch.tap()
        cleanupSearch.typeText("小程序")
        let cleanupSubmit = app.buttons["marketplace-search-submit"]
        XCTAssertTrue(cleanupSubmit.waitForExistence(timeout: 5))
        cleanupSubmit.tap()

        let remove = app.buttons["remove-global-dharma"]
        XCTAssertTrue(
            scrollToElement(remove, in: app, maxSwipes: 10),
            "Protected journey must expose real account-scoped Marketplace removal after install/update"
        )
        remove.tap()
        XCTAssertTrue(
            waitForNonExistence(remove, timeout: 30),
            "Account-scoped removal must settle and clear the canonical installed projection"
        )
        mark("removedAtEnd", true)
        checkpoint("100-removed-global-dharma")
        persistState()
    }


    @MainActor
    func testPrepareGlobalDharmaOfflineAcceptance() throws {
        let environment = ProcessInfo.processInfo.environment
        guard let protectedSession = environment["FABUSHI_CI_ACCOUNT_SESSION_BASE64"],
              !protectedSession.isEmpty
        else {
            throw XCTSkip("Requires the bounded Fabushi CI account session")
        }

        let app = XCUIApplication()
        configureRealCIEnvironment(for: app)
        app.launch()

        let skip = app.buttons["mobile-onboarding-skip"]
        if skip.waitForExistence(timeout: 4) { skip.tap() }

        let grokHome = app.descendants(matching: .any)["grok-mobile-home"]
        XCTAssertTrue(grokHome.waitForExistence(timeout: 25))
        let legacy = app.buttons["grok-mobile-legacy"]
        XCTAssertTrue(legacy.waitForExistence(timeout: 8))
        legacy.tap()
        XCTAssertTrue(app.descendants(matching: .any)["app-shell"].waitForExistence(timeout: 15))

        openMarketplace(in: app)
        let search = app.textFields["marketplace-search"]
        XCTAssertTrue(search.waitForExistence(timeout: 8))
        replaceText(in: search, with: "小程序")
        let submit = app.buttons["marketplace-search-submit"]
        XCTAssertTrue(submit.waitForExistence(timeout: 5))
        submit.tap()

        let plugin = app.descendants(matching: .any)["plugin-global-dharma"]
        let install = app.buttons["install-global-dharma"]
        let open = app.buttons["open-global-dharma"]
        XCTAssertTrue(
            scrollUntilAnyExists([plugin, install, open], in: app, maxSwipes: 10),
            "Canonical Marketplace must surface global-dharma before offline preparation"
        )
        if install.exists && install.isHittable {
            install.tap()
            let approve = app.buttons["permission-approve"]
            if approve.waitForExistence(timeout: 5), approve.isHittable { approve.tap() }
        }
        XCTAssertTrue(waitForAny([open, plugin], timeout: 30))

        XCTAssertTrue(scrollToElement(open, in: app, maxSwipes: 8))
        open.tap()
        let surface = app.descendants(matching: .any)["miniapp-webmcp-surface"]
        XCTAssertTrue(surface.waitForExistence(timeout: 20))
        let connected = app.staticTexts.matching(
            NSPredicate(format: "label BEGINSWITH %@", "本地 WebMCP 已连接 · 页面注册 ")
        ).firstMatch
        XCTAssertTrue(connected.waitForExistence(timeout: 30))
        XCTAssertTrue(connected.label.contains("4 个实时 Tool"))

        let revisionPrefix = "rev "
        let revisionLabel = app.descendants(matching: .any)
            .matching(NSPredicate(format: "label BEGINSWITH %@", revisionPrefix))
            .firstMatch
        XCTAssertTrue(revisionLabel.waitForExistence(timeout: 20))
        guard let baseline = parseRevision(revisionLabel.label, prefix: revisionPrefix) else {
            XCTFail("Unable to parse online preparation revision")
            return
        }
        let free = app.buttons["执行免费本地操作"]
        XCTAssertTrue(free.waitForExistence(timeout: 12))
        free.tap()
        guard let preparedRevision = waitForRevisionGreaterThan(
            baseline,
            element: revisionLabel,
            prefix: revisionPrefix,
            timeout: 15
        ) else {
            XCTFail("Online preparation must persist one local mutation before network isolation")
            return
        }

        state["offlinePrepared"] = true
        state["preparedRevision"] = preparedRevision
        checkpoint("110-offline-prepared-local-package")

        let close = app.buttons["miniapp-webmcp-close"]
        XCTAssertTrue(close.waitForExistence(timeout: 8))
        close.tap()
        XCTAssertTrue(waitForNonExistence(surface, timeout: 12))
        leaveMarketplaceForBotHome(in: app)
        XCTAssertTrue(grokHome.waitForExistence(timeout: 15))
        let botEntry = app.buttons["grok-mobile-miniapp-bot-global-dharma"]
        XCTAssertTrue(
            scrollToElement(botEntry, in: app, maxSwipes: 8),
            "Online preparation must persist the canonical Mini App Bot before network isolation"
        )
        state["offlineBotPrepared"] = true
        persistState()
        app.terminate()
    }

    @MainActor
    func testGlobalDharmaOfflineColdStartHostWebMcpAndPersistence() throws {
        let environment = ProcessInfo.processInfo.environment
        guard environment["FABUSHI_E2E_NETWORK_ISOLATED"] == "1" else {
            XCTFail("GD-OFF/GD-WEBMCP-05 requires runner-level network isolation")
            return
        }
        guard let protectedSession = environment["FABUSHI_CI_ACCOUNT_SESSION_BASE64"],
              !protectedSession.isEmpty
        else {
            throw XCTSkip("Requires the bounded Fabushi CI account session")
        }

        let app = XCUIApplication()
        configureRealCIEnvironment(for: app)
        app.launch()

        let skip = app.buttons["mobile-onboarding-skip"]
        if skip.waitForExistence(timeout: 4) { skip.tap() }
        let grokHome = app.descendants(matching: .any)["grok-mobile-home"]
        XCTAssertTrue(
            grokHome.waitForExistence(timeout: 25),
            "Installed Global Dharma must cold-start while the runner has no public network"
        )

        let botEntry = app.buttons["grok-mobile-miniapp-bot-global-dharma"]
        XCTAssertTrue(
            scrollToElement(botEntry, in: app, maxSwipes: 8),
            "Persisted canonical Mini App Bot must be available offline"
        )
        botEntry.tap()
        let botChat = app.descendants(matching: .any)["mobile-bot-chat"]
        XCTAssertTrue(botChat.waitForExistence(timeout: 15))
        let openApp = app.buttons["mobile-bot-open-miniapp"]
        XCTAssertTrue(openApp.waitForExistence(timeout: 8))
        openApp.tap()

        let surface = app.descendants(matching: .any)["miniapp-webmcp-surface"]
        XCTAssertTrue(surface.waitForExistence(timeout: 20))
        let connected = app.staticTexts.matching(
            NSPredicate(format: "label BEGINSWITH %@", "本地 WebMCP 已连接 · 页面注册 ")
        ).firstMatch
        XCTAssertTrue(connected.waitForExistence(timeout: 30))
        XCTAssertTrue(connected.label.contains("4 个实时 Tool"))

        let revisionPrefix = "rev "
        let revisionLabel = app.descendants(matching: .any)
            .matching(NSPredicate(format: "label BEGINSWITH %@", revisionPrefix))
            .firstMatch
        XCTAssertTrue(revisionLabel.waitForExistence(timeout: 20))
        guard let coldStartRevision = parseRevision(revisionLabel.label, prefix: revisionPrefix) else {
            XCTFail("Unable to parse offline cold-start revision")
            return
        }
        XCTAssertGreaterThan(
            coldStartRevision,
            0,
            "Online preparation state must survive full Fabushi termination and offline cold start"
        )

        let command = app.textFields["miniapp-tool-command-input"]
        let run = app.buttons["miniapp-tool-command-run"]
        XCTAssertTrue(command.waitForExistence(timeout: 10))
        XCTAssertTrue(run.waitForExistence(timeout: 5))

        replaceText(in: command, with: "/offline.state.read")
        run.tap()
        let commandStatus = app.staticTexts["miniapp-tool-command-status"]
        XCTAssertTrue(commandStatus.waitForExistence(timeout: 15))
        XCTAssertTrue(commandStatus.label.contains("已执行 /offline.state.read"))
        XCTAssertEqual(
            parseRevision(revisionLabel.label, prefix: revisionPrefix),
            coldStartRevision,
            "Free read-only Host WebMCP call must read the current page state without mutation"
        )

        replaceText(in: command, with: "/offline.practice.record offline-ci")
        run.tap()
        let approval = app.alerts.buttons["允许"]
        XCTAssertTrue(
            approval.waitForExistence(timeout: 10),
            "Free local mutation must still use the Host-owned approval boundary while offline"
        )
        approval.tap()
        guard let mutatedRevision = waitForRevisionGreaterThan(
            coldStartRevision,
            element: revisionLabel,
            prefix: revisionPrefix,
            timeout: 20
        ) else {
            XCTFail("Native Host WebMCP mutation did not update the active WKWebView revision offline")
            return
        }
        XCTAssertTrue(commandStatus.waitForExistence(timeout: 10))
        XCTAssertTrue(commandStatus.label.contains("已执行 /offline.practice.record"))

        state["offlineColdStart"] = true
        state["offlineHostRead"] = true
        state["offlineHostMutation"] = true
        state["offlineMutationRevision"] = mutatedRevision
        checkpoint("120-offline-host-webmcp-mutated")

        app.terminate()
        app.launch()
        if skip.waitForExistence(timeout: 4) { skip.tap() }
        XCTAssertTrue(grokHome.waitForExistence(timeout: 25))
        let restoredBot = app.buttons["grok-mobile-miniapp-bot-global-dharma"]
        XCTAssertTrue(scrollToElement(restoredBot, in: app, maxSwipes: 8))
        restoredBot.tap()
        XCTAssertTrue(app.descendants(matching: .any)["mobile-bot-chat"].waitForExistence(timeout: 15))
        let restoredOpen = app.buttons["mobile-bot-open-miniapp"]
        XCTAssertTrue(restoredOpen.waitForExistence(timeout: 8))
        restoredOpen.tap()
        XCTAssertTrue(app.descendants(matching: .any)["miniapp-webmcp-surface"].waitForExistence(timeout: 20))
        let restoredRevisionLabel = app.descendants(matching: .any)
            .matching(NSPredicate(format: "label BEGINSWITH %@", revisionPrefix))
            .firstMatch
        XCTAssertTrue(restoredRevisionLabel.waitForExistence(timeout: 20))
        XCTAssertEqual(
            parseRevision(restoredRevisionLabel.label, prefix: revisionPrefix),
            mutatedRevision,
            "localStorage/page-owned revision must survive a second full offline cold restart"
        )

        state["offlinePersistenceRecovered"] = true
        checkpoint("130-offline-second-cold-start-restored")
        persistState()
        app.terminate()
    }

    @MainActor
    private func replaceText(in element: XCUIElement, with text: String) {
        element.tap()
        if let current = element.value as? String, !current.isEmpty {
            element.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: current.count))
        }
        element.typeText(text)
    }

    @MainActor
    private func configureRealCIEnvironment(for app: XCUIApplication) {
        let environment = ProcessInfo.processInfo.environment
        if let session = environment["FABUSHI_CI_ACCOUNT_SESSION_BASE64"], !session.isEmpty {
            app.launchEnvironment["FABUSHI_CI_ACCOUNT_SESSION_BASE64"] = session
        }
        guard let rawAPIBaseURL = environment["FABUSHI_CI_API_BASE_URL"] else {
            XCTFail("Protected Fabushi CI API/model base is missing")
            return
        }
        let apiBaseURL = rawAPIBaseURL.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !apiBaseURL.isEmpty, apiBaseURL.lowercased().hasPrefix("https://") else {
            XCTFail("Protected Fabushi API/model base must use HTTPS")
            return
        }
        app.launchEnvironment["FABUSHI_API_BASE_URL"] = apiBaseURL
        app.launchEnvironment["MAHAYANA_API_BASE_URL"] = apiBaseURL
        app.launchEnvironment["FABUSHI_RESPONSES_URL"] = apiBaseURL
        XCTAssertNotEqual(
            environment["FABUSHI_FEATURE_HOST_TEST"],
            "1",
            "Protected acceptance must never enable the synthetic/test commerce rail"
        )
        app.launchEnvironment["FABUSHI_FEATURE_HOST_TEST"] = "0"
        for key in [
            "GITHUB_ACTIONS", "GITHUB_REPOSITORY", "GITHUB_WORKFLOW", "GITHUB_JOB",
            "GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT", "RUNNER_NAME", "RUNNER_OS", "RUNNER_ARCH",
            "FABUSHI_DEVICE_NAME"
        ] {
            if let value = environment[key], !value.isEmpty {
                app.launchEnvironment[key] = value
            }
        }
        if let sourceSHA = environment["FABUSHI_E2E_SOURCE_SHA"], sourceSHA.count == 40 {
            app.launchEnvironment["GITHUB_SHA"] = sourceSHA
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

    @MainActor
    private func waitForRevisionGreaterThan(
        _ baseline: Int,
        element: XCUIElement,
        prefix: String,
        timeout: TimeInterval
    ) -> Int? {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if element.exists,
               let revision = parseRevision(element.label, prefix: prefix),
               revision > baseline
            {
                return revision
            }
            RunLoop.current.run(until: Date().addingTimeInterval(0.25))
        }
        return nil
    }

    private func parseRevision(_ label: String, prefix: String) -> Int? {
        guard label.hasPrefix(prefix) else { return nil }
        let suffix = label.dropFirst(prefix.count)
        let digits = suffix.prefix(while: { $0.isNumber })
        guard !digits.isEmpty else { return nil }
        return Int(digits)
    }

    private func mark(_ key: String, _ value: Bool) {
        state[key] = value
        persistState()
    }

    @MainActor
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
        guard JSONSerialization.isValidJSONObject(state),
              let data = try? JSONSerialization.data(
                withJSONObject: state,
                options: [.prettyPrinted, .sortedKeys]
              )
        else { return }

        // UI tests execute inside the Simulator test-runner sandbox, so a
        // GitHub-runner workspace path is not a valid evidence transport.
        // Emit only this non-sensitive boolean/revision state to stdout; the
        // workflow decodes the final marker back into state.json on the host.
        let marker = "FABUSHI_E2E_STATE_BASE64=\(data.base64EncodedString())\n"
        if let markerData = marker.data(using: .utf8) {
            FileHandle.standardOutput.write(markerData)
        }
    }
}
