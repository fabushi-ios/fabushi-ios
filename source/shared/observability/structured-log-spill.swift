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

    func structuredLogCheckpoint() -> StructuredLogCheckpoint {
        let buffered = records.map { record in
            var metadata = record.metadata
            metadata["lifecycle_family"] = record.family
            return BufferedStructuredLog(
                level: StructuredLogLevel(rawValue: record.level) ?? .info,
                message: "sand.desktop.lifecycle",
                metadata: metadata,
                timestampMs: record.timestampMs
            )
        }
        return .init(
            counterID: createDropCounterID(),
            counters: emptyDropCounters(),
            records: buffered
        )
    }

    func replaceWithStructuredLogPending(_ pending: [BufferedStructuredLog]) {
        guard accountScope != nil else { return }
        records = pending.compactMap { record in
            guard record.message == "sand.desktop.lifecycle",
                  let family = record.metadata["lifecycle_family"]
            else { return nil }
            var metadata = record.metadata
            metadata.removeValue(forKey: "lifecycle_family")
            return IOSLifecycleTelemetryDiskRecord(
                family: family,
                level: record.level.rawValue,
                metadata: metadata,
                timestampMs: record.timestampMs
            )
        }
        persist(nowMs: Int64(Date().timeIntervalSince1970 * 1_000))
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
    typealias SubmitterFactory = @MainActor (_ accountSlot: String) -> StructuredLogSubmitter

    private let spill: IOSLifecycleTelemetrySpill
    private var tail: Task<Void, Never>?
    private var scheduledFlush: Task<Void, Never>?
    private var makeSubmitter: SubmitterFactory?
    private var transport: StructuredLogTransport?
    private var accountSlot: String?

    init(url: URL) {
        spill = IOSLifecycleTelemetrySpill(url: url)
    }

    func configureRemoteShipping(makeSubmitter: @escaping SubmitterFactory) {
        self.makeSubmitter = makeSubmitter
        if let accountSlot {
            rotate(accountSlot: accountSlot)
        }
    }

    func submit(_ record: IOSLifecycleTelemetryRecord) {
        let previous = tail
        let spill = spill
        tail = Task { @MainActor [weak self] in
            _ = await previous?.value
            await spill.append(record)
            guard let self, let transport = self.transport else { return }
            await transport.enqueue(
                StructuredLogLevel(rawValue: record.level.rawValue) ?? .info,
                message: "sand.desktop.lifecycle",
                metadata: record.metadata.merging(
                    ["lifecycle_family": record.family.rawValue]
                ) { current, _ in current }
            )
            self.scheduleRemoteFlush()
        }
    }

    func rotate(accountSlot: String?) {
        self.accountSlot = accountSlot
        scheduledFlush?.cancel()
        scheduledFlush = nil
        let previous = tail
        let spill = spill
        let makeSubmitter = makeSubmitter
        tail = Task { @MainActor [weak self] in
            _ = await previous?.value
            guard let self else { return }
            if let old = self.transport {
                await old.dispose(retainUndelivered: false)
            }
            self.transport = nil
            await spill.rotate(accountSlot: accountSlot)

            guard let accountSlot,
                  !accountSlot.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
                  let makeSubmitter
            else { return }

            let checkpoint = await spill.structuredLogCheckpoint()
            let next = StructuredLogTransport(
                key: IOS_SAND_LOG_KEY,
                platformTags: [
                    "client": "sand",
                    "client.type": "sand",
                    "platform": "ios",
                ],
                initialCheckpoint: checkpoint,
                holdForIdentity: false,
                submit: makeSubmitter(accountSlot)
            )
            await next.setIdentityTags([
                "account_scope": iosLifecycleTelemetryAccountScope(accountSlot),
            ])
            self.transport = next
            self.scheduleRemoteFlush(eager: !checkpoint.records.isEmpty)
        }
    }

    func settleLoggedOutIfNeeded() {
        guard accountSlot == nil else { return }
        rotate(accountSlot: nil)
    }

    func flushRemoteNow() {
        scheduledFlush?.cancel()
        scheduledFlush = nil
        let previous = tail
        let spill = spill
        tail = Task { @MainActor [weak self] in
            _ = await previous?.value
            guard let self, let transport = self.transport else { return }
            await self.flushAndPersist(transport: transport, spill: spill)
        }
    }

    func waitForPending() async {
        await tail?.value
    }

    private func scheduleRemoteFlush(eager: Bool = false) {
        guard transport != nil, scheduledFlush == nil else { return }
        scheduledFlush = Task { @MainActor [weak self] in
            if !eager {
                try? await Task.sleep(
                    nanoseconds: UInt64(LOG_SHIP_INTERVAL_MS) * 1_000_000
                )
            }
            guard !Task.isCancelled, let self else { return }
            self.scheduledFlush = nil
            self.flushRemoteNow()
        }
    }

    private func flushAndPersist(
        transport: StructuredLogTransport,
        spill: IOSLifecycleTelemetrySpill
    ) async {
        while true {
            let before = await transport.capturePending()
            guard !before.isEmpty else {
                await spill.replaceWithStructuredLogPending([])
                return
            }
            let shipped = await transport.flushNow()
            let pending = await transport.capturePending()
            await spill.replaceWithStructuredLogPending(pending)
            guard shipped, pending.count < before.count else { return }
        }
    }
}
