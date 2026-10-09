import Foundation

let LOG_SHIP_INTERVAL_MS = 15_000
let LOG_SHIP_MAX_BATCH_ENTRIES = 128
let LOG_SHIP_MAX_BATCH_BYTES = 256 * 1_024
let LOG_SHIP_FAILURE_MAX_BACKOFF_MS = 60_000
let LOG_SHIP_RATE_LIMIT_MAX_BACKOFF_MS = 5 * 60_000
let LOG_SHIP_JITTER_RATIO = 0.25

struct LogShipReceipt: Equatable, Sendable {
    let logsProcessed: Int
    let logsDropped: Int
}

enum LogShipResult {
    case delivered(LogShipReceipt?)
    case failed(Error)
}

func nextLogShipDelayMs(
    outcome: String,
    streak: Int,
    retryAfterMs: Int? = nil,
    random: () -> Double = { Double.random(in: 0..<1) }
) -> Int {
    func backoff(_ capMs: Int) -> Int {
        let exponent = max(0, min(streak, 30))
        let scaled = Double(LOG_SHIP_INTERVAL_MS) * pow(2.0, Double(exponent))
        return min(capMs, Int(min(scaled, Double(Int.max))))
    }
    let baseMs: Int
    switch outcome {
    case "shipped":
        baseMs = LOG_SHIP_INTERVAL_MS
    case "failed":
        baseMs = backoff(LOG_SHIP_FAILURE_MAX_BACKOFF_MS)
    case "rate_limited":
        baseMs = max(retryAfterMs ?? 0, backoff(LOG_SHIP_RATE_LIMIT_MAX_BACKOFF_MS))
    default:
        baseMs = LOG_SHIP_INTERVAL_MS
    }
    return Int((Double(baseMs) + random() * Double(baseMs) * LOG_SHIP_JITTER_RATIO).rounded())
}

final class LogShipSchedule {
    private var nextShipAtMs: Int64 = 0
    private var failureStreak = 0
    private var rateLimitStreak = 0

    func isDue(eager: Bool, nowMs: Int64 = Int64(Date().timeIntervalSince1970 * 1_000)) -> Bool {
        nowMs >= nextShipAtMs || (eager && !isBackingOff())
    }

    func shipNext() {
        if !isBackingOff() { nextShipAtMs = 0 }
    }

    func record(_ result: LogShipResult, nowMs: Int64 = Int64(Date().timeIntervalSince1970 * 1_000), random: () -> Double = { Double.random(in: 0..<1) }) {
        switch result {
        case .delivered:
            failureStreak = 0
            rateLimitStreak = 0
            nextShipAtMs = nowMs + Int64(nextLogShipDelayMs(outcome: "shipped", streak: 0, random: random))
        case .failed(let error):
            if isRateLimitConnectError(error) {
                failureStreak = 0
                rateLimitStreak += 1
                let retryAfter = getConnectRetryAfterMs(error, nowMs: nowMs)
                nextShipAtMs = nowMs + Int64(nextLogShipDelayMs(
                    outcome: "rate_limited",
                    streak: rateLimitStreak,
                    retryAfterMs: retryAfter,
                    random: random
                ))
            } else {
                rateLimitStreak = 0
                failureStreak += 1
                nextShipAtMs = nowMs + Int64(nextLogShipDelayMs(outcome: "failed", streak: failureStreak, random: random))
            }
        }
    }

    func isBackingOff() -> Bool {
        failureStreak + rateLimitStreak > 0
    }
}

struct LogShipBufferedEntry: Equatable, Sendable {
    let message: String
    let metadata: [String: String]
}

func takeLogShipBatch<T>(
    _ buffer: [T],
    entry: (T) -> LogShipBufferedEntry
) -> (batch: [T], remaining: [T]) {
    var bytes = 0
    var count = 0
    for item in buffer {
        guard count < LOG_SHIP_MAX_BATCH_ENTRIES else { break }
        let record = entry(item)
        // Desktop's canonical TypeScript contract uses JavaScript String.length,
        // which counts UTF-16 code units rather than UTF-8 bytes. Preserve that
        // exact batching boundary so non-ASCII log messages split identically.
        var entryBytes = record.message.utf16.count
        for (key, value) in record.metadata {
            entryBytes += key.utf16.count + value.utf16.count
        }
        if count > 0 && bytes + entryBytes > LOG_SHIP_MAX_BATCH_BYTES { break }
        bytes += entryBytes
        count += 1
    }
    return (Array(buffer.prefix(count)), Array(buffer.dropFirst(count)))
}

func takeLogShipBatch(_ buffer: [LogShipBufferedEntry]) -> (batch: [LogShipBufferedEntry], remaining: [LogShipBufferedEntry]) {
    takeLogShipBatch(buffer) { $0 }
}
