import Foundation

let MAX_BUFFER_SIZE = 1_000
let STRUCTURED_LOG_SUBMIT_DEADLINE_MS = 15_000
let STRUCTURED_LOG_REPLAY_MAX_AGE_MS = 17 * 60 * 60 * 1_000
let DEADLINE_EXPIRY_CODE = "deadline_exceeded"

enum StructuredLogLevel: String, Sendable {
    case debug
    case info
    case warn
    case error
}

enum ClientLogLevel: Int, Sendable {
    case unspecified = 0
    case info = 1
    case debug = 2
    case warn = 3
    case error = 4
}

struct StructuredLogEntry: Equatable, Sendable {
    let level: ClientLogLevel
    let message: String
    let metadata: [String: String]
    let timestamp: Int64
    let key: String
}

struct BufferedStructuredLog: Equatable, Sendable {
    let level: StructuredLogLevel
    let message: String
    let metadata: [String: String]
    let timestampMs: Int64
}

struct StructuredLogReceipt: Equatable, Sendable {
    let logsProcessed: Int
    let logsDropped: Int
}

struct StructuredLogDropCounter: Equatable, Sendable {
    var observed: Int
    var acknowledgedThrough: Int
}

typealias StructuredLogDropCounters = [String: StructuredLogDropCounter]

struct StructuredLogCheckpoint: Equatable, Sendable {
    let counterID: String
    let counters: StructuredLogDropCounters
    let records: [BufferedStructuredLog]
}

struct StructuredLogDeadlineError: Error, Equatable, Sendable {
    let code = DEADLINE_EXPIRY_CODE
}

func isDeadlineExpiry(_ error: Error) -> Bool {
    if let error = error as? StructuredLogDeadlineError {
        return error.code == DEADLINE_EXPIRY_CODE
    }
    let nsError = error as NSError
    return nsError.userInfo["code"] as? String == DEADLINE_EXPIRY_CODE
}

func createDropCounterID() -> String {
    UUID().uuidString.replacingOccurrences(of: "-", with: "").lowercased()
}

func emptyDropCounters() -> StructuredLogDropCounters {
    Dictionary(uniqueKeysWithValues: TELEMETRY_DROP_REASONS.map {
        ($0, StructuredLogDropCounter(observed: 0, acknowledgedThrough: 0))
    })
}

func cloneDropCounters(_ counters: StructuredLogDropCounters) -> StructuredLogDropCounters {
    counters
}

func isValidLogShipReceipt(_ response: StructuredLogReceipt, requestSize: Int) -> Bool {
    guard response.logsProcessed >= 0, response.logsDropped >= 0 else { return false }
    return response.logsProcessed + response.logsDropped == requestSize
}

func cleanStructuredLogMetadata(_ metadata: [String: String?]) -> [String: String] {
    metadata.reduce(into: [:]) { result, pair in
        if let value = pair.value, !value.isEmpty {
            result[pair.key] = value
        }
    }
}

func truncateStructuredLogValue(_ value: String, max: Int) -> String {
    guard max > 0, value.count > max else { return max <= 0 ? "" : value }
    return String(value.prefix(max))
}

func toClientLogLevel(_ level: StructuredLogLevel) -> ClientLogLevel {
    switch level {
    case .debug: return .debug
    case .info: return .info
    case .warn: return .warn
    case .error: return .error
    }
}

typealias StructuredLogSubmitter = @Sendable ([StructuredLogEntry]) async throws -> StructuredLogReceipt

