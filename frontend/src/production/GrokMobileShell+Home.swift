import SwiftUI
import UIKit

internal func mobileBotHomeSubtitle(_ bot: MobileBotSummary) -> String {
    if bot.isComposingMessage { return "正在输入…" }
    if let waitingReason = bot.waitingReason?.trimmingCharacters(in: .whitespacesAndNewlines),
       !waitingReason.isEmpty
    {
        return waitingReason
    }
    if let preview = bot.lastMessagePreview?.trimmingCharacters(in: .whitespacesAndNewlines),
       !preview.isEmpty
    {
        return preview
    }
    if bot.isRunning { return "正在运行…" }
    return bot.description.isEmpty ? "Ready" : bot.description
}

extension GrokMobileShell {
    var home: some View {
        ZStack {
            Color(red: 0.985, green: 0.985, blue: 0.975).ignoresSafeArea()
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    HStack {
                        Button { legacyOpen = true } label: {
                            ZStack {
                                Circle().fill(Color(red: 1.0, green: 0.78, blue: 0.82))
                                Text(String(model.accountName.prefix(1)).uppercased()).font(.system(size: 14, weight: .bold)).foregroundStyle(.black)
                            }
                            .frame(width: 38, height: 38)
                            .overlay(Circle().stroke(.white, lineWidth: 3)).shadow(color: .black.opacity(0.08), radius: 6)
                        }
                        .accessibilityIdentifier("grok-mobile-legacy")
                        Spacer()
                        Button { toggleCommandPalette() } label: { Image(systemName: "magnifyingglass") }
                            .accessibilityIdentifier("grok-mobile-search")
                        Button { composeOpen = true } label: { Image(systemName: "plus") }
                            .accessibilityIdentifier("grok-mobile-add")
                    }
                    .font(.system(size: 19, weight: .semibold)).foregroundStyle(.black)
                    .buttonStyle(.plain)
                    .padding(.horizontal, 18).padding(.top, 10)

                    VStack(spacing: 7) {
                        ZStack {
                            ClothGhostAvatar(botId: "all-hands-green", size: 50).offset(x: -25, y: 7).rotationEffect(.degrees(-8))
                            ClothGhostAvatar(botId: "all-hands-violet", size: 50).offset(x: -1, y: 17).rotationEffect(.degrees(7))
                            ClothGhostAvatar(botId: "mahayana-assistant", size: 55).offset(x: 24, y: -1)
                            Text("+2").font(.system(size: 29, weight: .bold)).foregroundStyle(Color.black.opacity(0.34)).offset(x: 48, y: 28)
                        }.frame(width: 130, height: 82)
                        Text("All Hands").font(.system(size: 14)).foregroundStyle(.secondary)
                    }
                    .frame(maxWidth: .infinity).padding(.top, 38).padding(.bottom, 34)

                    if searchOpen {
                        commandPaletteContent
                    }

                    if !searchOpen {
                        FabushiProductHub(
                            onOpenGlobalDharma: {
                                legacySection = .miniapps
                                legacyOpen = true
                            },
                            onOpenAI: { prompt in
                                if let prompt, !prompt.isEmpty {
                                    botDrafts["mahayana-assistant"] = prompt
                                }
                                selectBotForConversation(
                                    bots.first(where: { $0.id == "mahayana-assistant" })
                                        ?? MobileBotSummary(
                                            id: "mahayana-assistant",
                                            name: "Mahayana",
                                            description: "Ready to help"
                                        )
                                )
                            }
                        )
                        .padding(.bottom, 12)
                    }

                    if let botActionError {
                        Text(botActionError)
                            .font(.footnote)
                            .foregroundStyle(.red)
                            .padding(.horizontal, 18)
                            .padding(.bottom, 8)
                            .accessibilityIdentifier("grok-mobile-bot-action-error")
                    }

                    if !searchOpen {
                        sectionTitle("Board")
                        botRow(MobileBotSummary(id: "mahayana-assistant", name: "Mahayana", description: "Ready to help"), subtitle: "that's the only new one.", badge: "Board")

                        if !bots.isEmpty {
                            if agentSidebarSections.isEmpty {
                                sectionTitle("Bots  \(filteredBots.count)")
                                ForEach(filteredBots) { bot in
                                    botRow(
                                        bot,
                                        subtitle: mobileBotHomeSubtitle(bot),
                                        badge: bot.isGroup ? "Group" : (bot.miniAppId == nil ? "Bot" : "Mini App Bot")
                                    )
                                }
                            } else {
                                ForEach(agentSidebarSections) { section in
                                    let sectionBots = filteredBots.filter { section.agentIds.contains($0.id) }
                                    if !sectionBots.isEmpty {
                                        agentSidebarSectionHeader(
                                            section,
                                            count: sectionBots.count
                                        )
                                        ForEach(sectionBots) { bot in
                                            botRow(
                                                bot,
                                                subtitle: mobileBotHomeSubtitle(bot),
                                                badge: bot.isGroup ? "Group" : (bot.miniAppId == nil ? "Bot" : "Mini App Bot")
                                            )
                                        }
                                    }
                                }
                                let assignedIds = Set(agentSidebarSections.flatMap(\.agentIds))
                                let unassignedBots = filteredBots.filter { !assignedIds.contains($0.id) }
                                if !unassignedBots.isEmpty {
                                    sectionTitle("未分组  \(unassignedBots.count)")
                                    ForEach(unassignedBots) { bot in
                                        botRow(
                                            bot,
                                            subtitle: mobileBotHomeSubtitle(bot),
                                            badge: bot.isGroup ? "Group" : (bot.miniAppId == nil ? "Bot" : "Mini App Bot")
                                        )
                                    }
                                }
                            }

                            let hiddenBots = bots.filter { $0.hidden }
                            if !hiddenBots.isEmpty {
                                Menu {
                                    ForEach(hiddenBots) { bot in
                                        Button {
                                            Task { await setBotHidden(bot, hidden: false) }
                                        } label: {
                                            Label("恢复 \(bot.name)", systemImage: "eye")
                                        }
                                    }
                                } label: {
                                    Label(
                                        "隐藏的 Bots  \(hiddenBots.count)",
                                        systemImage: "eye.slash"
                                    )
                                    .font(.subheadline.weight(.semibold))
                                    .padding(.horizontal, 18)
                                    .padding(.vertical, 10)
                                }
                                .buttonStyle(.plain)
                                .disabled(botActionBusy)
                                .accessibilityIdentifier("grok-hidden-bots")
                            }
                        }

                        let projects = filteredConversations.filter { $0.kind == .group || $0.kind == .direct }
                        if !projects.isEmpty {
                            sectionTitle("Projects  \(projects.count)")
                            ForEach(projects.prefix(8)) { conversation in conversationRow(conversation) }
                        }
                        let channels = filteredConversations.filter { $0.kind == .channel }
                        if !channels.isEmpty {
                            sectionTitle("Channels  \(channels.count)")
                            ForEach(channels.prefix(8)) { conversation in conversationRow(conversation) }
                        }
                    }
                    Spacer(minLength: 40)
                }
            }

