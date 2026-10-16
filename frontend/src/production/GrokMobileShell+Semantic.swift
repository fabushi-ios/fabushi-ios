import SwiftUI

extension GrokMobileShell {
    @MainActor
    func publishAppAgentSurface() {
        var elements: [FabushiAppAgentSurface.Element] = []
        var actions: [String: FabushiAppAgentSurface.Action] = [:]
        func add(
            _ id: String,
            role: String,
            name: String,
            enabled: Bool = true,
            action: FabushiAppAgentSurface.Action? = nil
        ) {
            let normalizedId = Self.semanticId(id)
            elements.append(.init(
                agentId: normalizedId,
                role: String(role.prefix(80)),
                name: String(name.prefix(240)),
                enabled: enabled
            ))
            if let action { actions[normalizedId] = action }
        }

        if let bot = botRenameTarget {
            add("grok-rename-bot", role: "dialog", name: "重命名 \(bot.name)")
            add(
                "rename-bot-name",
                role: "textbox",
                name: "Bot 名称",
                action: .init(allowed: ["setValue"]) { value in botRenameDraft = value ?? "" }
            )
            add(
                "rename-bot-submit",
                role: "button",
                name: botActionBusy ? "正在保存 Bot 名称" : "保存 Bot 名称",
                enabled: !botActionBusy
                    && committedMobileBotName(
                        initialValue: bot.name,
                        draftValue: botRenameDraft
                    ) != nil,
                action: .init(allowed: ["invoke"]) { _ in
                    Task { await commitBotRename() }
                }
            )
            add(
                "rename-bot-cancel",
                role: "button",
                name: "取消重命名 Bot",
                enabled: !botActionBusy,
                action: .init(allowed: ["invoke"]) { _ in
                    botRenameTarget = nil
                    botRenameDraft = ""
                    botActionError = nil
                }
            )
            if botActionError != nil {
                add("rename-bot-error", role: "status", name: "Bot 重命名失败")
            }
            try? appAgentSurface.publish(screen: "grok-rename-bot", elements: elements, actions: actions)
            return
        }

        if let bot = botDeleteTarget {
            add("grok-delete-bot", role: "alertdialog", name: "删除 \(bot.name)")
            add(
                "delete-bot-cancel",
                role: "button",
                name: "取消删除 Bot",
                enabled: !botActionBusy,
                action: .init(allowed: ["invoke"]) { _ in botDeleteTarget = nil }
            )
            add(
                "delete-bot-confirm",
                role: "button",
                name: "永久删除 \(bot.name)",
                enabled: !botActionBusy,
                action: .init(allowed: ["invoke"]) { _ in
                    Task { await deleteBot(bot) }
                }
            )
            try? appAgentSurface.publish(screen: "grok-delete-bot", elements: elements, actions: actions)
            return
        }

        if createBotOpen {
            add("grok-create-bot", role: "dialog", name: "新建 Bot")
            add("new-bot-name", role: "textbox", name: "Bot 名称", action: .init(allowed: ["setValue"]) { value in botName = value ?? "" })
            add("new-bot-description", role: "textbox", name: "Bot 描述", action: .init(allowed: ["setValue"]) { value in botDescription = value ?? "" })
            add(
                "create-bot-submit",
                role: "button",
                name: botBusy ? "正在创建 Bot" : "创建 Bot",
                enabled: !botBusy && !botName.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
                action: .init(allowed: ["invoke"]) { _ in Task { await createBot() } }
            )
            add("create-bot-cancel", role: "button", name: "取消创建 Bot", action: .init(allowed: ["invoke"]) { _ in createBotOpen = false })
            if botError != nil { add("create-bot-error", role: "status", name: "Bot 创建失败") }
            try? appAgentSurface.publish(screen: "grok-create-bot", elements: elements, actions: actions)
            return
        }

        if composeOpen {
            add("grok-compose", role: "dialog", name: "创建")
            add("grok-compose-bot", role: "button", name: "新建 Bot", action: .init(allowed: ["invoke"]) { _ in composeOpen = false; createBotOpen = true })
            add("grok-compose-message", role: "button", name: "新消息", action: .init(allowed: ["invoke"]) { _ in composeOpen = false; legacyOpen = true })
            add("grok-compose-group", role: "button", name: "新建群组", action: .init(allowed: ["invoke"]) { _ in composeOpen = false; legacyOpen = true })
            add("grok-compose-channel", role: "button", name: "新建频道", action: .init(allowed: ["invoke"]) { _ in composeOpen = false; legacyOpen = true })
            add("grok-compose-cancel", role: "button", name: "取消", action: .init(allowed: ["invoke"]) { _ in composeOpen = false })
            try? appAgentSurface.publish(screen: "grok-compose", elements: elements, actions: actions)
            return
        }

        if searchOpen {
            add("grok-command-palette", role: "dialog", name: "Search")
            add(
                "grok-mobile-search-field",
                role: "textbox",
                name: "Search",
                action: .init(allowed: ["setValue"]) { value in query = value ?? "" }
            )
            for tab in MobileCommandPaletteTab.allCases {
                add(
                    "grok-palette-tab-\(tab.rawValue)",
                    role: "tab",
                    name: tab.title,
                    action: .init(allowed: ["invoke"]) { _ in paletteTab = tab }
                )
            }
            if paletteTab == .routines {
                add(
                    "grok-palette-routines-unavailable",
                    role: "status",
                    name: "Routines unavailable until the canonical Host automation roster is exposed to iOS",
                    enabled: false
                )
            }
            for entry in commandPaletteEntries.prefix(100) {
                add(
                    "grok-palette-result-\(entry.accessibilityKey)",
                    role: "button",
                    name: entry.label,
                    action: .init(allowed: ["invoke"]) { _ in activateCommandPaletteEntry(entry) }
                )
            }
            add(
                "grok-palette-close",
                role: "button",
                name: "Close search",
                action: .init(allowed: ["invoke"]) { _ in closeCommandPalette() }
            )
            try? appAgentSurface.publish(screen: "grok-command-palette", elements: elements, actions: actions)
            return
        }

        add("grok-mobile-home", role: "application", name: "Fabushi")
        add("grok-mobile-legacy", role: "button", name: "打开完整消息工作台", action: .init(allowed: ["invoke"]) { _ in legacyOpen = true })
        add("grok-mobile-search", role: "button", name: "打开搜索", action: .init(allowed: ["invoke"]) { _ in
            toggleCommandPalette()
        })
        add("grok-mobile-search-field", role: "textbox", name: "搜索", action: .init(allowed: ["setValue"]) { value in
            searchOpen = true
            query = value ?? ""
        })
        add("grok-mobile-add", role: "button", name: "创建", action: .init(allowed: ["invoke"]) { _ in composeOpen = true })
        add("grok-bot-mahayana-assistant", role: "button", name: "Mahayana", action: .init(allowed: ["invoke"]) { _ in selectedBot = MobileBotSummary(id: "mahayana-assistant", name: "Mahayana", description: "Ready to help") })
        for bot in filteredBots.prefix(100) {
            add("grok-bot-\(bot.id)", role: "button", name: bot.name, action: .init(allowed: ["invoke"]) { _ in selectedBot = bot })
            if bot.miniAppId == nil {
                add(
                    "grok-bot-rename-\(bot.id)",
                    role: "button",
                    name: "重命名 \(bot.name)",
                    enabled: !botActionBusy,
                    action: .init(allowed: ["invoke"]) { _ in beginBotRename(bot) }
                )
                add(
                    "grok-bot-duplicate-\(bot.id)",
                    role: "button",
                    name: "复制 \(bot.name)",
                    enabled: !botActionBusy,
                    action: .init(allowed: ["invoke"]) { _ in Task { await duplicateBot(bot) } }
                )
                add(
                    "grok-bot-delete-\(bot.id)",
                    role: "button",
                    name: "删除 \(bot.name)",
                    enabled: !botActionBusy,
                    action: .init(allowed: ["invoke"]) { _ in requestBotDelete(bot) }
                )
            }
        }
        for conversation in filteredConversations.prefix(100) {
            add(
                "grok-conversation-\(conversation.id)",
                role: "button",
                name: conversation.title,
                action: .init(allowed: ["invoke"]) { _ in openLegacyConversation(conversation.id) }
            )
        }
        try? appAgentSurface.publish(screen: "grok-home", elements: elements, actions: actions)
    }

    static func semanticId(_ value: String) -> String {
        String(value.map { character in
            character.isASCII && (character.isLetter || character.isNumber || "._:/@-".contains(character)) ? character : "-"
        }.prefix(200))
    }
}
