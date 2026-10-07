import SwiftUI

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
                                selectedBot = bots.first(where: { $0.id == "mahayana-assistant" })
                                    ?? MobileBotSummary(
                                        id: "mahayana-assistant",
                                        name: "Mahayana",
                                        description: "Ready to help"
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
                            sectionTitle("Bots  \(bots.count)")
                            ForEach(filteredBots) { bot in
                                botRow(
                                    bot,
                                    subtitle: bot.description.isEmpty ? "Ready" : bot.description,
                                    badge: bot.isGroup ? "Group" : (bot.miniAppId == nil ? "Bot" : "Mini App Bot")
                                )
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
        }
        .confirmationDialog("Create", isPresented: $composeOpen, titleVisibility: .visible) {
            Button("New Bot") { createBotOpen = true }
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
        guard !query.isEmpty else { return bots }
        return bots.filter { $0.name.localizedCaseInsensitiveContains(query) || $0.description.localizedCaseInsensitiveContains(query) }
    }

    var filteredConversations: [ConversationSummary] {
        let rows = messaging.conversations.filter { !$0.isArchived }
        guard !query.isEmpty else { return rows }
        return rows.filter { $0.title.localizedCaseInsensitiveContains(query) || $0.preview.localizedCaseInsensitiveContains(query) }
    }

    func sectionTitle(_ text: String) -> some View {
        Text(text).font(.system(size: 16)).foregroundStyle(Color.black.opacity(0.42)).padding(.horizontal, 18).padding(.top, 13).padding(.bottom, 7)
    }

    func botRow(_ bot: MobileBotSummary, subtitle: String, badge: String) -> some View {
        HStack(spacing: 0) {
            Button {
                if bot.isGroup && !bot.isSharedRoom {
                    groupMembersTarget = bot
                } else {
                    selectedBot = bot
                }
            } label: {
                HStack(spacing: 12) {
                    ClothGhostAvatar(botId: bot.id, size: 47, badge: .green)
                    VStack(alignment: .leading, spacing: 3) {
                        HStack(spacing: 7) {
                            Text(bot.name).font(.system(size: 17, weight: .semibold)).foregroundStyle(.black)
                            Text(badge).font(.caption).foregroundStyle(.secondary).padding(.horizontal, 7).padding(.vertical, 3).background(Color.black.opacity(0.045), in: Capsule())
                        }
                        Text(subtitle).font(.system(size: 14)).foregroundStyle(.secondary).lineLimit(1)
                    }
                    Spacer()
                    Text("now").font(.caption).foregroundStyle(.secondary)
                }
                .padding(.leading, 18).padding(.vertical, 9)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)

            if bot.miniAppId == nil {
                Menu {
                    Button {
                        botSettingsTarget = bot
                    } label: {
                        Label("设置", systemImage: "gearshape")
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
                        Button {
                            Task { await duplicateBot(bot) }
                        } label: {
                            Label("复制", systemImage: "doc.on.doc")
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
                Text(
                    bot.isGroup
                        ? "这会永久删除该群组及其聊天记录。群组中的 Bots 不会被删除，仍可单独使用。此操作无法撤销。"
                        : "这会永久删除该 Bot 及其聊天记录。此操作无法撤销。"
                )
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
                Section("Name") { TextField("Bot name", text: $botName).accessibilityIdentifier("new-bot-name") }
                Section("Description") { TextField("What does this Bot do?", text: $botDescription, axis: .vertical).lineLimit(2...5) }
                if let botError { Section { Text(botError).foregroundStyle(.red).font(.footnote) } }
            }
            .navigationTitle("New Bot")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { createBotOpen = false } }
                ToolbarItem(placement: .confirmationAction) {
                    Button(botBusy ? "Creating…" : "Create") { Task { await createBot() } }
                        .disabled(botBusy || botName.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                        .accessibilityIdentifier("create-bot-submit")
                }
            }
        }
    }
}
