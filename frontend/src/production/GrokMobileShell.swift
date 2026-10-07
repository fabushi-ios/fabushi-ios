import SwiftUI

internal struct GrokMobileShell: View {
    @Bindable var model: MarketplaceModel
    @Bindable var messaging: MessagingModel
    let bridge: IOSPreloadBridge
    let appAgentSurface: FabushiAppAgentSurface
    var reconnectGeneration: Int = 0
    @Environment(\.openURL) var openExternalURL

    @State var query = ""
    @State var paletteTab: MobileCommandPaletteTab = .all
    @State var searchOpen = false
    @State var composeOpen = false
    @State var createBotOpen = false
    @State var botName = ""
    @State var botDescription = ""
    @State var botBusy = false
    @State var botError: String?
    @State var botRenameTarget: MobileBotSummary?
    @State var botRenameDraft = ""
    @State var botDeleteTarget: MobileBotSummary?
    @State var botActionBusy = false
    @State var botActionError: String?
    @State var bots: [MobileBotSummary] = []
    @State var accessRosterSnapshot = AccessRosterSnapshot.initial
    @State var accessCoverFirstBox = FirstBoxGateState.initial
    @State var accessCoverAccess = AccessCoverSandAccess.checking
    @State var accessRosterGeneration = 0
    @State var pinnedBotIdOrder: [String] = []
    @State var asyncTasksTarget: MobileBotSummary?
    @State var asyncTasks: [MobileAgentAsyncTask] = []
    @State var asyncTasksBusy = false
    @State var asyncTasksError: String?
    @State var agentSidebarSections: [MobileAgentSidebarSection] = []
    @State var newSectionBot: MobileBotSummary?
    @State var newSectionName = ""
    @State var sectionRenameTarget: MobileAgentSidebarSection?
    @State var sectionRenameDraft = ""
    @State var sectionDeleteTarget: MobileAgentSidebarSection?
    @State var selectedBot: MobileBotSummary?
    @State var groupMembersTarget: MobileBotSummary?
    @State var botSettingsTarget: MobileBotSummary?
    @State var botSettingsRoutineID: String?
    @State var remoteComputerAgentTarget: MobileBotSummary?
    @State var botDrafts: [String: String] = [:]
    @State var botTranscripts: [String: [MobileChatMessage]] = [:]
    @State var legacyOpen = false
    @State var legacyConversationID: String?
    @State var legacyMessageID: String?
    @State var legacySection: MobileSection?
    @State var commandPaletteRoutines: [MobileCommandPaletteRoutine] = []
    @State var commandPaletteRoutineStatus: MobileCommandPaletteProviderStatus = .idle
    @State var commandPaletteLinkMetadata: [String: MobileCommandPaletteLinkMetadata] = [:]
    @State var commandPaletteLinkStatus: MobileCommandPaletteProviderStatus = .idle

