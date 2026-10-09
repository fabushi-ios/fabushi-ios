import Foundation
import SwiftUI

internal func committedMobileBotName(initialValue: String, draftValue: String) -> String? {
    let trimmed = draftValue.trimmingCharacters(in: .whitespacesAndNewlines)
    return trimmed.isEmpty || trimmed == initialValue ? nil : trimmed
}

internal func mobileBotDeleteDescription(_ bot: MobileBotSummary) -> String {
    if bot.isGroup {
        return "这会永久删除该群组及其聊天记录。群组中的 Bots 不会被删除，仍可单独使用。此操作无法撤销。"
    }
    return "这会永久删除该 Bot 及其聊天记录。此操作无法撤销。"
}

extension GrokMobileShell {

    @MainActor
    func refreshAgentNetworkGate() async {
        let expectedScope = mobileAccountScopeKey
        let expectedReconnect = reconnectGeneration
        do {
            let response = try await bridge.request(method: "getExperimentsSnapshot")
            try Task.checkCancellation()
            guard mobileAccountScopeKey == expectedScope,
                  reconnectGeneration == expectedReconnect
            else { return }
            agentNetworkGateEnabled = MobileAgentNetworkModel.gateEnabled(from: response.value)
                ?? BUNDLED_FEATURE_FLAGS["sand_agent_network"]?.defaultValue
                ?? false
            if !agentNetworkGateEnabled { agentNetworkOpen = false }
        } catch is CancellationError {
            return
        } catch {
            guard mobileAccountScopeKey == expectedScope,
                  reconnectGeneration == expectedReconnect
            else { return }
            agentNetworkGateEnabled = false
            agentNetworkOpen = false
        }
    }

    @MainActor
    func loadBots() async {
        await refreshAccessRoster()
    }

    @MainActor
    func runAccessRosterLifecycle() async {
        await refreshAccessRoster()
        while !Task.isCancelled {
            do {
                _ = try await bridge.receiveFeatureEvent(
                    deadlineMilliseconds: 2_560
                ) { event in
                    guard let type = event["type"] as? String else { return false }
                    return type == "bot.changed"
                        || type == "group.changed"
                        || type == "group.delta"
                }
                try Task.checkCancellation()
                await refreshAccessRoster()
            } catch IOSFeatureEventBrokerError.timedOut {
                continue
            } catch is CancellationError {
                return
            } catch {
                if Task.isCancelled { return }
                do {
                    try await Task.sleep(for: .milliseconds(320))
                } catch {
                    return
                }
                await refreshAccessRoster()
            }
        }
    }

