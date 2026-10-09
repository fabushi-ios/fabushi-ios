import SwiftUI

internal enum MobileGlobalShortcutAction: Hashable, Sendable {
    case commandPalette
    case focusSearch
    case newAgent
    case openSettings
    case openTools
    case previousAgent
    case nextAgent
    case navigateBack
    case focusAgent(Int)
    case escape
    case focusPrompt
}

internal struct MobileGlobalShortcutSpec: Identifiable {
    let id: String
    let action: MobileGlobalShortcutAction
    let label: String
    let keyName: String
    let command: Bool
    let option: Bool
    let shift: Bool

    init(
        id: String,
        action: MobileGlobalShortcutAction,
        label: String,
        keyName: String,
        command: Bool = false,
        option: Bool = false,
        shift: Bool = false
    ) {
        self.id = id
        self.action = action
        self.label = label
        self.keyName = keyName
        self.command = command
        self.option = option
        self.shift = shift
    }

    var hotkeyDescription: String {
        var parts: [String] = []
        if command { parts.append("cmd") }
        if option { parts.append("alt") }
        if shift { parts.append("shift") }
        parts.append(keyName)
        return parts.joined(separator: "+")
    }

    var modifiers: EventModifiers {
        var result: EventModifiers = []
        if command { result.insert(.command) }
        if option { result.insert(.option) }
        if shift { result.insert(.shift) }
        return result
    }

    var keyEquivalent: KeyEquivalent {
        switch keyName {
        case "up":
            return .upArrow
        case "down":
            return .downArrow
        case "escape":
            return .escape
        default:
            return KeyEquivalent(Character(keyName))
        }
    }
}

internal enum MobileHardwareKeyboardContract {
    static let rootSpecs: [MobileGlobalShortcutSpec] = {
        var items: [MobileGlobalShortcutSpec] = [
            .init(
                id: "sand.newAgent",
                action: .newAgent,
                label: "New Bot",
                keyName: "n",
                command: true
            ),
            .init(
                id: "sand.commandPalette",
                action: .commandPalette,
                label: "Jump to",
                keyName: "k",
                command: true
            ),
            .init(
                id: "sand.openSettings",
                action: .openSettings,
                label: "Open settings",
                keyName: ",",
                command: true
            ),
            .init(
                id: "sand.openTools",
                action: .openTools,
                label: "Customize",
                keyName: "m",
                command: true,
                shift: true
            ),
            .init(
                id: "sand.focusSearch",
                action: .focusSearch,
                label: "Search agents",
                keyName: "f",
                command: true,
                shift: true
            ),
            .init(
                id: "sand.previousAgent",
                action: .previousAgent,
                label: "Previous agent",
                keyName: "up",
                option: true
            ),
            .init(
                id: "sand.nextAgent",
                action: .nextAgent,
                label: "Next agent",
                keyName: "down",
                option: true
            ),
            .init(
                id: "sand.navigateBack",
                action: .navigateBack,
                label: "Back",
                keyName: "[",
                command: true
            ),
        ]

        for index in 1...9 {
            items.append(
                .init(
                    id: "sand.focusAgent\(index)",
                    action: .focusAgent(index),
                    label: "Focus sidebar agent \(index)",
                    keyName: String(index),
                    command: true
                )
            )
        }

        items.append(
            .init(
                id: "sand.escape",
                action: .escape,
                label: "Close",
                keyName: "escape"
            )
        )
        return items
    }()

    static let promptSpecs: [MobileGlobalShortcutSpec] = [
        .init(
            id: "sand.focusInput.i",
            action: .focusPrompt,
            label: "Focus prompt",
            keyName: "i",
            command: true
        ),
        .init(
            id: "sand.focusInput.l",
            action: .focusPrompt,
            label: "Focus prompt",
            keyName: "l",
            command: true
        ),
    ]

    // Desktop also exposes mod+] for browser-style forward navigation. The
    // shipping iOS shell has no forward stack and must not invent a second
    // navigation owner solely for a keyboard command.
    static let platformNotApplicableHotkeys = ["cmd+]", "cmd+b"]
}

internal struct MobileHardwareKeyboardShortcutLayer: View {
    let specs: [MobileGlobalShortcutSpec]
    let perform: (MobileGlobalShortcutAction) -> Void

