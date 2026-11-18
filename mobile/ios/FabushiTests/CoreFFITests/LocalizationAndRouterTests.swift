import Foundation
import XCTest
@testable import Fabushi

@MainActor
final class LocalizationAndRouterTests: XCTestCase {
    func testInitialLocaleEnvAndInstantBilingualToggle() throws {
        let suiteName = "com.ombhrum.fabushi.loc.\(UUID().uuidString)"
        let ud = try XCTUnwrap(UserDefaults(suiteName: suiteName))
        defer { ud.removePersistentDomain(forName: suiteName) }

        let loc = LocalizationManager(
            userDefaults: ud,
            environment: ["FABUSHI_INITIAL_LOCALE": "zh-CN"]
        )
        XCTAssertEqual(loc.language, .zhCN)
        XCTAssertEqual(loc.tr("tab.chats"), "对话")
        XCTAssertEqual(loc.tr("miniapp.global_dharma"), "全球法布施")

        let next = loc.toggleLanguage()
        XCTAssertEqual(next, .enUS)
        XCTAssertEqual(loc.tr("tab.chats"), "Chats")
        XCTAssertEqual(loc.tr("miniapp.global_dharma"), "Global Dharma")

        // Unknown locale code falls back safely to .enUS
        XCTAssertEqual(AppLanguage.normalize("fr-FR"), .enUS)
        XCTAssertEqual(AppLanguage.normalize("zh-Hans"), .zhCN)
    }

    func testAppRouterHandlesAllFiveRegisteredURLSchemesAndRejectsMalformed() throws {
        let router = AppRouter()
        XCTAssertEqual(SettingsSectionID.allCases.count, 26)
        XCTAssertEqual(HermesStudioTab.allCases.count, 7)

        // 1. fabushi:// Alipay OAuth callback
        XCTAssertTrue(router.handleURL(try XCTUnwrap(URL(string: "fabushi://auth/alipay/callback?auth_code=ali_99&state=s1"))))
        XCTAssertEqual(router.activeModalDestination, .alipayAuthCallback(authCode: "ali_99", state: "s1"))

        // 2. gpui-telegram:// chat deep link
        XCTAssertTrue(router.handleURL(try XCTUnwrap(URL(string: "gpui-telegram://chat?id=8801&msg_id=12"))))
        XCTAssertEqual(router.activeChatId, 8801)
        XCTAssertEqual(router.selectedTab, .chats)

        // 3. tg:// invite link
        XCTAssertTrue(router.handleURL(try XCTUnwrap(URL(string: "tg://join?invite=AbCdEf123"))))
        XCTAssertEqual(router.activeModalDestination, .joinInvite(code: "AbCdEf123"))

        // 4. ton:// wallet transfer
        XCTAssertTrue(router.handleURL(try XCTUnwrap(URL(string: "ton://transfer/UQAbcDef123?amount=500000000"))))
        XCTAssertEqual(router.activeModalDestination, .tonTransfer(address: "UQAbcDef123", amountNano: "500000000"))

        // 5. magpie:// provider import
        XCTAssertTrue(router.handleURL(try XCTUnwrap(URL(string: "magpie://import?payload=eyJwcm92aWRlciI6Im9wZW5haSJ9"))))
        XCTAssertEqual(router.activeModalDestination, .magpieImport(payload: "eyJwcm92aWRlciI6Im9wZW5haSJ9"))

        // Reject unregistered scheme or missing required query parameter
        XCTAssertFalse(router.handleURL(try XCTUnwrap(URL(string: "https://example.com/foo"))))
        XCTAssertFalse(router.handleURL(try XCTUnwrap(URL(string: "tg://join"))))
        XCTAssertFalse(router.handleURL(try XCTUnwrap(URL(string: "ton://transfer/"))))
    }
}
