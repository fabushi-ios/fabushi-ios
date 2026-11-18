import SwiftUI

/// Canonical 4-mode Telegram theme palette (`Day`, `Night`, `TintedNight`, `OLEDBlack`).
public enum TelegramAppearanceMode: String, CaseIterable, Codable, Sendable {
    case day = "day"
    case night = "night"
    case tintedNight = "tinted_night"
    case oledBlack = "oled_black"

    public var isDark: Bool {
        self != .day
    }
}

/// Deterministic RGBA color value supporting WCAG 2.1 relative luminance & contrast verification.
public struct RGBAColorToken: Codable, Sendable, Equatable {
    public let hex: UInt32
    public let alpha: Double

    public init(hex: UInt32, alpha: Double = 1.0) {
        self.hex = hex & 0xFFFFFF
        self.alpha = min(max(alpha, 0.0), 1.0)
    }

    public var red: Double { Double((hex >> 16) & 0xFF) / 255.0 }
    public var green: Double { Double((hex >> 8) & 0xFF) / 255.0 }
    public var blue: Double { Double(hex & 0xFF) / 255.0 }

    public var swiftUIColor: Color {
        Color(.sRGB, red: red, green: green, blue: blue, opacity: alpha)
    }

    public var relativeLuminance: Double {
        func linearize(_ c: Double) -> Double {
            c <= 0.03928 ? (c / 12.92) : pow((c + 0.055) / 1.055, 2.4)
        }
        return 0.2126 * linearize(red) + 0.7152 * linearize(green) + 0.0722 * linearize(blue)
    }

    public func contrastRatio(with other: RGBAColorToken) -> Double {
        let l1 = max(relativeLuminance, other.relativeLuminance)
        let l2 = min(relativeLuminance, other.relativeLuminance)
        return (l1 + 0.05) / (l2 + 0.05)
    }
}

/// Complete Telegram color & typography token bundle ported from `crates/gpui_telegram_desktop/src/theme`.
public struct TelegramPalette: Codable, Sendable, Equatable {
    public let mode: TelegramAppearanceMode
    public let background: RGBAColorToken
    public let secondaryBackground: RGBAColorToken
    public let elevatedSurface: RGBAColorToken
    public let outgoingBubble: RGBAColorToken
    public let incomingBubble: RGBAColorToken
    public let primaryText: RGBAColorToken
    public let secondaryText: RGBAColorToken
    public let accent: RGBAColorToken
    public let destructive: RGBAColorToken
    public let onlineIndicator: RGBAColorToken
    public let separator: RGBAColorToken

    public static func palette(for mode: TelegramAppearanceMode) -> TelegramPalette {
        switch mode {
        case .day:
            return TelegramPalette(
                mode: .day,
                background: RGBAColorToken(hex: 0xFFFFFF),
                secondaryBackground: RGBAColorToken(hex: 0xEFEFF4),
                elevatedSurface: RGBAColorToken(hex: 0xFFFFFF),
                outgoingBubble: RGBAColorToken(hex: 0xE1FFC7),
                incomingBubble: RGBAColorToken(hex: 0xFFFFFF),
                primaryText: RGBAColorToken(hex: 0x000000),
                secondaryText: RGBAColorToken(hex: 0x6D6D72),
                accent: RGBAColorToken(hex: 0x007AFF),
                destructive: RGBAColorToken(hex: 0xFF3B30),
                onlineIndicator: RGBAColorToken(hex: 0x34C759),
                separator: RGBAColorToken(hex: 0xC6C6C8)
            )
        case .night:
            return TelegramPalette(
                mode: .night,
                background: RGBAColorToken(hex: 0x1C1C1E),
                secondaryBackground: RGBAColorToken(hex: 0x000000),
                elevatedSurface: RGBAColorToken(hex: 0x2C2C2E),
                outgoingBubble: RGBAColorToken(hex: 0x2B5278),
                incomingBubble: RGBAColorToken(hex: 0x262628),
                primaryText: RGBAColorToken(hex: 0xFFFFFF),
                secondaryText: RGBAColorToken(hex: 0x8E8E93),
                accent: RGBAColorToken(hex: 0x0A84FF),
                destructive: RGBAColorToken(hex: 0xFF453A),
                onlineIndicator: RGBAColorToken(hex: 0x30D158),
                separator: RGBAColorToken(hex: 0x38383A)
            )
        case .tintedNight:
            return TelegramPalette(
                mode: .tintedNight,
                background: RGBAColorToken(hex: 0x17212B),
                secondaryBackground: RGBAColorToken(hex: 0x0E1621),
                elevatedSurface: RGBAColorToken(hex: 0x242F3D),
                outgoingBubble: RGBAColorToken(hex: 0x2B5278),
                incomingBubble: RGBAColorToken(hex: 0x182533),
                primaryText: RGBAColorToken(hex: 0xF5F5F5),
                secondaryText: RGBAColorToken(hex: 0x7F91A4),
                accent: RGBAColorToken(hex: 0x5288C1),
                destructive: RGBAColorToken(hex: 0xFF5B5B),
                onlineIndicator: RGBAColorToken(hex: 0x4ECC5E),
                separator: RGBAColorToken(hex: 0x101921)
            )
        case .oledBlack:
            return TelegramPalette(
                mode: .oledBlack,
                background: RGBAColorToken(hex: 0x000000),
                secondaryBackground: RGBAColorToken(hex: 0x000000),
                elevatedSurface: RGBAColorToken(hex: 0x121214),
                outgoingBubble: RGBAColorToken(hex: 0x1F3A52),
                incomingBubble: RGBAColorToken(hex: 0x121214),
                primaryText: RGBAColorToken(hex: 0xFFFFFF),
                secondaryText: RGBAColorToken(hex: 0x98989F),
                accent: RGBAColorToken(hex: 0x0A84FF),
                destructive: RGBAColorToken(hex: 0xFF453A),
                onlineIndicator: RGBAColorToken(hex: 0x30D158),
                separator: RGBAColorToken(hex: 0x222225)
            )
        }
    }
}

