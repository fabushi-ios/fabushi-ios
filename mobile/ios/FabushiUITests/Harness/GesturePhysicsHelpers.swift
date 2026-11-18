import CoreGraphics
import Foundation
import UIKit
import XCTest

public struct RGBAColor: Sendable, Equatable {
    public let r: UInt8
    public let g: UInt8
    public let b: UInt8
    public let a: UInt8

    public init(r: UInt8, g: UInt8, b: UInt8, a: UInt8 = 255) {
        self.r = r
        self.g = g
        self.b = b
        self.a = a
    }

    public init(hex: UInt32, alpha: UInt8 = 255) {
        self.r = UInt8((hex >> 16) & 0xFF)
        self.g = UInt8((hex >> 8) & 0xFF)
        self.b = UInt8(hex & 0xFF)
        self.a = alpha
    }

    public func matches(_ expected: RGBAColor, channelTolerance: Int = 14) -> Bool {
        abs(Int(r) - Int(expected.r)) <= channelTolerance &&
        abs(Int(g) - Int(expected.g)) <= channelTolerance &&
        abs(Int(b) - Int(expected.b)) <= channelTolerance
    }
}

/// Canonical Telegram-iOS sRGB palette tokens (`telegram_ios_uiux_spec.md § 2.1`).
public enum TelegramThemePalette: Sendable {
    public static let dayClassicPlainBg = RGBAColor(hex: 0xFFFFFF)
    public static let dayClassicBlocksBg = RGBAColor(hex: 0xEFEFF4)
    public static let dayClassicOutgoing = RGBAColor(hex: 0xE1FFC7)
    public static let dayBlocksBg = RGBAColor(hex: 0xF2F2F7)
    public static let dayOutgoing = RGBAColor(hex: 0x007AFF)
    public static let nightOledPlainBg = RGBAColor(hex: 0x000000)
    public static let nightOledCardBg = RGBAColor(hex: 0x1C1C1D)
    public static let nightOledIncoming = RGBAColor(hex: 0x1F1F1F)
    public static let nightOledOutgoing = RGBAColor(hex: 0x313131)
    public static let nightTintedPlainBg = RGBAColor(hex: 0x18222D)
    public static let nightTintedCardBg = RGBAColor(hex: 0x213040)
    public static let nightTintedAccent = RGBAColor(hex: 0x2EA6FF)
    public static let nightTintedOutgoing = RGBAColor(hex: 0x3D6A97)
    public static let draftPrefixRed = RGBAColor(hex: 0xDD4B39)
    public static let undoToastGray = RGBAColor(hex: 0x474747)

    public static let peerAvatarGradientTopColors: [RGBAColor] = [
        RGBAColor(hex: 0xFC5C51),
        RGBAColor(hex: 0xFA790F),
        RGBAColor(hex: 0x895DD5),
        RGBAColor(hex: 0x0FB297),
        RGBAColor(hex: 0x00C0C2),
        RGBAColor(hex: 0x3CA5EC),
        RGBAColor(hex: 0x3D72ED),
    ]
}

/// Canonical Telegram-iOS geometry constants (`telegram_ios_uiux_spec.md § 1–§ 9`).
public enum TelegramGeometryMetrics: Sendable {
    public static let splitViewBreakpointPt: CGFloat = 680.0
    public static let liquidGlassTabBarHeightPt: CGFloat = 62.0
    public static let liquidGlassTabBarSideInsetPt: CGFloat = 20.0
    public static let classicTabBarHeightPt: CGFloat = 49.0
    public static let compactLandscapeTabBarHeightPt: CGFloat = 31.0
    public static let edgeEffectFeatherHeightPt: CGFloat = 50.0
    public static let chatListRow3LineHeightPt: CGFloat = 76.0
    public static let chatListRow2LineHeightPt: CGFloat = 64.0
    public static let chatListAvatarSizePt: CGFloat = 60.0
    public static let chatListAvatarLeadingXPt: CGFloat = 16.0
    public static let chatListTextLeadingXPt: CGFloat = 86.0
    public static let unreadPillHeightPt: CGFloat = 20.0
    public static let collapsibleSearchBarHeightPt: CGFloat = 36.0
    public static let expandedStoriesHeaderHeightPt: CGFloat = 83.0
    public static let pinnedBannerHeightPt: CGFloat = 46.0
    public static let videoNoteCircleDiameterPt: CGFloat = 240.0
    public static let reactionCapsuleHeightPt: CGFloat = 30.0
    public static let swipeToReplyRingDiameterPt: CGFloat = 33.0
    public static let composerTextFloorHeightPt: CGFloat = 31.0
    public static let glassGroupedCardRadiusPt: CGFloat = 26.0
    public static let classicGroupedCardRadiusPt: CGFloat = 11.0
    public static let bubbleMainRadiusDefaultPt: CGFloat = 16.0
    public static let bubbleMergedInnerRadiusPt: CGFloat = 8.0
    public static let bubbleBezierTailWidthPt: CGFloat = 6.0
}