    @MainActor
    func refreshAccessRoster() async {
        await refreshAgentNetworkGate()
        accessRosterGeneration = accessRosterGeneration == Int.max ? 1 : accessRosterGeneration + 1
        let expectedGeneration = accessRosterGeneration
        let expectedScope = mobileAccountScopeKey
        let expectedReconnect = reconnectGeneration

        restoreRosterSelectionIfNeeded()
        let restored = AccessRosterPersistence.load(accountScopeKey: expectedScope)
        if !restored.isEmpty {
            let projectedRestored = hiddenChatsController.projectAgents(restored)
            bots = projectedRestored
            accessRosterSnapshot = AccessRosterSnapshotProjection.restore(projectedRestored)
            reconcileRosterSelection(with: projectedRestored, isComplete: false)
            accessCoverFirstBox = FirstBoxGate.project(
                previous: accessCoverFirstBox,
                roster: firstBoxSnapshot(from: accessRosterSnapshot)
            )
        } else {
            accessRosterSnapshot = .initial
            accessCoverFirstBox = .initial
        }
        accessCoverAccess = .checking
        accessRosterSnapshot = AccessRosterSnapshotProjection.beginFetch(accessRosterSnapshot)

        do {
            let live = try await GrokMobileBotService(bridge: bridge).loadCanonicalRoster()
            try Task.checkCancellation()
            guard accessRosterGeneration == expectedGeneration,
                  mobileAccountScopeKey == expectedScope,
                  reconnectGeneration == expectedReconnect
            else { return }

            hiddenChatsController.ingestAgents(live)
            let projected = hiddenChatsController.projectAgents(live)
            bots = projected
            AccessRosterPersistence.save(projected, accountScopeKey: expectedScope)
            accessRosterSnapshot = AccessRosterSnapshotProjection.complete(
                projected,
                previous: accessRosterSnapshot
            )
            reconcileRosterSelection(with: projected, isComplete: true)
            accessCoverAccess = .init(state: .granted, reason: .none)
            accessCoverFirstBox = FirstBoxGate.project(
                previous: accessCoverFirstBox,
                roster: firstBoxSnapshot(from: accessRosterSnapshot)
            )
            if agentNetworkGateEnabled {
                let relationshipFence = MobileAgentNetworkFence.capture(
                    accountScopeKey: expectedScope,
                    reconnectGeneration: expectedReconnect,
                    roster: projected
                )
                let relationships = try await MobileAgentNetworkHistorySource(bridge: bridge)
                    .loadPartnerIds(roster: projected)
                try Task.checkCancellation()
                guard accessRosterGeneration == expectedGeneration,
                      relationshipFence.matches(
                          accountScopeKey: mobileAccountScopeKey,
                          reconnectGeneration: reconnectGeneration,
                          roster: bots
                      )
                else { return }
                let enriched = bots.map { bot in
                    bot.replacingConversationPartnerIds(relationships[bot.id] ?? bot.conversationPartnerIds)
                }
                bots = enriched
                AccessRosterPersistence.save(enriched, accountScopeKey: expectedScope)
                accessRosterSnapshot = AccessRosterSnapshotProjection.complete(
                    enriched,
                    previous: accessRosterSnapshot
                )
                reconcileRosterSelection(with: enriched, isComplete: true)
            }
        } catch is CancellationError {
            return
        } catch {
            let access: AccessCoverSandAccess
            do {
                let result = try await bridge.request(method: "getSandAccessFresh")
                access = AccessCoverModel.project(foundationValue: result.value)
            } catch {
                access = .unknown
            }
            guard accessRosterGeneration == expectedGeneration,
                  mobileAccountScopeKey == expectedScope,
                  reconnectGeneration == expectedReconnect
            else { return }

            accessCoverAccess = access
            accessRosterSnapshot = AccessRosterSnapshotProjection.fail(
                AccessRosterFailureClassifier.failure(for: error, access: access),
                previous: accessRosterSnapshot
            )
            accessCoverFirstBox = FirstBoxGate.project(
                previous: accessCoverFirstBox,
                roster: firstBoxSnapshot(from: accessRosterSnapshot)
            )
        }
    }

    private func firstBoxSnapshot(
        from roster: AccessRosterSnapshot
    ) -> FirstBoxRosterSnapshot {
        .init(
            loadState: {
                switch roster.loadState {
                case .loading: return .loading
                case .ready: return .ready
                case .error: return .error
                }
            }(),
            isShowingRestoredRoster: roster.isShowingRestoredRoster,
            failureCode: roster.failure?.code,
            failureTransportKind: roster.failure?.transportKind
        )
    }

    @MainActor
    func createBot() async {
        let name = botName.trimmingCharacters(in: .whitespacesAndNewlines)
        let description = botDescription.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty, !botBusy else { return }

        botBusy = true
        botError = nil
        defer { botBusy = false }

        do {
            bots = try await GrokMobileBotService(bridge: bridge).createBot(
                name: name,
                description: description,
                avatarShape: botAvatarShape,
                avatarColor: botAvatarColor
            )
            botName = ""
            botDescription = ""
            botAvatarShape = "wedge"
            botAvatarColor = "cyan"
            createBotOpen = false
            await messaging.refresh()
        } catch {
            botError = error.localizedDescription
        }
    }

