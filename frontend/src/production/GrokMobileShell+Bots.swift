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
        bots = await GrokMobileBotService(bridge: bridge).loadBots()
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
    func assignBot(_ bot: MobileBotSummary, toSection sectionId: String?) {
        agentSidebarSections = MobileAgentSidebarSections.assigning(
            agentId: bot.id,
            to: sectionId,
            in: agentSidebarSections
        )
        MobileAgentSidebarSections.persist(
            agentSidebarSections,
            accountScopeKey: mobileAccountScopeKey
        )
    }

    @MainActor
    func beginCreateAgentSidebarSection(for bot: MobileBotSummary) {
        newSectionName = ""
        newSectionBot = bot
    }

    @MainActor
    func createAgentSidebarSection(for bot: MobileBotSummary) {
        guard let next = MobileAgentSidebarSections.creating(
            name: newSectionName,
            with: bot.id,
            in: agentSidebarSections
        ) else { return }
        agentSidebarSections = next
        MobileAgentSidebarSections.persist(
            next,
            accountScopeKey: mobileAccountScopeKey
        )
        newSectionBot = nil
        newSectionName = ""
    }

    @MainActor
    func showAsyncTasks(_ bot: MobileBotSummary) async {
        guard !bot.isGroup, bot.miniAppId == nil else { return }
        asyncTasksTarget = bot
        asyncTasks = []
        asyncTasksError = nil
        asyncTasksBusy = true
        defer { asyncTasksBusy = false }
        do {
            asyncTasks = try await GrokMobileBotService(bridge: bridge)
                .asyncTasks(agentId: bot.id)
        } catch {
            asyncTasksError = "加载异步任务失败：\(error.localizedDescription)"
        }
    }
}
