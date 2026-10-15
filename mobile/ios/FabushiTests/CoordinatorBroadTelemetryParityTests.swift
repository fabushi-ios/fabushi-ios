import XCTest
@testable import Fabushi

final class CoordinatorBroadTelemetryParityTests: XCTestCase {
    func testAgentsUnreachableMatchesDesktopContract() {
        let entered = IOSCoordinatorBroadTelemetryProjection.project(
            method: "reportAgentsUnreachable",
            report: [
                "phase": "entered",
                "isManual": false,
                "errorClass": "gateway_unreachable",
                "msSinceLastReachedMs": 1_250.4,
            ]
        )
        XCTAssertEqual(entered?.event, "agents_unreachable")
        XCTAssertEqual(entered?.level, .warn)
        XCTAssertEqual(entered?.metadata["phase"], "entered")
        XCTAssertEqual(entered?.metadata["is_manual"], "false")
        XCTAssertEqual(entered?.metadata["error_class"], "gateway_unreachable")
        XCTAssertEqual(entered?.metadata["ms_since_last_reached"], "1250")

        let retry = IOSCoordinatorBroadTelemetryProjection.project(
            method: "reportAgentsUnreachable",
            report: ["phase": "retry", "isManual": true]
        )
        XCTAssertEqual(retry?.level, .info)

        let recovered = IOSCoordinatorBroadTelemetryProjection.project(
            method: "reportAgentsUnreachable",
            report: ["phase": "recovered", "isManual": false]
        )
        XCTAssertEqual(recovered?.level, .info)

        XCTAssertNil(IOSCoordinatorBroadTelemetryProjection.project(
            method: "reportAgentsUnreachable",
            report: ["phase": "failed", "isManual": false]
        ))
        XCTAssertNil(IOSCoordinatorBroadTelemetryProjection.project(
            method: "reportAgentsUnreachable",
            report: ["phase": "entered"]
        ))
        XCTAssertNil(IOSCoordinatorBroadTelemetryProjection.project(
            method: "reportAgentsUnreachable",
            report: ["phase": "entered", "isManual": false, "msSinceLastReachedMs": -1]
        ))
    }

    func testRecoveryActionMatchesDesktopFutilityThreshold() {
        let second = IOSCoordinatorBroadTelemetryProjection.project(
            method: "reportRecoveryAction",
            report: [
                "action": "retry",
                "failureClass": "Gateway_Unreachable",
                "consecutiveIdentical": 2,
            ]
        )
        XCTAssertEqual(second?.event, "recovery_action")
        XCTAssertEqual(second?.level, .info)
        XCTAssertEqual(second?.metadata["action"], "retry")
        XCTAssertEqual(second?.metadata["failure_class"], "gateway_unreachable")
        XCTAssertEqual(second?.metadata["consecutive_identical"], "2")

        let third = IOSCoordinatorBroadTelemetryProjection.project(
            method: "reportRecoveryAction",
            report: [
                "action": "recover",
                "failureClass": "timeout",
                "consecutiveIdentical": 3,
            ]
        )
        XCTAssertEqual(third?.level, .warn)

        XCTAssertNil(IOSCoordinatorBroadTelemetryProjection.project(
            method: "reportRecoveryAction",
            report: ["outcome": "failed"]
        ))
        XCTAssertNil(IOSCoordinatorBroadTelemetryProjection.project(
            method: "reportRecoveryAction",
            report: [
                "action": "retry",
                "failureClass": "contains space",
                "consecutiveIdentical": 1,
            ]
        ))
    }

