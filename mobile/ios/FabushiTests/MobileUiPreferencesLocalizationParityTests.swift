import Foundation
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

    func testAccessibilityCountUsesSelectedLocaleNumberFormatting() {
        let preferences = MobileUiPreferences(
            localeRaw: MobileSettingsLocale.ar.rawValue,
            reducedMotion: true,
            highContrast: true
        )
        let formatter = NumberFormatter()
        formatter.locale = Locale(identifier: "ar")
        formatter.numberStyle = .decimal
        let expectedDigits = formatter.string(from: NSNumber(value: 2)) ?? "2"

        XCTAssertTrue(
            preferences.localizedAccessibilityCount(2).contains(expectedDigits)
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
                copy.aboutFabushi, copy.displayName, copy.email,
                copy.agentConfiguration, copy.router, copy.iosSelfReference,
                copy.mediaRuntimeUnavailable,
            ]
            XCTAssertTrue(required.allSatisfy { !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty })
        }
    }
}
