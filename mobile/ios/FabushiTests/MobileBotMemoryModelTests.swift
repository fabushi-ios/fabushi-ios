import XCTest
@testable import Fabushi

final class MobileBotMemoryModelTests: XCTestCase {
    func testScopedCommandsPreserveAgentUserAndProjectAuthority() {
        let agent = MobileBotMemoryModel.listCommand(
            agentId: "agent-a",
            scope: .agent,
            project: nil,
            requestId: "r-agent"
        )
        XCTAssertEqual(agent["type"] as? String, "memory.scopedList")
        XCTAssertEqual(agent["scope"] as? String, "agent")
        XCTAssertNil(agent["project"])

        let user = MobileBotMemoryModel.addCommand(
            agentId: "agent-a",
            scope: .user,
            project: nil,
            content: "  durable preference  ",
            kind: .profile,
            requestId: "r-user"
        )
        XCTAssertEqual(user?["scope"] as? String, "user")
        XCTAssertEqual(user?["content"] as? String, "durable preference")
        XCTAssertEqual(user?["kind"] as? String, "profile")

        let project = MobileBotMemoryModel.removeCommand(
            agentId: "agent-a",
            scope: .project,
            project: "alpha",
            memoryId: "memory-1",
            requestId: "r-project"
        )
        XCTAssertEqual(project["scope"] as? String, "project")
        XCTAssertEqual(project["project"] as? String, "alpha")
        XCTAssertEqual(project["id"] as? String, "memory-1")
    }

    func testProjectScopeRequiresNonEmptySlugBeforeUiRequest() {
        XCTAssertNil(MobileBotMemoryModel.normalizedProject("   ", scope: .project))
        XCTAssertEqual(
            MobileBotMemoryModel.normalizedProject("  alpha  ", scope: .project),
            "alpha"
        )
        XCTAssertNil(MobileBotMemoryModel.normalizedProject("ignored", scope: .agent))
    }

    func testMemoryEventProjectionRejectsWrongScopeAndMalformedRows() {
        let event: [String: Any] = [
            "type": "memory.listed",
            "agentId": "agent-a",
            "scope": "project",
            "project": "alpha",
            "memories": [[
                "id": "m1",
                "content": "Remember this",
                "createdAt": NSNumber(value: 1_700_000_000_000 as Int64),
                "kind": "profile",
            ]],
        ]
        XCTAssertTrue(
            MobileBotMemoryModel.matches(
                event: event,
                type: "memory.listed",
                agentId: "agent-a",
                scope: .project,
                project: "alpha"
            )
        )
        XCTAssertFalse(
            MobileBotMemoryModel.matches(
                event: event,
                type: "memory.listed",
                agentId: "agent-a",
                scope: .user,
                project: nil
            )
        )
        XCTAssertEqual(
            MobileBotMemoryModel.records(from: event),
            [
                MobileMemoryRecord(
                    id: "m1",
                    content: "Remember this",
                    createdAtMs: 1_700_000_000_000,
                    kind: .profile
                ),
            ]
        )
        var malformed = event
        malformed["memories"] = [["id": "m1", "content": "missing kind"]]
        XCTAssertNil(MobileBotMemoryModel.records(from: malformed))
    }
}
