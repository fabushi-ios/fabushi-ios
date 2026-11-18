import Foundation
import Observation
import SwiftUI
#if canImport(UIKit)
import UIKit
#endif

// MARK: - Non-Colliding Hex Color Initializers (`tgHex:` avoids private `init(hex:)` ambiguity)

public extension Color {
    init(tgHex hex: UInt32, alpha: Double = 1.0) {
        let r = Double((hex >> 16) & 0xFF) / 255.0
        let g = Double((hex >> 8) & 0xFF) / 255.0
        let b = Double(hex & 0xFF) / 255.0
        self.init(.sRGB, red: r, green: g, blue: b, opacity: alpha)
    }

    init(tgHexString raw: String, fallback: UInt32 = 0x007AFF, alpha: Double = 1.0) {
        let cleaned = raw.trimmingCharacters(in: .whitespacesAndNewlines)
            .replacingOccurrences(of: "#", with: "")
            .replacingOccurrences(of: "0x", with: "")
        let parsed = UInt32(cleaned, radix: 16) ?? fallback
        self.init(tgHex: parsed, alpha: alpha)
    }
}

#if canImport(UIKit)
public extension UIColor {
    convenience init(tgHex hex: UInt32, alpha: CGFloat = 1.0) {
        let r = CGFloat((hex >> 16) & 0xFF) / 255.0
        let g = CGFloat((hex >> 8) & 0xFF) / 255.0
        let b = CGFloat(hex & 0xFF) / 255.0
        self.init(red: r, green: g, blue: b, alpha: alpha)
    }
}
#endif

// MARK: - 4 Canonical Telegram Theme Modes (`F12`, `Telegram-iOS` PresentationTheme)

public enum TelegramThemeMode: String, CaseIterable, Codable, Sendable, Identifiable {
    case dayClassic
    case day
    case night
    case nightTinted

    public var id: String { rawValue }

    public var isDark: Bool {
        switch self {
        case .dayClassic, .day: return false
        case .night, .nightTinted: return true
        }
    }

    /// `Cmd+D` theme cycle order (`Night -> NightTinted -> Day -> DayClassic`).
    public func cycleNext() -> TelegramThemeMode {
        switch self {
        case .night: return .nightTinted
        case .nightTinted: return .day
        case .day: return .dayClassic
        case .dayClassic: return .night
        }
    }
}

public struct TelegramThemePalette: Sendable, Equatable {
    public let mode: TelegramThemeMode
    public let overallDarkAppearance: Bool
    public let accentHex: UInt32
    public let glassAccentHex: UInt32
    public let destructiveHex: UInt32
    public let constructiveHex: UInt32
    public let warningHex: UInt32
    public let navOpaqueHex: UInt32
    public let plainBgHex: UInt32
    public let blocksBgHex: UInt32
    public let cardBgHex: UInt32
    public let primaryTextHex: UInt32
    public let secondaryTextHex: UInt32
    public let separatorHex: UInt32
    public let unreadPillHex: UInt32
    public let mutedUnreadPillHex: UInt32
    public let draftRedHex: UInt32
    public let incomingBubbleHex: UInt32
    public let outgoingBubbleHex: UInt32
    public let outgoingPrimaryTextHex: UInt32
    public let outgoingTimestampHex: UInt32
    public let readCheckHex: UInt32
    public let wallpaperHex: UInt32

    public var accent: Color { Color(tgHex: accentHex) }
    public var destructive: Color { Color(tgHex: destructiveHex) }
    public var constructive: Color { Color(tgHex: constructiveHex) }
    public var warning: Color { Color(tgHex: warningHex) }
    public var navOpaque: Color { Color(tgHex: navOpaqueHex) }
    public var plainBackground: Color { Color(tgHex: plainBgHex) }
    public var blocksBackground: Color { Color(tgHex: blocksBgHex) }
    public var cardBackground: Color { Color(tgHex: cardBgHex) }
    public var primaryText: Color { Color(tgHex: primaryTextHex) }
    public var secondaryText: Color { Color(tgHex: secondaryTextHex) }
    public var separator: Color { Color(tgHex: separatorHex) }
    public var incomingBubble: Color { Color(tgHex: incomingBubbleHex) }
    public var outgoingBubble: Color { Color(tgHex: outgoingBubbleHex) }

    public static let dayClassic = TelegramThemePalette(
        mode: .dayClassic,
        overallDarkAppearance: false,
        accentHex: 0x007AFF,
        glassAccentHex: 0x0088FF,
        destructiveHex: 0xFF3B30,
        constructiveHex: 0x34C759,
        warningHex: 0xF09A37,
        navOpaqueHex: 0xF7F7F7,
        plainBgHex: 0xFFFFFF,
        blocksBgHex: 0xEFEFF4,
        cardBgHex: 0xFFFFFF,
        primaryTextHex: 0x000000,
        secondaryTextHex: 0x8E8E93,
        separatorHex: 0xC8C7CC,
        unreadPillHex: 0x007AFF,
        mutedUnreadPillHex: 0xC7C7CC,
        draftRedHex: 0xDD4B39,
        incomingBubbleHex: 0xFFFFFF,
        outgoingBubbleHex: 0xE1FFC7,
        outgoingPrimaryTextHex: 0x000000,
        outgoingTimestampHex: 0x55A646,
        readCheckHex: 0x35B32E,
        wallpaperHex: 0xD6E2EE
    )

