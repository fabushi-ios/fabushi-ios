//
//  TelegramTabBar.swift
//  Fabushi
//
//  Translucent 1:1 Telegram Tab Bar with standard 49pt height,
//  exact physical pixel top hairline separator, and dynamic badges.
//

import SwiftUI
#if canImport(UIKit)
import UIKit
#endif

public enum TelegramTabBarMetrics {
    public static let barHeight: CGFloat = 49.0
    public static let iconSize: CGFloat = 24.0
    public static let titleFontSize: CGFloat = 10.0
    public static let topSpacing: CGFloat = 4.0
    public static let bottomSpacing: CGFloat = 2.0

    public static var hairlineHeight: CGFloat {
        #if canImport(UIKit)
        return 1.0 / UIScreen.main.scale
        #else
        return 0.5
        #endif
    }
}

public struct TelegramTabBar: View {
    @ObservedObject public var coordinator: TelegramTabNavigationCoordinator
    public let chatsBadge: TabBarBadge
    public let callsBadge: TabBarBadge
    public let contactsBadge: TabBarBadge
    public let settingsBadge: TabBarBadge

    public init(
        coordinator: TelegramTabNavigationCoordinator,
        chatsBadge: TabBarBadge = TabBarBadge(count: 0),
        callsBadge: TabBarBadge = TabBarBadge(count: 0),
        contactsBadge: TabBarBadge = TabBarBadge(count: 0),
        settingsBadge: TabBarBadge = TabBarBadge(count: 0)
    ) {
        self.coordinator = coordinator
        self.chatsBadge = chatsBadge
        self.callsBadge = callsBadge
        self.contactsBadge = contactsBadge
        self.settingsBadge = settingsBadge
    }

    public var body: some View {
        VStack(spacing: 0) {
            // Top hairline separator (exact physical pixel)
            Rectangle()
                .fill(separatorColor)
                .frame(height: TelegramTabBarMetrics.hairlineHeight)

            // Tab bar items row (49pt standard height)
            HStack(spacing: 0) {
                tabItemView(tab: .chats, badge: chatsBadge)
                tabItemView(tab: .calls, badge: callsBadge)
                tabItemView(tab: .contacts, badge: contactsBadge)
                tabItemView(tab: .settings, badge: settingsBadge)
            }
            .frame(height: TelegramTabBarMetrics.barHeight)
        }
        .background {
            Rectangle()
                .fill(.ultraThinMaterial)
                .ignoresSafeArea(edges: .bottom)
        }
        .accessibilityIdentifier("telegram-root-tab-bar")
    }

    private var separatorColor: Color {
        #if canImport(UIKit)
        return Color(uiColor: .separator)
        #else
        return Color.gray.opacity(0.3)
        #endif
    }

    private var unselectedColor: Color {
        #if canImport(UIKit)
        return Color(uiColor: .systemGray)
        #else
        return Color(red: 0.557, green: 0.557, blue: 0.576)
        #endif
    }

    private func tabItemView(tab: TelegramTab, badge: TabBarBadge) -> some View {
        let isSelected = coordinator.selectedTab == tab

        return Button {
            coordinator.handleTabTap(tab)
        } label: {
            VStack(spacing: TelegramTabBarMetrics.bottomSpacing) {
                ZStack(alignment: .topTrailing) {
                    Image(systemName: isSelected ? tab.selectedIcon : tab.unselectedIcon)
                        .font(.system(size: TelegramTabBarMetrics.iconSize))
                        .frame(width: TelegramTabBarMetrics.iconSize + 8, height: TelegramTabBarMetrics.iconSize)

                    if badge.isVisible {
                        TabBarItemBadgeView(badge: badge)
                            .offset(x: 10, y: -6)
                    }
                }
                .padding(.top, TelegramTabBarMetrics.topSpacing)

                Text(tab.title)
                    .font(.system(size: TelegramTabBarMetrics.titleFontSize, weight: isSelected ? .medium : .regular))
                    .lineLimit(1)
            }
            .frame(maxWidth: .infinity)
            .foregroundStyle(isSelected ? Color(red: 0.0, green: 0.48, blue: 1.0) : unselectedColor)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier(tab.accessibilityIdentifier)
    }
}