    @ViewBuilder
    var body: some View {
        Group {
            if model.onboardingStep < 3 || !model.authResolved || !model.loggedIn {
                unauthenticatedContent
            } else if let selectedBot {
                selectedBotContent(selectedBot)
            } else if legacyOpen {
                legacyContent
            } else {
                homeContent
            }
        }
        .sheet(item: $groupMembersTarget) { group in
            MobileBotGroupMembersSheet(
                group: bots.first(where: { $0.id == group.id }) ?? group,
                roster: bots,
                bridge: bridge,
                accountScopeKey: mobileAccountScopeKey,
                onRosterChanged: { updated in
                    applyBotRosterUpdate(updated)
                    groupMembersTarget = updated.first(where: { $0.id == group.id })
                },
                onClose: { groupMembersTarget = nil }
            )
        }
        .sheet(item: $newSectionBot) { bot in
            NavigationStack {
                Form {
                    Section("分组名称") {
                        TextField("新分组", text: $newSectionName)
                            .accessibilityIdentifier("agent-new-section-name")
                    }
                }
                .navigationTitle("移动 \(bot.name)")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) {
                        Button("取消") {
                            newSectionBot = nil
                            newSectionName = ""
                        }
                    }
                    ToolbarItem(placement: .confirmationAction) {
                        Button("创建并移动") {
                            Task { await createAgentSidebarSection(for: bot) }
                        }
                        .disabled(newSectionName.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                        .accessibilityIdentifier("agent-new-section-submit")
                    }
                }
            }
            .accessibilityIdentifier("agent-new-section")
        }
        .sheet(item: $sectionRenameTarget) { section in
            NavigationStack {
                Form {
                    Section("分组名称") {
                        TextField("分组名称", text: $sectionRenameDraft)
                            .accessibilityIdentifier("agent-section-rename-name")
                    }
                }
                .navigationTitle("重命名分组")
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) {
                        Button("取消") {
                            sectionRenameTarget = nil
                            sectionRenameDraft = ""
                        }
                    }
                    ToolbarItem(placement: .confirmationAction) {
                        Button("保存") {
                            Task { await commitAgentSidebarSectionRename(section) }
                        }
                        .disabled(sectionRenameDraft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                    }
                }
            }
            .accessibilityIdentifier("agent-section-rename")
        }
        .confirmationDialog(
            sectionDeleteTarget.map { "删除“\($0.name)”" } ?? "删除分组",
            isPresented: Binding(
                get: { sectionDeleteTarget != nil },
                set: { if !$0 { sectionDeleteTarget = nil } }
            ),
            titleVisibility: .visible
        ) {
            if let section = sectionDeleteTarget {
                Button("删除", role: .destructive) {
                    Task { await deleteAgentSidebarSection(section) }
                }
            }
            Button("取消", role: .cancel) { sectionDeleteTarget = nil }
        } message: {
            Text("其中的 Bots 会移到“未分组”，不会删除任何 Bot。")
        }
        .sheet(item: $asyncTasksTarget) { agent in
            NavigationStack {
                List {
                    if asyncTasksBusy {
                        ProgressView("正在加载异步任务…")
                    } else if let asyncTasksError {
                        Text(asyncTasksError)
                            .foregroundStyle(.red)
                            .accessibilityIdentifier("agent-async-tasks-error")
                    } else if asyncTasks.isEmpty {
                        Text("当前没有运行中的异步任务")
                            .foregroundStyle(.secondary)
                    } else {
                        ForEach(asyncTasks) { task in
                            VStack(alignment: .leading, spacing: 4) {
                                HStack {
                                    Text(task.label).font(.headline)
                                    Spacer()
                                    Text(task.kind).font(.caption).foregroundStyle(.secondary)
                                }
                                if let detail = task.detail, !detail.isEmpty {
                                    Text(detail).font(.subheadline).foregroundStyle(.secondary)
                                }
                                if let resourceId = task.resourceId, !resourceId.isEmpty {
                                    Text(resourceId).font(.caption2).foregroundStyle(.tertiary)
                                }
                            }
                        }
                    }
                }
                .navigationTitle("\(agent.name) · 异步任务")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) {
                        Button("关闭") { asyncTasksTarget = nil }
                    }
                }
            }
            .accessibilityIdentifier("agent-async-tasks")
        }
        .sheet(item: $botSettingsTarget) { agent in
            MobileBotAgentSettingsSheet(
                agent: bots.first(where: { $0.id == agent.id }) ?? agent,
                roster: bots,
                bridge: bridge,
                marketplaceModel: model,
                accountScopeKey: mobileAccountScopeKey,
                reconnectGeneration: reconnectGeneration,
                focusedAutomationId: botSettingsRoutineID,
                onRosterChanged: { updated in
                    applyBotRosterUpdate(updated)
                    botSettingsTarget = updated.first(where: { $0.id == agent.id })
                },
                onOpenComputer: { scopedAgent in
                    botSettingsTarget = nil
                    remoteComputerAgentTarget =
                        bots.first(where: { $0.id == scopedAgent.id }) ?? scopedAgent
                },
                onClose: {
                    botSettingsRoutineID = nil
                    botSettingsTarget = nil
                }
            )
        }
        .fullScreenCover(item: $remoteComputerAgentTarget) { agent in
            RemoteComputerSurface(
                bridge: bridge,
                scope: .init(
                    accountScopeKey: mobileAccountScopeKey,
                    agentID: agent.id,
                    agentName: agent.name
                ),
                reconnectGeneration: reconnectGeneration
            ) {
                remoteComputerAgentTarget = nil
            }
        }
    }

    var mobileAccountScopeKey: String {
        [
            String(model.loggedIn),
            model.accountEmail,
            model.accountName,
        ].joined(separator: ":")
    }

    var accessRosterTaskKey: String {
        "\(mobileAccountScopeKey):\(reconnectGeneration)"
    }

    var accessCoverComposition: AccessCoverCompositionState {
        AccessCoverComposition.project(
            access: accessCoverAccess,
            roster: accessRosterSnapshot,
            firstBox: accessCoverFirstBox,
            isComputerRebuildLocked: remoteComputerAgentTarget != nil
        )
    }

    @MainActor
    func applyBotRosterUpdate(_ updated: [MobileBotSummary]) {
        bots = updated
        if let selectedBot,
           let refreshed = updated.first(where: { $0.id == selectedBot.id }) {
            self.selectedBot = refreshed
        }
        if let groupMembersTarget,
           let refreshed = updated.first(where: { $0.id == groupMembersTarget.id }) {
            self.groupMembersTarget = refreshed
        }
        if let remoteComputerAgentTarget,
           let refreshed = updated.first(where: { $0.id == remoteComputerAgentTarget.id }) {
            self.remoteComputerAgentTarget = refreshed
        }
    }

    private var unauthenticatedContent: some View {
        ContentView(
            model: model,
            messaging: messaging,
            appAgentSurface: appAgentSurface,
            bridge: bridge,
            reconnectGeneration: reconnectGeneration
        )
    }

    private func selectedBotContent(_ bot: MobileBotSummary) -> some View {
        MobileBotChat(
            bot: bot,
            bridge: bridge,
            model: model,
            appAgentSurface: appAgentSurface,
            onClose: { self.selectedBot = nil },
            onOpenSettings: {
                self.botSettingsRoutineID = nil
                self.botSettingsTarget = self.bots.first(where: { $0.id == bot.id }) ?? bot
            },
            onOpenAutomation: { automationId in
                self.botSettingsRoutineID = automationId
                self.botSettingsTarget = self.bots.first(where: { $0.id == bot.id }) ?? bot
            },
            draft: botDraftBinding(for: bot.id),
            entries: botTranscriptBinding(for: bot.id)
        )
    }

    private func botDraftBinding(for botID: String) -> Binding<String> {
        Binding(
            get: { botDrafts[botID] ?? "" },
            set: { botDrafts[botID] = $0 }
        )
    }

    private func botTranscriptBinding(for botID: String) -> Binding<[MobileChatMessage]> {
        Binding(
            get: { botTranscripts[botID] ?? [] },
            set: { botTranscripts[botID] = $0 }
        )
    }

    private var legacyContent: some View {
        VStack(spacing: 0) {
            HStack {
                Button {
                    legacyConversationID = nil
                    legacyMessageID = nil
                    legacySection = nil
                    legacyOpen = false
                } label: {
                    Image(systemName: "chevron.left")
                        .font(.system(size: 15, weight: .bold))
                        .frame(width: 36, height: 36)
                        .background(.ultraThinMaterial, in: Circle())
                }
                .accessibilityIdentifier("grok-mobile-back")
                Spacer()
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 8)
            .background(.ultraThinMaterial)

            ContentView(
                model: model,
                messaging: messaging,
                appAgentSurface: appAgentSurface,
                bridge: bridge,
                reconnectGeneration: reconnectGeneration,
                initialConversation: legacyConversationID.flatMap { id in
                    messaging.conversations.first(where: { $0.id == id })
                },
                initialMessageID: legacyMessageID,
                initialSection: legacySection
            )
        }
    }

    private var homeContent: some View {
        home
            .task(id: accessRosterTaskKey) { await refreshAccessRoster() }
            .task(id: mobileAccountScopeKey) {
                await loadAgentSidebarSections()
                await loadPinnedBotIds()
            }
            .task { await messaging.refresh() }
            .task(id: appAgentSurfaceFingerprint) { publishAppAgentSurface() }
    }

    var appAgentSurfaceFingerprint: String {
        var components: [String] = []
        components.append(query)
        components.append(String(searchOpen))
        components.append(paletteTab.rawValue)
        components.append(commandPaletteFingerprint)
        components.append(String(composeOpen))
        components.append(String(createBotOpen))
        components.append(botName)
        components.append(botDescription)
        components.append(String(botBusy))
        components.append(botError ?? "")
        components.append(botRenameTarget?.id ?? "")
        components.append(botRenameDraft)
        components.append(botDeleteTarget?.id ?? "")
        components.append(String(botActionBusy))
        components.append(botActionError ?? "")
        components.append(botRosterFingerprint)
        components.append(conversationFingerprint)
        return components.joined(separator: "|")
    }

    private var botRosterFingerprint: String {
        bots
            .map { bot in
                [
                    bot.id,
                    bot.name,
                    bot.description,
                    bot.title ?? "",
                    String(bot.notifyOnUpdatesEnabled),
                    String(bot.hidden),
                    String(bot.unread),
                    bot.conversationId ?? "",
                    String(pinnedBotIds.contains(bot.id)),
                    bot.miniAppId ?? "",
                    String(bot.isGroup),
                    bot.memberIds.joined(separator: "+"),
                    String(bot.isSharedRoom),
                ].joined(separator: ":")
            }
            .joined(separator: ",")
    }

    private var conversationFingerprint: String {
        messaging.conversations
            .map { conversation in
                [
                    conversation.id,
                    String(conversation.unreadCount),
                    String(conversation.isArchived),
                ].joined(separator: ":")
            }
            .joined(separator: ",")
    }
}
