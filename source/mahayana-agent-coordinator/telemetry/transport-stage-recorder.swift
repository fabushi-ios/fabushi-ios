import Foundation
import Dispatch

let COORDINATOR_SSE_ECHO_STAGE = "echo-coordinator-sse"
let COORDINATOR_PENDING_SEND_ECHO_MAX = 64
let COORDINATOR_PENDING_SEND_ECHO_TTL_MILLISECONDS: UInt64 = 120_000

func coordinatorTransportEpochMilliseconds() -> Int64 {
    Int64(Date().timeIntervalSince1970 * 1_000)
}

func coordinatorTransportMonotonicMilliseconds() -> UInt64 {
    DispatchTime.now().uptimeNanoseconds / 1_000_000
}

struct CoordinatorTransportStage: Equatable, Sendable {
    let requestId: String
    let stage: String
    let timestampMilliseconds: Int64
}

struct CoordinatorTransportStageReport: Equatable, Sendable {
    let accountSlot: String
    let clientNonce: String
    let stage: String
    let attempt: Int
    let traceparent: String?
    let startEpochMilliseconds: Int64
    let durationMilliseconds: UInt64
    let isError: Bool
}

struct CoordinatorSendTrace: Equatable, Sendable {
    let accountSlot: String
    let clientNonce: String
    let traceparent: String?

    func beginStage(
        _ stage: String,
        attempt: Int,
        startEpochMilliseconds: Int64,
        startMonotonicMilliseconds: UInt64
    ) -> CoordinatorTransportStageToken {
        CoordinatorTransportStageToken(
            report: CoordinatorTransportStageReport(
                accountSlot: accountSlot,
                clientNonce: clientNonce,
                stage: stage,
                attempt: attempt,
                traceparent: traceparent,
                startEpochMilliseconds: startEpochMilliseconds,
                durationMilliseconds: 0,
                isError: false
            ),
            startedMonotonicMilliseconds: startMonotonicMilliseconds
        )
    }
}

struct CoordinatorTransportStageToken: Equatable, Sendable {
    private let report: CoordinatorTransportStageReport
    private let startedMonotonicMilliseconds: UInt64
    private var settled = false

    init(
        report: CoordinatorTransportStageReport,
        startedMonotonicMilliseconds: UInt64
    ) {
        self.report = report
        self.startedMonotonicMilliseconds = startedMonotonicMilliseconds
    }

    mutating func settle(
        nowMonotonicMilliseconds: UInt64,
        isError: Bool
    ) -> CoordinatorTransportStageReport? {
        guard !settled else { return nil }
        settled = true
        let duration = nowMonotonicMilliseconds >= startedMonotonicMilliseconds
            ? nowMonotonicMilliseconds - startedMonotonicMilliseconds
            : 0
        return CoordinatorTransportStageReport(
            accountSlot: report.accountSlot,
            clientNonce: report.clientNonce,
            stage: report.stage,
            attempt: report.attempt,
            traceparent: report.traceparent,
            startEpochMilliseconds: report.startEpochMilliseconds,
            durationMilliseconds: duration,
            isError: isError
        )
    }

    mutating func complete(
        nowMonotonicMilliseconds: UInt64
    ) -> CoordinatorTransportStageReport? {
        settle(nowMonotonicMilliseconds: nowMonotonicMilliseconds, isError: false)
    }

    mutating func fail(
        nowMonotonicMilliseconds: UInt64
    ) -> CoordinatorTransportStageReport? {
        settle(nowMonotonicMilliseconds: nowMonotonicMilliseconds, isError: true)
    }
}

private struct CoordinatorPendingTransportEcho: Sendable {
    let traceparent: String
    let armedAtMonotonicMilliseconds: UInt64
}