    public static let day = TelegramThemePalette(
        mode: .day,
        overallDarkAppearance: false,
        accentHex: 0x007AFF,
        glassAccentHex: 0x0088FF,
        destructiveHex: 0xFF3B30,
        constructiveHex: 0x34C759,
        warningHex: 0xF09A37,
        navOpaqueHex: 0xFFFFFF,
        plainBgHex: 0xFFFFFF,
        blocksBgHex: 0xF2F2F7,
        cardBgHex: 0xFFFFFF,
        primaryTextHex: 0x000000,
        secondaryTextHex: 0x8E8E93,
        separatorHex: 0xC6C6C8,
        unreadPillHex: 0x007AFF,
        mutedUnreadPillHex: 0xC7C7CC,
        draftRedHex: 0xDD4B39,
        incomingBubbleHex: 0xF1F1F4,
        outgoingBubbleHex: 0x007AFF,
        outgoingPrimaryTextHex: 0xFFFFFF,
        outgoingTimestampHex: 0xE5F1FF,
        readCheckHex: 0xFFFFFF,
        wallpaperHex: 0xFFFFFF
    )

    public static let night = TelegramThemePalette(
        mode: .night,
        overallDarkAppearance: true,
        accentHex: 0x007AFF,
        glassAccentHex: 0x0A84FF,
        destructiveHex: 0xFF453A,
        constructiveHex: 0x30D158,
        warningHex: 0xFF9F0A,
        navOpaqueHex: 0x1C1C1D,
        plainBgHex: 0x000000,
        blocksBgHex: 0x000000,
        cardBgHex: 0x1C1C1D,
        primaryTextHex: 0xFFFFFF,
        secondaryTextHex: 0x8E8E93,
        separatorHex: 0x3D3D40,
        unreadPillHex: 0x0A84FF,
        mutedUnreadPillHex: 0x636366,
        draftRedHex: 0xFF453A,
        incomingBubbleHex: 0x1F1F1F,
        outgoingBubbleHex: 0x313131,
        outgoingPrimaryTextHex: 0xFFFFFF,
        outgoingTimestampHex: 0x98989E,
        readCheckHex: 0x0A84FF,
        wallpaperHex: 0x000000
    )

    public static let nightTinted = TelegramThemePalette(
        mode: .nightTinted,
        overallDarkAppearance: true,
        accentHex: 0x2EA6FF,
        glassAccentHex: 0x2EA6FF,
        destructiveHex: 0xFF5B5B,
        constructiveHex: 0x34C759,
        warningHex: 0xF09A37,
        navOpaqueHex: 0x212F3F,
        plainBgHex: 0x18222D,
        blocksBgHex: 0x18222D,
        cardBgHex: 0x213040,
        primaryTextHex: 0xFFFFFF,
        secondaryTextHex: 0x7E91A4,
        separatorHex: 0x131C26,
        unreadPillHex: 0x2EA6FF,
        mutedUnreadPillHex: 0x4B5C6E,
        draftRedHex: 0xEF5B5B,
        incomingBubbleHex: 0x213040,
        outgoingBubbleHex: 0x3D6A97,
        outgoingPrimaryTextHex: 0xFFFFFF,
        outgoingTimestampHex: 0x92B9E3,
        readCheckHex: 0x62B8FF,
        wallpaperHex: 0x18222D
    )

    public static func palette(for mode: TelegramThemeMode) -> TelegramThemePalette {
        switch mode {
        case .dayClassic: return .dayClassic
        case .day: return .day
        case .night: return .night
        case .nightTinted: return .nightTinted
        }
    }
}

// MARK: - 7 Deterministic Peer Avatar Gradients (`abs(peerId) % 7`)

public struct PeerAvatarGradientSpec: Sendable, Equatable {
    public let index: Int
    public let topHex: UInt32
    public let bottomHex: UInt32

    public var topColor: Color { Color(tgHex: topHex) }
    public var bottomColor: Color { Color(tgHex: bottomHex) }
}

// MARK: - Persisted Appearance Snapshot

private struct PersistedThemeSnapshot: Codable, Sendable {
    var currentMode: TelegramThemeMode
    var accentColorHex: String
    var fontSizeStep: PresentationFontSize
    var bubbleCorners: BubbleCornersConfig
    var useLiquidGlassChrome: Bool
    var wallpaperPresetId: String
    var isWallpaperBlurred: Bool
    var language: AppLanguage
}

/// Central `@MainActor @Observable` theme engine (`PROJECT.md § IC.2`).
@MainActor
@Observable
public final class TelegramThemeEngine {
    public static let shared = TelegramThemeEngine()

