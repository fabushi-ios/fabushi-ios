import Foundation
import SwiftUI
#if canImport(UIKit)
import UIKit
#endif

// MARK: - App-Store-Safe Liquid Glass Surface (`F45`, Public UIVisualEffectView + Specular Rim)

public struct GlassBackgroundView: View {
    public let cornerRadius: CGFloat
    public let isDark: Bool
    public let isInteractivePressed: Bool
    public let forceReduceTransparency: Bool?
    public let fallbackColor: Color

    @Environment(\.accessibilityReduceTransparency) private var envReduceTransparency

    public init(
        cornerRadius: CGFloat = 26.0,
        isDark: Bool = false,
        isInteractivePressed: Bool = false,
        forceReduceTransparency: Bool? = nil,
        fallbackColor: Color = Color(tgHex: 0xFFFFFF)
    ) {
        self.cornerRadius = cornerRadius
        self.isDark = isDark
        self.isInteractivePressed = isInteractivePressed
        self.forceReduceTransparency = forceReduceTransparency
        self.fallbackColor = fallbackColor
    }

    public static func shouldUseOpaqueSurface(
        useLiquidGlass: Bool,
        reduceTransparency: Bool
    ) -> Bool {
        !useLiquidGlass || reduceTransparency
    }

    public var effectiveReduceTransparency: Bool {
        if let forceReduceTransparency {
            return forceReduceTransparency
        }
        #if canImport(UIKit)
        return envReduceTransparency || UIAccessibility.isReduceTransparencyEnabled
        #else
        return envReduceTransparency
        #endif
    }

    public var body: some View {
        let shape = RoundedRectangle(cornerRadius: cornerRadius, style: .continuous)
        Group {
            if effectiveReduceTransparency {
                shape.fill(fallbackColor)
            } else {
                shape
                    .fill(isDark ? .ultraThinMaterial : .regularMaterial)
                    .overlay(
                        shape.strokeBorder(
                            LinearGradient(
                                colors: [
                                    Color.white.opacity(isDark ? 0.26 : 0.62),
                                    Color.white.opacity(isDark ? 0.06 : 0.14),
                                ],
                                startPoint: .topLeading,
                                endPoint: .bottomTrailing
                            ),
                            lineWidth: 0.5
                        )
                    )
                    .shadow(
                        color: Color.black.opacity(isDark ? 0.34 : 0.10),
                        radius: 14,
                        x: 0,
                        y: 6
                    )
            }
        }
        .scaleEffect(isInteractivePressed ? 1.06 : 1.0)
        .animation(.spring(response: 0.28, dampingFraction: 0.76), value: isInteractivePressed)
    }
}

// MARK: - Interactive Sliding & Lifting LiquidLensView (`fabushi.tabbar.liquidLens`)

public struct LiquidLensView: View {
    public let accentColor: Color
    public let isLifted: Bool
    public let isDark: Bool

    public init(
        accentColor: Color = Color(tgHex: 0x0088FF),
        isLifted: Bool = false,
        isDark: Bool = false
    ) {
        self.accentColor = accentColor
        self.isLifted = isLifted
        self.isDark = isDark
    }

    public var body: some View {
        Capsule(style: .continuous)
            .fill(accentColor.opacity(isDark ? 0.24 : 0.14))
            .overlay(
                Capsule(style: .continuous)
                    .strokeBorder(
                        LinearGradient(
                            colors: [
                                Color.white.opacity(isDark ? 0.35 : 0.75),
                                accentColor.opacity(0.35),
                            ],
                            startPoint: .topLeading,
                            endPoint: .bottomTrailing
                        ),
                        lineWidth: isLifted ? 1.2 : 0.7
                    )
            )
            .scaleEffect(isLifted ? 1.08 : 1.0)
            .animation(.spring(response: 0.26, dampingFraction: 0.74), value: isLifted)
    }
}

// MARK: - 50.0pt Top / Bottom Scroll-Edge Feathering (`EdgeEffectView`)

public enum EdgeEffectPosition: String, Sendable {
    case top
    case bottom
}

public struct EdgeEffectView: View {
    public static let defaultEdgeEffectHeight: CGFloat = 50.0

    public let position: EdgeEffectPosition
    public let height: CGFloat
    public let tintColor: Color

