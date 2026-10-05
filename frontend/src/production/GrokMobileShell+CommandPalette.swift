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
                keywords: ["messages", "workspace"],
                detail: "Views",
                kind: .openWorkspace
            ),
            .init(
                id: "open-contacts",
                label: "Members / Contacts",
                keywords: ["members", "contacts", "people"],
                detail: "Views",
                kind: .openContacts
            ),
            .init(
                id: "open-channels",
                label: "Channels",
                keywords: ["channels", "groups"],
                detail: "Views",
                kind: .openChannels
            ),
            .init(
                id: "open-settings",
                label: "Chat Settings",
                keywords: ["chat", "settings", "preferences"],
                detail: "Views",
                kind: .openSettings
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
            routines: commandPaletteRoutines,
            linkMetadata: commandPaletteLinkMetadata,
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

            if paletteTab == .routines && commandPaletteRoutineStatus == .loading && commandPaletteRoutines.isEmpty {
                ProgressView("Loading routines…")
                    .padding(.vertical, 10)
                    .accessibilityIdentifier("grok-palette-routines-loading")
            } else if paletteTab == .routines && commandPaletteRoutineStatus == .unavailable {
                Text("Routines are unavailable because the canonical Host roster is not available.")
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
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
        .task(id: commandPaletteProviderKey) {
            await refreshCommandPaletteProviders()
        }
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
    func openLegacyConversation(_ conversationId: String, messageId: String? = nil) {
        legacyConversationID = conversationId
        legacyMessageID = messageId
        legacySection = nil
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
            openLegacyConversation(message.conversationId, messageId: message.messageId)
        case .file(let file):
            openLegacyConversation(file.conversationId, messageId: file.messageId)
        case .link(let link):
            guard let url = URL(string: link.url) else { return }
            closeCommandPalette()
            openExternalURL(url)
        case .routine(let routine):
            if let bot = bots.first(where: { $0.id == routine.agentId }) {
                selectedBot = bot
                closeCommandPalette()
            } else if messaging.conversations.contains(where: { $0.id == routine.agentId }) {
                openLegacyConversation(routine.agentId)
            } else {
                model.message = "Routine owner is no longer available."
                closeCommandPalette()
            }
        case .action(let action):
            closeCommandPalette()
            switch action.kind {
            case .createBot:
                createBotOpen = true
            case .openWorkspace:
                openLegacySection(nil)
            case .openContacts:
                openLegacySection(.contacts)
            case .openChannels:
                openLegacySection(.channels)
            case .openSettings:
                openLegacySection(.settings)
            }
        }
    }


    @MainActor
    func openLegacySection(_ section: MobileSection?) {
        legacyConversationID = nil
        legacyMessageID = nil
        legacySection = section
        legacyOpen = true
        closeCommandPalette()
    }

    var commandPaletteProviderKey: String {
        [
            paletteTab.rawValue,
            query,
            String(searchOpen),
            messaging.messagesByConversation
                .sorted { $0.key < $1.key }
                .map { key, messages in "\(key):\(messages.count):\(messages.last?.id ?? "")" }
                .joined(separator: ","),
        ].joined(separator: "|")
    }

    @MainActor
    func refreshCommandPaletteProviders() async {
        guard searchOpen else {
            commandPaletteRoutineStatus = .idle
            commandPaletteLinkStatus = .idle
            return
        }

        if paletteTab == .routines || paletteTab == .all {
            await refreshCommandPaletteRoutines()
        }
        guard !Task.isCancelled else { return }
        if paletteTab == .links || paletteTab == .all {
            await refreshCommandPaletteLinkMetadata()
        }
    }

    @MainActor
    func refreshCommandPaletteRoutines() async {
        commandPaletteRoutineStatus = .loading
        do {
            let result = try await bridge.listAllAutomations()
            try Task.checkCancellation()
            let routines = GrokMobileCommandPaletteModel.routines(from: result.value)
            commandPaletteRoutines = routines
            commandPaletteRoutineStatus = routines.isEmpty ? .empty : .ready
        } catch is CancellationError {
            commandPaletteRoutineStatus = .cancelled
        } catch {
            commandPaletteRoutines = []
            commandPaletteRoutineStatus = commandPaletteProviderUnavailable(error) ? .unavailable : .failed(error.localizedDescription)
        }
    }

    @MainActor
    func refreshCommandPaletteLinkMetadata() async {
        let urls = Array(
            GrokMobileCommandPaletteModel.links(
                conversations: messaging.conversations,
                messagesByConversation: messaging.messagesByConversation
            )
            .map(\.url)
            .prefix(50)
        )
        guard !urls.isEmpty else {
            commandPaletteLinkMetadata = [:]
            commandPaletteLinkStatus = .empty
            return
        }

        commandPaletteLinkStatus = .loading
        var next = commandPaletteLinkMetadata.filter { urls.contains($0.key) }
        do {
            for url in urls where next[url] == nil {
                try Task.checkCancellation()
                do {
                    let result = try await bridge.getLinkMetadata(url: url)
                    try Task.checkCancellation()
                    if let metadata = GrokMobileCommandPaletteModel.linkMetadata(from: result.value) {
                        next[url] = metadata
                    }
                } catch is CancellationError {
                    throw CancellationError()
                } catch {
                    if commandPaletteProviderUnavailable(error) {
                        commandPaletteLinkMetadata = [:]
                        commandPaletteLinkStatus = .unavailable
                        return
                    }
                }
            }
            commandPaletteLinkMetadata = next
            commandPaletteLinkStatus = next.isEmpty ? .empty : .ready
        } catch is CancellationError {
            commandPaletteLinkStatus = .cancelled
        } catch {
            commandPaletteLinkStatus = .failed(error.localizedDescription)
        }
    }

    func commandPaletteProviderUnavailable(_ error: Error) -> Bool {
        let message = error.localizedDescription.lowercased()
        return message.contains("unknown method")
            || message.contains("capability-unavailable")
            || message.contains("no coordinator method")
    }

}
