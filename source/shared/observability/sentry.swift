import Foundation

let SAND_SENTRY_CONVERSATION_TAG = "sand.conversation_id"
let MAX_SPOOL_PURGE_SHIFTS = 64

struct SandSentryItem {
    var header: [String: Any]
    var payload: Any
}

struct SandSentryEnvelope {
    var header: [String: Any]
    var items: [SandSentryItem]
}

private let sandSentryEventIDRegex = try! NSRegularExpression(pattern: #"^[0-9a-f]{32}$"#)

private func sandSentryRecord(_ value: Any) -> [String: Any]? {
    value as? [String: Any]
}

func isUnknownProcessEnvelope(_ envelope: SandSentryEnvelope) -> Bool {
    for item in envelope.items {
        guard item.header["type"] as? String == "event",
              let event = sandSentryRecord(item.payload),
              let tags = event["tags"] as? [String: Any],
              tags["event.process"] as? String == "unknown" else {
            continue
        }
        return true
    }
    return false
}

enum SandSentryEnvelopeAccount: Equatable {
    case match
    case mismatch
    case missing
}

func envelopeAccount(
    _ envelope: SandSentryEnvelope,
    currentUserID: String?
) -> SandSentryEnvelopeAccount {
    guard let currentUserID else { return .match }

    var matched = false
    for item in envelope.items {
        guard let payload = sandSentryRecord(item.payload) else { continue }
        let id: Any?
        switch item.header["type"] as? String {
        case "event":
            id = (payload["user"] as? [String: Any])?["id"]
        case "session":
            id = payload["did"]
        default:
            id = nil
        }

        if let string = id as? String {
            guard string == currentUserID else { return .mismatch }
            matched = true
        } else if let number = id as? NSNumber {
            guard number.stringValue == currentUserID else { return .mismatch }
            matched = true
        }
    }
    return matched ? .match : .missing
}

final class SandSentryPrivacyGate {
    private var tier: SandSentryPrivacyTier = .fatalMetadata
    private var userID: String?

    func setTier(_ tier: SandSentryPrivacyTier) {
        self.tier = tier
    }

    func setUserID(_ userID: String?) {
        self.userID = userID
    }

    func handle(_ envelope: SandSentryEnvelope) -> SandSentryEnvelope? {
        guard !isUnknownProcessEnvelope(envelope) else { return nil }

        let account = envelopeAccount(envelope, currentUserID: userID)
        let effectiveTier: SandSentryPrivacyTier
        switch (account, tier) {
        case (.mismatch, _):
            effectiveTier = .fatalMetadata
        case (.missing, .full):
            effectiveTier = .scrubbed
        default:
            effectiveTier = tier
        }
        return projectSandSentryEnvelope(envelope, tier: effectiveTier)
    }

    func belongsToCurrentAccount(_ envelope: SandSentryEnvelope) -> Bool {
        envelopeAccount(envelope, currentUserID: userID) == .match
    }
}

final class SandSentryAccountPrivacySync {
    struct Request: Equatable {
        let sequence: Int
        let reset: Bool
        let identityChanged: Bool
    }

    private var sequence = 0
    private var authID: String?

    func begin(authID: String?) -> Request {
        let reset = authID == nil || authID != self.authID
        let identityChanged = self.authID != nil && authID != self.authID
        self.authID = authID
        sequence += 1
        return .init(sequence: sequence, reset: reset, identityChanged: identityChanged)
    }

    func isCurrent(_ request: Request) -> Bool {
        request.sequence == sequence
    }

    func finish<T>(_ request: Request, value: T) -> T? {
        isCurrent(request) ? value : nil
    }
}

final class SandSentryEventIDRing {
    private(set) var ids: [String] = []
    let capacity: Int

    init(capacity: Int) {
        self.capacity = max(0, capacity)
    }

    func record(_ envelope: SandSentryEnvelope) {
        guard capacity > 0 else {
            ids = []
            return
        }

        for item in envelope.items {
            guard item.header["type"] as? String == "event",
                  let event = sandSentryRecord(item.payload),
                  let eventID = event["event_id"] as? String,
                  sandSentryEventIDRegex.firstMatch(
                    in: eventID,
                    range: NSRange(eventID.startIndex..., in: eventID)
                  ) != nil else {
                continue
            }
            ids.removeAll { $0 == eventID }
            ids.insert(eventID, at: 0)
            if ids.count > capacity {
                ids.removeLast(ids.count - capacity)
            }
        }
    }

    func clear() {
        ids.removeAll(keepingCapacity: true)
    }
}

final class SandSentrySpoolStore {
    private var queue: [SandSentryEnvelope] = []

    func push(_ envelope: SandSentryEnvelope) {
        queue.append(envelope)
    }

    func unshift(_ envelope: SandSentryEnvelope) {
        queue.insert(envelope, at: 0)
    }

    func shift() -> SandSentryEnvelope? {
        guard !queue.isEmpty else { return nil }
        return queue.removeFirst()
    }

    @discardableResult
    func purge(maxShifts: Int = MAX_SPOOL_PURGE_SHIFTS) -> Int {
        let count = min(max(0, maxShifts), queue.count)
        if count > 0 {
            queue.removeFirst(count)
        }
        return count
    }

    var count: Int { queue.count }
}
