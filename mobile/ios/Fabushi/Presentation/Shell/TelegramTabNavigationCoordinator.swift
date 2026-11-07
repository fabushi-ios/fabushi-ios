//
//  TelegramTabNavigationCoordinator.swift
//  Fabushi
//
//  MainActor navigation coordinator for Telegram tabs, handling selection,
//  haptics, and reselection actions (scroll-to-top / scroll-to-first-unread).
//

import Foundation
import SwiftUI
#if canImport(UIKit)
import UIKit
#endif

@MainActor
public final class TelegramTabNavigationCoordinator: ObservableObject {
    public enum ReselectAction: Equatable, Sendable {
        case scrollToTop
        case scrollToFirstUnread
    }

    @Published public var selectedTab: TelegramTab = .chats
    @Published public private(set) var reselectTrigger: [TelegramTab: (action: ReselectAction, token: UUID)] = [:]

    private var isAtTopByTab: [TelegramTab: Bool] = [:]
    private var lastReselectTimestampByTab: [TelegramTab: Date] = [:]

    #if canImport(UIKit)
    private let lightImpact = UIImpactFeedbackGenerator(style: .light)
    private let selectionFeedback = UISelectionFeedbackGenerator()
    #endif

    public init() {
        #if canImport(UIKit)
        lightImpact.prepare()
        selectionFeedback.prepare()
        #endif
    }

    public func setIsAtTop(_ isAtTop: Bool, for tab: TelegramTab) {
        isAtTopByTab[tab] = isAtTop
    }

    public func isAtTop(for tab: TelegramTab) -> Bool {
        isAtTopByTab[tab] ?? true
    }

    public func handleTabTap(_ tab: TelegramTab) {
        let now = Date()
        if selectedTab == tab {
            // Re-selection on active tab
            let isAtTop = isAtTopByTab[tab] ?? true
            let previousReselectTimestamp = lastReselectTimestampByTab[tab] ?? .distantPast
            let isRapidDoubleTap = now.timeIntervalSince(previousReselectTimestamp) < 1.2

            let action: ReselectAction = (!isAtTop && !isRapidDoubleTap) ? .scrollToTop : .scrollToFirstUnread
            reselectTrigger[tab] = (action: action, token: UUID())
            lastReselectTimestampByTab[tab] = now

            #if canImport(UIKit)
            selectionFeedback.selectionChanged()
            selectionFeedback.prepare()
            #endif
        } else {
            // New tab selected
            selectedTab = tab
            lastReselectTimestampByTab[tab] = nil
            #if canImport(UIKit)
            lightImpact.impactOccurred()
            lightImpact.prepare()
            #endif
        }
    }
}
