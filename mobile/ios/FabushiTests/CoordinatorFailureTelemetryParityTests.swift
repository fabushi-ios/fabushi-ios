import XCTest
@testable import Fabushi

final class CoordinatorFailureTelemetryParityTests: XCTestCase {
    @MainActor
    func testFailureEpisodeDedupesSameCodeAndRecoversWithFailedDuration() {
        var now = 1_000
        var events: [IOSCoordinatorFailureTelemetryProjection] = []
        let tracker = IOSCoordinatorFailureTelemetryTracker(
            nowMilliseconds: { now },
            reporter: { events.append($0) }
        )
        tracker.activate(generation: 7)

        let failure = IOSCoordinatorPortClient.PortError(
            code: "gateway-unreachable",
            message: "offline",
            transportKind: "network"
        )
        tracker.noteFailure(method: "sendPrompt", error: failure, generation: 7)
        now = 1_500
        tracker.noteFailure(method: "sendPrompt", error: failure, generation: 7)

        XCTAssertEqual(events.count, 1)
        XCTAssertEqual(events[0].domain, "send")
        XCTAssertEqual(events[0].operation, "sendPrompt")
        XCTAssertEqual(events[0].failureCode, "gateway-unreachable")
        XCTAssertEqual(events[0].state, .failed)
        XCTAssertNil(events[0].failedMilliseconds)

        now = 2_600
        tracker.noteSuccess(method: "sendPrompt", generation: 7)
        XCTAssertEqual(events.count, 2)
        XCTAssertEqual(events[1].state, .recovered)
        XCTAssertEqual(events[1].failedMilliseconds, 1_600)
        XCTAssertEqual(events[1].metadata["failed_ms"], "1600")
    }

    @MainActor
    func testDifferentFailureCodeStartsNewReportButKeepsEpisodeStart() {
        var now = 100
        var events: [IOSCoordinatorFailureTelemetryProjection] = []
        let tracker = IOSCoordinatorFailureTelemetryTracker(
            nowMilliseconds: { now },
            reporter: { events.append($0) }
        )
        tracker.activate(generation: 2)

        tracker.noteFailure(
            method: "listRoutedMcpTools",
            error: .init(code: "gateway-unreachable", message: "first", transportKind: "network"),
            generation: 2
        )
        now = 300
        tracker.noteFailure(
            method: "listRoutedMcpTools",
            error: .init(code: "gateway-command-failed", message: "second"),
            generation: 2
        )
        now = 900
        tracker.noteSuccess(method: "listRoutedMcpTools", generation: 2)

        XCTAssertEqual(events.map(\.state), [.failed, .failed, .recovered])
        XCTAssertEqual(events.map(\.failureCode), [
            "gateway-unreachable",
            "gateway-command-failed",
            "gateway-command-failed",
        ])
        XCTAssertEqual(events.last?.failedMilliseconds, 800)
        XCTAssertEqual(events.last?.domain, "plugins")
    }

    @MainActor
    func testCancellationAndCapabilityUnavailableDoNotCreateEpisodes() {
        var events: [IOSCoordinatorFailureTelemetryProjection] = []
        let tracker = IOSCoordinatorFailureTelemetryTracker(reporter: { events.append($0) })
        tracker.activate(generation: 4)

        tracker.noteFailure(
            method: "sendPrompt",
            error: .init(code: "source/cancelled", message: "cancelled"),
            generation: 4
        )
        tracker.noteFailure(
            method: "sendPrompt",
            error: .init(code: "source/capability-unavailable", message: "not supported"),
            generation: 4
        )
        tracker.noteFailure(
            method: "sendPrompt",
            error: .init(code: "port-settled", message: "pre-port", transportKind: "pre_port_settle"),
            generation: 4
        )

        XCTAssertTrue(events.isEmpty)
    }

    @MainActor
    func testStaleGenerationCannotReportOrRecoverCurrentEpisode() {
        var now = 10
        var events: [IOSCoordinatorFailureTelemetryProjection] = []
        let tracker = IOSCoordinatorFailureTelemetryTracker(
            nowMilliseconds: { now },
            reporter: { events.append($0) }
        )
        tracker.activate(generation: 10)

        tracker.noteFailure(
            method: "getAgentTranscriptWindow",
            error: .init(code: "gateway-unreachable", message: "old", transportKind: "network"),
            generation: 9
        )
        XCTAssertTrue(events.isEmpty)

        tracker.noteFailure(
            method: "getAgentTranscriptWindow",
            error: .init(code: "gateway-unreachable", message: "current", transportKind: "network"),
            generation: 10
        )
        XCTAssertEqual(events.count, 1)

        tracker.activate(generation: 11)
        now = 50
        tracker.noteSuccess(method: "getAgentTranscriptWindow", generation: 10)
        XCTAssertEqual(events.count, 1, "retired transport must not recover a current episode")

        tracker.noteSuccess(method: "getAgentTranscriptWindow", generation: 11)
        XCTAssertEqual(events.count, 2)
        XCTAssertEqual(events.last?.state, .recovered)
        XCTAssertEqual(events.last?.failedMilliseconds, 40)
        XCTAssertEqual(events.last?.domain, "transcript")
    }

    func testShippingPreloadSharesFailureTrackerAcrossTransportReplacement() throws {
        let repositoryRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let preload = try String(
            contentsOf: repositoryRoot.appendingPathComponent("source/ios-preload/preload.swift"),
            encoding: .utf8
        )
        let port = try String(
            contentsOf: repositoryRoot.appendingPathComponent("source/ios-preload/coordinator-port-bridge.swift"),
            encoding: .utf8
        )

        XCTAssertTrue(preload.contains("failureTelemetryTracker.activate(generation: generation)"))
        XCTAssertTrue(preload.contains("failureTelemetry: failureTelemetryTracker"))
        XCTAssertTrue(port.contains("failureTelemetry?.noteFailure("))
        XCTAssertTrue(port.contains("failureTelemetry?.noteSuccess("))
        XCTAssertTrue(port.contains("catch let error as PortError"))
    }
}
