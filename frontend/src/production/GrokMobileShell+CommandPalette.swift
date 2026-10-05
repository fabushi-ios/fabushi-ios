import SwiftUI

extension GrokMobileShell {
    var commandPaletteActions: [MobileCommandPaletteAction] {
        [
            .init(
                id: "create-bot",
                label: "New Bot",
                keywords: ["new", "create", "bot", "agent"],
                detail: "Actions",
                kind: .createBot
            ),
            .init(
                id: "open-workspace",
                label: "Open Full Messaging",
                keywords: ["messages", "settings", "channels", "contacts", "workspace"],
                detail: "Views",
                kind: .openWorkspace
            ),
        ]
    }

    var commandPaletteEntries: [MobileCommandPaletteEntry] {
        let board = MobileBotSummary(
            id: "mahayana-assistant",
            name: "Mahayana",
            description: "Ready to help"
        )
        return GrokMobileCommandPaletteModel.entries(
            bots: [board] + bots,
            conversations: messaging.conversations,
            messagesByConversation: messaging.messagesByConversation,
            actions: commandPaletteActions,
            query: query,
            tab: paletteTab
        )
    }

    var commandPaletteFingerprint: String {
        guard searchOpen else { return "" }
        return commandPaletteEntries
            .prefix(100)
            .map { $0.id + ":" + $0.label }
            .joined(separator: ",")
    }

    var commandPaletteContent: some View {
        VStack(alignment: .leading, spacing: 10) {
            TextField("Search", text: $query)
                .textFieldStyle(.plain)
                .padding(12)
                .background(.white, in: RoundedRectangle(cornerRadius: 14))
                .accessibilityIdentifier("grok-mobile-search-field")

            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 8) {
                    ForEach(MobileCommandPaletteTab.allCases) { tab in
                        Button {
                            paletteTab = tab
                        } label: {
                            Text(tab.title)
                                .font(.caption.weight(.semibold))
                                .foregroundStyle(paletteTab == tab ? .white : .primary)
                                .padding(.horizontal, 11)
                                .padding(.vertical, 7)
                                .background(
                                    paletteTab == tab ? Color.black : Color.black.opacity(0.06),
                                    in: Capsule()
                                )
                        }
                        .buttonStyle(.plain)
                        .accessibilityIdentifier("grok-palette-tab-\(tab.rawValue)")
                    }
                }
            }

            if paletteTab == .routines {
                VStack(alignment: .leading, spacing: 4) {
                    Text("Routines are not available on this iOS runtime yet.")
                        .font(.subheadline.weight(.semibold))
                    Text("The palette stays fail-closed until the canonical Host automation roster is exposed to iOS.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                .padding(.vertical, 10)
                .accessibilityIdentifier("grok-palette-routines-unavailable")
            } else if commandPaletteEntries.isEmpty {
                Text(query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? "No items" : "No results")
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                    .padding(.vertical, 12)
                    .accessibilityIdentifier("grok-palette-empty")
            } else {
                LazyVStack(spacing: 2) {
                    ForEach(commandPaletteEntries) { entry in
                        commandPaletteRow(entry)
                    }
                }
            }
        }
        .padding(.horizontal, 16)
        .padding(.top, 18)
        .padding(.bottom, 24)
        .accessibilityIdentifier("grok-command-palette")
    }

    func commandPaletteRow(_ entry: MobileCommandPaletteEntry) -> some View {
        Button {
            activateCommandPaletteEntry(entry)
        } label: {
            HStack(spacing: 12) {
                Image(systemName: entry.systemImage)
                    .font(.system(size: 16, weight: .semibold))
                    .frame(width: 30, height: 30)
                    .background(Color.black.opacity(0.05), in: RoundedRectangle(cornerRadius: 9))
                VStack(alignment: .leading, spacing: 2) {
                    Text(entry.label)
                        .font(.system(size: 16, weight: .semibold))
                        .foregroundStyle(.primary)
                        .lineLimit(1)
                    Text(entry.detail)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
                Spacer(minLength: 8)
                Image(systemName: "chevron.right")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(.tertiary)
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 8)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("grok-palette-result-\(entry.accessibilityKey)")
    }

    @MainActor
    func toggleCommandPalette() {
        searchOpen.toggle()
        if searchOpen {
            paletteTab = .all
        } else {
            query = ""
        }
    }

    @MainActor
    func closeCommandPalette() {
        searchOpen = false
        query = ""
        paletteTab = .all
    }

    @MainActor
    func openLegacyConversation(_ conversationId: String) {
        legacyConversationID = conversationId
        legacyOpen = true
        closeCommandPalette()
    }

    @MainActor
    func activateCommandPaletteEntry(_ entry: MobileCommandPaletteEntry) {
        switch entry {
        case .bot(let bot):
            selectedBot = bot
            closeCommandPalette()
        case .conversation(let conversation):
            openLegacyConversation(conversation.id)
        case .message(let message):
            openLegacyConversation(message.conversationId)
        case .file(let file):
            openLegacyConversation(file.conversationId)
        case .link(let link):
            guard let url = URL(string: link.url) else { return }
            closeCommandPalette()
            openExternalURL(url)
        case .action(let action):
            closeCommandPalette()
            switch action.kind {
            case .createBot:
                createBotOpen = true
            case .openWorkspace:
                legacyConversationID = nil
                legacyOpen = true
            }
        }
    }
}
