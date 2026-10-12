import XCTest
@testable import Fabushi

final class CoordinatorBroadTelemetryParityTests: XCTestCase {
    func testAgentsUnreachableRetryIsInfoAndTerminalPhaseWarns() {
        let retry = IOSCoordinatorBroadTelemetryProjection.project(
            method: "reportAgentsUnreachable",
            report: ["phase": "retry", "attempt": 2]
        )
        XCTAssertEqual(retry?.event, "agents_unreachable")
        XCTAssertEqual(retry?.level, .info)
        XCTAssertEqual(retry?.metadata["attempt"], "2")

        let failed = IOSCoordinatorBroadTelemetryProjection.project(
            method: "reportAgentsUnreachable",
            report: ["phase": "failed", "cause": "host_unreachable"]
        )
        XCTAssertEqual(failed?.level, .warn)
        XCTAssertEqual(failed?.metadata["cause"], "host_unreachable")
    }

    func testRecoveryRebuildReconciliationAndResyncProjection() {
        XCTAssertEqual(
            IOSCoordinatorBroadTelemetryProjection.project(
                method: "reportRecoveryAction",
                report: ["outcome": "failed"]
            )?.level,
            .warn
        )
        XCTAssertEqual(
            IOSCoordinatorBroadTelemetryProjection.project(
                method: "reportRebuildLifecycle",
                report: ["phase": "escalation"]
            )?.level,
            .warn
        )
        XCTAssertEqual(
            IOSCoordinatorBroadTelemetryProjection.project(
                method: "reportReconciliation",
                report: ["outcome": "partial"]
            )?.level,
            .warn
        )
        let resync = IOSCoordinatorBroadTelemetryProjection.project(
            method: "reportResyncCompleted",
            report: ["failed": 1, "completed": 4]
        )
        XCTAssertEqual(resync?.event, "resync_completed")
        XCTAssertEqual(resync?.level, .warn)
        XCTAssertEqual(resync?.metadata["completed"], "4")
    }

    func testProjectionBoundsMetadataAndRejectsUnknownMethods() {
        let long = String(repeating: "x", count: 800)
        let projection = IOSCoordinatorBroadTelemetryProjection.project(
            method: "reportRecoveryAction",
            report: ["outcome": "ok", "detail": long, "nested": ["secret": "omit"]]
        )
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
