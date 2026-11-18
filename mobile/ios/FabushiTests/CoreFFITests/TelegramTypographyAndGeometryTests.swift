import SwiftUI
import XCTest
@testable import Fabushi

final class TelegramTypographyAndGeometryTests: XCTestCase {
    func testAllSevenPresentationFontSizeStepsAnd31ptComposerFloor() {
        let steps = PresentationFontSize.allCases
        XCTAssertEqual(steps.map(\.rawValue), [14, 15, 16, 17, 19, 23, 26])
        XCTAssertEqual(steps.map(\.secondaryLabelSize), [11, 12, 13, 14, 16, 20, 23])
        XCTAssertEqual(steps.map(\.sectionHeaderSize), [10, 11, 12, 13, 14, 17, 19])
        XCTAssertEqual(steps.map(\.emptyComposerMinHeight), [31, 31, 31, 31, 33, 38, 42])

        for s in steps {
            XCTAssertGreaterThanOrEqual(s.emptyComposerMinHeight, 31.0)
        }
        XCTAssertEqual(PresentationFontSize.regular.scaledSize(17.0), 17.0)
        XCTAssertEqual(PresentationFontSize.extraLargeX2.scaledSize(17.0), 26.0)
    }

    func testBubbleCornersClampingMergeThresholdAndBubbleFreeToggle() {
        let defaultCfg = BubbleCornersConfig()
        XCTAssertEqual(defaultCfg.mainRadius, 16.0)
        XCTAssertEqual(defaultCfg.auxiliaryRadius, 8.0)
        XCTAssertEqual(defaultCfg.tailWidth, 6.0)
        XCTAssertEqual(defaultCfg.effectiveMergedRadius, 8.0)

        let clampedLow = BubbleCornersConfig(mainRadius: 1.0)
        XCTAssertEqual(clampedLow.mainRadius, 4.0)
        // Below 10pt, merged radius equals mainRadius
        XCTAssertEqual(clampedLow.effectiveMergedRadius, 4.0)

        let clampedHigh = BubbleCornersConfig(mainRadius: 40.0)
        XCTAssertEqual(clampedHigh.mainRadius, 17.0)

        let pageCfg = BubbleCornersConfig(mainRadius: 16.0, hasTails: true, isBubbleFreeMode: true)
        let radii = pageCfg.makeCornerRadii(isOutgoing: true, mergeTop: false, mergeBottom: false, isLastInCluster: true)
        XCTAssertFalse(radii.hasTail, "Bubble-Free Page mode must suppress bubble tail")
    }

    func testTelegramBubbleShapePathsAndInlineStripeDashes() {
        let rect = CGRect(x: 0, y: 0, width: 220, height: 54)
        let tailed = TelegramBubbleShape(
            isOutgoing: true,
            corners: BubbleCornersConfig(mainRadius: 16.0, hasTails: true)
        ).path(in: rect)
        let tailless = TelegramBubbleShape(
            isOutgoing: false,
            corners: BubbleCornersConfig(mainRadius: 16.0, hasTails: false)
        ).path(in: rect)
        XCTAssertFalse(tailed.isEmpty)
        XCTAssertFalse(tailless.isEmpty)

        XCTAssertEqual(MessageInlineStripeView(style: .solid).dashCount, 1)
        XCTAssertEqual(MessageInlineStripeView(style: .doubleDashed).dashCount, 2)
        XCTAssertEqual(MessageInlineStripeView(style: .tripleDashed).dashCount, 3)
        XCTAssertEqual(MessageInlineStripeView(style: .solid, isPageMode: false).containerCornerRadius, 4.0)
        XCTAssertEqual(MessageInlineStripeView(style: .solid, isPageMode: true).containerCornerRadius, 10.0)
    }
}
