import SwiftUI

internal struct MobileBotAgentSettingsSheet: View {
    let agent: MobileBotSummary
    let roster: [MobileBotSummary]
    let bridge: IOSPreloadBridge
    @Bindable var marketplaceModel: MarketplaceModel
    let accountScopeKey: String
    let reconnectGeneration: Int
    let focusedAutomationId: String?
    let onRosterChanged: ([MobileBotSummary]) -> Void
    let onOpenComputer: (MobileBotSummary) -> Void
    let onClose: () -> Void

    @State private var currentAgent: MobileBotSummary
    @State private var nameDraft: String
    @State private var titleDraft: String
    @State private var descriptionDraft: String
    @State private var pending: GrokMobileAgentSettingsModel.Pending?
    @State private var failure: String?
    @State private var avatarEditorPresented = false
    @State private var channelsPresented = false
    @State private var generation = 0
    @State private var mutationTask: Task<Void, Never>?

    init(
        agent: MobileBotSummary,
        roster: [MobileBotSummary],
        bridge: IOSPreloadBridge,
        marketplaceModel: MarketplaceModel,
        accountScopeKey: String,
        reconnectGeneration: Int = 0,
        focusedAutomationId: String? = nil,
        onRosterChanged: @escaping ([MobileBotSummary]) -> Void,
        onOpenComputer: @escaping (MobileBotSummary) -> Void,
        onClose: @escaping () -> Void
    ) {
        self.agent = agent
        self.roster = roster
        self.bridge = bridge
        self.marketplaceModel = marketplaceModel
        self.accountScopeKey = accountScopeKey
        self.reconnectGeneration = reconnectGeneration
        self.focusedAutomationId = focusedAutomationId
        self.onRosterChanged = onRosterChanged
        self.onOpenComputer = onOpenComputer
        self.onClose = onClose
        _currentAgent = State(initialValue: agent)
        _nameDraft = State(initialValue: agent.name)
        _titleDraft = State(initialValue: agent.title ?? "")
        _descriptionDraft = State(initialValue: agent.description)
    }

    private var candidateProfile: GrokMobileAgentSettingsModel.Profile? {
        GrokMobileAgentSettingsModel.normalizedProfile(
            name: nameDraft,
            title: currentAgent.title == nil ? nil : titleDraft,
            description: descriptionDraft,
            isGroup: currentAgent.isGroup
        )
    }

    private var profileChanged: Bool {
        guard let candidateProfile else { return false }
        return candidateProfile != GrokMobileAgentSettingsModel.profile(from: currentAgent)
    }