    public init(
        position: EdgeEffectPosition,
        height: CGFloat = EdgeEffectView.defaultEdgeEffectHeight,
        tintColor: Color = Color(tgHex: 0xFFFFFF)
    ) {
        self.position = position
        self.height = height
        self.tintColor = tintColor
    }

    public var body: some View {
        LinearGradient(
            colors: position == .top
                ? [tintColor.opacity(0.90), tintColor.opacity(0.0)]
                : [tintColor.opacity(0.0), tintColor.opacity(0.90)],
            startPoint: .top,
            endPoint: .bottom
        )
        .frame(height: height)
        .allowsHitTesting(false)
        .accessibilityElement(children: .ignore)
        .accessibilityIdentifier("fabushi.chrome.edgeEffect.\(position.rawValue)")
    }
}

// MARK: - PeerAvatarView (`60pt` ChatList, `40pt` Contacts/Calls, `34pt` Transcript, `104pt` Hero)

public struct PeerAvatarView: View {
    public let peerId: Int64
    public let title: String
    public let diameter: CGFloat
    public let hasActiveStory: Bool
    public let isOnline: Bool

    public init(
        peerId: Int64,
        title: String,
        diameter: CGFloat = 60.0,
        hasActiveStory: Bool = false,
        isOnline: Bool = false
    ) {
        self.peerId = peerId
        self.title = title
        self.diameter = diameter
        self.hasActiveStory = hasActiveStory
        self.isOnline = isOnline
    }

    public var initials: String {
        let words = title.split(whereSeparator: \.isWhitespace)
        if words.count >= 2,
           let first = words.first?.first,
           let second = words.dropFirst().first?.first {
            return String([first, second]).uppercased()
        }
        if let single = title.trimmingCharacters(in: .whitespacesAndNewlines).first {
            return String(single).uppercased()
        }
        return "?"
    }

    public var body: some View {
        let grad = TelegramThemeEngine.peerAvatarGradient(for: peerId)
        ZStack(alignment: .bottomTrailing) {
            ZStack {
                if hasActiveStory {
                    Circle()
                        .strokeBorder(
                            LinearGradient(
                                colors: [Color(tgHex: 0x34C759), Color(tgHex: 0x0088FF)],
                                startPoint: .topLeading,
                                endPoint: .bottomTrailing
                            ),
                            lineWidth: 2.0
                        )
                        .frame(width: diameter + 6.0, height: diameter + 6.0)
                }

                Circle()
                    .fill(
                        LinearGradient(
                            colors: [grad.topColor, grad.bottomColor],
                            startPoint: .top,
                            endPoint: .bottom
                        )
                    )
                    .frame(width: diameter, height: diameter)

                Text(initials)
                    .font(TelegramTypography.font(size: floor(diameter * 0.38), weight: .semibold, family: .rounded))
                    .foregroundStyle(.white)
            }

            if isOnline {
                Circle()
                    .fill(Color(tgHex: 0x34C759))
                    .frame(width: 12.0, height: 12.0)
                    .overlay(Circle().strokeBorder(Color.white, lineWidth: 2.0))
            }
        }
    }
}

// MARK: - Grouped Inset Card Modifier (`26.0pt` Glass / `11.0pt` Classic)

public struct TelegramGroupedCardModifier: ViewModifier {
    public static let glassCornerRadius: CGFloat = 26.0
    public static let classicCornerRadius: CGFloat = 11.0

    public let useLiquidGlass: Bool
    public let cardColor: Color

    public init(useLiquidGlass: Bool = true, cardColor: Color = Color(tgHex: 0xFFFFFF)) {
        self.useLiquidGlass = useLiquidGlass
        self.cardColor = cardColor
    }

    public var cornerRadius: CGFloat {
        useLiquidGlass ? Self.glassCornerRadius : Self.classicCornerRadius
    }

    public func body(content: Content) -> some View {
        content
            .background(
                RoundedRectangle(cornerRadius: cornerRadius, style: .continuous)
                    .fill(cardColor)
            )
            .clipShape(RoundedRectangle(cornerRadius: cornerRadius, style: .continuous))
    }
}

public extension View {
    func telegramGroupedCard(useLiquidGlass: Bool = true, cardColor: Color = Color(tgHex: 0xFFFFFF)) -> some View {
        modifier(TelegramGroupedCardModifier(useLiquidGlass: useLiquidGlass, cardColor: cardColor))
    }
}
