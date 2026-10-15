import Foundation

@MainActor
final class IOSProductionSentryController {
    typealias Reporter = @MainActor (_ level: IOSLifecycleTelemetryLevel, _ metadata: [String: String]) -> Void

    private let privacyGate = SandSentryPrivacyGate()
    private let accountSync = SandSentryAccountPrivacySync()
    private let eventIDs = SandSentryEventIDRing(capacity: 64)
    private let spool = SandSentrySpoolStore()
    private let report: Reporter

    private(set) var accountScopeHash: String?
    private(set) var conversationID: String?
    private(set) var privacyTier: SandSentryPrivacyTier = .fatalMetadata

    init(report: @escaping Reporter) {
        self.report = report
        privacyGate.setTier(.fatalMetadata)
        privacyGate.setUserID(nil)
    }

    func rotate(accountSlot: String?) {
        let nextHash = accountSlot.map { iosLifecycleTelemetryAccountScope($0) }
        let request = accountSync.begin(authID: nextHash)
        if request.reset {
            conversationID = nil
            eventIDs.clear()
            if request.identityChanged || nextHash == nil {
                _ = spool.purge()
            }
        }
        guard accountSync.isCurrent(request) else { return }
        accountScopeHash = nextHash
        privacyGate.setUserID(nextHash)
    }

    func setPrivacyMode(_ mode: SandPrivacyMode?) {
        let tier = sandSentryPrivacyTierForMode(mode)
        privacyTier = tier
        privacyGate.setTier(tier)
        if tier == .fatalMetadata {
            _ = spool.purge()
        }
    }

    @discardableResult
    func setConversationReport(_ value: Any?) -> Bool {
        guard isValidSentryConversationReport(value),
              let report = value as? [String: Any],
              let raw = report["agentId"]
        else { return false }

        if raw is NSNull {
            conversationID = nil
            return true
        }
        guard let value = raw as? String, isSandSentryBoundedTagValue(value) else {
            return false
        }
        conversationID = value
        return true
    }

    func captureFatal(
        eventID: String? = nil,
        origin: String,
        process: String = "ios-main",
        failureCode: String? = nil,
        crashKind: String? = nil
    ) {
        let eventID = eventID ?? Self.makeEventID()
        var tags: [String: Any] = [
            "event.origin": Self.boundedTag(origin, fallback: "ios-native"),
            "event.process": Self.boundedTag(process, fallback: "ios-main"),
            "sand.process": "ios-main",
        ]
        if let failureCode, isSandSentryBoundedTagValue(failureCode) {
            tags["sand.failure_code"] = failureCode
        }
        if let crashKind, isSandSentryBoundedTagValue(crashKind) {
            tags["crash.kind"] = crashKind
        }
        if let conversationID, privacyTier != .fatalMetadata {
            tags[SAND_SENTRY_CONVERSATION_TAG] = conversationID
        }

        var event: [String: Any] = [
            "event_id": eventID,
            "level": "fatal",
            "platform": "cocoa",
            "timestamp": Date().timeIntervalSince1970,
            "tags": tags,
        ]
        if let accountScopeHash {
            event["user"] = ["id": accountScopeHash]
        }

        let envelope = SandSentryEnvelope(
            header: ["event_id": eventID],
            items: [.init(header: ["type": "event"], payload: event)]
        )
        accept(envelope)
    }

    func accept(_ envelope: SandSentryEnvelope) {
        guard privacyGate.belongsToCurrentAccount(envelope),
              let gated = privacyGate.handle(envelope)
        else { return }
        eventIDs.record(gated)
        spool.push(gated)
        flush()
    }

    func flush() {
        while let envelope = spool.shift() {
            emit(envelope)
        }
    }

    func purge() {
        _ = spool.purge()
        eventIDs.clear()
    }

    var recentEventIDs: [String] { eventIDs.ids }
    var spooledCount: Int { spool.count }

    private func emit(_ envelope: SandSentryEnvelope) {
        for item in envelope.items {
            guard item.header["type"] as? String == "event",
                  let event = item.payload as? [String: Any]
            else { continue }

            var metadata: [String: String] = [
                "event": "sentry_event",
                "privacy_tier": privacyTier.rawValue,
                "platform_replacement": "ios_structured_log_transport",
            ]
            if let eventID = event["event_id"] as? String {
                metadata["event_id"] = String(eventID.prefix(64))
            }
            if let level = event["level"] as? String {
                metadata["level"] = String(level.prefix(32))
            }
            if let platform = event["platform"] as? String {
                metadata["platform"] = String(platform.prefix(32))
            }
            if let tags = event["tags"] as? [String: Any] {
                for key in [
                    "event.origin", "event.process", "exit.reason", "crash.kind",
                    "sand.failure_code", "sand.process", SAND_SENTRY_CONVERSATION_TAG,
                ] {
                    if let value = tags[key] as? String, isSandSentryBoundedTagValue(value) {
                        metadata["sentry_tag.\(key)"] = value
                    }
                }
            }
            report(.error, metadata)
        }
    }

    private static func makeEventID() -> String {
        UUID().uuidString.replacingOccurrences(of: "-", with: "").lowercased()
    }

    private static func boundedTag(_ value: String, fallback: String) -> String {
        isSandSentryBoundedTagValue(value) ? value : fallback
    }
}

@MainActor
final class IOSProductionSentryPrivacySynchronizer {
    typealias ReadMode = @MainActor () async -> SandPrivacyMode?

    private let controller: IOSProductionSentryController
    private let readMode: ReadMode
    private var generation: UInt64 = 0
    private var task: Task<Void, Never>?

    init(controller: IOSProductionSentryController, readMode: @escaping ReadMode) {
        self.controller = controller
        self.readMode = readMode
    }

    func refresh(loggedIn: Bool) {
        generation = generation == UInt64.max ? 1 : generation + 1
        let expectedGeneration = generation
        task?.cancel()
        task = nil
        controller.setPrivacyMode(nil)
        guard loggedIn, controller.accountScopeHash != nil else { return }

        task = Task { @MainActor [weak self] in
            guard let self else { return }
            let mode = await self.readMode()
            guard !Task.isCancelled, self.generation == expectedGeneration else { return }
            self.controller.setPrivacyMode(mode)
            self.task = nil
        }
    }

    func cancel() {
        generation = generation == UInt64.max ? 1 : generation + 1
        task?.cancel()
        task = nil
        controller.setPrivacyMode(nil)
    }
}