    var body: some View {
        NavigationStack {
            settingsForm
                .navigationTitle(currentAgent.isGroup ? "群组设置" : "Agent 设置")
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) {
                        Button("完成", action: onClose)
                            .disabled(pending != nil)
                    }
                }
        }
        .accessibilityIdentifier("mobile-agent-settings")
        .task(id: "\(currentAgent.id)|\(accountScopeKey)|\(reconnectGeneration)") {
            marketplaceModel.bindPrivateSkillAgentScope(
                agentId: currentAgent.id,
                agentName: currentAgent.name
            )
            await marketplaceModel.refreshPrivateSkills()
        }
        .sheet(isPresented: $avatarEditorPresented) {
            MobileAvatarEditorSheet(
                agent: currentAgent,
                bridge: bridge,
                onSaved: { updated in
                    guard let authoritative = updated.first(where: { $0.id == currentAgent.id }) else {
                        return
                    }
                    applyAuthoritative(authoritative)
                    onRosterChanged(updated)
                },
                onClose: { avatarEditorPresented = false }
            )
        }
        .sheet(isPresented: $channelsPresented) {
            MobileAgentChannelsPanel(
                agentId: currentAgent.id,
                agentName: currentAgent.name,
                bridge: bridge,
                accountScopeKey: accountScopeKey,
                reconnectGeneration: reconnectGeneration,
                onClose: { channelsPresented = false }
            )
        }
        .onChange(of: agent.id) { _, _ in invalidatePending() }
        .onChange(of: accountScopeKey) { _, _ in invalidatePending() }
        .onDisappear {
            marketplaceModel.clearPrivateSkillAgentScope(agentId: currentAgent.id)
            invalidatePending()
        }
    }

    private var settingsForm: some View {
        Form {
            profileSection
            notificationSection
            sharedRoomTrigger
            agentOnlySections
            failureSection
        }
    }

    private var profileSection: some View {
        Section("资料") {
            Button {
                avatarEditorPresented = true
            } label: {
                Label(
                    currentAgent.avatarDataURL == nil
                        ? (currentAgent.isGroup ? "设置群组头像" : "设置头像与角色")
                        : (currentAgent.isGroup ? "编辑群组头像" : "编辑头像与角色"),
                    systemImage: currentAgent.isGroup ? "person.2.crop.square.stack" : "person.crop.circle"
                )
            }
            .disabled(pending != nil)
            .accessibilityIdentifier("mobile-agent-settings-avatar")

            TextField("名称", text: $nameDraft)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .disabled(pending != nil)
                .accessibilityLabel("Agent name")
                .accessibilityIdentifier("mobile-agent-settings-name")

            if !currentAgent.isGroup, currentAgent.title != nil {
                TextField("标题", text: $titleDraft)
                    .disabled(pending != nil)
                    .accessibilityLabel("Agent title")
                    .accessibilityIdentifier("mobile-agent-settings-title")
            }

            TextField("描述", text: $descriptionDraft, axis: .vertical)
                .lineLimit(3...7)
                .disabled(pending != nil)
                .accessibilityLabel("Agent description")
                .accessibilityIdentifier("mobile-agent-settings-description")

            Button(pending == .profile ? "保存中…" : "保存资料") {
                beginProfileUpdate()
            }
            .disabled(pending != nil || !profileChanged)
            .accessibilityIdentifier("mobile-agent-settings-save")
        }
    }

    @ViewBuilder
    private var notificationSection: some View {
        if !currentAgent.isGroup {
            Section {
                Toggle(
                    "通知",
                    isOn: Binding(
                        get: { currentAgent.notifyOnUpdatesEnabled },
                        set: { beginNotificationUpdate($0) }
                    )
                )
                .disabled(pending != nil)
                .accessibilityLabel("Agent update notifications")
                .accessibilityIdentifier("mobile-agent-settings-notifications")
            } footer: {
                Text("当这个 Agent 完成任务或需要输入时通知我。")
            }
        }
    }

    private var sharedRoomTrigger: some View {
        MobileBotSharedRoomTrigger(
            agent: currentAgent,
            roster: roster,
            bridge: bridge,
            accountScopeKey: accountScopeKey
        )
    }

    @ViewBuilder
    private var agentOnlySections: some View {
        if !currentAgent.isGroup {
            Section {
                Button {
                    channelsPresented = true
                } label: {
                    Label("Channels", systemImage: "antenna.radiowaves.left.and.right")
                }
                .disabled(pending != nil)
                .accessibilityIdentifier("mobile-agent-settings-channels")
            } header: {
                Text("Channels")
            } footer: {
                Text("连接状态由 Rust Host 按账号与 Agent 持有；凭据加密保存且不会回传到界面。")
            }

            MobileBotMemorySection(
                agentId: currentAgent.id,
                bridge: bridge,
                accountScopeKey: accountScopeKey,
                reconnectGeneration: reconnectGeneration
            )

            MobileBotRoutinesSection(
                agentId: currentAgent.id,
                bridge: bridge,
                accountScopeKey: accountScopeKey,
                reconnectGeneration: reconnectGeneration,
                focusedAutomationId: focusedAutomationId
            )

            Section {
                if marketplaceModel.privateSkillsLoading {
                    ProgressView("正在读取 \(currentAgent.name) 的 Skills…")
                }
                if let error = marketplaceModel.privateSkillError {
                    Text(error).font(.caption).foregroundStyle(.red)
                        .accessibilityIdentifier("mobile-agent-skills-error")
                }
                HStack(spacing: 8) {
                    TextField("搜索 Skills", text: $marketplaceModel.privateSkillQuery)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .accessibilityIdentifier("mobile-agent-skills-search")
                    Button("刷新") { Task { await marketplaceModel.refreshPrivateSkills() } }
                        .disabled(marketplaceModel.privateSkillsLoading)
                        .accessibilityIdentifier("mobile-agent-skills-refresh")
                }
                Picker("来源", selection: $marketplaceModel.privateSkillOwnershipFilter) {
                    ForEach(MarketplaceSkillOwnershipFilter.allCases) { filter in
                        Text(filter.rawValue).tag(filter)
                    }
                }
                .pickerStyle(.segmented)
                .accessibilityIdentifier("mobile-agent-skills-filter")
                if marketplaceModel.visiblePrivateSkills.isEmpty
                    && !marketplaceModel.privateSkillsLoading
                    && marketplaceModel.privateSkillError == nil {
                    Text("此 Agent 当前没有匹配的 Skills。").foregroundStyle(.secondary)
                }
                ForEach(marketplaceModel.visiblePrivateSkills) { skill in
                    DisclosureGroup {
                        VStack(alignment: .leading, spacing: 8) {
                            TextField("名称", text: Binding(
                                get: { marketplaceModel.privateSkillNameDrafts[skill.id] ?? skill.name },
                                set: { marketplaceModel.privateSkillNameDrafts[skill.id] = $0 }
                            )).disabled(!skill.canEdit)
                            TextField("Description / Use when", text: Binding(
                                get: { marketplaceModel.privateSkillDescriptionDrafts[skill.id] ?? skill.description },
                                set: { marketplaceModel.privateSkillDescriptionDrafts[skill.id] = $0 }
                            ), axis: .vertical).disabled(!skill.canEdit)
                            TextEditor(text: Binding(
                                get: { marketplaceModel.privateSkillBodyDrafts[skill.id] ?? skill.body },
                                set: { marketplaceModel.privateSkillBodyDrafts[skill.id] = $0 }
                            )).frame(minHeight: 88).disabled(!skill.canEdit)
                            if skill.canEdit {
                                HStack(spacing: 8) {
                                    Button("保存") { Task { await marketplaceModel.savePrivateSkill(skill) } }
                                        .disabled(marketplaceModel.privateSkillMutatingId != nil)
                                    Button("删除", role: .destructive) { Task { await marketplaceModel.deletePrivateSkill(skill) } }
                                        .disabled(marketplaceModel.privateSkillMutatingId != nil)
                                }
                            }
                        }
                    } label: {
                        HStack(spacing: 10) {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(skill.name).font(.headline)
                                Text(skill.sourceLabel).font(.caption).foregroundStyle(.secondary)
                            }
                            Spacer()
                            if skill.canToggle {
                                Toggle("启用", isOn: Binding(
                                    get: { skill.isEnabledForAgent },
                                    set: { enabled in
                                        Task { await marketplaceModel.setPrivateSkillEnabled(skill, enabled: enabled) }
                                    }
                                ))
                                .labelsHidden()
                                .disabled(marketplaceModel.privateSkillMutatingId != nil)
                            }
                        }
                    }
                    .accessibilityIdentifier("mobile-agent-skill-\(skill.id)")
                }
            } header: {
                Text("Skills")
            } footer: {
                Text("Yours 始终绑定当前 Agent。保存、启停和删除后会从 Host 重新读取权威状态；团队发布/同步/取消发布仍需后续迁移 Desktop Host owner。")
            }

            Section {
                Button {
                    invalidatePending()
                    onOpenComputer(currentAgent)
                } label: {
                    Label("打开此 Agent 的电脑", systemImage: "desktopcomputer")
                }
                .disabled(pending != nil)
                .accessibilityIdentifier("mobile-agent-settings-open-computer")
            } header: {
                Text("电脑")
            } footer: {
                Text("从 Agent 设置打开时会显式绑定当前账号与 Agent 作用域。")
            }
        }
    }

    @ViewBuilder
    private var failureSection: some View {
        if let failure {
            Section {
                Text(failure)
                    .foregroundStyle(.red)
                    .accessibilityIdentifier("mobile-agent-settings-error")
            }
        }
    }

    @MainActor
    private func beginProfileUpdate() {
        guard pending == nil,
              let profile = candidateProfile,
              profile != GrokMobileAgentSettingsModel.profile(from: currentAgent)
        else { return }

        let fence = GrokMobileAgentSettingsModel.MutationFence(
            accountScopeKey: accountScopeKey,
            agentId: currentAgent.id,
            generation: generation
        )
        pending = .profile
        failure = nil
        mutationTask?.cancel()
        mutationTask = Task { @MainActor in
            defer { finishMutation(fence) }
            do {
                let updated = try await GrokMobileBotService(bridge: bridge).updateAgentProfile(
                    id: fence.agentId,
                    isGroup: currentAgent.isGroup,
                    name: profile.name,
                    title: profile.title,
                    description: profile.description
                )
                try Task.checkCancellation()
                guard accepts(fence),
                      let authoritative = updated.first(where: { $0.id == fence.agentId })
                else { return }
                applyAuthoritative(authoritative)
                onRosterChanged(updated)
            } catch is CancellationError {
                return
            } catch {
                guard accepts(fence) else { return }
                failure = "保存 Agent 设置失败：\(error.localizedDescription)"
            }
        }
    }

    @MainActor
    private func beginNotificationUpdate(_ isEnabled: Bool) {
        guard pending == nil,
              !currentAgent.isGroup,
              currentAgent.notifyOnUpdatesEnabled != isEnabled
        else { return }

        let fence = GrokMobileAgentSettingsModel.MutationFence(
            accountScopeKey: accountScopeKey,
            agentId: currentAgent.id,
            generation: generation
        )
        pending = .notifications
        failure = nil
        mutationTask?.cancel()
        mutationTask = Task { @MainActor in
            defer { finishMutation(fence) }
            do {
                let updated = try await GrokMobileBotService(bridge: bridge).setAgentNotifyOnUpdates(
                    id: fence.agentId,
                    isEnabled: isEnabled
                )
                try Task.checkCancellation()
                guard accepts(fence),
                      let authoritative = updated.first(where: { $0.id == fence.agentId })
                else { return }
                applyAuthoritative(authoritative)
                onRosterChanged(updated)
            } catch is CancellationError {
                return
            } catch {
                guard accepts(fence) else { return }
                failure = "更新 Agent 通知失败：\(error.localizedDescription)"
            }
        }
    }

    @MainActor
    private func applyAuthoritative(_ authoritative: MobileBotSummary) {
        currentAgent = authoritative
        nameDraft = authoritative.name
        titleDraft = authoritative.title ?? ""
        descriptionDraft = authoritative.description
        failure = nil
    }

    @MainActor
    private func accepts(_ fence: GrokMobileAgentSettingsModel.MutationFence) -> Bool {
        GrokMobileAgentSettingsModel.accepts(
            fence,
            accountScopeKey: accountScopeKey,
            agentId: currentAgent.id,
            generation: generation
        )
    }

    @MainActor
    private func finishMutation(_ fence: GrokMobileAgentSettingsModel.MutationFence) {
        guard accepts(fence) else { return }
        pending = nil
        mutationTask = nil
    }

    @MainActor
    private func invalidatePending() {
        generation += 1
        mutationTask?.cancel()
        mutationTask = nil
        pending = nil
        failure = nil
    }
}
