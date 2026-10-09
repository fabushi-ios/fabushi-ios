import Foundation

enum AccessRosterLoadState: String, Codable, Equatable, Sendable {
    case loading
    case ready
    case error
}

enum AccessRosterTransportState: String, Codable, Equatable, Sendable {
    case connecting
    case connected
    case down
}

struct AccessRosterFailure: Equatable, Sendable {
    let code: String
    let message: String?
    let transportKind: String?
}

struct AccessRosterReadiness: Equatable, Sendable {
    let accountScopeKey: String?
    let isAccountBound: Bool
    let isConnected: Bool
    let isLoaded: Bool
    let hasReachedBox: Bool
    let hasSelectedAgent: Bool
    let isSelectionReady: Bool
    let rosterFailureCode: String?
    let rosterFailureTransportKind: String?
    let isShowingRestoredRoster: Bool
    let isPrivacyBlocked: Bool
}

struct AccessRosterSnapshot: Equatable, Sendable {
    let bots: [MobileBotSummary]
    let hasCompleteRoster: Bool
    let isShowingRestoredRoster: Bool
    let loadState: AccessRosterLoadState
    let failure: AccessRosterFailure?
    let isFetching: Bool
    let confirmedFetches: Int
    let transport: AccessRosterTransportState

    static let initial = Self(
        bots: [],
        hasCompleteRoster: false,
        isShowingRestoredRoster: false,
        loadState: .loading,
        failure: nil,
        isFetching: false,
        confirmedFetches: 0,
        transport: .connecting
    )
}

enum AccessRosterReadinessProjection {
    static func select(
        accountScopeKey: String?,
        roster: AccessRosterSnapshot,
        selectedAgentID: String?,
        isPrivacyBlocked: Bool
    ) -> AccessRosterReadiness {
        let normalizedAccount = accountScopeKey?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let account = normalizedAccount?.isEmpty == false ? normalizedAccount : nil
        let isAccountBound = account != nil
        let agentIDs = roster.bots.map(\.id)
        let hasLoadedAgents = roster.hasCompleteRoster || !roster.bots.isEmpty
        let hasSelectedAgent = isAccountBound
            && selectedAgentID != nil
            && agentIDs.contains(selectedAgentID!)
        let isLoaded = isAccountBound
            && hasLoadedAgents
            && roster.loadState == .ready
        let isConnected = isAccountBound && roster.transport == .connected

        return .init(
            accountScopeKey: account,
            isAccountBound: isAccountBound,
            isConnected: isConnected,
            isLoaded: isLoaded,
            hasReachedBox: isAccountBound && hasLoadedAgents,
            hasSelectedAgent: hasSelectedAgent,
            isSelectionReady: isLoaded && isConnected && hasSelectedAgent,
            rosterFailureCode: roster.failure?.code,
            rosterFailureTransportKind: roster.failure?.transportKind,
            isShowingRestoredRoster: roster.isShowingRestoredRoster,
            isPrivacyBlocked: isPrivacyBlocked
        )
    }
}

enum AccessRosterSnapshotProjection {
    static func beginFetch(_ previous: AccessRosterSnapshot) -> AccessRosterSnapshot {
        .init(
            bots: previous.bots,
            hasCompleteRoster: previous.hasCompleteRoster,
            isShowingRestoredRoster: previous.isShowingRestoredRoster,
            loadState: previous.bots.isEmpty ? .loading : .ready,
            failure: previous.failure,
            isFetching: true,
            confirmedFetches: previous.confirmedFetches,
            transport: previous.transport
        )
    }

    static func restore(_ bots: [MobileBotSummary]) -> AccessRosterSnapshot {
        .init(
            bots: bots,
            hasCompleteRoster: false,
            isShowingRestoredRoster: !bots.isEmpty,
            loadState: bots.isEmpty ? .loading : .ready,
            failure: nil,
            isFetching: false,
            confirmedFetches: 0,
            transport: .connecting
        )
    }

    static func complete(
        _ bots: [MobileBotSummary],
        previous: AccessRosterSnapshot
    ) -> AccessRosterSnapshot {
        .init(
            bots: bots,
            hasCompleteRoster: true,
            isShowingRestoredRoster: false,
            loadState: .ready,
            failure: nil,
            isFetching: false,
            confirmedFetches: previous.confirmedFetches + 1,
            transport: .connected
        )
    }

