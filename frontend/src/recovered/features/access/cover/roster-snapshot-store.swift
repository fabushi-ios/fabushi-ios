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
    static let schemaVersion = 2
    private static let keyPrefix = "fabushi.ios.roster.last-roster.v2:"

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
