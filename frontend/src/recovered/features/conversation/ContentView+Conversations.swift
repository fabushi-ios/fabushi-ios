import SwiftUI
import UniformTypeIdentifiers
import UIKit

extension ContentView {
    var visibleConversations: [ConversationSummary] {
        messaging.conversations.filter { !$0.isArchived }
            .sorted { lhs, rhs in lhs.isPinned != rhs.isPinned ? lhs.isPinned : lhs.time > rhs.time }
    }

    var archivedConversations: [ConversationSummary] { messaging.conversations.filter(\.isArchived) }

    var filteredConversations: [ConversationSummary] {
        let query = homeQuery.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !query.isEmpty else { return visibleConversations }
        return visibleConversations.filter { $0.title.localizedCaseInsensitiveContains(query) || $0.preview.localizedCaseInsensitiveContains(query) }
    }

    func startCompose(_ kind: ConversationKind) {
        composeName = ""
        composeDescription = ""
        composeParticipantIds = []
        composeKind = kind
    }

    @ViewBuilder
    func composeSheet(_ kind: ConversationKind) -> some View {
        NavigationStack {
            Form {
                Section {
                    TextField(kind == .direct ? "联系人名称" : "名称", text: $composeName)
                        .accessibilityIdentifier("compose-name")
                    if kind == .channel { TextField("描述", text: $composeDescription, axis: .vertical) }
                }
                if kind == .group {
                    Section("添加成员") {
                        if messaging.contacts.isEmpty {
                            Text("暂无可用联系人").foregroundStyle(.secondary)
                        } else {
                            ForEach(messaging.contacts) { contact in
                                Button {
                                    if composeParticipantIds.contains(contact.id) { composeParticipantIds.remove(contact.id) } else { composeParticipantIds.insert(contact.id) }
                                } label: {
                                    HStack {
                                        VStack(alignment: .leading) { Text(contact.displayName).foregroundStyle(.primary); Text(contact.username.map { "@\($0)" } ?? contact.kind).font(.caption).foregroundStyle(.secondary) }
                                        Spacer()
                                        if composeParticipantIds.contains(contact.id) { Image(systemName: "checkmark.circle.fill").foregroundStyle(Color.accentColor) }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            .navigationTitle("新建\(kind.label)")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("取消") { composeKind = nil } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("创建") { createConversation(kind: kind) }.accessibilityIdentifier("compose-create").disabled(composeName.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || (kind == .group && composeParticipantIds.isEmpty))
                }
            }
        }
    }

    func createConversation(kind: ConversationKind) {
        let title = composeName.trimmingCharacters(in: .whitespacesAndNewlines)
        let description = composeDescription
        let participantIds = Array(composeParticipantIds)
        Task {
            do {
                let created = try await messaging.createConversation(kind: kind, title: title, description: description, participantActorIds: participantIds)
                composeKind = nil
                if let created { selectedConversation = created }
            } catch {
                model.message = "创建会话失败：\(error.localizedDescription)"
            }
        }
    }

    func handleSection(_ section: MobileSection) {
        switch section {
        case .chats: activeSection = nil
        case .miniapps: destination = .marketplace
        default: activeSection = section
        }
    }

    @ViewBuilder
    func sectionSheet(_ section: MobileSection) -> some View {
        if section == .folders {
            NavigationStack {
                List {
                    Section {
                        Button { folderTitle = ""; folderConversationIds = []; folderIncludeGroups = false; folderIncludeChannels = false; folderEditorPresented = true } label: { Label("新建文件夹", systemImage: "folder.badge.plus") }
                    }
                    Section("我的文件夹") {
                        if messaging.folders.isEmpty { Text("暂无会话文件夹").foregroundStyle(.secondary) }
                        ForEach(messaging.folders) { folder in
                            NavigationLink {
                                List {
                                    ForEach(folderConversations(folder)) { conversation in conversationRow(conversation) }
                                }.navigationTitle(folder.title)
                            } label: {
                                HStack { Image(systemName: "folder.fill").foregroundStyle(Color.accentColor); Text(folder.title); Spacer(); Text("\(folderConversations(folder).count)").foregroundStyle(.secondary) }
                            }
                            .swipeActions { Button("删除", role: .destructive) { Task { await messaging.deleteFolder(folder.id) } } }
                        }
                    }
                }
                .navigationTitle("文件夹")
                .toolbar { ToolbarItem(placement: .topBarLeading) { Button("完成") { activeSection = nil } } }
                .sheet(isPresented: $folderEditorPresented) { folderEditorSheet }
            }
        } else if section == .contacts || section == .bots {
            NavigationStack {
                List {
                    let rows = section == .bots ? messaging.contacts.filter { $0.kind == "bot" || $0.kind == "assistant" } : messaging.contacts
                    if rows.isEmpty {
                        ContentUnavailableView(section.label, systemImage: section.symbol, description: Text("暂无可用联系人"))
                    } else {
                        ForEach(rows) { contact in
                            Button {
                                Task {
                                    do {
                                        if let conversation = try await messaging.createDirect(contact: contact) {
                                            activeSection = nil
                                            selectedConversation = conversation
                                        }
                                    } catch {
                                        model.message = "创建私聊失败：\(error.localizedDescription)"
                                    }
                                }
                            } label: {
                                HStack(spacing: 12) {
                                    ZStack {
                                        Circle().fill(Color.accentColor)
                                        Text(String(contact.displayName.prefix(1)).uppercased()).foregroundStyle(.white).fontWeight(.bold)
                                    }.frame(width: 42, height: 42)
                                    VStack(alignment: .leading) {
                                        Text(contact.displayName).foregroundStyle(.primary)
                                        Text(contact.username.map { "@\($0)" } ?? contact.kind).font(.caption).foregroundStyle(.secondary)
                                    }
                                }
                            }
                        }
                    }
                }
                .navigationTitle(section.label)
                .toolbar { ToolbarItem(placement: .topBarLeading) { Button("完成") { activeSection = nil } } }
            }
        } else if section == .calls {
            HumanCallsView(
                conversations: messaging.conversations,
                bridge: bridge,
                onClose: { activeSection = nil }
            )
        } else if section == .groups || section == .channels || section == .archive || section == .saved {
            NavigationStack {
                List {
                    let rows = messaging.conversations.filter { conversation in
                        switch section {
                        case .groups: conversation.kind == .group && !conversation.isArchived
                        case .channels: conversation.kind == .channel && !conversation.isArchived
                        case .archive: conversation.isArchived
                        case .saved: conversation.kind == .savedMessages
                        default: false
                        }
                    }
                    if rows.isEmpty { ContentUnavailableView(section.label, systemImage: section.symbol) }
                    ForEach(rows) { conversation in
                        conversationRow(conversation)
                            .swipeActions(edge: .trailing, allowsFullSwipe: true) {
                                if section == .archive {
                                    Button("恢复") { Task { await messaging.setArchived(conversation.id, archived: false) } }.tint(.blue)
                                }
                            }
                    }
                }
                .navigationTitle(section.label)
                .toolbar { ToolbarItem(placement: .topBarLeading) { Button("完成") { activeSection = nil } } }
            }
        } else {
            simpleSectionSheet(title: section.label, symbol: section.symbol)
        }
    }

    func folderConversations(_ folder: MessagingFolder) -> [ConversationSummary] {
        messaging.conversations.filter { conversation in
            guard !conversation.isArchived || !folder.excludeArchived else { return false }
            guard !conversation.isMuted || !folder.excludeMuted else { return false }
            guard conversation.unreadCount > 0 || !folder.excludeRead else { return false }
            return folder.conversationIds.contains(conversation.id) || (folder.includeGroups && conversation.kind == .group) || (folder.includeChannels && conversation.kind == .channel)
        }
    }

    var folderEditorSheet: some View {
        NavigationStack {
            Form {
                Section("名称") { TextField("文件夹名称", text: $folderTitle) }
                Section("自动包含") {
                    Toggle("群组", isOn: $folderIncludeGroups)
                    Toggle("频道", isOn: $folderIncludeChannels)
                }
                Section("选择会话") {
                    ForEach(messaging.conversations.filter { !$0.isArchived }) { conversation in
                        Button {
                            if folderConversationIds.contains(conversation.id) { folderConversationIds.remove(conversation.id) } else { folderConversationIds.insert(conversation.id) }
                        } label: {
                            HStack { Text(conversation.title).foregroundStyle(.primary); Spacer(); if folderConversationIds.contains(conversation.id) { Image(systemName: "checkmark.circle.fill").foregroundStyle(Color.accentColor) } }
                        }
                    }
                }
            }
            .navigationTitle("新建文件夹")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("取消") { folderEditorPresented = false } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("创建") {
                        let folder = MessagingFolder(
                            id: "folder-\(UUID().uuidString.lowercased())", title: folderTitle.trimmingCharacters(in: .whitespacesAndNewlines), icon: "folder",
                            conversationIds: Array(folderConversationIds), includeContacts: false, includeBots: false, includeGroups: folderIncludeGroups, includeChannels: folderIncludeChannels,
                            excludeMuted: false, excludeRead: false, excludeArchived: true
                        )
                        Task { await messaging.upsertFolder(folder) }
                        folderEditorPresented = false
                    }.disabled(folderTitle.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                }
            }
        }
    }

    @ViewBuilder
    func simpleSectionSheet(title: String, symbol: String) -> some View {
        NavigationStack {
            ContentUnavailableView(title, systemImage: symbol, description: Text("此入口与桌面端共用同一业务能力；移动端采用 Telegram 式单栈导航。"))
                .navigationTitle(title)
                .toolbar { ToolbarItem(placement: .topBarLeading) { Button("完成") { activeSection = nil; contactGroupsPresented = false } } }
        }
    }

    func conversationRow(_ conversation: ConversationSummary) -> some View {
        Button {
            selectedConversation = conversation
            Task { await messaging.markRead(conversation.id) }
        } label: {
            HStack(spacing: 12) {
                ZStack {
                    Circle().fill(Color.accentColor.opacity(0.85))
                    Text(conversation.badge.isEmpty ? "✦" : conversation.badge).font(.headline).foregroundStyle(.white)
                }.frame(width: 54, height: 54)
                VStack(alignment: .leading, spacing: 5) {
                    HStack(spacing: 5) {
                        Text(conversation.title).font(.system(size: 17, weight: .semibold)).foregroundStyle(.primary).lineLimit(1)
                        if conversation.isMuted { Image(systemName: "speaker.slash.fill").font(.caption2).foregroundStyle(.secondary) }
                        if conversation.isPinned { Image(systemName: "pin.fill").font(.caption2).foregroundStyle(.secondary) }
                    }
                    Text(conversation.preview).font(.system(size: 15)).foregroundStyle(.secondary).lineLimit(1)
                }
                Spacer(minLength: 8)
                VStack(alignment: .trailing, spacing: 6) {
                    Text(conversation.time).font(.caption).foregroundStyle(.secondary)
                    if conversation.unreadCount > 0 {
                        Text("\(conversation.unreadCount)").font(.caption2.bold()).foregroundStyle(.white)
                            .padding(.horizontal, 6).frame(minWidth: 20, minHeight: 20).background(Color.accentColor, in: Capsule())
                    } else if conversation.markedUnread {
                        Circle().fill(Color.accentColor).frame(width: 10, height: 10)
                    }
                }
            }
            .padding(.horizontal, 14).padding(.vertical, 8)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .contextMenu {
            Button(conversation.isPinned ? "取消置顶" : "置顶", systemImage: "pin") { Task { await messaging.setPinned(conversation.id, pinned: !conversation.isPinned) } }
            Button(conversation.isMuted ? "取消静音" : "静音", systemImage: "speaker.slash") { Task { await messaging.setMuted(conversation.id, muted: !conversation.isMuted) } }
            Button(conversation.markedUnread ? "取消标为未读" : "标为未读", systemImage: "circle.fill") { Task { await messaging.setMarkedUnread(conversation.id, markedUnread: !conversation.markedUnread) } }
            Button(conversation.isArchived ? "恢复" : "归档", systemImage: "archivebox") { Task { await messaging.setArchived(conversation.id, archived: !conversation.isArchived) } }
        }
        .accessibilityIdentifier("conversation-\(conversation.id)")
    }
}