    func testRebuildLifecycleMatchesDesktopProjection() {
        let escalation = IOSCoordinatorBroadTelemetryProjection.project(
            method: "reportRebuildLifecycle",
            report: [
                "kind": "escalation",
                "rebuildKind": "update",
                "elapsedMs": 1_200.4,
                "action": "shown",
                "escalation": "taking-longer",
            ]
        )
        XCTAssertEqual(escalation?.event, "escalation")
        XCTAssertEqual(escalation?.level, .warn)
        XCTAssertEqual(escalation?.metadata["kind"], "update")
        XCTAssertEqual(escalation?.metadata["elapsed_ms"], "1200")

        let keepWaiting = IOSCoordinatorBroadTelemetryProjection.project(
            method: "reportRebuildLifecycle",
            report: [
                "kind": "escalation",
                "rebuildKind": "recover",
                "elapsedMs": 2_000,
                "action": "keep_waiting",
                "escalation": "unreachable",
            ]
        )
        XCTAssertEqual(keepWaiting?.level, .info)

        let pending = IOSCoordinatorBroadTelemetryProjection.project(
            method: "reportRebuildLifecycle",
            report: [
                "kind": "pending_stall",
                "rebuildKind": "reconnecting",
                "elapsedMs": 5_000,
                "hasAck": true,
                "streamAttached": false,
            ]
        )
        XCTAssertEqual(pending?.event, "pending_stall")
        XCTAssertEqual(pending?.level, .warn)
        XCTAssertEqual(pending?.metadata["has_ack"], "true")
        XCTAssertEqual(pending?.metadata["stream_attached"], "false")

        XCTAssertNil(IOSCoordinatorBroadTelemetryProjection.project(
            method: "reportRebuildLifecycle",
            report: ["phase": "escalation"]
        ))
    }

    func testReconciliationMatchesDesktopProjection() {
        let covered = IOSCoordinatorBroadTelemetryProjection.project(
            method: "reportReconciliation",
            report: [
                "kind": "replica_resync",
                "replica": "roster",
                "outcome": "covered",
                "attempts": 2,
                "durationMs": 60_000,
                "bufferOverflowed": false,
                "epochChanged": true,
            ]
        )
        XCTAssertEqual(covered?.event, "replica_resync")
        XCTAssertEqual(covered?.level, .info)
        XCTAssertEqual(covered?.metadata["attempts"], "2")
        XCTAssertEqual(covered?.metadata["duration_ms"], "60000")
        XCTAssertEqual(covered?.metadata["buffer_overflowed"], "false")
        XCTAssertEqual(covered?.metadata["epoch_changed"], "true")

        let slow = IOSCoordinatorBroadTelemetryProjection.project(
            method: "reportReconciliation",
            report: [
                "kind": "replica_resync",
                "replica": "transcript",
                "outcome": "superseded",
                "attempts": 1,
                "durationMs": 60_001,
                "bufferOverflowed": false,
                "epochChanged": false,
            ]
        )
        XCTAssertEqual(slow?.level, .warn)

        let parked = IOSCoordinatorBroadTelemetryProjection.project(
            method: "reportReconciliation",
            report: [
                "kind": "send_journal_restore",
                "outcome": "parked",
                "phase": "accepted-awaiting-echo",
                "ageMs": 4_000,
                "recordCount": 3,
            ]
        )
        XCTAssertEqual(parked?.event, "send_journal_restore")
        XCTAssertEqual(parked?.level, .warn)
        XCTAssertEqual(parked?.metadata["age_ms"], "4000")
        XCTAssertEqual(parked?.metadata["record_count"], "3")

        XCTAssertNil(IOSCoordinatorBroadTelemetryProjection.project(
            method: "reportReconciliation",
            report: ["outcome": "partial"]
        ))
    }

    func testResyncProjectionRemainsBoundedAndUnknownMethodsAreRejected() {
        let long = String(repeating: "x", count: 800)
        let projection = IOSCoordinatorBroadTelemetryProjection.project(
            method: "reportResyncCompleted",
            report: ["failed": 1, "completed": 4, "detail": long, "nested": ["secret": "omit"]]
        )
        XCTAssertEqual(projection?.event, "resync_completed")
        XCTAssertEqual(projection?.level, .warn)
        XCTAssertEqual(projection?.metadata["detail"]?.count, 512)
        XCTAssertNil(projection?.metadata["nested"])
        XCTAssertNil(
            IOSCoordinatorBroadTelemetryProjection.project(
                method: "notTelemetry",
                report: [:]
            )
        )
    }

    func testShippingMainRuntimeInterceptsBroadTelemetryBeforeHost() throws {
        let repositoryRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let source = try String(
            contentsOf: repositoryRoot.appendingPathComponent("source/ios-main/IOSMainRuntime.swift"),
            encoding: .utf8
        )
        XCTAssertTrue(source.contains("IOSCoordinatorBroadTelemetryProjection.routedMethods.contains(method)"))
        XCTAssertTrue(source.contains("lifecycleReporter.report("))
        XCTAssertTrue(source.contains("lifecycleRecovery.markResyncCompleted()"))
        XCTAssertTrue(source.contains("return .ok(.object([:]))"))
    }
}
