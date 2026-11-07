//
//  TelegramTab.swift
//  Fabushi
//
//  Canonical 4-tab definition and badge models matching Telegram iOS upstream.
//

import Foundation
import SwiftUI

/// Canonical 4-tab definition matching Telegram iOS upstream
public enum TelegramTab: Int, CaseIterable, Identifiable, Sendable {
    case chats = 0
    case calls = 1
    case contacts = 2
    case settings = 3

    public var id: Int { rawValue }

    public var title: String {
        switch self {
        case .chats: return "Chats"
        case .calls: return "Calls"
        case .contacts: return "Contacts"
        case .settings: return "Settings"
        }
    }

    public var selectedIcon: String {
        switch self {
        case .chats: return "bubble.left.and.bubble.right.fill"
        case .calls: return "phone.fill"
        case .contacts: return "person.2.fill"
        case .settings: return "gearshape.fill"
        }
    }

    public var unselectedIcon: String {
        switch self {
        case .chats: return "bubble.left.and.bubble.right"
        case .calls: return "phone"
        case .contacts: return "person.2"
        case .settings: return "gearshape"
        }
    }

    public var accessibilityIdentifier: String {
        switch self {
        case .chats: return "tab-bar-item-chats"
        case .calls: return "tab-bar-item-calls"
        case .contacts: return "tab-bar-item-contacts"
        case .settings: return "tab-bar-item-settings"
        }
    }
}

/// Dynamic tab bar badge model
public struct TabBarBadge: Equatable, Sendable {
    public let count: Int
    public let isMuted: Bool

    public init(count: Int, isMuted: Bool = false) {
        self.count = count
        self.isMuted = isMuted
    }

    public var text: String {
        if count > 999 { return "999+" }
        return "\(count)"
    }

    public var isVisible: Bool {
        count > 0
    }
}
