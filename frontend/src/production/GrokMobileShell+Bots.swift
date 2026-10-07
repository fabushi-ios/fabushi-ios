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
        accessRosterGeneration = accessRosterGeneration == Int.max ? 1 : accessRosterGeneration + 1
        let expectedGeneration = accessRosterGeneration
        let expectedScope = mobileAccountScopeKey
        let expectedReconnect = reconnectGeneration

        restoreRosterSelectionIfNeeded()
        let restored = AccessRosterPersistence.load(accountScopeKey: expectedScope)
        if !restored.isEmpty {
            bots = restored
            accessRosterSnapshot = AccessRosterSnapshotProjection.restore(restored)
            reconcileRosterSelection(with: restored, isComplete: false)
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

            bots = live
            AccessRosterPersistence.save(live, accountScopeKey: expectedScope)
            accessRosterSnapshot = AccessRosterSnapshotProjection.complete(
                live,
                previous: accessRosterSnapshot
            )
            reconcileRosterSelection(with: live, isComplete: true)
            accessCoverAccess = .init(state: .granted, reason: .none)
            accessCoverFirstBox = FirstBoxGate.project(
                previous: accessCoverFirstBox,
                roster: firstBoxSnapshot(from: accessRosterSnapshot)
            )
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
                description: description
            )
            botName = ""
            botDescription = ""
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
        guard !bot.isGroup, bot.miniAppId == nil, !botActionBusy else { return }
        botActionBusy = true
        botActionError = nil
        defer { botActionBusy = false }
        do {
            bots = try await GrokMobileBotService(bridge: bridge)
                .setBotHidden(id: bot.id, hidden: hidden)
        } catch {
            botActionError = hidden
                ? "隐藏 Bot 失败：\(error.localizedDescription)"
                : "恢复 Bot 失败：\(error.localizedDescription)"
        }
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
