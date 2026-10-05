import SwiftUI

internal struct GrokMobileShell: View {
    @Bindable var model: MarketplaceModel
    @Bindable var messaging: MessagingModel
    let bridge: IOSPreloadBridge
    let appAgentSurface: FabushiAppAgentSurface
    @Environment(\.openURL) var openExternalURL

    @State var query = ""
    @State var paletteTab: MobileCommandPaletteTab = .all
    @State var searchOpen = false
    @State var composeOpen = false
    @State var createBotOpen = false
    @State var botName = ""
    @State var botDescription = ""
    @State var botBusy = false
    @State var botError: String?
    @State var botRenameTarget: MobileBotSummary?
    @State var botRenameDraft = ""
    @State var botDeleteTarget: MobileBotSummary?
    @State var botActionBusy = false
    @State var botActionError: String?
    @State var bots: [MobileBotSummary] = []
    @State var selectedBot: MobileBotSummary?
    @State var botDrafts: [String: String] = [:]
    @State var botTranscripts: [String: [MobileChatMessage]] = [:]
    @State var legacyOpen = false
    @State var legacyConversationID: String?
    @State var legacyMessageID: String?
    @State var legacySection: MobileSection?
    @State var commandPaletteRoutines: [MobileCommandPaletteRoutine] = []
    @State var commandPaletteRoutineStatus: MobileCommandPaletteProviderStatus = .idle
    @State var commandPaletteLinkMetadata: [String: MobileCommandPaletteLinkMetadata] = [:]
    @State var commandPaletteLinkStatus: MobileCommandPaletteProviderStatus = .idle

    @ViewBuilder
    var body: some View {
        if model.onboardingStep < 3 || !model.authResolved || !model.loggedIn {
            unauthenticatedContent
        } else if let selectedBot {
            selectedBotContent(selectedBot)
        } else if legacyOpen {
            legacyContent
        } else {
            homeContent
        }
    }

    private var unauthenticatedContent: some View {
        ContentView(model: model, messaging: messaging, appAgentSurface: appAgentSurface)
    }

    private func selectedBotContent(_ bot: MobileBotSummary) -> some View {
        MobileBotChat(
            bot: bot,
            bridge: bridge,
            model: model,
            appAgentSurface: appAgentSurface,
            onClose: { self.selectedBot = nil },
            draft: botDraftBinding(for: bot.id),
            entries: botTranscriptBinding(for: bot.id)
        )
    }

    private func botDraftBinding(for botID: String) -> Binding<String> {
        Binding(
            get: { botDrafts[botID] ?? "" },
            set: { botDrafts[botID] = $0 }
        )
    }

    private func botTranscriptBinding(for botID: String) -> Binding<[MobileChatMessage]> {
        Binding(
            get: { botTranscripts[botID] ?? [] },
            set: { botTranscripts[botID] = $0 }
        )
    }

    private var legacyContent: some View {
        VStack(spacing: 0) {
            HStack {
                Button {
                    legacyConversationID = nil
                    legacyMessageID = nil
                    legacySection = nil
                    legacyOpen = false
                } label: {
                    Image(systemName: "chevron.left")
                        .font(.system(size: 15, weight: .bold))
                        .frame(width: 36, height: 36)
                        .background(.ultraThinMaterial, in: Circle())
                }
                .accessibilityIdentifier("grok-mobile-back")
                Spacer()
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 8)
            .background(.ultraThinMaterial)

            ContentView(
                model: model,
                messaging: messaging,
                appAgentSurface: appAgentSurface,
                initialConversation: legacyConversationID.flatMap { id in
                    messaging.conversations.first(where: { $0.id == id })
                },
                initialMessageID: legacyMessageID,
                initialSection: legacySection
            )
        }
    }

    private var homeContent: some View {
        home
            .task { await loadBots() }
            .task { await messaging.refresh() }
            .task(id: appAgentSurfaceFingerprint) { publishAppAgentSurface() }
    }

    var appAgentSurfaceFingerprint: String {
        var components: [String] = []
        components.append(query)
        components.append(String(searchOpen))
        components.append(paletteTab.rawValue)
        components.append(commandPaletteFingerprint)
        components.append(String(composeOpen))
        components.append(String(createBotOpen))
        components.append(botName)
        components.append(botDescription)
        components.append(String(botBusy))
        components.append(botError ?? "")
        components.append(botRenameTarget?.id ?? "")
        components.append(botRenameDraft)
        components.append(botDeleteTarget?.id ?? "")
        components.append(String(botActionBusy))
        components.append(botActionError ?? "")
        components.append(botRosterFingerprint)
        components.append(conversationFingerprint)
        return components.joined(separator: "|")
    }

    private var botRosterFingerprint: String {
        bots
            .map { bot in
                [bot.id, bot.name, bot.miniAppId ?? ""].joined(separator: ":")
            }
            .joined(separator: ",")
    }

    private var conversationFingerprint: String {
        messaging.conversations
            .map { conversation in
                [
                    conversation.id,
                    String(conversation.unreadCount),
                    String(conversation.isArchived),
                ].joined(separator: ":")
            }
            .joined(separator: ",")
    }
}
