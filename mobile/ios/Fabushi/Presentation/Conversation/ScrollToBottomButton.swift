//
//  ScrollToBottomButton.swift
//  Fabushi
//
//  Floating 38x38 circular button with chevron-down icon, unread message badge,
//  hysteretic visibility control (>200pt appear, <100pt dismiss),
//  and keyboard clearance offset.
//

import SwiftUI

public struct ScrollToBottomButton: View {
    public let unreadCount: Int
    public let isVisible: Bool
    public let keyboardOffset: CGFloat
    public let onTap: () -> Void

    nonisolated public static let buttonSize: CGFloat = 38.0
    nonisolated public static let appearThreshold: CGFloat = 200.0
    nonisolated public static let dismissThreshold: CGFloat = 100.0
    nonisolated public static let unreadBadgeOffset: CGFloat = 8.0

    /// Pure hysteresis calculation logic:
    /// - Turns ON when distance > 200pt.
    /// - Remains ON until distance drops below 100pt.
    nonisolated public static func evaluateVisibility(distance: CGFloat, currentlyVisible: Bool) -> Bool {
        if currentlyVisible {
            return distance >= dismissThreshold
        } else {
            return distance > appearThreshold
        }
    }

    public init(
        unreadCount: Int,
        isVisible: Bool,
        keyboardOffset: CGFloat = 0.0,
        onTap: @escaping () -> Void
    ) {
        self.unreadCount = unreadCount
        self.isVisible = isVisible
        self.keyboardOffset = keyboardOffset
        self.onTap = onTap
    }

    private var badgeText: String {
        if unreadCount > 999 {
            return "999+"
        }
        return "\(unreadCount)"
    }

    public var body: some View {
        Button(action: onTap) {
            ZStack(alignment: .topTrailing) {
                // 38x38 circular pill with ultraThinMaterial
                Circle()
                    .fill(.ultraThinMaterial)
                    .frame(width: Self.buttonSize, height: Self.buttonSize)
                    .overlay(
                        Circle()
                            .stroke(Color.primary.opacity(0.08), lineWidth: 0.5)
                    )
                    .overlay(
                        Image(systemName: "chevron.down")
                            .font(.system(size: 15, weight: .semibold))
                            .foregroundStyle(Color.secondary)
                    )
                    // TC-T1-F18-05: Telegram-spec drop shadow (radius 4, opacity 0.25, y-offset 2)
                    .shadow(
                        color: Color.black.opacity(0.25),
                        radius: 4,
                        x: 0,
                        y: 2
                    )

                // Unread capsule badge
                if unreadCount > 0 {
                    Text(badgeText)
                        .font(.system(size: 11, weight: .bold, design: .rounded))
                        .foregroundStyle(Color.white)
                        .padding(.horizontal, 6)
                        .frame(height: 18)
                        .background(Capsule().fill(Color.red))
                        .offset(x: 6, y: -6)
                        .accessibilityIdentifier("scroll-bottom-unread-badge")
                }
            }
        }
        .buttonStyle(.plain)
        .opacity(isVisible ? 1.0 : 0.0)
        .scaleEffect(isVisible ? 1.0 : 0.6)
        .offset(y: -keyboardOffset)
        .animation(.spring(response: 0.3, dampingFraction: 0.75), value: isVisible)
        .animation(.easeOut(duration: 0.25), value: keyboardOffset)
        .accessibilityIdentifier("chat-scroll-to-bottom-button")
    }
}
