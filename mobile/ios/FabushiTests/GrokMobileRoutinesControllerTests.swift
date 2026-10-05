import XCTest
@testable import Fabushi

final class GrokMobileRoutinesControllerTests: XCTestCase {
    private let valid: [String: Any] = [
        "id": "routine-1",
        "agentId": "agent-1",
        "name": "Daily research",
        "prompt": "Summarize sources",
        "schedule": "@daily",
        "enabled": true,
        "createdAtMs": 10,
        "lastRunAtMs": 20,
        "nextRunAtMs": 30,
    ]

    func testRoutineProjectionRequiresDesktopLifecycleFields() {
        let projected = MobileBotRoutinesModel.parseAutomation(valid)
        XCTAssertEqual(projected?.id, "routine-1")
        XCTAssertEqual(projected?.agentId, "agent-1")
        XCTAssertEqual(projected?.name, "Daily research")
        XCTAssertEqual(projected?.prompt, "Summarize sources")
        XCTAssertEqual(projected?.schedule, "@daily")
        XCTAssertEqual(projected?.isEnabled, true)
        XCTAssertEqual(projected?.createdAtMs, 10)
        XCTAssertEqual(projected?.lastRunAtMs, 20)
        XCTAssertEqual(projected?.nextRunAtMs, 30)
    }

    func testRoutineProjectionFailsClosedOnMalformedRows() {
        for field in ["id", "agentId", "name", "prompt", "schedule", "enabled", "createdAtMs"] {
            var row = valid
            row.removeValue(forKey: field)
            XCTAssertNil(
                MobileBotRoutinesModel.parseAutomation(row),
                "missing \(field) must fail closed"
            )
        }

        for field in ["id", "agentId"] {
            var row = valid
            row[field] = "   "
            XCTAssertNil(
                MobileBotRoutinesModel.parseAutomation(row),
                "blank \(field) must fail closed"
            )
        }

        var badEnabled = valid
        badEnabled["enabled"] = "true"
        XCTAssertNil(MobileBotRoutinesModel.parseAutomation(badEnabled))

        var badLastRun = valid
        badLastRun["lastRunAtMs"] = "later"
        XCTAssertNil(MobileBotRoutinesModel.parseAutomation(badLastRun))
    }

    func testRoutineListRejectsPartialProjectionAndFiltersAgentScope() throws {
        var other = valid
        other["id"] = "routine-2"
        other["agentId"] = "agent-2"

        let rows = try MobileBotRoutinesModel.parseAutomations(
            [valid, other],
            agentId: "agent-1"
        )
        XCTAssertEqual(rows.map(\.id), ["routine-1"])

        var malformed = valid
        malformed.removeValue(forKey: "schedule")
        XCTAssertThrowsError(
            try MobileBotRoutinesModel.parseAutomations(
                [valid, malformed],
                agentId: "agent-1"
            )
        )
    }

    func testSnapshotPreservesDesktopLoadingReadyFailedAndUnavailableStates() {
        let routine = MobileBotRoutinesModel.parseAutomation(valid)!

        XCTAssertEqual(
            MobileBotRoutinesModel.snapshot(
                value: nil,
                error: nil,
                refreshing: false,
                capabilityUnavailable: false
            ),
            .loading(previous: [])
        )
        XCTAssertEqual(
            MobileBotRoutinesModel.snapshot(
                value: [],
                error: nil,
                refreshing: false,
                capabilityUnavailable: false
            ),
            .empty
        )
        XCTAssertEqual(
            MobileBotRoutinesModel.snapshot(
                value: [routine],
                error: nil,
                refreshing: true,
                capabilityUnavailable: false
            ),
            .ready([routine])
        )
        XCTAssertEqual(
            MobileBotRoutinesModel.snapshot(
                value: [routine],
                error: "network",
                refreshing: false,
                capabilityUnavailable: false
            ),
            .failed(
                value: [routine],
                previous: [routine],
                message: "network"
            )
        )
        XCTAssertEqual(
            MobileBotRoutinesModel.snapshot(
                value: nil,
                error: "source/capability-unavailable",
                refreshing: false,
                capabilityUnavailable: true
            ),
            .unavailable
        )
    }

    func testPendingPolicyDeduplicatesOnlyMatchingAutomation() {
        let pending = Set(["routine-1"])
        XCTAssertFalse(
            MobileBotRoutinesModel.canBegin("routine-1", pending: pending)
        )
        XCTAssertTrue(
            MobileBotRoutinesModel.canBegin("routine-2", pending: pending)
        )
    }

    func testCommandsStayOnCanonicalHostAutomationSurface() {
        let list = MobileBotRoutinesModel.commandList(
            agentId: "agent-1",
            requestId: "req-list"
        )
        XCTAssertEqual(list["type"] as? String, "automation.list")
        XCTAssertEqual(list["agentId"] as? String, "agent-1")

        let enabled = MobileBotRoutinesModel.commandSetEnabled(
            agentId: "agent-1",
            automationId: "routine-1",
            isEnabled: false,
            requestId: "req-enabled"
        )
        XCTAssertEqual(enabled["type"] as? String, "automation.setEnabled")
        XCTAssertEqual(enabled["id"] as? String, "routine-1")
        XCTAssertEqual(enabled["enabled"] as? Bool, false)

        let run = MobileBotRoutinesModel.commandRun(
            agentId: "agent-1",
            automationId: "routine-1",
            requestId: "req-run"
        )
        XCTAssertEqual(run["type"] as? String, "automation.run")
    }
}
