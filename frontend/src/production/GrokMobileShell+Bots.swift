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
            bots = try await GrokMobileBotService(bridge: bridge).deleteBot(id: bot.id)
            botDeleteTarget = nil
            await messaging.refresh()
        } catch {
            botActionError = bot.isGroup
                ? "删除群组失败：\(error.localizedDescription)"
                : "删除 Bot 失败：\(error.localizedDescription)"
        }
    }
}