    @MainActor
    func beginBotRename(_ bot: MobileBotSummary) {
        guard bot.miniAppId == nil, !botActionBusy else { return }
        botActionError = nil
        botRenameDraft = bot.name
        botRenameTarget = bot
    }

    @MainActor
    func commitBotRename() async {
        guard let bot = botRenameTarget,
              let name = committedMobileBotName(
                initialValue: bot.name,
                draftValue: botRenameDraft
              ),
              !botActionBusy
        else {
            if botRenameTarget != nil,
               committedMobileBotName(
                   initialValue: botRenameTarget?.name ?? "",
                   draftValue: botRenameDraft
               ) == nil
            {
                botRenameTarget = nil
                botRenameDraft = ""
                botActionError = nil
            }
            return
        }

        botActionBusy = true
        botActionError = nil
        defer { botActionBusy = false }

        do {
            bots = try await GrokMobileBotService(bridge: bridge).renameBot(
                id: bot.id,
                name: name
            )
            botRenameTarget = nil
            botRenameDraft = ""
            await messaging.refresh()
        } catch {
            botActionError = "重命名 Bot 失败：\(error.localizedDescription)"
        }
    }

    @MainActor
    func duplicateBot(_ bot: MobileBotSummary) async {
        guard bot.miniAppId == nil, !botActionBusy else { return }
        botActionBusy = true
        botActionError = nil
        defer { botActionBusy = false }

        do {
            bots = try await GrokMobileBotService(bridge: bridge).duplicateBot(id: bot.id)
            await messaging.refresh()
        } catch {
            botActionError = "复制 Bot 失败：\(error.localizedDescription)"
        }
    }

    @MainActor
    func requestBotDelete(_ bot: MobileBotSummary) {
        guard bot.miniAppId == nil, !botActionBusy else { return }
        botActionError = nil
        botDeleteTarget = bot
    }

    @MainActor
    func deleteBot(_ bot: MobileBotSummary) async {
        guard bot.miniAppId == nil, !botActionBusy else { return }
        botActionBusy = true
        botActionError = nil
        defer { botActionBusy = false }

        do {
            let service = GrokMobileBotService(bridge: bridge)
            bots = try await service.deleteBot(id: bot.id)
            botDeleteTarget = nil
            if pinnedBotIds.contains(bot.id) {
                let next = pinnedBotIdOrder.filter { $0 != bot.id }
                do {
                    pinnedBotIdOrder = try await service.setPinnedBotIds(next)
                } catch is CancellationError {
                    return
                } catch {
                    pinnedBotIdOrder = next
                    botActionError = "Bot 已删除，但更新置顶顺序失败：\(error.localizedDescription)"
                }
            }
            await messaging.refresh()
        } catch {
            botActionError = bot.isGroup
                ? "删除群组失败：\(error.localizedDescription)"
                : "删除 Bot 失败：\(error.localizedDescription)"
        }
    }

    var pinnedBotIds: Set<String> {
        Set(pinnedBotIdOrder)
    }

    @MainActor
    func loadPinnedBotIds() async {
        pinnedBotIdOrder = []
        do {
            let ids = try await GrokMobileBotService(bridge: bridge).loadPinnedBotIds()
            try Task.checkCancellation()
            pinnedBotIdOrder = ids
        } catch is CancellationError {
            return
        } catch {
            // Match Desktop: a failed pin read is non-fatal and leaves no
            // account from the previous session projected into the new shell.
        }
    }

    @MainActor
    func toggleBotPin(_ bot: MobileBotSummary) async {
        guard !botActionBusy else { return }
        botActionBusy = true
        botActionError = nil
        defer { botActionBusy = false }

        let next = pinnedBotIds.contains(bot.id)
            ? pinnedBotIdOrder.filter { $0 != bot.id }
            : pinnedBotIdOrder + [bot.id]
        do {
            pinnedBotIdOrder = try await GrokMobileBotService(bridge: bridge)
                .setPinnedBotIds(next)
        } catch is CancellationError {
            return
        } catch {
            botActionError = "更新置顶状态失败：\(error.localizedDescription)"
        }
    }

