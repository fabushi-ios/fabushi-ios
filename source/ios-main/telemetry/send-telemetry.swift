import Foundation

struct IOSSendLatencyReport: Equatable, Sendable {
    let durationMs: Double
    let commitMs: Double?
    let attachmentCount: Double
    let isFork: Bool
    let traceId: String?
    let spanId: String?
    let conversationId: String?
}

struct IOSTelemetryLogEvent: Equatable, Sendable {
    let level: String
    let metadata: [String: String]
}

private func iosFiniteNonnegativeNumber(_ value: Any?) -> Double? {
    guard !(value is Bool), let number = value as? NSNumber else {
        return nil
    }
    let value = number.doubleValue
    return value.isFinite && value >= 0 ? value : nil
}

func parseValidSendLatencyReport(_ value: Any?) -> IOSSendLatencyReport? {
    guard let record = value as? [String: Any],
          let durationMs = iosFiniteNonnegativeNumber(record["durationMs"]),
          let attachmentCount = iosFiniteNonnegativeNumber(record["attachmentCount"]),
          let isFork = record["isFork"] as? Bool
    else {
        return nil
    }

    let commitMs: Double?
    if record.keys.contains("commitMs") {
        guard let parsed = iosFiniteNonnegativeNumber(record["commitMs"]) else {
            return nil
        }
        commitMs = parsed
    } else {
        commitMs = nil
    }

    func optionalString(_ key: String) -> String?? {
        guard record.keys.contains(key) else { return .some(nil) }
        guard let string = record[key] as? String else { return nil }
        return .some(string)
    }

    guard let trace = optionalString("traceId"),
          let span = optionalString("spanId"),
          let conversation = optionalString("conversationId")
    else {
        return nil
    }

    return IOSSendLatencyReport(
        durationMs: durationMs,
        commitMs: commitMs,
        attachmentCount: attachmentCount,
        isFork: isFork,
        traceId: trace,
        spanId: span,
        conversationId: conversation
    )
}

func isValidSendLatencyReport(_ value: Any?) -> Bool {
    parseValidSendLatencyReport(value) != nil
}

func sendLatencyReportToTelemetry(_ report: IOSSendLatencyReport) -> IOSTelemetryLogEvent {
    var metadata: [String: String] = [
        "duration_ms": String(Int(report.durationMs.rounded())),
        "attachment_count": String(Int(report.attachmentCount.rounded())),
        "is_fork": String(report.isFork),
    ]
    if let commitMs = report.commitMs {
        metadata["commit_ms"] = String(Int(commitMs.rounded()))
    }
    if let traceId = report.traceId { metadata["trace_id"] = traceId }
    if let spanId = report.spanId { metadata["span_id"] = spanId }
    if let conversationId = report.conversationId {
        metadata["conversation_id"] = conversationId
    }
    return IOSTelemetryLogEvent(level: "info", metadata: metadata)
}