/// Complete typography, spacing, and surface tokens shared across iPhone & iPad layouts.
public struct TelegramThemeTokens: Codable, Sendable, Equatable {
    public static let minBodyFontPt: CGFloat = 13
    public static let maxBodyFontPt: CGFloat = 23
    public static let defaultBodyFontPt: CGFloat = 17

    public static let tabBarHeightPt: CGFloat = 49
    public static let tabBarIconPt: CGFloat = 24
    public static let tabBarLabelFontPt: CGFloat = 10
    public static let spacingUnitPt: CGFloat = 8

    public var palette: TelegramPalette
    public var bodyFontSizePt: CGFloat
    public var bubbleFreeMode: Bool
    public var liquidGlassEnabled: Bool

    public init(
        mode: TelegramAppearanceMode = .day,
        bodyFontSizePt: CGFloat = Self.defaultBodyFontPt,
        bubbleFreeMode: Bool = false,
        liquidGlassEnabled: Bool = true
    ) {
        self.palette = TelegramPalette.palette(for: mode)
        self.bodyFontSizePt = min(max(bodyFontSizePt, Self.minBodyFontPt), Self.maxBodyFontPt)
        self.bubbleFreeMode = bubbleFreeMode
        self.liquidGlassEnabled = liquidGlassEnabled
    }

    public var captionFontSizePt: CGFloat { max(11, bodyFontSizePt - 4) }
    public var titleFontSizePt: CGFloat { bodyFontSizePt + 3 }
}

/// Deterministic 7-bucket peer avatar gradient hasher matching Telegram iOS & Desktop.
public struct PeerAvatarGradient: Sendable, Equatable {
    public let bucketIndex: Int
    public let name: String
    public let top: RGBAColorToken
    public let bottom: RGBAColorToken
}

public enum PeerAvatarColorHasher {
    public static let buckets: [PeerAvatarGradient] = [
        PeerAvatarGradient(bucketIndex: 0, name: "red_orange", top: RGBAColorToken(hex: 0xFF885E), bottom: RGBAColorToken(hex: 0xFF516A)),
        PeerAvatarGradient(bucketIndex: 1, name: "orange_yellow", top: RGBAColorToken(hex: 0xFFCD6A), bottom: RGBAColorToken(hex: 0xFFA85C)),
        PeerAvatarGradient(bucketIndex: 2, name: "purple_violet", top: RGBAColorToken(hex: 0x82B1FF), bottom: RGBAColorToken(hex: 0x665FFF)),
        PeerAvatarGradient(bucketIndex: 3, name: "green_mint", top: RGBAColorToken(hex: 0xA0DE7E), bottom: RGBAColorToken(hex: 0x54CB68)),
        PeerAvatarGradient(bucketIndex: 4, name: "cyan_teal", top: RGBAColorToken(hex: 0x53EDD6), bottom: RGBAColorToken(hex: 0x28C9B7)),
        PeerAvatarGradient(bucketIndex: 5, name: "blue_sky", top: RGBAColorToken(hex: 0x72D5FD), bottom: RGBAColorToken(hex: 0x2A9EF1)),
        PeerAvatarGradient(bucketIndex: 6, name: "pink_rose", top: RGBAColorToken(hex: 0xE0A2F3), bottom: RGBAColorToken(hex: 0xD669ED)),
    ]

    public static func gradient(for peerId: Int64) -> PeerAvatarGradient {
        let magnitude = peerId == Int64.min ? UInt64(Int64.max) : UInt64(abs(peerId))
        let idx = Int(magnitude % UInt64(buckets.count))
        return buckets[idx]
    }
}

/// iOS 17+ Liquid Glass material helper with automatic `accessibilityReduceTransparency` solid fallback.
public struct LiquidGlassSurfaceModifier: ViewModifier {
    @Environment(\.accessibilityReduceTransparency) private var reduceTransparency
    public let tokens: TelegramThemeTokens
    public let cornerRadius: CGFloat

    public init(tokens: TelegramThemeTokens, cornerRadius: CGFloat = 18) {
        self.tokens = tokens
        self.cornerRadius = cornerRadius
    }

    public static func shouldUseOpaqueFallback(
        liquidGlassEnabled: Bool,
        reduceTransparency: Bool
    ) -> Bool {
        !liquidGlassEnabled || reduceTransparency
    }

    public func body(content: Content) -> some View {
        let useSolid = Self.shouldUseOpaqueFallback(
            liquidGlassEnabled: tokens.liquidGlassEnabled,
            reduceTransparency: reduceTransparency
        )
        if useSolid {
            content.background(
                RoundedRectangle(cornerRadius: cornerRadius, style: .continuous)
                    .fill(tokens.palette.elevatedSurface.swiftUIColor)
            )
        } else {
            content.background(
                .ultraThinMaterial,
                in: RoundedRectangle(cornerRadius: cornerRadius, style: .continuous)
            )
        }
    }
}

extension View {
    public func telegramLiquidGlass(
        tokens: TelegramThemeTokens,
        cornerRadius: CGFloat = 18
    ) -> some View {
        modifier(LiquidGlassSurfaceModifier(tokens: tokens, cornerRadius: cornerRadius))
    }
}
