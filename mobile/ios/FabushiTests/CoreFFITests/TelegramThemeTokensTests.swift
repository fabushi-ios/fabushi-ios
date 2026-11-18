#if canImport(XCTest)
import XCTest
@testable import Fabushi

final class TelegramThemeTokensTests: XCTestCase {

    func testAllFourPalettesMeetWCAGContrastAndHexInvariants() {
        let day = TelegramPalette.palette(for: .day)
        XCTAssertEqual(day.background.hex, 0xFFFFFF)
        XCTAssertEqual(day.outgoingBubble.hex, 0xE1FFC7)
        XCTAssertGreaterThan(day.primaryText.contrastRatio(with: day.background), 15.0)

        let night = TelegramPalette.palette(for: .night)
        XCTAssertEqual(night.background.hex, 0x1C1C1E)
        XCTAssertGreaterThan(night.primaryText.contrastRatio(with: night.background), 12.0)

        let tinted = TelegramPalette.palette(for: .tintedNight)
        XCTAssertEqual(tinted.background.hex, 0x17212B)
        XCTAssertGreaterThan(tinted.primaryText.contrastRatio(with: tinted.background), 11.0)

        let oled = TelegramPalette.palette(for: .oledBlack)
        XCTAssertEqual(oled.background.hex, 0x000000)
        XCTAssertEqual(oled.primaryText.contrastRatio(with: oled.background), 21.0, accuracy: 0.01)
    }

    func testDynamicTypeFontClampingAndCanonicalMetrics() {
        XCTAssertEqual(TelegramThemeTokens.tabBarHeightPt, 49)
        XCTAssertEqual(TelegramThemeTokens.tabBarIconPt, 24)
        XCTAssertEqual(TelegramThemeTokens.tabBarLabelFontPt, 10)
        XCTAssertEqual(TelegramThemeTokens.spacingUnitPt, 8)

        let tooSmall = TelegramThemeTokens(mode: .day, bodyFontSizePt: 8)
        XCTAssertEqual(tooSmall.bodyFontSizePt, 13)

        let tooLarge = TelegramThemeTokens(mode: .oledBlack, bodyFontSizePt: 40)
        XCTAssertEqual(tooLarge.bodyFontSizePt, 23)
    }

    func testDeterministicPeerAvatarSevenBucketHashing() {
        XCTAssertEqual(PeerAvatarColorHasher.buckets.count, 7)
        for id: Int64 in [-999_999, -1, 0, 1, 42, 1_000_007, Int64.max, Int64.min] {
            let g1 = PeerAvatarColorHasher.gradient(for: id)
            let g2 = PeerAvatarColorHasher.gradient(for: id)
            XCTAssertEqual(g1, g2)
            XCTAssertTrue((0..<7).contains(g1.bucketIndex))
        }
    }

    func testLiquidGlassReduceTransparencyOpaqueFallback() {
        XCTAssertFalse(
            LiquidGlassSurfaceModifier.shouldUseOpaqueFallback(
                liquidGlassEnabled: true,
                reduceTransparency: false
            )
        )
        XCTAssertTrue(
            LiquidGlassSurfaceModifier.shouldUseOpaqueFallback(
                liquidGlassEnabled: true,
                reduceTransparency: true
            ),
            "Accessibility Reduce Transparency MUST force solid surface fallback"
        )
        XCTAssertTrue(
            LiquidGlassSurfaceModifier.shouldUseOpaqueFallback(
                liquidGlassEnabled: false,
                reduceTransparency: false
            )
        )
    }
}
