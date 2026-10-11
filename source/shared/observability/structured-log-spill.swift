import CryptoKit
import Foundation

let DESKTOP_STRUCTURED_LOG_SPILL_MAX_BYTES = 1024 * 1024
let DESKTOP_STRUCTURED_LOG_SPILL_MAX_AGE_MS: Int64 = 18 * 60 * 60 * 1_000
let DESKTOP_STRUCTURED_LOG_SPILL_MAX_ENTRIES = 1_000
let IOS_LIFECYCLE_TELEMETRY_SPILL_FILE_NAME = "ios-lifecycle-telemetry-spill.v1.json"

private struct IOSLifecycleTelemetryDiskRecord: Codable, Equatable, Sendable {
    let family: String
    let level: String
    let metadata: [String: String]
    let timestampMs: Int64
}

private struct IOSLifecycleTelemetryDiskState: Codable, Equatable, Sendable {
    let version: Int
    let accountScope: String
    var records: [IOSLifecycleTelemetryDiskRecord]
}

func iosLifecycleTelemetryAccountScope(_ accountSlot: String?) -> String {
    guard let accountSlot, !accountSlot.isEmpty else { return "logged-out" }
    let digest = SHA256.hash(data: Data(accountSlot.utf8))
    return digest.map { String(format: "%02x", $0) }.joined()
}

actor IOSLifecycleTelemetrySpill {
    private let url: URL
    private var accountScope: String?
    private var pendingBeforeScope: [IOSLifecycleTelemetryDiskRecord] = []
    private var records: [IOSLifecycleTelemetryDiskRecord] = []

    init(url: URL) {
        self.url = url
    }

    func append(
        _ record: IOSLifecycleTelemetryRecord,
        timestampMs: Int64 = Int64(Date().timeIntervalSince1970 * 1_000)
    ) {
        let disk = IOSLifecycleTelemetryDiskRecord(
            family: record.family.rawValue,
            level: record.level.rawValue,
            metadata: record.metadata,
            timestampMs: timestampMs
        )
        guard accountScope != nil else {
            pendingBeforeScope.append(disk)
            if pendingBeforeScope.count > IOSLifecycleReporter.preAttachBufferLimit {
                pendingBeforeScope.removeFirst(pendingBeforeScope.count - IOSLifecycleReporter.preAttachBufferLimit)
            }
            return
        }
        records.append(disk)
        persist(nowMs: Int64(Date().timeIntervalSince1970 * 1_000))
    }

    func rotate(accountSlot: String?) {
        let next = iosLifecycleTelemetryAccountScope(accountSlot)
        if accountScope == next { return }

        accountScope = next
        records = load(scope: next)
        if !pendingBeforeScope.isEmpty {
            records.append(contentsOf: pendingBeforeScope)
            pendingBeforeScope.removeAll(keepingCapacity: false)
        }
        persist(nowMs: Int64(Date().timeIntervalSince1970 * 1_000))
    }

    func settleLoggedOutIfNeeded() {
        guard accountScope == nil else { return }
        rotate(accountSlot: nil)
    }

    func snapshotCount() -> Int {
        records.count + pendingBeforeScope.count
    }

    func snapshotFamilies() -> [String] {
        (records + pendingBeforeScope).map(\.family)
    }

    func persistedByteCount() -> Int {
        (try? Data(contentsOf: url).count) ?? 0
    }

    private func load(scope: String) -> [IOSLifecycleTelemetryDiskRecord] {
        guard let data = try? Data(contentsOf: url),
              data.count <= DESKTOP_STRUCTURED_LOG_SPILL_MAX_BYTES,
              let state = try? JSONDecoder().decode(IOSLifecycleTelemetryDiskState.self, from: data),
              state.version == 1,
              state.accountScope == scope
        else {
            try? FileManager.default.removeItem(at: url)
            return []
        }
        return state.records
    }

    private func persist(nowMs: Int64) {
        guard let accountScope else { return }
        let cutoff = nowMs - DESKTOP_STRUCTURED_LOG_SPILL_MAX_AGE_MS
        records.removeAll { $0.timestampMs < cutoff }
        if records.count > DESKTOP_STRUCTURED_LOG_SPILL_MAX_ENTRIES {
            records.removeFirst(records.count - DESKTOP_STRUCTURED_LOG_SPILL_MAX_ENTRIES)
        }

        let encoder = JSONEncoder()
        var state = IOSLifecycleTelemetryDiskState(
            version: 1,
            accountScope: accountScope,
            records: records
        )
        var data = (try? encoder.encode(state)) ?? Data()
        while data.count > DESKTOP_STRUCTURED_LOG_SPILL_MAX_BYTES, !state.records.isEmpty {
            state.records.removeFirst()
            data = (try? encoder.encode(state)) ?? Data()
        }
        records = state.records

        guard !data.isEmpty else { return }
        do {
            try FileManager.default.createDirectory(
                at: url.deletingLastPathComponent(),
                withIntermediateDirectories: true
            )
            try data.write(to: url, options: .atomic)
            try? FileManager.default.setAttributes(
                [.protectionKey: FileProtectionType.completeUntilFirstUserAuthentication],
                ofItemAtPath: url.path
            )
        } catch {
            // Telemetry durability is non-critical. The in-memory caller must
            // never fail the product flow because a spill write was unavailable.
        }
    }
}

@MainActor
final class IOSLifecycleTelemetryDurableSink {
    private let spill: IOSLifecycleTelemetrySpill
    private var tail: Task<Void, Never>?

    init(url: URL) {
        spill = IOSLifecycleTelemetrySpill(url: url)
    }

    func submit(_ record: IOSLifecycleTelemetryRecord) {
        let previous = tail
        let spill = spill
        tail = Task {
            _ = await previous?.value
            await spill.append(record)
        }
    }

    func rotate(accountSlot: String?) {
        let previous = tail
        let spill = spill
        tail = Task {
            _ = await previous?.value
            await spill.rotate(accountSlot: accountSlot)
        }
    }

    func settleLoggedOutIfNeeded() {
        let previous = tail
        let spill = spill
        tail = Task {
            _ = await previous?.value
            await spill.settleLoggedOutIfNeeded()
        }
    }

    func waitForPending() async {
        await tail?.value
    }
}
