import SwiftUI
import XCTest
@testable import Fabushi

final class MobileUiPreferencesLocalizationParityTests: XCTestCase {
    func testExplicitDesktopMainLocalesResolveNativeSettingsCopy() {
        let expectedTitles: [(MobileSettingsLocale, String)] = [
            (.en, "Settings"),
            (.zhHans, "设置"),
            (.zhHant, "設定"),
            (.ja, "設定"),
            (.ko, "설정"),
            (.ar, "الإعدادات"),
            (.he, "הגדרות"),
        ]

        for (locale, title) in expectedTitles {
            let preferences = MobileUiPreferences(localeRaw: locale.rawValue)
            XCTAssertEqual(preferences.shellCopy().settings, title)
            XCTAssertFalse(preferences.accessibilityCopy().sectionTitle.isEmpty)
            XCTAssertFalse(preferences.featureCopy().privacy.isEmpty)
        }
    }

    func testDisplayNameValidationUsesSelectedSettingsLocale() {
        XCTAssertEqual(
            MobileUiPreferences(localeRaw: MobileSettingsLocale.en.rawValue)
                .shellCopy()
                .displayNameValidationError,
            "Display name must be 1–200 characters."
        )
        XCTAssertEqual(
            MobileUiPreferences(localeRaw: MobileSettingsLocale.zhHans.rawValue)
                .shellCopy()
                .displayNameValidationError,
            "显示名称必须为 1–200 个字符。"
        )
        XCTAssertEqual(
            MobileUiPreferences(localeRaw: MobileSettingsLocale.ar.rawValue)
                .shellCopy()
                .displayNameValidationError,
            "يجب أن يكون اسم العرض من 1 إلى 200 حرف."
        )
    }

    func testAutomaticDirectionFollowsSelectedRtlLocale() {
        XCTAssertEqual(
            MobileUiPreferences(localeRaw: MobileSettingsLocale.ar.rawValue)
                .resolvedLayoutDirection(system: .leftToRight),
            .rightToLeft
        )
        XCTAssertEqual(
            MobileUiPreferences(localeRaw: MobileSettingsLocale.he.rawValue)
                .resolvedLayoutDirection(system: .leftToRight),
            .rightToLeft
        )
        XCTAssertEqual(
            MobileUiPreferences(localeRaw: MobileSettingsLocale.ja.rawValue)
                .resolvedLayoutDirection(system: .rightToLeft),
            .leftToRight
        )
    }

    func testSystemLocalePreservesNativeSystemDirectionAndLocale() {
        let preferences = MobileUiPreferences(localeRaw: MobileSettingsLocale.system.rawValue)
        XCTAssertEqual(
            preferences.resolvedLocaleIdentifier(systemIdentifier: "zh-Hant-TW"),
            "zh-Hant-TW"
        )
        XCTAssertEqual(
            preferences.resolvedLayoutDirection(system: .rightToLeft),
            .rightToLeft
        )
        XCTAssertEqual(
            preferences.shellCopy(systemIdentifier: "zh-Hant-TW").settings,
            "設定"
        )
    }

    func testDynamicTypeScalingNeverShrinksAccessibilitySizes() {
        let preferences = MobileUiPreferences(textScale: 0.9)
        XCTAssertEqual(
            preferences.adjustedDynamicTypeSize(system: .accessibility2),
            .accessibility2
        )
    }

    func testShellCopyCoversShippingSettingsSurfaceLabels() {
        for locale in MobileSettingsLocale.allCases where locale != .system {
            let copy = MobileUiPreferences(localeRaw: locale.rawValue).shellCopy()
            let required = [
                copy.settings, copy.account, copy.usage, copy.support,
                copy.workspace, copy.navigation, copy.signOut, copy.computer,
                copy.marketplace, copy.helpCenter, copy.sendFeedback,
                copy.aboutFabushi, copy.displayName, copy.displayNameValidationError,
                copy.email, copy.agentConfiguration, copy.router, copy.iosSelfReference,
                copy.mediaRuntimeUnavailable,
            ]
            XCTAssertTrue(required.allSatisfy { !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty })
        }
    }
}