actor TransportStageRecorder {
    private let capacity: Int
    private var records: [CoordinatorTransportStage] = []
    private var reports: [CoordinatorTransportStageReport] = []
    private var pendingEchoes: [String: CoordinatorPendingTransportEcho] = [:]
    private var pendingOrder: [String] = []

    init(capacity: Int = 512) {
        self.capacity = max(1, capacity)
    }

    func record(requestId: String, stage: String, now: Date = Date()) {
        guard !requestId.isEmpty, !stage.isEmpty else { return }
        records.append(CoordinatorTransportStage(
            requestId: requestId,
            stage: stage,
            timestampMilliseconds: Int64(now.timeIntervalSince1970 * 1_000)
        ))
        if records.count > capacity {
            records.removeFirst(records.count - capacity)
        }
    }

    func record(report: CoordinatorTransportStageReport) {
        reports.append(report)
        if reports.count > capacity {
            reports.removeFirst(reports.count - capacity)
        }
    }

    func beginSend(
        accountSlot: String,
        clientNonce: String?,
        traceparent: String?,
        nowMonotonicMilliseconds: UInt64
    ) -> CoordinatorSendTrace? {
        guard let rawNonce = clientNonce else { return nil }
        let nonce = rawNonce.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !nonce.isEmpty else { return nil }
        let rawTrace = traceparent?.trimmingCharacters(in: .whitespacesAndNewlines)
        let trace = rawTrace?.isEmpty == false ? rawTrace : nil

        purgeExpired(nowMonotonicMilliseconds)
        if let trace {
            let key = Self.key(accountSlot: accountSlot, clientNonce: nonce)
            removePending(key)
            while pendingEchoes.count >= COORDINATOR_PENDING_SEND_ECHO_MAX,
                  !pendingOrder.isEmpty {
                pendingEchoes.removeValue(forKey: pendingOrder.removeFirst())
            }
            pendingOrder.append(key)
            pendingEchoes[key] = CoordinatorPendingTransportEcho(
                traceparent: trace,
                armedAtMonotonicMilliseconds: nowMonotonicMilliseconds
            )
        }

        return CoordinatorSendTrace(
            accountSlot: accountSlot,
            clientNonce: nonce,
            traceparent: trace
        )
    }

    func recordSendEcho(
        accountSlot: String,
        clientNonce: String,
        nowEpochMilliseconds: Int64,
        nowMonotonicMilliseconds: UInt64
    ) -> CoordinatorTransportStageReport? {
        purgeExpired(nowMonotonicMilliseconds)
        let key = Self.key(accountSlot: accountSlot, clientNonce: clientNonce)
        guard let pending = removePending(key) else { return nil }
        let report = CoordinatorTransportStageReport(
            accountSlot: accountSlot,
            clientNonce: clientNonce,
            stage: COORDINATOR_SSE_ECHO_STAGE,
            attempt: 0,
            traceparent: pending.traceparent,
            startEpochMilliseconds: nowEpochMilliseconds,
            durationMilliseconds: 0,
            isError: false
        )
        record(report: report)
        return report
    }

    func pendingEchoCount() -> Int {
        pendingEchoes.count
    }

    func snapshot() -> [CoordinatorTransportStage] {
        records
    }

    func reportSnapshot() -> [CoordinatorTransportStageReport] {
        reports
    }

    func reset() {
        records.removeAll(keepingCapacity: true)
        reports.removeAll(keepingCapacity: true)
        pendingEchoes.removeAll(keepingCapacity: true)
        pendingOrder.removeAll(keepingCapacity: true)
    }

    private static func key(accountSlot: String, clientNonce: String) -> String {
        "\(accountSlot)\u{0}\(clientNonce)"
    }

    @discardableResult
    private func removePending(_ key: String) -> CoordinatorPendingTransportEcho? {
        let removed = pendingEchoes.removeValue(forKey: key)
        if removed != nil {
            pendingOrder.removeAll { $0 == key }
        }
        return removed
    }

    private func purgeExpired(_ now: UInt64) {
        let expired = pendingEchoes.compactMap { key, pending -> String? in
            let age = now >= pending.armedAtMonotonicMilliseconds
                ? now - pending.armedAtMonotonicMilliseconds
                : 0
            return age > COORDINATOR_PENDING_SEND_ECHO_TTL_MILLISECONDS ? key : nil
        }
        for key in expired {
            removePending(key)
        }
    }
}

func coordinatorTransportEchoClientNonce(from payload: Any) -> String? {
    guard let event = payload as? [String: Any],
          let type = event["type"] as? String
    else { return nil }

    if type == "chat.message",
       event["role"] as? String == "user",
       let raw = event["messageId"] as? String {
        let value = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        return value.isEmpty ? nil : value
    }

    if type == "transcript.card",
       let card = event["card"] as? [String: Any],
       card["kind"] as? String == "user-attachment",
       let raw = card["clientNonce"] as? String {
        let value = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        return value.isEmpty ? nil : value
    }

    return nil
}
