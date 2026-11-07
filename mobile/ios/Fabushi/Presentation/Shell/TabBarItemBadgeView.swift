//
//  TabBarItemBadgeView.swift
//  Fabushi
//
//  Dynamic unread capsule badge matching Telegram iOS.
//

import SwiftUI
#if canImport(UIKit)
import UIKit
#endif

/// 1:1 Telegram iOS dynamic unread counter badge
public struct TabBarItemBadgeView: View {
    public let badge: TabBarBadge

    public init(badge: TabBarBadge) {
        self.badge = badge
    }

    public var body: some View {
        if badge.isVisible {
            Text(badge.text)
                .font(.system(size: 12, weight: .bold, design: .rounded))
                .foregroundStyle(.white)
                .padding(.horizontal, max(0, badge.count >= 10 ? 5 : 2))
                .frame(minWidth: 18, minHeight: 18)
                .background(
                    Capsule()
                        .fill(badge.isMuted ? mutedColor : unmutedColor)
                )
                .accessibilityElement(children: .ignore)
                .accessibilityLabel(badge.isMuted ? "\(badge.count) unread muted" : "\(badge.count) unread")
                .accessibilityIdentifier("tab-item-badge")
        }
    }

    private var mutedColor: Color {
        #if canImport(UIKit)
        return Color(uiColor: .systemGray)
        #else
        return Color(red: 0.557, green: 0.557, blue: 0.576)
        #endif
    }

    private var unmutedColor: Color {
        #if canImport(UIKit)
        return Color(uiColor: .systemRed)
        #else
        return Color(red: 1.0, green: 0.231, blue: 0.188)
        #endif
    }
}
