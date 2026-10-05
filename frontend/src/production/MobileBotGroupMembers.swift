import SwiftUI

internal struct MobileBotGroupMembersSheet: View {
    let group: MobileBotSummary
    let roster: [MobileBotSummary]
    let bridge: IOSPreloadBridge
    let accountScopeKey: String
    let onRosterChanged: ([MobileBotSummary]) -> Void
    let onClose: () -> Void

    @State private var pendingAgentId: String?
    @State private var failure: String?
    @State private var generation = 0
    @State private var mutationTask: Task<Void, Never>?
    @State private var removalTarget: MobileBotSummary?

    private var currentGroup: MobileBotSummary? {
        GrokMobileGroupMembersModel.group(id: group.id, fallback: group, roster: roster)
    }

    private var members: [MobileBotSummary] {
        guard let currentGroup else { return [] }
        return GrokMobileGroupMembersModel.members(group: currentGroup, roster: roster)
    }

    private var candidates: [MobileBotSummary] {
        guard let currentGroup else { return [] }
        return GrokMobileGroupMembersModel.candidates(group: currentGroup, roster: roster)
    }

    var body: some View {
        NavigationStack {
            List {
                if let currentGroup {
                    Section("成员") {
                        ForEach(members) { member in
                            HStack(spacing: 12) {
                                ClothGhostAvatar(botId: member.id, size: 36)
                                Text(member.name)
                                Spacer()
                                if pendingAgentId == member.id {
                                    ProgressView().controlSize(.small)
                                } else {
                                    Button(role: .destructive) {
                                        removalTarget = member
                                    } label: {
                                        Image(systemName: "minus.circle")
                                    }
                                    .buttonStyle(.borderless)
                                    .disabled(!GrokMobileGroupMembersModel.canRemove(
                                        group: currentGroup,
                                        pending: pendingAgentId != nil
                                    ))
                                    .accessibilityLabel("移除 \(member.name)")
                                }
                            }
                        }
                    }

                    if GrokMobileGroupMembersModel.canAdd(
                        group: currentGroup,
                        roster: roster,
                        pending: pendingAgentId != nil
                    ) {
                        Section("添加成员") {
                            ForEach(candidates) { candidate in
                                Button {
                                    beginAdd(candidate)
                                } label: {
                                    HStack(spacing: 12) {
                                        ClothGhostAvatar(botId: candidate.id, size: 34)
                                        Text(candidate.name)
                                        Spacer()
                                        if pendingAgentId == candidate.id {
                                            ProgressView().controlSize(.small)
                                        } else {
                                            Image(systemName: "plus.circle")
                                        }
                                    }
                                }
                                .disabled(pendingAgentId != nil)
                                .accessibilityIdentifier("mobile-group-add-\(candidate.id)")
                            }
                        }
                    }

                    Section {
                        if currentGroup.memberIds.count >= GrokMobileGroupMembersModel.maximumMembers {
                            Text("群组最多可以有 \(GrokMobileGroupMembersModel.maximumMembers) 个 Bot 成员。")
                                .foregroundStyle(.secondary)
                        } else if candidates.isEmpty {
                            Text("创建更多独立 Bot 后即可把它们加入此群组。")
                                .foregroundStyle(.secondary)
                        } else {
                            Text("群组至少保留 1 个成员；群组不能嵌套群组。")
                                .foregroundStyle(.secondary)
                        }
                    }
                } else {
                    Section {
                        Text("此会话不是可编辑的 Bot 群组。")
                            .foregroundStyle(.secondary)
                    }
                }

                if let failure {
                    Section {
                        Text(failure)
                            .foregroundStyle(.red)
                            .accessibilityIdentifier("mobile-group-members-error")
                    }
                }
            }
            .navigationTitle("成员")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("完成", action: onClose)
                }
            }
        }
        .accessibilityIdentifier("mobile-group-members")
        .onChange(of: group.id) { _, _ in invalidatePending() }
        .onChange(of: accountScopeKey) { _, _ in invalidatePending() }
        .onDisappear { invalidatePending() }
        .alert(item: $removalTarget) { member in
            Alert(
                title: Text("从此群组移除“\(member.name)”？"),
                message: Text("移除后，该 Bot 将不再参与这个群组中的对话。"),
                primaryButton: .destructive(Text("移除")) {
                    beginRemove(member)
                },
                secondaryButton: .cancel(Text("取消"))
            )
        }
    }

    @MainActor
    private func beginAdd(_ member: MobileBotSummary) {
        guard let currentGroup,
              pendingAgentId == nil,
              let memberIds = GrokMobileGroupMembersModel.adding(
                memberId: member.id,
                to: currentGroup,
                roster: roster
              )
        else { return }
        beginMutation(agentId: member.id, memberIds: memberIds)
    }

    @MainActor
    private func beginRemove(_ member: MobileBotSummary) {
        guard let currentGroup,
              pendingAgentId == nil,
              let memberIds = GrokMobileGroupMembersModel.removing(
                memberId: member.id,
                from: currentGroup
              )
        else { return }
        beginMutation(agentId: member.id, memberIds: memberIds)
    }

    @MainActor
    private func beginMutation(agentId: String, memberIds: [String]) {
        guard pendingAgentId == nil else { return }
        let fence = GrokMobileGroupMembersModel.MutationFence(
            accountScopeKey: accountScopeKey,
            generation: generation
        )
        pendingAgentId = agentId
        failure = nil
        mutationTask?.cancel()
        mutationTask = Task { @MainActor in
            defer {
                if GrokMobileGroupMembersModel.accepts(
                    fence,
                    accountScopeKey: accountScopeKey,
                    generation: generation
                ) {
                    pendingAgentId = nil
                    mutationTask = nil
                }
            }
            do {
                let updated = try await GrokMobileBotService(bridge: bridge).updateGroupMembers(
                    groupId: group.id,
                    memberIds: memberIds
                )
                try Task.checkCancellation()
                guard GrokMobileGroupMembersModel.accepts(
                    fence,
                    accountScopeKey: accountScopeKey,
                    generation: generation
                ) else { return }
                onRosterChanged(updated)
            } catch is CancellationError {
                return
            } catch {
                guard GrokMobileGroupMembersModel.accepts(
                    fence,
                    accountScopeKey: accountScopeKey,
                    generation: generation
                ) else { return }
                failure = "更新群组成员失败：\(error.localizedDescription)"
            }
        }
    }

    @MainActor
    private func invalidatePending() {
        generation += 1
        mutationTask?.cancel()
        mutationTask = nil
        pendingAgentId = nil
        failure = nil
        removalTarget = nil
    }
}
