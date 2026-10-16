import Foundation

struct IOSSendTraceSpanProjection: Equatable, Sendable {
    let level: IOSLifecycleTelemetryLevel
    let metadata: [String: String]
}

@MainActor
final class IOSProductionSendTraceExporter {
    typealias Reporter = @MainActor (IOSSendTraceSpanProjection) -> Void

    private let reporter: Reporter

    init(reporter: @escaping Reporter) {
        self.reporter = reporter
    }

    func ingest(method: String, report: [String: Any]) {
        for span in Self.project(method: method, report: report) {
            reporter(span)
        }
    }

    static func project(method: String, report: [String: Any]) -> [IOSSendTraceSpanProjection] {
        switch method {
        case "reportSendAck":
            return projectSendAck(report)
        case "reportRenderTtfr":
            return projectRenderFirstChunk(report)
        default:
            return []
        }
    }

    private static func projectSendAck(_ report: [String: Any]) -> [IOSSendTraceSpanProjection] {
        guard let traceparent = report["traceparent"] as? String,
              let parent = parseTraceparent(traceparent),
              let enterEpochMs = finiteNonnegative(report["enterEpochMs"]),
              let ackMs = finiteNonnegative(report["ackMs"]),
              let outcome = report["outcome"] as? String,
              ["acked", "failed", "timeout", "queued_offline"].contains(outcome),
              let isFork = report["isFork"] as? Bool,
              let attachmentCount = finiteNonnegative(report["attachmentCount"])
        else { return [] }

        var spans: [IOSSendTraceSpanProjection] = [
            makeSpan(
                name: "sand.send.ack",
                parent: parent,
                startEpochMs: enterEpochMs,
                durationMs: ackMs,
                isError: outcome == "failed" || outcome == "timeout",
                attributes: [
                    "sand.ack_ms": rounded(ackMs),
                    "sand.ack_outcome": outcome,
                    "sand.is_fork": String(isFork),
                    "sand.attachment_count": rounded(attachmentCount),
                ],
                clientNonce: boundedString(report["clientNonce"]),
                conversationID: boundedString(report["conversationId"])
            )
        ]

        if let commitOffsetMs = finiteNonnegative(report["commitOffsetMs"]),
           let commitMs = finiteNonnegative(report["commitMs"]) {
            spans.append(
                makeSpan(
                    name: "commit-attachments",
                    parent: parent,
                    startEpochMs: enterEpochMs + commitOffsetMs,
                    durationMs: commitMs,
                    isError: false,
                    attributes: [
                        "sand.attachment_count": rounded(attachmentCount),
                    ],
                    clientNonce: boundedString(report["clientNonce"]),
                    conversationID: nil
                )
            )
        }
        return spans
    }

    private static func projectRenderFirstChunk(
        _ report: [String: Any]
    ) -> [IOSSendTraceSpanProjection] {
        guard report["outcome"] as? String == "rendered",
              let traceparent = report["traceparent"] as? String,
              let parent = parseTraceparent(traceparent),
              let enterEpochMs = finiteNonnegative(report["enterEpochMs"]),
              let ttfrMs = finiteNonnegative(report["ttfrMs"])
        else { return [] }

        var attributes = [
            "sand.render_ttfr_ms": rounded(ttfrMs),
        ]
        if let chunkType = boundedString(report["chunkType"]) {
            attributes["sand.render_chunk_type"] = chunkType
        }
        return [
            makeSpan(
                name: "render-first-chunk",
                parent: parent,
                startEpochMs: enterEpochMs,
                durationMs: ttfrMs,
                isError: false,
                attributes: attributes,
                clientNonce: boundedString(report["clientNonce"]),
                conversationID: boundedString(report["conversationId"])
            )
        ]
    }

    private static func makeSpan(
        name: String,
        parent: ParsedTraceparent,
        startEpochMs: Double,
        durationMs: Double,
        isError: Bool,
        attributes: [String: String],
        clientNonce: String?,
        conversationID: String?
    ) -> IOSSendTraceSpanProjection {
        let child = deriveChildTraceparent(
            "00-\(parent.traceId)-\(parent.spanId)-\((parent.traceFlags & 1) == 1 ? "01" : "00")"
        )
        var metadata: [String: String] = [
            "event": "trace_span",
            "span_name": name,
            "span_kind": "internal",
            "trace_id": parent.traceId,
            "parent_span_id": parent.spanId,
            "start_epoch_ms": rounded(startEpochMs),
            "duration_ms": rounded(durationMs),
            "status": isError ? "error" : "ok",
            "platform_replacement": "ios_structured_log_trace_exporter",
        ]
        if let spanId = child?.spanId {
            metadata["span_id"] = spanId
        }
        if let clientNonce {
            metadata["sand.client_nonce"] = clientNonce
        }
        if let conversationID {
            metadata["sand.conversation_id"] = conversationID
        }
        for (key, value) in attributes {
            metadata[key] = value
        }
        return .init(
            level: isError ? .warn : .info,
            metadata: metadata
        )
    }

    private static func finiteNonnegative(_ raw: Any?) -> Double? {
        guard !(raw is Bool), let number = raw as? NSNumber else { return nil }
        let value = number.doubleValue
        return value.isFinite && value >= 0 ? value : nil
    }

    private static func rounded(_ value: Double) -> String {
        String(Int(value.rounded()))
    }

    private static func boundedString(_ raw: Any?) -> String? {
        guard let value = raw as? String,
              !value.isEmpty,
              value.count <= 256
        else { return nil }
        return value
    }
}