            AccessCoverView(
                access: accessCoverComposition.access,
                isVisible: accessCoverComposition.isVisible,
                onOpenAccess: {
                    _ = openExternalURL(ACCESS_ONBOARDING_URL)
                }
            )
        }
        .confirmationDialog("Create", isPresented: $composeOpen, titleVisibility: .visible) {
            Button("New Bot") {
                botName = "New chat"
                botDescription = ""
                botAvatarShape = "wedge"
                botAvatarColor = "cyan"
                botError = nil
                createBotOpen = true
            }
            Button("New message") { legacyOpen = true }
            Button("New group") { legacyOpen = true }
            Button("New channel") { legacyOpen = true }
            Button("Cancel", role: .cancel) { }
        }
        .sheet(isPresented: $createBotOpen) { createBotSheet }
        .sheet(item: $botRenameTarget) { bot in renameBotSheet(bot) }
        .sheet(item: $botDeleteTarget) { bot in botDeleteConfirmationSheet(bot) }
        .accessibilityIdentifier("grok-mobile-home")
    }

    var filteredBots: [MobileBotSummary] {
        let visible = bots.filter { !$0.hidden }
        let filtered = query.isEmpty
            ? visible
            : visible.filter {
                $0.name.localizedCaseInsensitiveContains(query)
                    || $0.description.localizedCaseInsensitiveContains(query)
            }
        let pinned = pinnedBotIds
        let pinnedRank = Dictionary(
            uniqueKeysWithValues: pinnedBotIdOrder.enumerated().map { ($0.element, $0.offset) }
        )
        return filtered.sorted {
            let lhsPinned = pinned.contains($0.id)
            let rhsPinned = pinned.contains($1.id)
            if lhsPinned != rhsPinned { return lhsPinned }
            if lhsPinned, rhsPinned {
                let lhsRank = pinnedRank[$0.id] ?? Int.max
                let rhsRank = pinnedRank[$1.id] ?? Int.max
                if lhsRank != rhsRank { return lhsRank < rhsRank }
            }
            let lhsUpdated = $0.updatedAtMs ?? 0
            let rhsUpdated = $1.updatedAtMs ?? 0
            if lhsUpdated != rhsUpdated { return lhsUpdated > rhsUpdated }
            return $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending
        }
    }

    var filteredConversations: [ConversationSummary] {
        let rows = messaging.conversations.filter { !$0.isArchived }
        guard !query.isEmpty else { return rows }
        return rows.filter { $0.title.localizedCaseInsensitiveContains(query) || $0.preview.localizedCaseInsensitiveContains(query) }
    }

    func sectionTitle(_ text: String) -> some View {
        Text(text).font(.system(size: 16)).foregroundStyle(Color.black.opacity(0.42)).padding(.horizontal, 18).padding(.top, 13).padding(.bottom, 7)
    }

    func agentSidebarSectionHeader(
        _ section: MobileAgentSidebarSection,
        count: Int
    ) -> some View {
        HStack(spacing: 8) {
            Text("\(section.name)  \(count)")
                .font(.system(size: 16))
                .foregroundStyle(Color.black.opacity(0.42))
            Spacer()
            Menu {
                Button {
                    sectionRenameDraft = section.name
                    sectionRenameTarget = section
                } label: {
                    Label("重命名分组", systemImage: "pencil")
                }
                Button {
                    Task { await moveAgentSidebarSection(section, offset: -1) }
                } label: {
                    Label("上移分组", systemImage: "arrow.up")
                }
                .disabled(agentSidebarSections.first?.id == section.id)
                Button {
                    Task { await moveAgentSidebarSection(section, offset: 1) }
                } label: {
                    Label("下移分组", systemImage: "arrow.down")
                }
                .disabled(agentSidebarSections.last?.id == section.id)
                Divider()
                Button(role: .destructive) {
                    sectionDeleteTarget = section
                } label: {
                    Label("删除分组", systemImage: "trash")
                }
            } label: {
                Image(systemName: "ellipsis.circle")
                    .foregroundStyle(.secondary)
                    .frame(width: 32, height: 32)
            }
            .accessibilityIdentifier("agent-section-actions-\(section.id)")
        }
        .padding(.leading, 18)
        .padding(.trailing, 10)
        .padding(.top, 13)
        .padding(.bottom, 7)
    }

    func botRow(_ bot: MobileBotSummary, subtitle: String, badge: String) -> some View {
        HStack(spacing: 0) {
            Button {
                if bot.isGroup && !bot.isSharedRoom {
                    groupMembersTarget = bot
                } else {
                    selectBotForConversation(bot)
                }
            } label: {
                HStack(spacing: 12) {
                    ClothGhostAvatar(botId: bot.id, size: 47, badge: .green)
                    VStack(alignment: .leading, spacing: 3) {
                        HStack(spacing: 7) {
                            Text(bot.name).font(.system(size: 17, weight: .semibold)).foregroundStyle(.black)
                            Text(badge).font(.caption).foregroundStyle(.secondary).padding(.horizontal, 7).padding(.vertical, 3).background(Color.black.opacity(0.045), in: Capsule())
                        }
                        Text(subtitle)
                            .font(.system(size: 14))
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                    }
                    Spacer()
                    if bot.unread {
                        Circle()
                            .fill(Color.accentColor)
                            .frame(width: 8, height: 8)
                            .accessibilityLabel("未读")
                    }
                    if let updatedAtMs = bot.updatedAtMs, updatedAtMs > 0 {
                        Text(Date(timeIntervalSince1970: Double(updatedAtMs) / 1000), style: .relative)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                    }
                }
                .padding(.leading, 18).padding(.vertical, 9)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier(
                bot.miniAppId.map { "grok-mobile-miniapp-bot-\($0)" }
                    ?? "grok-mobile-bot-\(bot.id)"
            )

            if bot.miniAppId == nil {
                Menu {
                    Button {
                        botSettingsTarget = bot
                    } label: {
                        Label("设置", systemImage: "gearshape")
                    }
                    if MobileAgentSidebarSections.canAssign(
                        isPinned: pinnedBotIds.contains(bot.id),
                        isHidden: bot.hidden
                    ) {
                        Menu {
                            Button {
                                Task { await assignBot(bot, toSection: nil) }
                            } label: {
                                Label("未分组", systemImage: "tray")
                            }
                            ForEach(agentSidebarSections) { section in
                                Button {
                                    Task { await assignBot(bot, toSection: section.id) }
                                } label: {
                                    Label(section.name, systemImage: "folder")
                                }
                            }
                            Divider()
                            Button {
                                beginCreateAgentSidebarSection(for: bot)
                            } label: {
                                Label("新建分组…", systemImage: "folder.badge.plus")
                            }
                        } label: {
                            Label("移到分组", systemImage: "folder")
                        }
                    }
                    if let pinnedIndex = pinnedBotIdOrder.firstIndex(of: bot.id) {
                        Button {
                            Task { await movePinnedBot(bot, offset: -1) }
                        } label: {
                            Label("上移置顶", systemImage: "arrow.up")
                        }
                        .disabled(pinnedIndex == 0)

                        Button {
                            Task { await movePinnedBot(bot, offset: 1) }
                        } label: {
                            Label("下移置顶", systemImage: "arrow.down")
                        }
                        .disabled(pinnedIndex == pinnedBotIdOrder.count - 1)
                    }
                    Button {
                        Task { await toggleBotPin(bot) }
                    } label: {
                        Label(
                            pinnedBotIds.contains(bot.id) ? "取消置顶" : "置顶",
                            systemImage: pinnedBotIds.contains(bot.id) ? "pin.slash" : "pin"
                        )
                    }
                    if bot.isGroup {
                        Button {
                            groupMembersTarget = bot
                        } label: {
                            Label("成员", systemImage: "person.2")
                        }
                        Divider()
                        Button(role: .destructive) {
                            requestBotDelete(bot)
                        } label: {
                            Label("删除群组", systemImage: "trash")
                        }
                    } else {
                        Button {
                            beginBotRename(bot)
                        } label: {
                            Label("重命名", systemImage: "pencil")
                        }
                        if let conversationId = bot.conversationId, !conversationId.isEmpty {
                            Button {
                                openLegacyConversation(conversationId)
                            } label: {
                                Label("显示完整会话", systemImage: "list.bullet.rectangle")
                            }
                            Button {
                                UIPasteboard.general.string = conversationId
                            } label: {
                                Label("复制会话 ID", systemImage: "doc.on.doc")
                            }
                        }
                        Button {
                            showAsyncTasks(bot)
                        } label: {
                            Label("异步任务", systemImage: "clock")
                        }
                        Button {
                            Task { await setBotUnread(bot, unread: !bot.unread) }
                        } label: {
                            Label(
                                bot.unread ? "标记已读" : "标记未读",
                                systemImage: bot.unread ? "envelope.open" : "envelope.badge"
                            )
                        }
                        Button {
                            Task { await duplicateBot(bot) }
                        } label: {
                            Label("复制", systemImage: "square.on.square")
                        }
                        Button {
                            Task { await setBotHidden(bot, hidden: true) }
                        } label: {
                            Label("从首页隐藏", systemImage: "eye.slash")
                        }
                        Divider()
                        Button(role: .destructive) {
                            requestBotDelete(bot)
                        } label: {
                            Label("删除", systemImage: "trash")
                        }
                    }
                } label: {
                    Image(systemName: "ellipsis")
                        .font(.system(size: 16, weight: .semibold))
                        .frame(width: 44, height: 44)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .disabled(botActionBusy)
                .accessibilityIdentifier("grok-bot-actions-\(bot.id)")
                .padding(.trailing, 6)
            }
        }
    }

    func conversationRow(_ conversation: ConversationSummary) -> some View {
        Button { openLegacyConversation(conversation.id) } label: {
            HStack(spacing: 12) {
                ClothGhostAvatar(botId: "conversation:\(conversation.id)", size: 45, badge: conversation.unreadCount > 0 ? .blue : nil)
                VStack(alignment: .leading, spacing: 3) {
                    HStack(spacing: 7) {
                        Text(conversation.title).font(.system(size: 17, weight: .semibold)).foregroundStyle(.black).lineLimit(1)
                        Text(conversation.kind == .channel ? "Channel" : "Engineering").font(.caption).foregroundStyle(.secondary).padding(.horizontal, 7).padding(.vertical, 3).background(Color.black.opacity(0.045), in: Capsule())
                    }
                    Text(conversation.preview.isEmpty ? "Ready" : conversation.preview).font(.system(size: 14)).foregroundStyle(.secondary).lineLimit(1)
                }
                Spacer()
                Text(conversation.time).font(.caption).foregroundStyle(.secondary)
            }
            .padding(.horizontal, 18).padding(.vertical, 9)
        }.buttonStyle(.plain)
    }

    func renameBotSheet(_ bot: MobileBotSummary) -> some View {
        NavigationStack {
            Form {
                Section("名称") {
                    TextField("Bot 名称", text: $botRenameDraft)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .accessibilityIdentifier("rename-bot-name")
                }
                if let botActionError {
                    Section {
                        Text(botActionError)
                            .foregroundStyle(.red)
                            .font(.footnote)
                            .accessibilityIdentifier("rename-bot-error")
                    }
                }
            }
            .navigationTitle("重命名 Bot")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("取消") {
                        botRenameTarget = nil
                        botRenameDraft = ""
                        botActionError = nil
                    }
                    .disabled(botActionBusy)
                    .accessibilityIdentifier("rename-bot-cancel")
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button(botActionBusy ? "保存中…" : "保存") {
                        Task { await commitBotRename() }
                    }
                    .disabled(
                        botActionBusy
                            || committedMobileBotName(
                                initialValue: bot.name,
                                draftValue: botRenameDraft
                            ) == nil
                    )
                    .accessibilityIdentifier("rename-bot-submit")
                }
            }
        }
    }

    func botDeleteConfirmationSheet(_ bot: MobileBotSummary) -> some View {
        NavigationStack {
            VStack(alignment: .leading, spacing: 18) {
                Text(bot.isGroup ? "永久删除群组？" : "永久删除 Bot？")
                    .font(.title2.weight(.semibold))
                Text("“\(bot.name)”")
                    .font(.headline)
                Text(mobileBotDeleteDescription(bot))
                    .foregroundStyle(.secondary)

                if let botActionError {
                    Text(botActionError)
                        .foregroundStyle(.red)
                        .font(.footnote)
                        .accessibilityIdentifier("delete-bot-error")
                }

                Spacer()

                HStack {
                    Button("取消") {
                        guard !botActionBusy else { return }
                        botDeleteTarget = nil
                        botActionError = nil
                    }
                    .buttonStyle(.bordered)
                    .disabled(botActionBusy)
                    .accessibilityIdentifier("delete-bot-cancel")

                    Spacer()

                    Button(role: .destructive) {
                        Task { await deleteBot(bot) }
                    } label: {
                        Text(botActionBusy ? "删除中…" : "删除")
                    }
                    .buttonStyle(.borderedProminent)
                    .disabled(botActionBusy)
                    .accessibilityIdentifier("delete-bot-confirm")
                }
            }
            .padding(24)
            .navigationTitle("删除确认")
            .navigationBarTitleDisplayMode(.inline)
        }
        .interactiveDismissDisabled(botActionBusy)
        .presentationDetents([.medium])
        .accessibilityIdentifier("delete-bot-confirmation")
    }

    var createBotSheet: some View {
        NavigationStack {
            Form {
                Section {
                    HStack { Spacer(); ClothGhostAvatar(botId: botName.isEmpty ? "new-bot" : botName, size: 82, active: botBusy); Spacer() }
                }
                Section("Name") {
                    TextField("Bot name", text: $botName)
                        .accessibilityIdentifier("new-bot-name")
                }
                Section("Description") {
                    TextField("What does this Bot do?", text: $botDescription, axis: .vertical)
                        .lineLimit(2...5)
                        .accessibilityIdentifier("new-bot-description")
                }
                Section("Avatar shape") {
                    LazyVGrid(columns: Array(repeating: GridItem(.flexible()), count: 4), spacing: 10) {
                        ForEach(AvatarImagePolicy.shapes, id: \.self) { shape in
                            Button {
                                botAvatarShape = shape
                            } label: {
                                Text(shape.capitalized)
                                    .font(.caption.weight(botAvatarShape == shape ? .bold : .regular))
                                    .frame(maxWidth: .infinity, minHeight: 36)
                                    .background(
                                        botAvatarShape == shape
                                            ? Color.accentColor.opacity(0.2)
                                            : Color.secondary.opacity(0.10),
                                        in: RoundedRectangle(cornerRadius: 9)
                                    )
                            }
                            .buttonStyle(.plain)
                            .accessibilityLabel("Avatar shape \(shape)")
                            .accessibilityValue(botAvatarShape == shape ? "Selected" : "Not selected")
                            .accessibilityIdentifier("new-bot-avatar-shape-\(shape)")
                        }
                    }
                }
                Section("Avatar color") {
                    LazyVGrid(columns: Array(repeating: GridItem(.flexible()), count: 3), spacing: 10) {
                        ForEach(AvatarImagePolicy.colors, id: \.id) { color in
                            Button {
                                botAvatarColor = color.id
                            } label: {
                                Text(color.label)
                                    .font(.caption.weight(botAvatarColor == color.id ? .bold : .regular))
                                    .frame(maxWidth: .infinity, minHeight: 36)
                                    .background(
                                        botAvatarColor == color.id
                                            ? Color.accentColor.opacity(0.2)
                                            : Color.secondary.opacity(0.10),
                                        in: RoundedRectangle(cornerRadius: 9)
                                    )
                            }
                            .buttonStyle(.plain)
                            .accessibilityLabel("Avatar color \(color.label)")
                            .accessibilityValue(botAvatarColor == color.id ? "Selected" : "Not selected")
                            .accessibilityIdentifier("new-bot-avatar-color-\(color.id)")
                        }
                    }
                }
                if let botError { Section { Text(botError).foregroundStyle(.red).font(.footnote) } }
            }
            .navigationTitle("New Bot")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") {
                        createBotOpen = false
                        botError = nil
                    }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button(botBusy ? "Creating…" : "Create") { Task { await createBot() } }
                        .disabled(botBusy || botName.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                        .accessibilityIdentifier("create-bot-submit")
                }
            }
        }
    }
}
