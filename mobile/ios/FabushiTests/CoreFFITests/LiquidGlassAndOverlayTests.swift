import XCTest
@testable import Fabushi

@MainActor
final class LiquidGlassAndOverlayTests: XCTestCase {
    func testGlassBackgroundReduceTransparencyFallbackAndCardRadii() {
        XCTAssertFalse(GlassBackgroundView.shouldUseOpaqueSurface(useLiquidGlass: true, reduceTransparency: false))
        XCTAssertTrue(GlassBackgroundView.shouldUseOpaqueSurface(useLiquidGlass: true, reduceTransparency: true))
        XCTAssertTrue(GlassBackgroundView.shouldUseOpaqueSurface(useLiquidGlass: false, reduceTransparency: false))

        let forcedOpaque = GlassBackgroundView(cornerRadius: 26, forceReduceTransparency: true)
        XCTAssertTrue(forcedOpaque.effectiveReduceTransparency)

        let glassMod = TelegramGroupedCardModifier(useLiquidGlass: true)
        let classicMod = TelegramGroupedCardModifier(useLiquidGlass: false)
        XCTAssertEqual(glassMod.cornerRadius, 26.0)
        XCTAssertEqual(classicMod.cornerRadius, 11.0)
    }

    func testEdgeEffectSplitBreakpointAndTabBarMetrics() {
        XCTAssertEqual(EdgeEffectView.defaultEdgeEffectHeight, 50.0)
        XCTAssertEqual(FabushiRootShellView.liquidGlassBarHeightPt, 62.0)
        XCTAssertEqual(FabushiRootShellView.classicBarHeightPt, 49.0)
        XCTAssertEqual(FabushiRootShellView.liquidGlassHorizontalInsetPt, 20.0)

        #if canImport(UIKit)
        XCTAssertFalse(TelegramRootSplitViewController.isRegularSplitWidth(393.0))
        XCTAssertFalse(TelegramRootSplitViewController.isRegularSplitWidth(680.0))
        XCTAssertTrue(TelegramRootSplitViewController.isRegularSplitWidth(681.0))
        XCTAssertTrue(TelegramRootSplitViewController.isRegularSplitWidth(1024.0))
        #endif
    }

    func testOverlayBackdropMetricsAndUndoToastProgress() {
        XCTAssertEqual(SheetContainer<EmptyView>.backdropDimOpacity, 0.40, accuracy: 0.001)
        XCTAssertEqual(SheetContainer<EmptyView>.attachmentTopInsetFraction, 0.2488, accuracy: 0.0001)
        XCTAssertEqual(GlassAlertPresenter.backdropDimOpacity, 0.20, accuracy: 0.001)
        XCTAssertEqual(GlassAlertPresenter.initialScale, 1.15, accuracy: 0.001)
        XCTAssertEqual(UndoToastBanner.toastHex, 0x474747)
        XCTAssertEqual(UndoToastBanner.defaultDurationSeconds, 5.0, accuracy: 0.001)

        let toastHalf = UndoToastBanner(message: "Archived", remainingSeconds: 3, totalSeconds: 5.0, onUndo: {})
        XCTAssertEqual(toastHalf.progressFraction, 0.6, accuracy: 0.001)

        let avatar = PeerAvatarView(peerId: 42, title: "Dharma Sangha", diameter: 60)
        XCTAssertEqual(avatar.initials, "DS")
    }
}

private struct EmptyView: SwiftUI.View {
    var body: some SwiftUI.View { SwiftUI.EmptyView() }
}
