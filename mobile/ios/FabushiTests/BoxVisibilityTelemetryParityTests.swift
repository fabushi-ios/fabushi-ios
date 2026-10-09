import XCTest
@testable import Fabushi

@MainActor
final class BoxVisibilityTelemetryParityTests: XCTestCase {
    func testSetupDurationUsesWarnLevelAndDedupesImmediateRebegin() {
        var now: Int64 = 1_000
        let reporter = IOSLifecycleReporter()
        var records: [IOSLifecycleTelemetryRecord] = []
        reporter.attach { records.append($0) }
        let tracker = IOSBoxVisibilityTracker(reporter: reporter, nowMs: { now })

        tracker.handle([
            "event": "setup",
            "phase": "begin",
            "trigger": "startup",
            "operationId": "op-1",
            "surface": "foreground",
        ], documentKey: "scene-a")
        now = 1_250
        tracker.handle([
            "event": "setup",
            "phase": "end",
            "outcome": "ready",
        ], documentKey: "scene-a")

        XCTAssertEqual(records.count, 1)
        XCTAssertEqual(records[0].family, .boxSetupVisible)
        XCTAssertEqual(records[0].level, .warn)
        XCTAssertEqual(records[0].metadata["duration_ms"], "250")
        XCTAssertEqual(records[0].metadata["operation_id"], "op-1")
        XCTAssertEqual(records[0].metadata["surface"], "foreground")

        now = 1_300
        tracker.handle([
            "event": "setup",
            "phase": "begin",
            "trigger": "duplicate",
        ], documentKey: "scene-a")
        tracker.handle([
            "event": "setup",
            "phase": "end",
            "outcome": "ready",
        ], documentKey: "scene-a")
        XCTAssertEqual(records.count, 1)
    }

    func testRecreateHeartbeatEscalatesAfterDesktopStallThreshold() {
        var now: Int64 = 10
        let reporter = IOSLifecycleReporter()
        var records: [IOSLifecycleTelemetryRecord] = []
        reporter.attach { records.append($0) }
        let tracker = IOSBoxVisibilityTracker(reporter: reporter, nowMs: { now })

        tracker.handle([
            "event": "recreate",
            "phase": "begin",
            "trigger": "reset",
            "operationId": "op-reset",
        ], documentKey: "scene-a")
        now += 600_000
        tracker.handle([
            "event": "recreate",
            "phase": "heartbeat",
            "stage": "creating",
            "migrationPhase": "moving",
        ], documentKey: "scene-a")

        XCTAssertEqual(records.count, 1)
        XCTAssertEqual(records[0].family, .boxRecreateVisible)
        XCTAssertEqual(records[0].level, .warn)
        XCTAssertEqual(records[0].metadata["phase"], "heartbeat")
        XCTAssertEqual(records[0].metadata["elapsed_ms"], "600000")
        XCTAssertEqual(records[0].metadata["stage"], "creating")
        XCTAssertEqual(records[0].metadata["migration_phase"], "moving")
        XCTAssertEqual(records[0].metadata["operation_id"], "op-reset")
    }

    func testStageTransitionIsBoundedAndAccountChangeAbandonsSetup() {
        var now: Int64 = 100
        let reporter = IOSLifecycleReporter()
        var records: [IOSLifecycleTelemetryRecord] = []
        reporter.attach { records.append($0) }
        let tracker = IOSBoxVisibilityTracker(reporter: reporter, nowMs: { now })

        tracker.noteAccountSlot("account-a")
        tracker.handle([
            "event": "recreate",
            "phase": "begin",
            "trigger": "upgrade",
            "operationId": "op-upgrade",
        ], documentKey: "scene-a")
        tracker.handle([
            "event": "recreate",
            "phase": "stage_transition",
            "kind": "update",
            "fromStage": "pulling",
            "stage": "restarting",
            "migrationPhase": "done",
            "stageElapsedMs": 42.6,
        ], documentKey: "scene-a")

        XCTAssertEqual(records.count, 1)
        XCTAssertEqual(records[0].family, .boxRebuildStage)
        XCTAssertEqual(records[0].level, .info)
        XCTAssertEqual(records[0].metadata["kind"], "update")
        XCTAssertEqual(records[0].metadata["from_stage"], "pulling")
        XCTAssertEqual(records[0].metadata["to_stage"], "restarting")
        XCTAssertEqual(records[0].metadata["stage_elapsed_ms"], "43")

        tracker.handle([
            "event": "setup",
            "phase": "begin",
            "trigger": "account",
        ], documentKey: "scene-a")
        now = 250
        tracker.noteAccountSlot("account-b")

        XCTAssertEqual(records.count, 2)
        XCTAssertEqual(records[1].family, .boxSetupVisible)
        XCTAssertEqual(records[1].metadata["outcome"], "abandoned")
        XCTAssertEqual(records[1].metadata["duration_ms"], "150")
    }

    func testInvalidTokensAndWrongDocumentDoNotLeakIntoTelemetry() {
        var now: Int64 = 0
        let reporter = IOSLifecycleReporter()
        var records: [IOSLifecycleTelemetryRecord] = []
        reporter.attach { records.append($0) }
        let tracker = IOSBoxVisibilityTracker(reporter: reporter, nowMs: { now })

        tracker.handle([
            "event": "recreate",
            "phase": "begin",
            "trigger": "reset",
            "operationId": "secret token with spaces",
            "surface": "hidden",
        ], documentKey: "scene-a")
        now = 20
        tracker.handle([
            "event": "recreate",
            "phase": "end",
            "outcome": "ready",
        ], documentKey: "scene-b")
        XCTAssertTrue(records.isEmpty)

        tracker.handle([
            "event": "recreate",
            "phase": "end",
            "outcome": "ready",
        ], documentKey: "scene-a")
        XCTAssertEqual(records.count, 1)
        XCTAssertEqual(records[0].family, .boxRecreateVisible)
        XCTAssertNil(records[0].metadata["operation_id"])
        XCTAssertNil(records[0].metadata["surface"])
    }
}