    static func fail(
        _ failure: AccessRosterFailure,
        previous: AccessRosterSnapshot
    ) -> AccessRosterSnapshot {
        .init(
            bots: previous.bots,
            hasCompleteRoster: previous.hasCompleteRoster,
            isShowingRestoredRoster: previous.isShowingRestoredRoster,
            loadState: previous.bots.isEmpty ? .error : .ready,
            failure: failure,
            isFetching: false,
            confirmedFetches: previous.confirmedFetches,
            transport: failure.transportKind == nil ? previous.transport : .down
        )
    }
}

enum AccessRosterPersistence {
    static let schemaVersion = 3
    private static let keyPrefix = "fabushi.ios.roster.last-roster.v3:"

    private struct Envelope: Codable {
        let schemaVersion: Int
        let rows: [Row]
    }

    private struct Row: Codable {
        let id: String
        let name: String
        let description: String
        let title: String?
        let notifyOnUpdatesEnabled: Bool
        let hidden: Bool
        let unread: Bool
        let conversationId: String?
        let lastMessagePreview: String?
        let updatedAtMs: Int64?
        let isComposingMessage: Bool
        let waitingReason: String?
        let isRunning: Bool
        let draftPrompt: String?
        let miniAppId: String?
        let menuButtonText: String?
        let isGroup: Bool
        let memberIds: [String]
        let conversationPartnerIds: [String]
        let isSharedRoom: Bool

        init(_ bot: MobileBotSummary) {
            id = bot.id
            name = bot.name
            description = bot.description
            title = bot.title
            notifyOnUpdatesEnabled = bot.notifyOnUpdatesEnabled
            hidden = bot.hidden
            unread = bot.unread
            conversationId = bot.conversationId
            lastMessagePreview = bot.lastMessagePreview
            updatedAtMs = bot.updatedAtMs
            isComposingMessage = bot.isComposingMessage
            waitingReason = bot.waitingReason
            isRunning = bot.isRunning
            draftPrompt = bot.draftPrompt
            miniAppId = bot.miniAppId
            menuButtonText = bot.menuButtonText
            isGroup = bot.isGroup
            memberIds = bot.memberIds
            conversationPartnerIds = bot.conversationPartnerIds
            isSharedRoom = bot.isSharedRoom
        }

        var bot: MobileBotSummary {
            .init(
                id: id,
                name: name,
                description: description,
                title: title,
                notifyOnUpdatesEnabled: notifyOnUpdatesEnabled,
                hidden: hidden,
                unread: unread,
                conversationId: conversationId,
                lastMessagePreview: lastMessagePreview,
                updatedAtMs: updatedAtMs,
                isComposingMessage: isComposingMessage,
                waitingReason: waitingReason,
                isRunning: isRunning,
                draftPrompt: draftPrompt,
                miniAppId: miniAppId,
                menuButtonText: menuButtonText,
                isGroup: isGroup,
                memberIds: memberIds,
                conversationPartnerIds: conversationPartnerIds,
                isSharedRoom: isSharedRoom
            )
        }
    }

    static func load(accountScopeKey: String, defaults: UserDefaults = .standard) -> [MobileBotSummary] {
        guard !accountScopeKey.isEmpty,
              let data = defaults.data(forKey: key(accountScopeKey)),
              let envelope = try? JSONDecoder().decode(Envelope.self, from: data),
              envelope.schemaVersion == schemaVersion
        else { return [] }

        var ids = Set<String>()
        var bots: [MobileBotSummary] = []
        for row in envelope.rows {
            guard !row.id.isEmpty, ids.insert(row.id).inserted else { return [] }
            bots.append(row.bot)
        }
        return bots
    }

    static func save(
        _ bots: [MobileBotSummary],
        accountScopeKey: String,
        defaults: UserDefaults = .standard
    ) {
        guard !accountScopeKey.isEmpty else { return }
        let envelope = Envelope(schemaVersion: schemaVersion, rows: bots.map(Row.init))
        guard let data = try? JSONEncoder().encode(envelope) else { return }
        defaults.set(data, forKey: key(accountScopeKey))
    }

    static func clear(accountScopeKey: String, defaults: UserDefaults = .standard) {
        defaults.removeObject(forKey: key(accountScopeKey))
    }

    private static func key(_ accountScopeKey: String) -> String {
        keyPrefix + Data(accountScopeKey.utf8).base64EncodedString()
    }
}

struct AccessRosterSelectionState: Equatable, Sendable {
    let currentAgentID: String?
    let isLoadPending: Bool

    static let empty = Self(currentAgentID: nil, isLoadPending: false)
}

enum AccessRosterSelectionPersistence {
    static let schemaVersion = 1
    private static let keyPrefix = "fabushi.ios.roster.selection.last-agent.v1:"