    @MainActor
    func movePinnedBot(_ bot: MobileBotSummary, offset: Int) async {
        guard !botActionBusy else { return }
        let next = GrokMobileBotService.movedPinnedBotIds(
            pinnedBotIdOrder,
            movedId: bot.id,
            offset: offset
        )
        guard next != pinnedBotIdOrder else { return }

        botActionBusy = true
        botActionError = nil
        defer { botActionBusy = false }
        do {
            pinnedBotIdOrder = try await GrokMobileBotService(bridge: bridge)
                .setPinnedBotIds(next)
        } catch is CancellationError {
            return
        } catch {
            botActionError = "调整置顶顺序失败：\(error.localizedDescription)"
        }
    }

    @MainActor
    func setBotUnread(_ bot: MobileBotSummary, unread: Bool) async {
        guard !bot.isGroup, bot.miniAppId == nil, !botActionBusy else { return }
        botActionBusy = true
        botActionError = nil
        defer { botActionBusy = false }
        do {
            bots = try await GrokMobileBotService(bridge: bridge)
                .setBotUnread(id: bot.id, unread: unread)
        } catch {
            botActionError = "更新未读状态失败：\(error.localizedDescription)"
        }
    }

    @MainActor
    func setBotHidden(_ bot: MobileBotSummary, hidden: Bool) async {
        guard !bot.isGroup,
              bot.miniAppId == nil,
              !hiddenChatsController.isPending(bot.id)
        else { return }

        botActionError = nil
        do {
            try await hiddenChatsController.setAgentHidden(
                agentId: bot.id,
                isHidden: hidden,
                readAgent: { id in bots.first(where: { $0.id == id }) },
                onOptimisticChange: { id, nextHidden in
                    applyHiddenProjection(agentId: id, hidden: nextHidden)
                },
                call: { id, nextHidden in
                    try await GrokMobileBotService(bridge: bridge)
                        .setBotHiddenMutation(id: id, hidden: nextHidden)
                },
                onRollback: { id, _, previousHidden in
                    applyHiddenProjection(agentId: id, hidden: previousHidden)
                }
            )
        } catch {
            if MobileHiddenChatsMutationController.isTransportFailure(error) {
                botActionError = "连接暂时不可用；隐藏状态已保留，将在重连后自动重试。"
            } else {
                botActionError = hidden
                    ? "隐藏 Bot 失败：\(error.localizedDescription)"
                    : "恢复 Bot 失败：\(error.localizedDescription)"
            }
        }
    }

    @MainActor
    func applyHiddenProjection(agentId: String, hidden: Bool) {
        guard let index = bots.firstIndex(where: { $0.id == agentId }) else { return }
        bots[index] = bots[index].replacingHidden(hidden)
    }

    @MainActor
    func retryHeldHiddenChatMutations() {
        hiddenChatsController.noteReconnect(
            call: { id, hidden in
                try await GrokMobileBotService(bridge: bridge)
                    .setBotHiddenMutation(id: id, hidden: hidden)
            },
            onRollback: { id, _, previousHidden in
                applyHiddenProjection(agentId: id, hidden: previousHidden)
                botActionError = "重连后恢复隐藏状态失败，已回滚到服务器状态。"
            }
        )
    }

    @MainActor
    func loadAgentSidebarSections() async {
        agentSidebarSections = []
        let fallback = MobileAgentSidebarSections.loadFallback(
            accountScopeKey: mobileAccountScopeKey
        )
        agentSidebarSections = fallback
        do {
            let result = try await bridge.request(method: "getHostSidebarSections")
            try Task.checkCancellation()
            guard let authoritative = MobileAgentSidebarSections.canonical(from: result.value) else {
                throw NSError(
                    domain: "Fabushi.MobileAgentSidebarSections",
                    code: 1,
                    userInfo: [NSLocalizedDescriptionKey: "Host 返回了无效的分组状态"]
                )
            }
            agentSidebarSections = authoritative
            MobileAgentSidebarSections.persistFallback(
                authoritative,
                accountScopeKey: mobileAccountScopeKey
            )
        } catch is CancellationError {
            return
        } catch {
            // Account-scoped durable fallback remains visible until Host reconnect.
        }
    }