    var body: some View {
        ZStack {
            ForEach(specs) { spec in
                Button(spec.label) {
                    perform(spec.action)
                }
                .keyboardShortcut(spec.keyEquivalent, modifiers: spec.modifiers)
                .buttonStyle(.plain)
                .frame(width: 1, height: 1)
                .opacity(0.001)
                .allowsHitTesting(false)
                .accessibilityHidden(true)
            }
        }
        .frame(width: 1, height: 1)
        .accessibilityHidden(true)
    }
}

internal enum MobileHardwareKeyboardProjection {
    static func navigableAgents(_ bots: [MobileBotSummary]) -> [MobileBotSummary] {
        bots.filter { !$0.hidden && !$0.isGroup && $0.miniAppId == nil }
    }

    static func target(
        for action: MobileGlobalShortcutAction,
        currentAgentID: String?,
        bots: [MobileBotSummary]
    ) -> MobileBotSummary? {
        let agents = navigableAgents(bots)
        guard !agents.isEmpty else { return nil }

        switch action {
        case .focusAgent(let oneBasedIndex):
            let index = oneBasedIndex - 1
            guard agents.indices.contains(index) else { return nil }
            return agents[index]

        case .previousAgent:
            guard let currentAgentID,
                  let currentIndex = agents.firstIndex(where: { $0.id == currentAgentID })
            else {
                return agents.last
            }
            return agents[(currentIndex - 1 + agents.count) % agents.count]

        case .nextAgent:
            guard let currentAgentID,
                  let currentIndex = agents.firstIndex(where: { $0.id == currentAgentID })
            else {
                return agents.first
            }
            return agents[(currentIndex + 1) % agents.count]

        default:
            return nil
        }
    }
}

extension GrokMobileShell {
    @ViewBuilder
    var rootHardwareKeyboardShortcuts: some View {
        MobileHardwareKeyboardShortcutLayer(
            specs: MobileHardwareKeyboardContract.rootSpecs
                + (selectedBot == nil ? [] : MobileHardwareKeyboardContract.promptSpecs),
            perform: performGlobalHardwareShortcut
        )
    }

    @MainActor
    func performGlobalHardwareShortcut(_ action: MobileGlobalShortcutAction) {
        guard model.onboardingStep >= 3, model.authResolved, model.loggedIn else { return }

        switch action {
        case .commandPalette:
            if selectedBot == nil, !legacyOpen, searchOpen {
                closeCommandPalette()
            } else {
                let scopedAgentID = selectedBot?.id ?? rosterSelection.currentAgentID
                returnToHomeForHardwareShortcut()
                setCommandPaletteComputerScope(agentID: scopedAgentID)
                searchOpen = true
                paletteTab = .all
            }

        case .focusSearch:
            returnToHomeForHardwareShortcut()
            query = ""
            searchOpen = true
            paletteTab = .agents

        case .newAgent:
            returnToHomeForHardwareShortcut()
            searchOpen = false
            query = ""
            botName = "New chat"
            botDescription = ""
            botAvatarShape = "wedge"
            botAvatarColor = "cyan"
            botError = nil
            createBotOpen = true

        case .openSettings:
            returnToHomeForHardwareShortcut()
            openLegacySection(.settings)

        case .openTools:
            returnToHomeForHardwareShortcut()
            openLegacySection(.miniapps)

        case .previousAgent, .nextAgent, .focusAgent(_):
            guard let target = MobileHardwareKeyboardProjection.target(
                for: action,
                currentAgentID: selectedBot?.id ?? rosterSelection.currentAgentID,
                bots: bots
            ) else { return }
            legacyOpen = false
            closeCommandPalette()
            selectBotForConversation(target)

        case .navigateBack, .escape:
            closeTopLevelSurfaceForHardwareShortcut()

        case .focusPrompt:
            guard selectedBot != nil else { return }
            promptFocusGeneration &+= 1
        }
    }

    @MainActor
    private func returnToHomeForHardwareShortcut() {
        groupMembersTarget = nil
        botSettingsTarget = nil
        botSettingsRoutineID = nil
        agentNetworkOpen = false
        legacyConversationID = nil
        legacyMessageID = nil
        legacySection = nil
        legacyOpen = false
        if selectedBot != nil {
            clearRosterSelection()
        }
    }

    @MainActor
    private func closeTopLevelSurfaceForHardwareShortcut() {
        if searchOpen {
            closeCommandPalette()
            return
        }
        if selectedBot != nil {
            clearRosterSelection()
            return
        }
        if legacyOpen {
            legacyConversationID = nil
            legacyMessageID = nil
            legacySection = nil
            legacyOpen = false
            return
        }
        if agentNetworkOpen {
            agentNetworkOpen = false
        }
    }
}
