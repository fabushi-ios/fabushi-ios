import Foundation
import SwiftUI

/// 7-Step `PresentationFontSize` scale (`14..26pt`) from `TelegramMessenger/Telegram-iOS` (`F46`).
public enum PresentationFontSize: Int, CaseIterable, Codable, Sendable, Identifiable {
    case extraSmall = 14
    case small = 15
    case medium = 16
    case regular = 17
    case large = 19
    case extraLarge = 23
    case extraLargeX2 = 26

    public var id: Int { rawValue }

    public var baseDisplaySize: CGFloat {
        CGFloat(rawValue)
    }

    /// Proportional scaling formula anchored at `.regular` (`17pt`).
    public func scaledSize(_ targetAtRegular: CGFloat) -> CGFloat {
        floor(CGFloat(rawValue) * targetAtRegular / 17.0)
    }

    /// Secondary label font sizes (`11, 12, 13, 14, 16, 20, 23pt`).
    public var secondaryLabelSize: CGFloat {
        switch self {
        case .extraSmall: return 11
        case .small: return 12
        case .medium: return 13
        case .regular: return 14
        case .large: return 16
        case .extraLarge: return 20
        case .extraLargeX2: return 23
        }
    }

    /// Grouped section header sizes (`10, 11, 12, 13, 14, 17, 19pt`).
    public var sectionHeaderSize: CGFloat {
        switch self {
        case .extraSmall: return 10
        case .small: return 11
        case .medium: return 12
        case .regular: return 13
        case .large: return 14
        case .extraLarge: return 17
        case .extraLargeX2: return 19
        }
    }

    /// Strict `31.0pt` floor for `<= 17pt` composer text field (`31, 31, 31, 31, 33, 38, 42pt`).
    public var emptyComposerMinHeight: CGFloat {
        switch self {
        case .extraSmall, .small, .medium, .regular:
            return 31.0
        case .large:
            return 33.0
        case .extraLarge:
            return 38.0
        case .extraLargeX2:
            return 42.0
        }
    }
}

public enum TelegramFontFamily: String, Codable, Sendable {
    case regular
    case rounded
    case monospace
    case serif
}

public enum TelegramTypography {
    public static func font(
        size: CGFloat,
        weight: Font.Weight = .regular,
        family: TelegramFontFamily = .regular
    ) -> Font {
        switch family {
        case .regular:
            return .system(size: size, weight: weight, design: .default)
        case .rounded:
            return .system(size: size, weight: weight, design: .rounded)
        case .monospace:
            return .custom("Menlo-Regular", size: size)
        case .serif:
            return .custom("Georgia", size: size)
        }
    }

    public static func body(_ step: PresentationFontSize = .regular, weight: Font.Weight = .regular) -> Font {
        font(size: step.baseDisplaySize, weight: weight, family: .regular)
    }

    public static func monospacedTimestamp(_ step: PresentationFontSize = .regular) -> Font {
        .system(size: step.scaledSize(11), weight: .regular, design: .default).monospacedDigit()
    }
}
