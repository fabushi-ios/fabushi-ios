//
//  TelegramRootShellView.swift
//  Fabushi
//
//  1:1 Telegram Root Shell View orchestrating 4-tab navigation (Chats, Calls,
//  Contacts, Settings), translucent bottom bar, dynamic badges, and isolated tab stacks.
//

import SwiftUI
#if canImport(UIKit)
import UIKit
#endif

/// 1:1 Telegram Root Shell View orchestrating primary navigation and bottom tab bar
struct TelegramRootShellView: View {
    @Bindable var messaging: MessagingModel
    @Bindable var model: MarketplaceModel
    let bridge: IOSPreloadBridge?
    let appAgentSurface: FabushiAppAgentSurface
    let reconnectGeneration: Int

    @StateObject private var coordinator = TelegramTabNavigationCoordinator()

    init(
        messaging: MessagingModel,
        model: MarketplaceModel,
        bridge: IOSPreloadBridge? = nil,
        appAgentSurface: FabushiAppAgentSurface,
        reconnectGeneration: Int = 0
    ) {
        self.messaging = messaging
        self.model = model
        self.bridge = bridge
        self.appAgentSurface = appAgentSurface
        self.reconnectGeneration = reconnectGeneration
    }

    // Dynamic badge calculation from MessagingModel
    private var chatsBadge: TabBarBadge {
        let unmutedCount = messaging.conversations
            .filter { !$0.isArchived && !$0.isMuted && ($0.unreadCount > 0 || $0.markedUnread) }
            .reduce(0) { $0 + max(1, $1.unreadCount) }

        if unmutedCount > 0 {
            return TabBarBadge(count: unmutedCount, isMuted: false)
        }

        let mutedCount = messaging.conversations
            .filter { !$0.isArchived && $0.isMuted && ($0.unreadCount > 0 || $0.markedUnread) }
            .reduce(0) { $0 + max(1, $1.unreadCount) }

        if mutedCount > 0 {
            return TabBarBadge(count: mutedCount, isMuted: true)
        }

        return TabBarBadge(count: 0)
    }

    var body: some View {
        ZStack(alignment: .bottom) {
            // Tab content stage (independent navigation stacks)
            Group {
                switch coordinator.selectedTab {
                case .chats:
                    chatsTabStage
                case .calls:
                    callsTabStage
                case .contacts:
                    contactsTabStage
                case .settings:
                    settingsTabStage
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)

            // 1:1 Translucent Telegram Tab Bar
            TelegramTabBar(
                coordinator: coordinator,
                chatsBadge: chatsBadge,
                callsBadge: TabBarBadge(count: 0),
                contactsBadge: TabBarBadge(count: 0),
                settingsBadge: TabBarBadge(count: 0)
            )
        }
        .ignoresSafeArea(.keyboard, edges: .bottom)
        .accessibilityIdentifier("telegram-root-shell")
    }

    // MARK: - Tab Stages
    private var chatsTabStage: some View {
        NavigationStack {
            ChatListView(
                messaging: messaging,
                coordinator: coordinator,
                bridge: bridge
            )
        }
    }

    private var callsTabStage: some View {
        NavigationStack {
            TelegramCallsStageView(
                messaging: messaging,
                coordinator: coordinator
            )
        }
    }

    private var contactsTabStage: some View {
        NavigationStack {
            TelegramContactsStageView(
                messaging: messaging,
                coordinator: coordinator
            )
        }
    }

    private var settingsTabStage: some View {
        NavigationStack {
            TelegramSettingsView(
                model: model,
                messaging: messaging,
                coordinator: coordinator
            )
        }
    }
}

// MARK: - Calls Stage View
struct TelegramCallsStageView: View {
    @Bindable var messaging: MessagingModel
    @ObservedObject var coordinator: TelegramTabNavigationCoordinator
    @State private var callFilter: Int = 0 // 0 = All, 1 = Missed

    var body: some View {
        List {
            Section {
                VStack(spacing: 12) {
                    Image(systemName: "phone.circle.fill")
                        .font(.system(size: 64))
                        .foregroundStyle(Color.accentColor)
                    Text("No Recent Calls")
                        .font(.system(size: 18, weight: .semibold))
                    Text("Your recent voice and video calls will appear here.")
                        .font(.system(size: 14))
                        .foregroundStyle(.secondary)
                        .multilineTextAlignment(.center)
                }
                .frame(maxWidth: .infinity)
                .padding(.vertical, 40)
                .listRowBackground(Color.clear)
            }
        }
        .listStyle(.insetGrouped)
        .navigationTitle("Calls")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .principal) {
                Picker("Calls", selection: $callFilter) {
                    Text("All").tag(0)
                    Text("Missed").tag(1)
                }
                .pickerStyle(.segmented)
                .frame(width: 160)
            }
            ToolbarItem(placement: .topBarTrailing) {
                Button {
                    // New Call
                } label: {
                    Image(systemName: "phone.badge.plus")
                        .font(.system(size: 16, weight: .semibold))
                }
            }
        }
    }
}