    private struct Envelope: Codable {
        let schemaVersion: Int
        let agentID: String
    }

    static func load(
        accountScopeKey: String,
        defaults: UserDefaults = .standard
    ) -> AccessRosterSelectionState {
        guard !accountScopeKey.isEmpty,
              let data = defaults.data(forKey: key(accountScopeKey)),
              let envelope = try? JSONDecoder().decode(Envelope.self, from: data),
              envelope.schemaVersion == schemaVersion
        else {
            if !accountScopeKey.isEmpty {
                defaults.removeObject(forKey: key(accountScopeKey))
            }
            return .empty
        }
        let agentID = envelope.agentID.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !agentID.isEmpty else {
            defaults.removeObject(forKey: key(accountScopeKey))
            return .empty
        }
        return .init(currentAgentID: agentID, isLoadPending: false)
    }

    static func save(
        _ state: AccessRosterSelectionState,
        accountScopeKey: String,
        defaults: UserDefaults = .standard
    ) {
        guard !accountScopeKey.isEmpty else { return }
        guard let agentID = state.currentAgentID, !agentID.isEmpty else {
            clear(accountScopeKey: accountScopeKey, defaults: defaults)
            return
        }
        let envelope = Envelope(schemaVersion: schemaVersion, agentID: agentID)
        guard let data = try? JSONEncoder().encode(envelope) else { return }
        defaults.set(data, forKey: key(accountScopeKey))
    }

    static func clear(
        accountScopeKey: String,
        defaults: UserDefaults = .standard
    ) {
        guard !accountScopeKey.isEmpty else { return }
        defaults.removeObject(forKey: key(accountScopeKey))
    }

    private static func key(_ accountScopeKey: String) -> String {
        keyPrefix + Data(accountScopeKey.utf8).base64EncodedString()
    }
}

enum AccessRosterSelectionProjection {
    static func select(
        _ agentID: String?,
        previous: AccessRosterSelectionState
    ) -> AccessRosterSelectionState {
        let normalized = agentID?.trimmingCharacters(in: .whitespacesAndNewlines)
        let next = normalized?.isEmpty == false ? normalized : nil
        guard next != previous.currentAgentID else { return previous }
        return .init(currentAgentID: next, isLoadPending: next != nil)
    }

    static func reconcile(
        _ previous: AccessRosterSelectionState,
        agentIDs: [String],
        isRosterComplete: Bool
    ) -> AccessRosterSelectionState {
        guard isRosterComplete else { return previous }
        if let current = previous.currentAgentID {
            if agentIDs.contains(current) { return previous }
            if previous.isLoadPending { return previous }
        }
        return .init(currentAgentID: agentIDs.first, isLoadPending: false)
    }

    static func settle(
        _ previous: AccessRosterSelectionState,
        attemptedAgentID: String,
        completeAgentIDs: [String]?
    ) -> AccessRosterSelectionState {
        guard previous.currentAgentID == attemptedAgentID,
              previous.isLoadPending
        else { return previous }
        let next: String?
        if let completeAgentIDs, !completeAgentIDs.contains(attemptedAgentID) {
            next = completeAgentIDs.first
        } else {
            next = attemptedAgentID
        }
        return .init(currentAgentID: next, isLoadPending: false)
    }
}

enum AccessRosterFailureClassifier {
    static func transportKind(for error: Error) -> String? {
        if let urlError = error as? URLError {
            switch urlError.code {
            case .dnsLookupFailed, .cannotFindHost:
                return "dns"
            case .notConnectedToInternet,
                 .networkConnectionLost,
                 .cannotConnectToHost,
                 .timedOut,
                 .internationalRoamingOff,
                 .dataNotAllowed:
                return "network"
            default:
                break
            }
        }

        let text = error.localizedDescription.lowercased()
        if text.contains("dns") || text.contains("cannot find host") {
            return "dns"
        }
        if text.contains("network")
            || text.contains("offline")
            || text.contains("timed out")
            || text.contains("connection")
        {
            return "network"
        }
        return nil
    }

    static func failure(for error: Error, access: AccessCoverSandAccess) -> AccessRosterFailure {
        if access.state == .unavailable || access.state == .paymentRequired {
            return .init(
                code: ACCESS_BLOCKED_FAILURE_CODE,
                message: error.localizedDescription,
                transportKind: nil
            )
        }
        let transportKind = transportKind(for: error)
        return .init(
            code: transportKind ?? "roster-load-failed",
            message: error.localizedDescription,
            transportKind: transportKind
        )
    }
}