actor StructuredLogTransport {
    private let key: String
    private let platformTags: [String: String]
    private let submit: StructuredLogSubmitter
    private var identityTags: [String: String] = [:]
    private var buffer: [BufferedStructuredLog]
    private var activeBatch: [BufferedStructuredLog] = []
    private var counters: StructuredLogDropCounters
    private var counterID: String
    private var holdForIdentity: Bool
    private var disposed = false
    private var deliveryGeneration = 0

    init(
        key: String,
        platformTags: [String: String?],
        initialCheckpoint: StructuredLogCheckpoint? = nil,
        holdForIdentity: Bool = false,
        submit: @escaping StructuredLogSubmitter
    ) {
        self.key = key
        self.platformTags = cleanStructuredLogMetadata(platformTags)
        self.submit = submit
        self.buffer = Array((initialCheckpoint?.records ?? []).suffix(MAX_BUFFER_SIZE))
        self.counters = initialCheckpoint?.counters ?? emptyDropCounters()
        self.counterID = initialCheckpoint?.counterID ?? createDropCounterID()
        self.holdForIdentity = holdForIdentity
    }

    func setIdentityTags(_ tags: [String: String?]) {
        identityTags = cleanStructuredLogMetadata(tags)
        holdForIdentity = false
    }

    func enqueue(
        _ level: StructuredLogLevel,
        message: String,
        metadata: [String: String?] = [:],
        timestampMs: Int64 = Int64(Date().timeIntervalSince1970 * 1_000)
    ) {
        guard !disposed else { return }

        var merged = platformTags
        for (key, value) in cleanStructuredLogMetadata(metadata) {
            merged[key] = value
        }

        buffer.append(.init(
            level: level,
            message: message,
            metadata: merged,
            timestampMs: timestampMs
        ))

        dropBufferOverflow()
    }

    private func dropBufferOverflow() {
        var overflow = buffer.count - MAX_BUFFER_SIZE
        guard overflow > 0 else { return }

        var evicted = 0
        buffer.removeAll { entry in
            guard overflow > 0,
                  entry.message == HOST_LOG_EVENT || entry.message == BOX_LOG_EVENT else {
                return false
            }
            overflow -= 1
            evicted += 1
            return true
        }
        if overflow > 0 {
            buffer.removeFirst(overflow)
            evicted += overflow
        }
        recordDropped(reason: "overflow_evicted", count: evicted)
    }

    func capturePending() -> [BufferedStructuredLog] {
        activeBatch + buffer
    }

    func captureCheckpoint() -> StructuredLogCheckpoint {
        .init(counterID: counterID, counters: counters, records: buffer)
    }

    func recordDropped(reason: String, count: Int) {
        guard count > 0 else { return }
        var counter = counters[reason] ?? .init(observed: 0, acknowledgedThrough: 0)
        counter.observed += count
        counters[reason] = counter
    }

    func clearPending() {
        deliveryGeneration += 1
        activeBatch.removeAll()
        buffer.removeAll()
        counterID = createDropCounterID()
        counters = emptyDropCounters()
    }

    private func pendingDropSnapshot() -> [(reason: String, through: Int)] {
        TELEMETRY_DROP_REASONS.compactMap { reason in
            guard let counter = counters[reason],
                  counter.observed > counter.acknowledgedThrough else { return nil }
            return (reason, counter.observed)
        }
    }

    private func buildEntry(
        level: StructuredLogLevel,
        message: String,
        metadata: [String: String],
        timestampMs: Int64
    ) -> StructuredLogEntry {
        StructuredLogEntry(
            level: toClientLogLevel(level),
            message: message,
            metadata: metadata.merging(identityTags) { _, identity in identity },
            timestamp: timestampMs,
            key: key
        )
    }

    private func reportPendingDrops(generation: Int, nowMs: Int64) async -> Bool {
        let snapshot = pendingDropSnapshot()
        guard !snapshot.isEmpty else { return true }
        let reports = snapshot.map { item in
            var metadata = platformTags
            metadata["reason"] = item.reason
            metadata["unit"] = TELEMETRY_DROP_UNIT_BY_REASON[item.reason] ?? "entries"
            metadata["count"] = String(item.through)
            metadata["counter_id"] = counterID
            return buildEntry(
                level: .warn,
                message: TELEMETRY_DROPPED_EVENT,
                metadata: metadata,
                timestampMs: nowMs
            )
        }

        do {
            let receipt = try await submit(reports)
            guard generation == deliveryGeneration else { return true }
            guard isValidLogShipReceipt(receipt, requestSize: reports.count),
                  receipt.logsProcessed == reports.count,
                  receipt.logsDropped == 0 else { return false }
            for item in snapshot {
                guard var counter = counters[item.reason] else { continue }
                counter.acknowledgedThrough = max(counter.acknowledgedThrough, item.through)
                counters[item.reason] = counter
            }
            return true
        } catch {
            return false
        }
    }

    func flushNow(nowMs: Int64 = Int64(Date().timeIntervalSince1970 * 1_000)) async -> Bool {
        guard !disposed else { return activeBatch.isEmpty && buffer.isEmpty && pendingDropSnapshot().isEmpty }
        guard !holdForIdentity else { return false }
        let generation = deliveryGeneration

        let expiredCutoff = nowMs - Int64(STRUCTURED_LOG_REPLAY_MAX_AGE_MS)
        let expiredCount = buffer.lazy.filter { $0.timestampMs < expiredCutoff }.count
        if expiredCount > 0 {
            buffer.removeAll { $0.timestampMs < expiredCutoff }
            recordDropped(reason: "replay_expired", count: expiredCount)
        }

        guard !buffer.isEmpty else {
            return await reportPendingDrops(generation: generation, nowMs: nowMs)
        }

        let split = takeLogShipBatch(buffer) {
            LogShipBufferedEntry(message: $0.message, metadata: $0.metadata)
        }
        let batch = split.batch
        buffer = split.remaining
        activeBatch = batch

        let entries = batch.map { record in
            buildEntry(
                level: record.level,
                message: record.message,
                metadata: record.metadata,
                timestampMs: record.timestampMs
            )
        }

        do {
            let receipt = try await submit(entries)
            guard generation == deliveryGeneration else {
                activeBatch.removeAll()
                return true
            }
            guard isValidLogShipReceipt(receipt, requestSize: entries.count) else {
                buffer.insert(contentsOf: batch, at: 0)
                activeBatch.removeAll()
                recordDropped(reason: "ship_failed", count: 1)
                dropBufferOverflow()
                return false
            }
            activeBatch.removeAll()
            if receipt.logsDropped > 0 {
                recordDropped(reason: "backend_dropped", count: receipt.logsDropped)
            }
            return await reportPendingDrops(generation: generation, nowMs: nowMs)
        } catch {
            guard generation == deliveryGeneration else {
                activeBatch.removeAll()
                return true
            }
            buffer.insert(contentsOf: batch, at: 0)
            activeBatch.removeAll()
            recordDropped(reason: "ship_failed", count: 1)
            dropBufferOverflow()
            return false
        }
    }

    func dispose(retainUndelivered: Bool = true) {
        disposed = true
        deliveryGeneration += 1
        if !retainUndelivered {
            activeBatch.removeAll()
            buffer.removeAll()
        } else if !activeBatch.isEmpty {
            buffer.insert(contentsOf: activeBatch, at: 0)
            activeBatch.removeAll()
            dropBufferOverflow()
        }
    }
}