public enum PixelColorSampler: Sendable {
    @MainActor
    public static func sampleNormalizedPixel(
        in element: XCUIElement,
        normalizedPoint: CGPoint = CGPoint(x: 0.5, y: 0.5)
    ) throws -> RGBAColor {
        let image = element.screenshot().image
        guard let cgImage = image.cgImage else {
            throw NSError(
                domain: "PixelColorSampler",
                code: 1,
                userInfo: [NSLocalizedDescriptionKey: "Missing CGImage in element screenshot"]
            )
        }
        let width = cgImage.width
        let height = cgImage.height
        let px = max(0, min(width - 1, Int(CGFloat(width) * normalizedPoint.x)))
        let py = max(0, min(height - 1, Int(CGFloat(height) * normalizedPoint.y)))

        guard let sRGB = CGColorSpace(name: CGColorSpace.sRGB) else {
            throw NSError(domain: "PixelColorSampler", code: 2, userInfo: nil)
        }
        var buffer = [UInt8](repeating: 0, count: width * height * 4)
        let bitmapInfo = CGImageAlphaInfo.premultipliedLast.rawValue | CGBitmapInfo.byteOrder32Big.rawValue
        guard let ctx = CGContext(
            data: &buffer,
            width: width,
            height: height,
            bitsPerComponent: 8,
            bytesPerRow: width * 4,
            space: sRGB,
            bitmapInfo: bitmapInfo
        ) else {
            throw NSError(domain: "PixelColorSampler", code: 3, userInfo: nil)
        }
        ctx.draw(cgImage, in: CGRect(x: 0, y: 0, width: width, height: height))
        let offset = (py * width + px) * 4
        return RGBAColor(
            r: buffer[offset],
            g: buffer[offset + 1],
            b: buffer[offset + 2],
            a: buffer[offset + 3]
        )
    }
}

@MainActor
public enum GesturePhysicsHelpers {
    /// Leading swipe (Left -> Right) on a 76pt ChatList row (`Read` / `Pin` reveal or full-swipe trigger).
    public static func leadingSwipeChatRow(_ row: XCUIElement, fullSwipe: Bool = false) {
        let start = row.coordinate(withNormalizedOffset: CGVector(dx: 0.08, dy: 0.5))
        let dx: CGFloat = fullSwipe ? max(260.0, row.frame.width * 0.78) : 140.0
        let end = start.withOffset(CGVector(dx: dx, dy: 0.0))
        start.press(
            forDuration: 0.05,
            thenDragTo: end,
            withVelocity: fullSwipe ? .fast : .default,
            thenHoldForDuration: fullSwipe ? 0.05 : 0.15
        )
    }

    /// Trailing swipe (Right -> Left) on a 76pt ChatList row (`Mute` / `Delete` / `Archive`, 24pt corner reveal).
    public static func trailingSwipeChatRow(_ row: XCUIElement, fullSwipe: Bool = false) {
        let start = row.coordinate(withNormalizedOffset: CGVector(dx: 0.92, dy: 0.5))
        let dx: CGFloat = fullSwipe ? -max(260.0, row.frame.width * 0.78) : -185.0
        let end = start.withOffset(CGVector(dx: dx, dy: 0.0))
        start.press(
            forDuration: 0.05,
            thenDragTo: end,
            withVelocity: fullSwipe ? .fast : .default,
            thenHoldForDuration: fullSwipe ? 0.05 : 0.15
        )
    }