    @MainActor
    func applyAgentSidebarSections(
        _ proposed: [MobileAgentSidebarSection]
    ) async -> Bool {
        guard !botActionBusy else { return false }
        let previous = agentSidebarSections
        let normalized = MobileAgentSidebarSections.normalized(proposed)
        agentSidebarSections = normalized
        botActionBusy = true
        botActionError = nil
        defer { botActionBusy = false }
        do {
            let result = try await bridge.request(
                method: "setHostSidebarSections",
                params: ["sections": MobileAgentSidebarSections.foundationValue(normalized)]
            )
            try Task.checkCancellation()
            guard let authoritative = MobileAgentSidebarSections.canonical(from: result.value) else {
                throw NSError(
                    domain: "Fabushi.MobileAgentSidebarSections",
                    code: 2,
                    userInfo: [NSLocalizedDescriptionKey: "Host 返回了无效的分组状态"]
                )
            }
            agentSidebarSections = authoritative
            MobileAgentSidebarSections.persistFallback(
                authoritative,
                accountScopeKey: mobileAccountScopeKey
            )
            return true
        } catch is CancellationError {
            agentSidebarSections = previous
            return false
        } catch {
            agentSidebarSections = previous
            botActionError = "更新分组失败：\(error.localizedDescription)"
            Task { await loadAgentSidebarSections() }
            return false
        }
    }

    @MainActor
    func assignBot(_ bot: MobileBotSummary, toSection sectionId: String?) async {
        let next = MobileAgentSidebarSections.assigning(
            agentId: bot.id,
            to: sectionId,
            in: agentSidebarSections
        )
        _ = await applyAgentSidebarSections(next)
    }

    @MainActor
    func beginCreateAgentSidebarSection(for bot: MobileBotSummary) {
        newSectionName = ""
        newSectionBot = bot
    }

    @MainActor
    func createAgentSidebarSection(for bot: MobileBotSummary) async {
        guard let next = MobileAgentSidebarSections.creating(
            name: newSectionName,
            with: bot.id,
            in: agentSidebarSections
        ) else { return }
        if await applyAgentSidebarSections(next) {
            newSectionBot = nil
            newSectionName = ""
        }
    }

    @MainActor
    func commitAgentSidebarSectionRename(
        _ section: MobileAgentSidebarSection
    ) async {
        guard let next = MobileAgentSidebarSections.renamed(
            agentSidebarSections,
            sectionId: section.id,
            name: sectionRenameDraft
        ) else { return }
        if await applyAgentSidebarSections(next) {
            sectionRenameTarget = nil
            sectionRenameDraft = ""
        }
    }

    @MainActor
    func deleteAgentSidebarSection(
        _ section: MobileAgentSidebarSection
    ) async {
        let next = MobileAgentSidebarSections.removing(
            agentSidebarSections,
            sectionId: section.id
        )
        if await applyAgentSidebarSections(next) {
            sectionDeleteTarget = nil
        }
    }

    @MainActor
    func moveAgentSidebarSection(
        _ section: MobileAgentSidebarSection,
        offset: Int
    ) async {
        let next = MobileAgentSidebarSections.moving(
            agentSidebarSections,
            sectionId: section.id,
            offset: offset
        )
        guard next != agentSidebarSections else { return }
        _ = await applyAgentSidebarSections(next)
    }

    @MainActor
    func showAsyncTasks(_ bot: MobileBotSummary) {
        guard !bot.isGroup, bot.miniAppId == nil else { return }
        asyncTasksTarget = bot
    }
}
