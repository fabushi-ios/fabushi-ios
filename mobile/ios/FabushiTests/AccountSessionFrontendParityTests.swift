import XCTest
@testable import Fabushi

final class AccountSessionFrontendParityTests: XCTestCase {
    func testSignInPhaseKeepsActiveBrowserAttemptAheadOfBusyFlag() {
        XCTAssertEqual(
            AccountSignInPhase.resolve(attemptID: "attempt-12345678", busy: true),
            .awaitingBrowser(attemptID: "attempt-12345678")
        )
        XCTAssertEqual(AccountSignInPhase.resolve(attemptID: nil, busy: true), .starting)
        XCTAssertEqual(AccountSignInPhase.resolve(attemptID: nil, busy: false), .idle)
    }

    func testSignOutSurfaceClosesOnlyAfterLoggedOutProjection() {
        XCTAssertEqual(
            accountSignOutResult(isLoggedIn: false, message: "已退出登录"),
            .signedOut
        )
        XCTAssertEqual(
            accountSignOutResult(isLoggedIn: true, message: "退出登录失败：network"),
            .failed(message: "退出登录失败：network")
        )
        XCTAssertEqual(
            accountSignOutResult(isLoggedIn: true, message: "   "),
            .failed(message: "退出登录失败，请重试。")
        )
    }

    func testUsageProjectionParsesServerAuthoritativeBudget() {
        let usage = AccountUsageProjection(payload: [
            "windowStart": 1_725_235_200,
            "windowEnd": 1_725_840_000,
            "tokenLimit": 100_000,
            "usedTokens": 25_000,
            "reservedTokens": 5_000,
            "remainingTokens": 70_000,
            "unlimited": false,
        ])

        XCTAssertNotNil(usage)
        XCTAssertEqual(usage?.committedTokens, 30_000)
        XCTAssertEqual(usage?.usagePercent, 30)
        XCTAssertEqual(usage?.semanticSummary, "当前周期用量 30% · 剩余 70000 tokens")
    }

    func testUsageProjectionRejectsMalformedOrBooleanNumbers() {
        XCTAssertNil(AccountUsageProjection(payload: [
            "windowStart": 10,
            "windowEnd": 9,
            "tokenLimit": 100,
            "usedTokens": 1,
            "reservedTokens": 0,
            "remainingTokens": 99,
            "unlimited": false,
        ]))
        XCTAssertNil(AccountUsageProjection(payload: [
            "windowStart": 1,
            "windowEnd": 2,
            "tokenLimit": true,
            "usedTokens": 1,
            "reservedTokens": 0,
            "remainingTokens": 99,
            "unlimited": false,
        ]))
    }
    func testAccountDisplayNameNormalizationMatchesDesktopMenuContract() {
        XCTAssertEqual(MarketplaceModel.normalizedAccountDisplayName("  Ada   Lovelace  "), "Ada Lovelace")
        XCTAssertEqual(MarketplaceModel.normalizedAccountDisplayName("\nFabushi\tUser\n"), "Fabushi User")
        XCTAssertEqual(MarketplaceModel.normalizedAccountDisplayName("   "), "")
    }

}
