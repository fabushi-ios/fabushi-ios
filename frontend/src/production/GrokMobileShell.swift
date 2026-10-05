import SwiftUI

internal struct GrokMobileShell: View {
    @Bindable var model: MarketplaceModel
    @Bindable var messaging: MessagingModel
    let bridge: IOSPreloadBridge
    let appAgentSurface: FabushiAppAgentSurface

    @State var query = ""
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

    var body: some View {
        if model.onboardingStep < 3 || !model.authResolved || !model.loggedIn {
            ContentView(model: model, messaging: messaging, appAgentSurface: appAgentSurface)
        } else if let selectedBot {
            MobileBotChat(
                bot: selectedBot,
                bridge: bridge,
                model: model,
                appAgentSurface: appAgentSurface,
                onClose: { self.selectedBot = nil },
                draft: Binding(
                    get: { botDrafts[selectedBot.id] ?? "" },
                    set: { botDrafts[selectedBot.id] = $0 }
                ),
                entries: Binding(
                    get: { botTranscripts[selectedBot.id] ?? [] },
                    set: { botTranscripts[selectedBot.id] = $0 }
                )
            )
        } else if legacyOpen {
            VStack(spacing: 0) {
                HStack {
                    Button { legacyOpen = false } label: {
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

                ContentView(model: model, messaging: messaging, appAgentSurface: appAgentSurface)
            }
        } else {
            home
                .task { await loadBots() }
                .task { await messaging.refresh() }
                .task(id: appAgentSurfaceFingerprint) { publishAppAgentSurface() }
        }
    }

    var appAgentSurfaceFingerprint: String {
        [
            query,
            String(searchOpen),
            String(composeOpen),
            String(createBotOpen),
            botName,
            botDescription,
            String(botBusy),
            botError ?? "",
            botRenameTarget?.id ?? "",
            botRenameDraft,
            botDeleteTarget?.id ?? "",
            String(botActionBusy),
            botActionError ?? "",
            bots.map { "\($0.id):\($0.name):\($0.miniAppId ?? "")" }.joined(separator: ","),
            messaging.conversations.map { "\($0.id):\($0.unreadCount):\($0.isArchived)" }.joined(separator: ","),
        ].joined(separator: "|")
    }
}
