import XCTest
@testable import Fabushi

final class AccountSessionFrontendParityTests: XCTestCase {
    func testSettingsFeatureCopyFollowsSelectedLocaleAcrossNativeSections() {
        let english = MobileUiPreferences(localeRaw: "en").featureCopy()
        XCTAssertEqual(english.mediaDevices, "Media & Devices")
        XCTAssertEqual(english.privacyMode, "Privacy mode")
        XCTAssertEqual(english.allowAutomatically, "Allow automatically")

        let japanese = MobileUiPreferences(localeRaw: "ja").featureCopy()
        XCTAssertEqual(japanese.mediaDevices, "メディアとデバイス")
        XCTAssertEqual(japanese.privacyMode, "プライバシーモード")
        XCTAssertEqual(japanese.askFirst, "先に確認")

        let arabic = MobileUiPreferences(localeRaw: "ar").featureCopy()
        XCTAssertEqual(arabic.mediaDevices, "الوسائط والأجهزة")
        XCTAssertEqual(arabic.autoReviewTitle, "المراجعة التلقائية")

        let hebrew = MobileUiPreferences(localeRaw: "he").featureCopy()
        XCTAssertEqual(hebrew.privacy, "פרטיות")
        XCTAssertEqual(hebrew.delete, "מחיקה")
    }

    func testMobileConfigurationSnapshotCarriesServerAuthoritativePrivacyProjection() {
        let snapshot = MobileConfigurationSettingsSnapshot(
            autoReview: DEFAULT_SAND_AUTO_REVIEW_INSTRUCTIONS,
            inferenceProvider: .fabushi,
            privacyModeEnabled: false
        )
        XCTAssertFalse(snapshot.privacyModeEnabled)
    }

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

    func testMobileUiPreferencesNormalizeDesktopPreferenceDomain() {
        let preferences = MobileUiPreferences(
            localeRaw: "unsupported",
            directionRaw: "sideways",
            textScale: 1.18,
            reducedMotion: true,
            highContrast: true
        )
        XCTAssertEqual(preferences.locale, .system)
        XCTAssertEqual(preferences.direction, .auto)
        XCTAssertEqual(preferences.textScale, 1.25)
        XCTAssertEqual(preferences.activeAccessibilityPreferenceCount, 3)
    }

    func testMobileUiPreferencesResolveLocaleAliasesAndRtlDirection() {
        XCTAssertEqual(MobileUiPreferences.copyLocaleKey("zh-TW"), .zhHant)
        XCTAssertEqual(MobileUiPreferences.copyLocaleKey("zh-CN"), .zhHans)
        XCTAssertEqual(MobileUiPreferences.copyLocaleKey("iw-IL"), .he)

        let arabic = MobileUiPreferences(localeRaw: "ar", directionRaw: "auto")
        XCTAssertEqual(arabic.resolvedLayoutDirection(system: .leftToRight), .rightToLeft)

        let english = MobileUiPreferences(localeRaw: "en", directionRaw: "auto")
        XCTAssertEqual(english.resolvedLayoutDirection(system: .rightToLeft), .leftToRight)

        let system = MobileUiPreferences(localeRaw: "system", directionRaw: "auto")
        XCTAssertEqual(system.resolvedLayoutDirection(system: .rightToLeft), .rightToLeft)
    }

    func testMobileUiPreferencesAdjustDynamicTypeFromSystemBaseline() {
        let unchanged = MobileUiPreferences(textScale: 1)
        XCTAssertEqual(unchanged.adjustedDynamicTypeSize(system: .large), .large)

        let enlarged = MobileUiPreferences(textScale: 1.5)
        XCTAssertEqual(enlarged.adjustedDynamicTypeSize(system: .large), .xxxLarge)

        let reduced = MobileUiPreferences(textScale: 0.9)
        XCTAssertEqual(reduced.adjustedDynamicTypeSize(system: .large), .medium)
        XCTAssertEqual(
            reduced.adjustedDynamicTypeSize(system: .accessibility3),
            .accessibility3
        )
    }


}
