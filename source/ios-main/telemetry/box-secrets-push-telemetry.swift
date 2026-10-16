import CryptoKit
import Foundation

let IOS_BOX_SECRETS_PUSH_STATE_FILE_NAME = "box-secrets-push-state.v1.json"
let IOS_BOX_SECRETS_PUSH_COUNT_CAP = 999

private struct IOSBoxSecretsPushAckState: Codable, Equatable {
    let version: Int
    let ackedCount: Int
    let scopeHash: String?
}

@MainActor
final class IOSBoxSecretsPushTelemetryState {
    typealias Reporter = @MainActor (IOSBoxSecretsTelemetryProjection) -> Void

    private let stateURL: URL?
    private let reporter: Reporter
    private var state: IOSBoxSecretsPushAckState?
    private var failureStreak = 0
    private var lastFailureClass: String?

    init(stateURL: URL?, reporter: @escaping Reporter) {
        self.stateURL = stateURL
        self.reporter = reporter
        load()
    }

    func recordFailure(
        trigger: String,
        errorClass: String,
        scopePresent: Bool,
        keyCount: Int?
    ) {
        if failureStreak == 0 || errorClass != lastFailureClass {
            var metadata = [
                "area": "box_secrets_push",
                "outcome": "failed",
                "trigger": String(trigger.prefix(64)),
                "error_class": String(errorClass.prefix(64)),
                "scope_present": String(scopePresent),
            ]
            if let keyCount {
                metadata["key_count"] = countTag(keyCount)
            }
            if let state {
                metadata["prev_count"] = countTag(state.ackedCount)
            }
            reporter(.init(level: .warn, metadata: metadata))
        }
        failureStreak = min(failureStreak + 1, IOS_BOX_SECRETS_PUSH_COUNT_CAP)
        lastFailureClass = errorClass
    }

    func recordSuccess(
        trigger: String,
        accountSlot: String?,
        keyCount: Int,
        applied: Bool,
        departing: Bool
    ) {
        let scopeHash = Self.scopeHash(accountSlot)
        let previous = state
        let delta = previous.map { keyCount - $0.ackedCount }
        let scopeChanged = scopeHash != nil
            && previous?.scopeHash != nil
            && scopeHash != previous?.scopeHash
        let departed = accountSlot == nil && departing
        let recoveredAfter = failureStreak
        failureStreak = 0
        lastFailureClass = nil

        let firstBaseline = previous == nil && keyCount > 0
        if recoveredAfter > 0 || firstBaseline || (delta != nil && delta != 0) {
            let shrankOutsideEdit = (delta ?? 0) < 0
                && trigger != "edit"
                && !scopeChanged
                && !departed
            var metadata = [
                "area": "box_secrets_push",
                "outcome": "ok",
                "trigger": String(trigger.prefix(64)),
                "key_count": countTag(keyCount),
                "applied": String(applied),
                "scope_present": String(accountSlot != nil),
            ]
            if let previous {
                metadata["prev_count"] = countTag(previous.ackedCount)
            }
            if let delta {
                metadata["delta"] = deltaTag(delta)
            }
            if scopeChanged { metadata["scope_changed"] = "true" }
            if departed { metadata["scope_departed"] = "true" }
            if recoveredAfter > 0 {
                metadata["recovered_after"] = countTag(recoveredAfter)
            }
            reporter(.init(level: shrankOutsideEdit ? .warn : .info, metadata: metadata))
        }

        let next = IOSBoxSecretsPushAckState(
            version: 1,
            ackedCount: cappedCount(keyCount),
            scopeHash: scopeHash
        )
        if next != previous {
            state = next
            save(next)
        }
    }

    var ackedCountForTests: Int? { state?.ackedCount }
    var scopeHashForTests: String? { state?.scopeHash }

    private func load() {
        guard let stateURL else { return }
        do {
            let data = try Data(contentsOf: stateURL)
            let decoded = try JSONDecoder().decode(IOSBoxSecretsPushAckState.self, from: data)
            guard decoded.version == 1,
                  decoded.ackedCount >= 0,
                  decoded.ackedCount <= IOS_BOX_SECRETS_PUSH_COUNT_CAP
            else {
                throw IOSBoxSecretsError.invalidStoredPayload
            }
            state = decoded
        } catch {
            if (error as NSError).code != NSFileReadNoSuchFileError {
                reporter(.init(
                    level: .warn,
                    metadata: [
                        "area": "box_secrets_push",
                        "outcome": "marker_read_failed",
                        "error_type": String(reflecting: type(of: error)),
                    ]
                ))
            }
        }
    }

    private func save(_ state: IOSBoxSecretsPushAckState) {
        guard let stateURL else { return }
        do {
            try FileManager.default.createDirectory(
                at: stateURL.deletingLastPathComponent(),
                withIntermediateDirectories: true
            )
            let data = try JSONEncoder().encode(state)
            try data.write(to: stateURL, options: [.atomic])
        } catch {
            reporter(.init(
                level: .warn,
                metadata: [
                    "area": "box_secrets_push",
                    "outcome": "marker_write_failed",
                    "error_type": String(reflecting: type(of: error)),
                ]
            ))
        }
    }

    private static func scopeHash(_ accountSlot: String?) -> String? {
        guard let accountSlot else { return nil }
        let digest = SHA256.hash(data: Data(accountSlot.utf8))
        return digest.prefix(8).map { String(format: "%02x", $0) }.joined()
    }

    private func cappedCount(_ count: Int) -> Int {
        min(max(count, 0), IOS_BOX_SECRETS_PUSH_COUNT_CAP)
    }

    private func countTag(_ count: Int) -> String {
        String(cappedCount(count))
    }

    private func deltaTag(_ delta: Int) -> String {
        String(min(max(delta, -IOS_BOX_SECRETS_PUSH_COUNT_CAP), IOS_BOX_SECRETS_PUSH_COUNT_CAP))
    }
}