    public static let peerAvatarGradientTable: [PeerAvatarGradientSpec] = [
        PeerAvatarGradientSpec(index: 0, topHex: 0xFF885E, bottomHex: 0xFC5C51), // Red
        PeerAvatarGradientSpec(index: 1, topHex: 0xFFCD6A, bottomHex: 0xFA790F), // Orange
        PeerAvatarGradientSpec(index: 2, topHex: 0xB694F9, bottomHex: 0x895DD5), // Violet
        PeerAvatarGradientSpec(index: 3, topHex: 0x53EDD6, bottomHex: 0x0FB297), // Green
        PeerAvatarGradientSpec(index: 4, topHex: 0x5CE2E6, bottomHex: 0x00C0C2), // Cyan
        PeerAvatarGradientSpec(index: 5, topHex: 0x72D5FD, bottomHex: 0x3CA5EC), // Light Blue
        PeerAvatarGradientSpec(index: 6, topHex: 0x82B1FF, bottomHex: 0x3D72ED), // Blue
    ]

    public nonisolated static func peerAvatarGradient(for peerId: Int64) -> PeerAvatarGradientSpec {
        let mag = peerId.magnitude
        let idx = Int(mag % 7)
        return peerAvatarGradientTable[idx]
    }

    public var currentMode: TelegramThemeMode {
        didSet { persist() }
    }
    public var accentColorHex: String {
        didSet { persist() }
    }
    public var fontSizeStep: PresentationFontSize {
        didSet { persist() }
    }
    public var bubbleCorners: BubbleCornersConfig {
        didSet { persist() }
    }
    public var useLiquidGlassChrome: Bool {
        didSet { persist() }
    }
    public var wallpaperPresetId: String {
        didSet { persist() }
    }
    public var isWallpaperBlurred: Bool {
        didSet { persist() }
    }
    public var language: AppLanguage {
        didSet { persist() }
    }

    public var palette: TelegramThemePalette {
        TelegramThemePalette.palette(for: currentMode)
    }

    public var effectiveAccentColor: Color {
        Color(tgHexString: accentColorHex, fallback: palette.accentHex)
    }

    public var colorScheme: ColorScheme {
        currentMode.isDark ? .dark : .light
    }

    private let preferencesURL: URL
    private let userDefaults: UserDefaults
    private var isRestoring = false

    public init(
        appSupportDir: URL? = nil,
        userDefaults: UserDefaults = .standard
    ) {
        self.userDefaults = userDefaults
        let baseDir = appSupportDir ?? FileManager.default.urls(
            for: .applicationSupportDirectory,
            in: .userDomainMask
        )[0].appendingPathComponent("Fabushi", isDirectory: true)
        self.preferencesURL = baseDir.appendingPathComponent("preferences.json", isDirectory: false)

        if let data = try? Data(contentsOf: preferencesURL),
           let snap = try? JSONDecoder().decode(PersistedThemeSnapshot.self, from: data) {
            self.isRestoring = true
            self.currentMode = snap.currentMode
            self.accentColorHex = snap.accentColorHex
            self.fontSizeStep = snap.fontSizeStep
            self.bubbleCorners = snap.bubbleCorners
            self.useLiquidGlassChrome = snap.useLiquidGlassChrome
            self.wallpaperPresetId = snap.wallpaperPresetId
            self.isWallpaperBlurred = snap.isWallpaperBlurred
            self.language = snap.language
            self.isRestoring = false
        } else {
            self.currentMode = .dayClassic
            self.accentColorHex = "#007AFF"
            self.fontSizeStep = .regular
            self.bubbleCorners = BubbleCornersConfig()
            self.useLiquidGlassChrome = true
            self.wallpaperPresetId = "classic-doodle"
            self.isWallpaperBlurred = false
            self.language = LocalizationManager.shared.language
        }
    }

    @discardableResult
    public func cycleThemeMode() -> TelegramThemeMode {
        currentMode = currentMode.cycleNext()
        return currentMode
    }

    private func persist() {
        guard !isRestoring else { return }
        let snap = PersistedThemeSnapshot(
            currentMode: currentMode,
            accentColorHex: accentColorHex,
            fontSizeStep: fontSizeStep,
            bubbleCorners: bubbleCorners,
            useLiquidGlassChrome: useLiquidGlassChrome,
            wallpaperPresetId: wallpaperPresetId,
            isWallpaperBlurred: isWallpaperBlurred,
            language: language
        )
        if let encoded = try? JSONEncoder().encode(snap) {
            try? FileManager.default.createDirectory(
                at: preferencesURL.deletingLastPathComponent(),
                withIntermediateDirectories: true
            )
            try? encoded.write(to: preferencesURL, options: .atomic)
            userDefaults.set(currentMode.rawValue, forKey: "com.ombhrum.fabushi.theme_mode")
        }
    }
}

private struct TelegramThemeEngineKey: EnvironmentKey {
    @MainActor static let defaultValue = TelegramThemeEngine.shared
}

public extension EnvironmentValues {
    var telegramThemeEngine: TelegramThemeEngine {
        get { self[TelegramThemeEngineKey.self] }
        set { self[TelegramThemeEngineKey.self] = newValue }
    }
}
