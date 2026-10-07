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


    @MainActor
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
            "id": "other-team-skill",
            "name": "Other member skill",
            "description": "Must not leak into Yours.",
            "body": "Shared body",
            "source": "plugin",
            "pluginId": "team-plugin",
            "publishedByCurrentUser": false,
            "isEnabledForAgent": true,
        ]))

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


    @MainActor
    func testPrivateSkillScopeFailsClosedAndCanBeExplicitlyBoundToAgent() throws {
        let appDataDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("fabushi-private-skill-scope-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(
            at: appDataDirectory,
            withIntermediateDirectories: true
        )
        defer { try? FileManager.default.removeItem(at: appDataDirectory) }
        let main = try IOSMainRuntime(
            appDataDirectory: appDataDirectory,
            featureHostTest: true
        )
        let bridge = IOSPreloadBridge(main: main)
        let model = MarketplaceModel(bridge: bridge)
        XCTAssertFalse(model.hasPrivateSkillAgentScope)
        XCTAssertNil(model.privateSkillAgentId)
        model.bindPrivateSkillAgentScope(agentId: " agent-42 ", agentName: " Research ")
        XCTAssertTrue(model.hasPrivateSkillAgentScope)
        XCTAssertEqual(model.privateSkillAgentId, "agent-42")
        XCTAssertEqual(model.privateSkillAgentName, "Research")
        model.clearPrivateSkillAgentScope(agentId: "other-agent")
        XCTAssertEqual(model.privateSkillAgentId, "agent-42")
        model.clearPrivateSkillAgentScope(agentId: "agent-42")
        XCTAssertFalse(model.hasPrivateSkillAgentScope)
        XCTAssertTrue(model.privateSkills.isEmpty)
        XCTAssertTrue(model.privateSkillNameDrafts.isEmpty)
    }

}


final class MarketplaceMcpTeamPolicyProjectionTests: XCTestCase {
    @MainActor
    func testMcpServerProjectionPreservesTeamPolicyFacts() throws {
        let server = try XCTUnwrap(MarketplaceModel.mcpServer(from: [
            "id": "17",
            "name": "Team GitHub",
            "serverIdentifier": "github",
            "accountKey": "default",
            "transport": "http",
            "status": "disabledByTeamAdminPolicy",
            "statusDetail": "Disabled by team admin",
            "toolCount": 0,
            "disabledToolCount": 0,
            "isTeamServer": true,
            "pluginId": "42",
            "isRequired": true,
            "managedByTeamPluginPolicy": true,
        ]))

        XCTAssertTrue(server.isTeamServer)
        XCTAssertEqual(server.pluginId, "42")
        XCTAssertTrue(server.isRequired)
        XCTAssertTrue(server.managedByTeamPluginPolicy)
        XCTAssertTrue(server.isDisabledByTeamAdminPolicy)
    }

    @MainActor
    func testMcpServerProjectionDefaultsPolicyFactsForPersonalRows() throws {
        let server = try XCTUnwrap(MarketplaceModel.mcpServer(from: [
            "id": "18",
            "name": "Personal",
            "serverIdentifier": "personal",
            "accountKey": "default",
            "transport": "http",
            "status": "connected",
            "toolCount": 1,
            "disabledToolCount": 0,
        ]))

        XCTAssertFalse(server.isTeamServer)
        XCTAssertNil(server.pluginId)
        XCTAssertFalse(server.isRequired)
        XCTAssertFalse(server.managedByTeamPluginPolicy)
        XCTAssertFalse(server.isDisabledByTeamAdminPolicy)
    }
}
