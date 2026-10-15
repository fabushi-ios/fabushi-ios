import XCTest
@testable import Fabushi

final class ProductionSendTraceParityTests: XCTestCase {
    private let traceparent = "00-0123456789abcdef0123456789abcdef-0123456789abcdef-01"

    func testSendAckProjectsRootAndCommitStageThroughSameTrace() {
        let spans = IOSProductionSendTraceExporter.project(
            method: "reportSendAck",
            report: [
                "traceparent": traceparent,
                "enterEpochMs": 1_000.0,
                "ackMs": 250.0,
                "outcome": "acked",
                "isFork": false,
                "attachmentCount": 2.0,
                "clientNonce": "nonce-1",
                "conversationId": "conversation-1",
                "commitOffsetMs": 100.0,
                "commitMs": 40.0,
            ]
        )

        XCTAssertEqual(spans.count, 2)
        XCTAssertEqual(spans[0].metadata["span_name"], "sand.send.ack")
        XCTAssertEqual(spans[0].metadata["trace_id"], "0123456789abcdef0123456789abcdef")
        XCTAssertEqual(spans[0].metadata["parent_span_id"], "0123456789abcdef")
        XCTAssertEqual(spans[0].metadata["status"], "ok")
        XCTAssertEqual(spans[0].metadata["sand.ack_outcome"], "acked")
        XCTAssertEqual(spans[0].metadata["sand.client_nonce"], "nonce-1")
        XCTAssertEqual(spans[1].metadata["span_name"], "commit-attachments")
        XCTAssertEqual(spans[1].metadata["start_epoch_ms"], "1100")
        XCTAssertEqual(spans[1].metadata["duration_ms"], "40")
        XCTAssertEqual(spans[1].metadata["trace_id"], spans[0].metadata["trace_id"])
    }

    func testSendAckFailureMarksTraceSpanError() {
        let spans = IOSProductionSendTraceExporter.project(
            method: "reportSendAck",
            report: [
                "traceparent": traceparent,
                "enterEpochMs": 2_000.0,
                "ackMs": 500.0,
                "outcome": "timeout",
                "isFork": true,
                "attachmentCount": 0.0,
            ]
        )

        XCTAssertEqual(spans.count, 1)
        XCTAssertEqual(spans[0].level, .warn)
        XCTAssertEqual(spans[0].metadata["status"], "error")
    }

    func testRenderTraceOnlyEmitsForRenderedOutcomeAndValidParent() {
        let rendered = IOSProductionSendTraceExporter.project(
            method: "reportRenderTtfr",
            report: [
                "traceparent": traceparent,
                "enterEpochMs": 3_000.0,
                "ttfrMs": 120.0,
                "outcome": "rendered",
                "isFork": false,
                "chunkType": "assistant_text",
                "conversationId": "conversation-1",
            ]
        )
        XCTAssertEqual(rendered.count, 1)
        XCTAssertEqual(rendered[0].metadata["span_name"], "render-first-chunk")
        XCTAssertEqual(rendered[0].metadata["sand.render_chunk_type"], "assistant_text")
        XCTAssertEqual(rendered[0].metadata["duration_ms"], "120")

        XCTAssertTrue(IOSProductionSendTraceExporter.project(
            method: "reportRenderTtfr",
            report: [
                "traceparent": traceparent,
                "enterEpochMs": 3_000.0,
                "ttfrMs": 120.0,
                "outcome": "timeout",
                "isFork": false,
            ]
        ).isEmpty)

        XCTAssertTrue(IOSProductionSendTraceExporter.project(
            method: "reportSendAck",
            report: [
                "traceparent": "invalid",
                "enterEpochMs": 1.0,
                "ackMs": 1.0,
                "outcome": "acked",
                "isFork": false,
                "attachmentCount": 0.0,
            ]
        ).isEmpty)
    }

    func testShippingRuntimeReusesCanonicalLifecycleTransport() throws {
        let root = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let runtime = try String(
            contentsOf: root.appendingPathComponent("source/ios-main/IOSMainRuntime.swift"),
            encoding: .utf8
        )
        let exporter = try String(
            contentsOf: root.appendingPathComponent(
                "source/ios-main/telemetry/production-send-trace.swift"
            ),
            encoding: .utf8
        )

        XCTAssertTrue(runtime.contains("IOSProductionSendTraceExporter("))
        XCTAssertTrue(runtime.contains("sendTraceExporter.ingest(method: method, report: report)"))
        XCTAssertTrue(runtime.contains("lifecycleTelemetrySink.submit(record)"))
        XCTAssertTrue(exporter.contains("platform_replacement"))
        XCTAssertTrue(exporter.contains("ios_structured_log_trace_exporter"))
        XCTAssertFalse(exporter.contains("URLSession"))
        XCTAssertFalse(exporter.contains("authorization"))
        XCTAssertFalse(exporter.contains("OTLP"))
    }
}
