import SwiftUI

internal func mobileBotConversationScopeKey(
    accountScopeKey: String,
    agentID: String
) -> String {
    "\(accountScopeKey.utf8.count)#\(accountScopeKey)\(agentID.utf8.count)#\(agentID)"
}

internal struct GrokMobileShell: View {
    @Bindable var model: MarketplaceModel
    @Bindable var messaging: MessagingModel
    let bridge: IOSPreloadBridge
    let appAgentSurface: FabushiAppAgentSurface
    var reconnectGeneration: Int = 0
    let onRetryConnection: @MainActor () async -> Void
    @Environment(\.openURL) var openExternalURL

    @State var query = ""
    @State var paletteTab: MobileCommandPaletteTab = .all
    @State var searchOpen = false
    @State var composeOpen = false
    @State var createBotOpen = false
    @State var botName = ""
    @State var botDescription = ""
    @State var botAvatarShape = "wedge"
    @State var botAvatarColor = "cyan"
    @State var botBusy = false
    @State var botError: String?
    @State var botRenameTarget: MobileBotSummary?
    @State var botRenameDraft = ""
    @State var botDeleteTarget: MobileBotSummary?
    @State var botActionBusy = false
    @State var botActionError: String?
    @State var bots: [MobileBotSummary] = []
    @State var agentNetworkOpen = false
    @State var agentNetworkGateEnabled = false
    @State var hiddenChatsOpen = false
    @State var hiddenChatsController = MobileHiddenChatsMutationController()
    @State var accessRosterSnapshot = AccessRosterSnapshot.initial
    @State var accessCoverFirstBox = FirstBoxGateState.initial
    @State var accessCoverAccess = AccessCoverSandAccess.checking
    @State var accessRosterGeneration = 0
    @State var connectionRetrying = false
    @State var rosterSelection = AccessRosterSelectionState.empty
    @State var rosterSelectionScopeKey = ""
    @State var pinnedBotIdOrder: [String] = []
    @State var asyncTasksTarget: MobileBotSummary?
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
    @State var botComposerAttachments: [String: [MobileComposerAttachment]] = [:]
    @State var botComposerRecoveries: [String: MobileComposerRecovery] = [:]
    @State var botTranscripts: [String: [MobileChatMessage]] = [:]
    @State var legacyOpen = false
    @State var legacyConversationID: String?
    @State var legacyMessageID: String?
    @State var legacySection: MobileSection?
    @State var commandPaletteRoutines: [MobileCommandPaletteRoutine] = []
    @State var commandPaletteRoutineStatus: MobileCommandPaletteProviderStatus = .idle
    @State var commandPaletteLinkMetadata: [String: MobileCommandPaletteLinkMetadata] = [:]
    @State var commandPaletteLinkStatus: MobileCommandPaletteProviderStatus = .idle
    @State var commandPaletteAgentID: String?
    @State var commandPaletteComputerStatus: RemoteComputerAgentBoxSnapshot?
    @State var commandPaletteComputerPending = false
    @State var commandPaletteComputerQueued = false
    @State var commandPaletteComputerConfirmation: MobileCommandPaletteComputerUpdateAction?
    @State var commandPaletteComputerConfirmationSeconds = 0
    @State var commandPaletteComputerGeneration = 0
    @State var commandPaletteComputerRebuildOwner: RemoteComputerRebuildOwner?
    @State var rootNotificationTrays: [MobileRootNotificationTray] = []
    @State var rootNotificationActionPending: Set<String> = []
    @State var rootNotificationActionNotice: [String: MobileRootNotificationActionNotice] = [:]
    @State var rootNotificationCopiedRequestID: String?
    @State var promptFocusGeneration = 0

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
        .overlay(alignment: .topLeading) {
            rootHardwareKeyboardShortcuts
        }
        .overlay(alignment: .top) {
            rootNotificationStack
        }
        .task(id: rootNotificationLifecycleKey) {
            await runRootNotificationLifecycle()
        }
        .task(id: "\(mobileAccountScopeKey)|\(selectedBot?.id ?? "")") {
            hiddenChatsController.setScope(
                accountScopeKey: mobileAccountScopeKey,
                activeAgentId: selectedBot?.id
            )
        }
        .onChange(of: reconnectGeneration) { _, _ in
            retryHeldHiddenChatMutations()
        }
        .onChange(of: model.accountRosterRevision) { _, _ in
            Task { await refreshAccessRoster() }
        }
        .onChange(of: mobileAccountScopeKey) { _, _ in
            resetCommandPaletteComputerScope()
        }
        .onDisappear {
            commandPaletteComputerGeneration &+= 1
            commandPaletteComputerQueued = false
            commandPaletteComputerConfirmationSeconds = 0
            commandPaletteComputerRebuildOwner?.dispose()
            commandPaletteComputerRebuildOwner = nil
            rootNotificationTrays = []
            rootNotificationActionPending.removeAll()
            rootNotificationActionNotice.removeAll()
            rootNotificationCopiedRequestID = nil
            hiddenChatsController.dispose()
        }
        .sheet(item: $groupMembersTarget) { group in
            MobileBotGroupMembersSheet(
                group: bots.first(where: { $0.id == group.id }) ?? group,
                roster: bots,
                bridge: bridge,
                accountScopeKey: mobileAccountScopeKey,
                reconnectGeneration: reconnectGeneration,
                onRosterChanged: { updated in
                    applyBotRosterUpdate(updated)
                    groupMembersTarget = updated.first(where: { $0.id == group.id })
                },
                onOpenAgentChat: { member in
                    groupMembersTarget = nil
                    guard let current = bots.first(where: { $0.id == member.id }),
                          !current.isGroup
                    else { return }
                    selectBotForConversation(current)
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
            MobileAsyncTasksPanel(
                agentId: agent.id,
                agentName: agent.name,
                bridge: bridge,
                reconnectGeneration: reconnectGeneration,
                onClose: { asyncTasksTarget = nil }
            )
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
        .fullScreenCover(isPresented: $agentNetworkOpen) {
            FabushiAgentNetworkView(
                agents: bots,
                onOpenAgent: { id in
                    guard let agent = MobileAgentNetworkModel.openTarget(id: id, agents: bots) else { return }
                    agentNetworkOpen = false
                    selectBotForConversation(agent)
                },
                onClose: { agentNetworkOpen = false }
            )
        }
        .confirmationDialog(
            commandPaletteComputerConfirmationTitle,
            isPresented: Binding(
                get: { commandPaletteComputerConfirmation != nil },
                set: {
                    if !$0 {
                        commandPaletteComputerConfirmation = nil
                        commandPaletteComputerConfirmationSeconds = 0
                    }
                }
            ),
            titleVisibility: .visible
        ) {
            if commandPaletteComputerConfirmation == .busyOverride {
                Button(
                    commandPaletteComputerConfirmationSeconds > 0
                        ? "Update when done (\(commandPaletteComputerConfirmationSeconds))"
                        : commandPaletteComputerWorkingAgentNames.count > 1
                            ? "Update when agents are done"
                            : "Update when done"
                ) {
                    commandPaletteComputerConfirmation = nil
                    commandPaletteComputerConfirmationSeconds = 0
                    queueCommandPaletteComputerUpdate()
                }
                .disabled(commandPaletteComputerConfirmationSeconds > 0)
                Button(
                    commandPaletteComputerConfirmationSeconds > 0
                        ? "Update anyway (\(commandPaletteComputerConfirmationSeconds))"
                        : "Update anyway",
                    role: .destructive
                ) {
                    commandPaletteComputerConfirmation = nil
                    commandPaletteComputerConfirmationSeconds = 0
                    Task { await performCommandPaletteComputerUpdate(force: true) }
                }
                .disabled(commandPaletteComputerConfirmationSeconds > 0)
            } else if commandPaletteComputerConfirmation == .ready {
                Button(
                    commandPaletteComputerConfirmationSeconds > 0
                        ? "Update Fabushi's Computer (\(commandPaletteComputerConfirmationSeconds))"
                        : "Update Fabushi's Computer"
                ) {
                    commandPaletteComputerConfirmation = nil
                    commandPaletteComputerConfirmationSeconds = 0
                    Task { await performCommandPaletteComputerUpdate(force: false) }
                }
                .disabled(commandPaletteComputerConfirmationSeconds > 0)
            }
            Button("Cancel", role: .cancel) {
                commandPaletteComputerConfirmation = nil
                commandPaletteComputerConfirmationSeconds = 0
            }
        } message: {
            Text(commandPaletteComputerConfirmationMessage)
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

    var agentNetworkAvailability: MobileAgentNetworkAvailability {
        MobileAgentNetworkAvailability.resolve(
            gateEnabled: agentNetworkGateEnabled,
            hasAgents: !bots.isEmpty
        )
    }

    var accessCoverComposition: AccessCoverCompositionState {
        AccessCoverComposition.project(
            access: accessCoverAccess,
            roster: accessRosterSnapshot,
            firstBox: accessCoverFirstBox,
            isComputerRebuildLocked: remoteComputerAgentTarget != nil
        )
    }

    var coordinatorConnectionSnapshot: MobileCoordinatorConnectionSnapshot {
        MobileCoordinatorConnectionProjection.project(
            loggedIn: model.loggedIn,
            access: accessCoverAccess,
            roster: accessRosterSnapshot,
            firstBox: accessCoverFirstBox,
            isRetrying: connectionRetrying
        )
    }

    var isRosterPrivacyBlocked: Bool {
        isMobileRosterPrivacyBlocked(
            access: accessCoverAccess,
            failure: accessRosterSnapshot.failure
        )
    }

    var rosterStatusKind: MobileRosterStatusKind? {
        MobileRosterStatusProjection.project(
            roster: accessRosterSnapshot,
            bots: bots
        )
    }

    @MainActor
    func retryCoordinatorConnection() {
        guard !connectionRetrying else { return }
        connectionRetrying = true
        Task { @MainActor in
            await onRetryConnection()
            connectionRetrying = false
        }
    }

    @MainActor
    func applyBotRosterUpdate(_ updated: [MobileBotSummary]) {
        bots = updated
        reconcileRosterSelection(with: updated, isComplete: accessRosterSnapshot.hasCompleteRoster)
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

    @MainActor
    func restoreRosterSelectionIfNeeded() {
        let scope = mobileAccountScopeKey
        guard rosterSelectionScopeKey != scope else { return }
        rosterSelectionScopeKey = scope
        rosterSelection = AccessRosterSelectionPersistence.load(accountScopeKey: scope)
        selectedBot = rosterSelection.currentAgentID.flatMap { id in
            bots.first(where: { $0.id == id })
        }
    }

    @MainActor
    func selectBotForConversation(_ bot: MobileBotSummary) {
        let next = AccessRosterSelectionProjection.select(bot.id, previous: rosterSelection)
        rosterSelection = next
        AccessRosterSelectionPersistence.save(next, accountScopeKey: mobileAccountScopeKey)
        selectedBot = bot

        if accessRosterSnapshot.hasCompleteRoster {
            let settled = AccessRosterSelectionProjection.settle(
                next,
                attemptedAgentID: bot.id,
                completeAgentIDs: bots.map(\.id)
            )
            rosterSelection = settled
            AccessRosterSelectionPersistence.save(settled, accountScopeKey: mobileAccountScopeKey)
            if settled.currentAgentID != bot.id {
                selectedBot = settled.currentAgentID.flatMap { id in
                    bots.first(where: { $0.id == id })
                }
            }
        }
    }

    @MainActor
    func clearRosterSelection() {
        rosterSelection = .empty
        selectedBot = nil
        AccessRosterSelectionPersistence.clear(accountScopeKey: mobileAccountScopeKey)
    }

    @MainActor
    func reconcileRosterSelection(
        with roster: [MobileBotSummary],
        isComplete: Bool
    ) {
        let previous = rosterSelection
        let next = AccessRosterSelectionProjection.reconcile(
            previous,
            agentIDs: roster.map(\.id),
            isRosterComplete: isComplete
        )
        guard next != previous else {
            if let id = next.currentAgentID,
               let refreshed = roster.first(where: { $0.id == id }) {
                self.selectedBot = refreshed
            }
            return
        }
        rosterSelection = next
        AccessRosterSelectionPersistence.save(next, accountScopeKey: mobileAccountScopeKey)
        if previous.currentAgentID != nil {
            selectedBot = next.currentAgentID.flatMap { id in
                roster.first(where: { $0.id == id })
            }
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
            availableBots: bots,
            bridge: bridge,
            model: model,
            messaging: messaging,
            appAgentSurface: appAgentSurface,
            reconnectGeneration: reconnectGeneration,
            suggestionTransportConnected: coordinatorConnectionSnapshot.phase == .connected,
            focusPromptGeneration: promptFocusGeneration,
            onClose: { clearRosterSelection() },
            onOpenSettings: {
                self.botSettingsRoutineID = nil
                self.botSettingsTarget = self.bots.first(where: { $0.id == bot.id }) ?? bot
            },
            onOpenAutomation: { automationId in
                self.botSettingsRoutineID = automationId
                self.botSettingsTarget = self.bots.first(where: { $0.id == bot.id }) ?? bot
            },
            draft: botDraftBinding(for: bot.id),
            composerAttachments: botComposerAttachmentBinding(for: bot.id),
            composerRecovery: botComposerRecoveryBinding(for: bot.id),
            entries: botTranscriptBinding(for: bot.id)
        )
        .task(id: mobileBotConversationScopeKey(
            accountScopeKey: mobileAccountScopeKey,
            agentID: bot.id
        )) {
            restoreMobileComposerDraft(for: bot.id)
        }
    }

    private func botDraftBinding(for botID: String) -> Binding<String> {
        let storageKey = mobileBotConversationScopeKey(
            accountScopeKey: mobileAccountScopeKey,
            agentID: botID
        )
        return Binding(
            get: { botDrafts[storageKey] ?? "" },
            set: {
                botDrafts[storageKey] = $0
                persistMobileComposerDraft(for: botID)
            }
        )
    }

    private func botComposerAttachmentBinding(
        for botID: String
    ) -> Binding<[MobileComposerAttachment]> {
        let storageKey = mobileBotConversationScopeKey(
            accountScopeKey: mobileAccountScopeKey,
            agentID: botID
        )
        return Binding(
            get: { botComposerAttachments[storageKey] ?? [] },
            set: {
                botComposerAttachments[storageKey] = $0
                persistMobileComposerDraft(for: botID)
            }
        )
    }

    private func botComposerRecoveryBinding(
        for botID: String
    ) -> Binding<MobileComposerRecovery?> {
        let storageKey = mobileBotConversationScopeKey(
            accountScopeKey: mobileAccountScopeKey,
            agentID: botID
        )
        return Binding(
            get: { botComposerRecoveries[storageKey] },
            set: {
                if let value = $0 {
                    botComposerRecoveries[storageKey] = value
                } else {
                    botComposerRecoveries.removeValue(forKey: storageKey)
                }
                persistMobileComposerDraft(for: botID)
            }
        )
    }

    @MainActor
    private func restoreMobileComposerDraft(for botID: String) {
        let storageKey = mobileBotConversationScopeKey(
            accountScopeKey: mobileAccountScopeKey,
            agentID: botID
        )
        let snapshot = MobileComposerDraftPersistence.load(
            accountScopeKey: mobileAccountScopeKey,
            agentID: botID
        )
        if snapshot.hasActivePayload {
            botDrafts[storageKey] = snapshot.text
            botComposerAttachments[storageKey] = snapshot.attachments
            if let recovery = snapshot.recovery {
                botComposerRecoveries[storageKey] = recovery
            }
            return
        }
        if let recovery = snapshot.recovery {
            botDrafts[storageKey] = recovery.text
            botComposerAttachments[storageKey] = recovery.attachments
            botComposerRecoveries.removeValue(forKey: storageKey)
            persistMobileComposerDraft(for: botID)
        }
    }

    @MainActor
    private func persistMobileComposerDraft(for botID: String) {
        let storageKey = mobileBotConversationScopeKey(
            accountScopeKey: mobileAccountScopeKey,
            agentID: botID
        )
        MobileComposerDraftPersistence.save(
            accountScopeKey: mobileAccountScopeKey,
            agentID: botID,
            snapshot: .init(
                text: botDrafts[storageKey] ?? "",
                attachments: botComposerAttachments[storageKey] ?? [],
                recovery: botComposerRecoveries[storageKey]
            )
        )
    }

    private func botTranscriptBinding(for botID: String) -> Binding<[MobileChatMessage]> {
        let storageKey = mobileBotConversationScopeKey(
            accountScopeKey: mobileAccountScopeKey,
            agentID: botID
        )
        return Binding(
            get: { botTranscripts[storageKey] ?? [] },
            set: { botTranscripts[storageKey] = $0 }
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
            .task(id: accessRosterTaskKey) { await runAccessRosterLifecycle() }
            .task(id: mobileAccountScopeKey) {
                restoreRosterSelectionIfNeeded()
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
                    bot.conversationPartnerIds.joined(separator: "+"),
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
