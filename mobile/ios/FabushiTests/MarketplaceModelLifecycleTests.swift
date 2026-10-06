import XCTest
@testable import Fabushi

final class MarketplaceModelLifecycleTests: XCTestCase {
    func testMahayanaChatPumpOutcomeOnlySettlesOnTerminalEvent() {
        XCTAssertTrue(MahayanaChatPumpOutcome.terminal.shouldSettleLifecycle)
        XCTAssertFalse(MahayanaChatPumpOutcome.nonTerminal.shouldSettleLifecycle)
    }

    func testListenerConnectTranscriptCardProjectsHostTruthWithoutOwningResumeState() throws {
        let pending = try XCTUnwrap(projectListenerConnectTranscriptCard(
            event: [
                "type": "transcript.card",
                "entryId": "listener-connect:mahayana-assistant:slack",
                "card": [
                    "kind": "listenerConnect",
                    "platform": "slack",
                    "reason": "so this routine can fire",
                    "connected": false,
                    "pending": true,
                ],
            ],
            operationId: "op-1"
        ))
        XCTAssertEqual(pending.kind, .action)
        XCTAssertEqual(pending.actionTitle, "连接 Slack")
        XCTAssertEqual(pending.actionDetail, "so this routine can fire")
        XCTAssertEqual(pending.actionStatus, "pending")
        XCTAssertEqual(pending.operationId, "op-1")

        let connected = try XCTUnwrap(projectListenerConnectTranscriptCard(
            event: [
                "type": "transcript.card",
                "entryId": pending.id,
                "card": [
                    "kind": "listenerConnect",
                    "platform": "slack",
                    "connected": true,
                    "pending": false,
                ],
            ],
            operationId: "op-1"
        ))
        XCTAssertEqual(connected.id, pending.id)
        XCTAssertEqual(connected.actionTitle, "Slack 已连接")
        XCTAssertEqual(connected.actionStatus, "completed")
        XCTAssertTrue(connected.actionDetail?.contains("已连接") == true)
    }
    func testListenerProjectionFallbackIdentityAndDefaultDetailArePlatformSpecific() throws {
        func card(_ platform: String, connected: Bool) throws -> MobileChatMessage {
            try XCTUnwrap(projectListenerConnectTranscriptCard(
                event: ["card": ["kind": "listenerConnect", "platform": platform,
                                  "connected": connected, "reason": "  "]],
                operationId: nil
            ))
        }
        let slack = try card("Slack", connected: false)
        let github = try card("github", connected: true)
        XCTAssertEqual(slack.id, "listener-connect:slack")
        XCTAssertEqual(github.id, "listener-connect:github")
        XCTAssertNotEqual(slack.id, github.id)
        XCTAssertEqual(slack.actionDetail, "连接 Slack 后，此例程才能接收对应事件。")
        XCTAssertEqual(github.actionDetail, "GitHub 已连接。")
    }


    func testPrivateSkillProjectionUsesWorkflowOwnerAndRejectsAutomationRows() throws {
        let skill = try XCTUnwrap(MarketplaceModel.marketplacePrivateSkill(from: [
            "id": "review-release",
            "name": "Review release",
            "description": "Use before publishing.",
            "body": "Inspect the diff and verify evidence.",
            "source": "workflow",
            "sourceRef": "local://review-release",
            "pluginId": NSNull(),
            "publishedByCurrentUser": false,
            "isEnabledForAgent": true,
        ]))
        XCTAssertEqual(skill.id, "review-release")
        XCTAssertEqual(skill.sourceLabel, "Private skill")
        XCTAssertTrue(skill.canEdit)
        XCTAssertTrue(skill.canToggle)
        XCTAssertTrue(skill.isEnabledForAgent)

        XCTAssertNil(MarketplaceModel.marketplacePrivateSkill(from: [
            "id": "nightly",
            "name": "Nightly",
            "body": "Run nightly.",
            "source": "automation",
        ]))
    }

    func testPrivateSkillOwnershipAndSearchFilteringMatchDesktopYoursSemantics() {
        let privateSkill = MarketplacePrivateSkill(
            id: "private",
            name: "Release check",
            description: "Verify a release",
            body: "Inspect diffs",
            source: "workflow",
            sourceRef: nil,
            pluginId: nil,
            publishedByCurrentUser: false,
            isEnabledForAgent: true,
            triggerSchedule: nil,
            triggerEnabled: nil
        )
        let teamSkill = MarketplacePrivateSkill(
            id: "team",
            name: "Team triage",
            description: "Shared workflow",
            body: "Triage failures",
            source: "plugin",
            sourceRef: nil,
            pluginId: "team-plugin",
            publishedByCurrentUser: true,
            isEnabledForAgent: true,
            triggerSchedule: nil,
            triggerEnabled: nil
        )
        XCTAssertEqual(
            MarketplaceModel.filterPrivateSkills(
                [privateSkill, teamSkill],
                query: "",
                ownership: .team
            ).map(\.id),
            ["team"]
        )
        XCTAssertEqual(
            MarketplaceModel.filterPrivateSkills(
                [privateSkill, teamSkill],
                query: "release",
                ownership: .all
            ).map(\.id),
            ["private"]
        )
        XCTAssertEqual(
            MarketplaceModel.filterPrivateSkills(
                [privateSkill, teamSkill],
                query: "",
                ownership: .publicItems
            ).map(\.id),
            ["private"]
        )
    }

}
