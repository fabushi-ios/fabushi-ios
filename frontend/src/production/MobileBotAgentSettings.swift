import SwiftUI

internal struct MobileBotAgentSettingsSheet: View {
    let agent: MobileBotSummary
    let roster: [MobileBotSummary]
    let bridge: IOSPreloadBridge
    let accountScopeKey: String
    let onRosterChanged: ([MobileBotSummary]) -> Void
    let onClose: () -> Void

    @State private var currentAgent: MobileBotSummary
    @State private var nameDraft: String
    @State private var titleDraft: String
    @State private var descriptionDraft: String
    @State private var pending: GrokMobileAgentSettingsModel.Pending?
    @State private var failure: String?
    @State private var generation = 0
    @State private var mutationTask: Task<Void, Never>?

    init(
        agent: MobileBotSummary,
        roster: [MobileBotSummary],
        bridge: IOSPreloadBridge,
        accountScopeKey: String,
        onRosterChanged: @escaping ([MobileBotSummary]) -> Void,
        onClose: @escaping () -> Void
    ) {
        self.agent = agent
        self.roster = roster
        self.bridge = bridge
        self.accountScopeKey = accountScopeKey
        self.onRosterChanged = onRosterChanged
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
            Form {
                Section("资料") {
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

                MobileBotSharedRoomTrigger(
                    agent: currentAgent,
                    roster: roster,
                    bridge: bridge,
                    accountScopeKey: accountScopeKey
                )

                if !currentAgent.isGroup {
                    MobileBotRoutinesSection(
                        agentId: currentAgent.id,
                        bridge: bridge,
                        accountScopeKey: accountScopeKey
                    )
                }

                if let failure {
                    Section {
                        Text(failure)
                            .foregroundStyle(.red)
                            .accessibilityIdentifier("mobile-agent-settings-error")
                    }
                }
            }
            .navigationTitle(currentAgent.isGroup ? "群组设置" : "Agent 设置")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("完成", action: onClose)
                        .disabled(pending != nil)
                }
            }
        }
        .accessibilityIdentifier("mobile-agent-settings")
        .onChange(of: agent.id) { _, _ in invalidatePending() }
        .onChange(of: accountScopeKey) { _, _ in invalidatePending() }
        .onDisappear { invalidatePending() }
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