// MARK: - Contacts Stage View
struct TelegramContactsStageView: View {
    @Bindable var messaging: MessagingModel
    @ObservedObject var coordinator: TelegramTabNavigationCoordinator
    @State private var searchText: String = ""

    var filteredContacts: [MessagingContact] {
        let trimmed = searchText.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard !trimmed.isEmpty else { return messaging.contacts }
        return messaging.contacts.filter {
            $0.displayName.lowercased().contains(trimmed) || ($0.username?.lowercased().contains(trimmed) ?? false)
        }
    }

    var body: some View {
        List {
            Section {
                Button {
                    // Find People Nearby
                } label: {
                    Label("Find People Nearby", systemImage: "location.circle.fill")
                        .foregroundStyle(Color.accentColor)
                }
                Button {
                    // Invite Friends
                } label: {
                    Label("Invite Friends", systemImage: "person.badge.plus")
                        .foregroundStyle(Color.accentColor)
                }
            }

            if filteredContacts.isEmpty {
                Section {
                    Text(searchText.isEmpty ? "No Contacts Found" : "No Matching Contacts")
                        .font(.system(size: 14))
                        .foregroundStyle(.secondary)
                }
            } else {
                Section(header: Text("CONTACTS").font(.system(size: 12, weight: .bold))) {
                    ForEach(filteredContacts) { contact in
                        HStack(spacing: 12) {
                            Circle()
                                .fill(Color.accentColor.opacity(0.85))
                                .frame(width: 40, height: 40)
                                .overlay {
                                    Text(String(contact.displayName.prefix(1)).uppercased())
                                        .font(.headline)
                                        .foregroundStyle(.white)
                                }
                            VStack(alignment: .leading, spacing: 2) {
                                Text(contact.displayName)
                                    .font(.system(size: 16, weight: .medium))
                                Text(contact.isOnline ? "online" : "last seen recently")
                                    .font(.system(size: 13))
                                    .foregroundStyle(contact.isOnline ? Color.accentColor : .secondary)
                            }
                        }
                        .padding(.vertical, 2)
                    }
                }
            }
        }
        .listStyle(.insetGrouped)
        .navigationTitle("Contacts")
        .navigationBarTitleDisplayMode(.inline)
        .searchable(text: $searchText, prompt: "Search contacts")
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button {
                    // Add contact
                } label: {
                    Image(systemName: "plus")
                        .font(.system(size: 16, weight: .semibold))
                }
            }
        }
    }
}

// MARK: - Settings Stage View
struct TelegramSettingsStageView: View {
    @Bindable var model: MarketplaceModel
    @Bindable var messaging: MessagingModel
    @ObservedObject var coordinator: TelegramTabNavigationCoordinator

    var body: some View {
        List {
            // Profile Card
            Section {
                HStack(spacing: 14) {
                    Circle()
                        .fill(Color(hex: 0x0072FF))
                        .frame(width: 60, height: 60)
                        .overlay {
                            Text("✦")
                                .font(.title2)
                                .foregroundStyle(.white)
                        }

                    VStack(alignment: .leading, spacing: 4) {
                        Text(model.accountName.isEmpty ? "Telegram User" : model.accountName)
                            .font(.system(size: 18, weight: .semibold))
                        Text("+1 (555) 019-2834")
                            .font(.system(size: 14))
                            .foregroundStyle(.secondary)
                        Text("@fabushi_user")
                            .font(.system(size: 13))
                            .foregroundStyle(Color.accentColor)
                    }
                }
                .padding(.vertical, 6)
            }

            // Telegram Core Features
            Section {
                Label("Saved Messages", systemImage: "bookmark.fill")
                    .tint(.blue)
                Label("Recent Calls", systemImage: "phone.fill")
                    .tint(.green)
                Label("Devices", systemImage: "laptopcomputer.and.iphone")
                    .tint(.orange)
                Label("Chat Folders", systemImage: "folder.fill")
                    .tint(.blue)
            }

            // Telegram Preferences
            Section {
                Label("Notifications and Sounds", systemImage: "bell.badge.fill")
                    .tint(.red)
                Label("Privacy and Security", systemImage: "lock.fill")
                    .tint(.gray)
                Label("Data and Storage", systemImage: "cylinder.split.1x2.fill")
                    .tint(.green)
                Label("Appearance", systemImage: "paintpalette.fill")
                    .tint(.cyan)
                Label("Language", systemImage: "globe")
                    .tint(.purple)
            }
        }
        .listStyle(.insetGrouped)
        .navigationTitle("Settings")
        .navigationBarTitleDisplayMode(.inline)
    }
}

// Color hex helper for Settings
private extension Color {
    init(hex: UInt32, alpha: Double = 1.0) {
        let red = Double((hex >> 16) & 0xFF) / 255.0
        let green = Double((hex >> 8) & 0xFF) / 255.0
        let blue = Double(hex & 0xFF) / 255.0
        self.init(.sRGB, red: red, green: green, blue: blue, opacity: alpha)
    }
}

// Extension to determine contact presence
extension MessagingContact {
    var isOnline: Bool {
        kind.caseInsensitiveCompare("online") == .orderedSame
    }
}