    /// Telegram-iOS `ChatSwipeToReplyRecognizer` vector gesture (`absX > 2 && absY * 2 < absX`, 33pt ring).
    public static func swipeMessageVector(
        _ bubble: XCUIElement,
        dx: CGFloat,
        dy: CGFloat,
        holdBeforeRelease: TimeInterval = 0.08
    ) {
        let start = bubble.coordinate(withNormalizedOffset: CGVector(dx: 0.78, dy: 0.5))
        let end = start.withOffset(CGVector(dx: dx, dy: dy))
        start.press(
            forDuration: 0.04,
            thenDragTo: end,
            withVelocity: .default,
            thenHoldForDuration: holdBeforeRelease
        )
    }

    /// Pull-down overscroll on ChatList (reveals collapsible Archived Chats / Stories strip).
    public static func pullDownOverscroll(in scrollContainer: XCUIElement, distance: CGFloat = 220.0) {
        let top = scrollContainer.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.18))
        let target = top.withOffset(CGVector(dx: 0.0, dy: distance))
        top.press(forDuration: 0.05, thenDragTo: target, withVelocity: .default, thenHoldForDuration: 0.12)
    }

    /// Pinch-to-zoom on `GalleryController` (`0.5x..3.0x`).
    public static func pinchZoom(_ element: XCUIElement, scale: CGFloat, velocity: CGFloat) {
        element.pinch(withScale: scale, velocity: velocity)
    }

    /// Long-press (`0.65s`) triggering `PortalView` bubble lift + 7-reaction strip or ChatList `Peek`.
    public static func longPressForPortalMenu(_ element: XCUIElement, duration: TimeInterval = 0.65) {
        element.press(forDuration: duration)
    }

    /// Double-tap quick reaction gesture on a message bubble (`F30`).
    public static func doubleTapBubble(_ bubble: XCUIElement) {
        bubble.doubleTap()
    }

    /// Hardware keyboard shortcut helper (`Cmd+L`, `Cmd+K`, `Esc`, etc.).
    public static func sendHardwareKey(
        in app: XCUIApplication,
        key: String,
        modifiers: XCUIKeyModifierFlags = [.command]
    ) {
        app.typeKey(key, modifierFlags: modifiers)
    }

    public static func sendSpecialKey(
        in app: XCUIApplication,
        key: XCUIKeyboardKey,
        modifiers: XCUIKeyModifierFlags = []
    ) {
        app.typeKey(key, modifierFlags: modifiers)
    }

    /// Native custom URL scheme dispatch (`fabushi://`, `gpui-telegram://`, `tg://`, `ton://`, `magpie://`).
    public static func openCustomURL(_ urlString: String) throws {
        guard let url = URL(string: urlString) else {
            throw URLError(.badURL)
        }
        XCUIDevice.shared.system.open(url)
    }

    /// Sub-pixel safe geometry assertion (`@3x` Retina `UIScreenPixel = 0.333pt`).
    public static func assertMetric(
        _ actual: CGFloat,
        equals expected: CGFloat,
        accuracy: CGFloat = 0.75,
        _ message: String = "",
        file: StaticString = #filePath,
        line: UInt = #line
    ) {
        XCTAssertEqual(actual, expected, accuracy: accuracy, message, file: file, line: line)
    }

    /// Verifies canonical `76pt` `ChatListItem` geometry (`60pt` avatar at `x=16pt`, text at `x=86pt`).
    public static func assertCanonicalChatListRowGeometry(
        row: XCUIElement,
        avatar: XCUIElement,
        title: XCUIElement,
        expectedRowHeight: CGFloat = TelegramGeometryMetrics.chatListRow3LineHeightPt,
        file: StaticString = #filePath,
        line: UInt = #line
    ) {
        assertMetric(row.frame.height, equals: expectedRowHeight, accuracy: 1.0, "ChatList row height", file: file, line: line)
        assertMetric(avatar.frame.width, equals: TelegramGeometryMetrics.chatListAvatarSizePt, accuracy: 1.0, "Avatar width", file: file, line: line)
        assertMetric(avatar.frame.height, equals: TelegramGeometryMetrics.chatListAvatarSizePt, accuracy: 1.0, "Avatar height", file: file, line: line)
        let avatarRelX = avatar.frame.minX - row.frame.minX
        assertMetric(avatarRelX, equals: TelegramGeometryMetrics.chatListAvatarLeadingXPt, accuracy: 1.0, "Avatar leading inset x=16pt", file: file, line: line)
        let titleRelX = title.frame.minX - row.frame.minX
        assertMetric(titleRelX, equals: TelegramGeometryMetrics.chatListTextLeadingXPt, accuracy: 1.5, "Title leading inset x=86pt", file: file, line: line)
    }
}
