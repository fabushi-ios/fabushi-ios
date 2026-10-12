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
        XCTAssertEqual(pending.listenerPlatform, "slack")
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


    func testListenerIntegrationHostProjectionAndAuthorizationURLStayFailClosed() throws {
        let projected = try XCTUnwrap(projectMobileListenerIntegrations([
            [
                "platform": "GitHub",
                "displayName": "GitHub",
                "blurb": "Watch repositories.",
                "isConnected": true,
                "accountLabel": "octocat",
            ],
            [
                "platform": "slack",
                "displayName": "Slack",
                "blurb": "Watch messages.",
                "isConnected": false,
                "error": "reauth required",
            ],
            ["platform": "", "displayName": "Broken", "blurb": "", "isConnected": false],
            ["platform": "linear", "displayName": "Linear", "blurb": "Watch issues."],
        ]))
        XCTAssertEqual(projected.count, 2)
        XCTAssertEqual(projected["github"]?.accountLabel, "octocat")
        XCTAssertTrue(projected["github"]?.isConnected == true)
        XCTAssertEqual(projected["slack"]?.error, "reauth required")
        XCTAssertFalse(projected["slack"]?.isConnected == true)

        XCTAssertEqual(
            validatedMobileListenerAuthorizationURL("https://example.com/oauth?state=abc")?.scheme,
            "https"
        )
        XCTAssertNil(validatedMobileListenerAuthorizationURL("http://example.com/oauth"))
        XCTAssertNil(validatedMobileListenerAuthorizationURL("javascript:alert(1)"))
        XCTAssertNil(validatedMobileListenerAuthorizationURL("https:///missing-host"))
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

    @MainActor
    func testPublishedCurrentUserPluginProjectsIntoYoursAsReadOnlyLifecycleItem() throws {
        let skill = try XCTUnwrap(MarketplaceModel.marketplacePrivateSkill(from: [
            "id": "plugin-91-release-check",
            "name": "Release check",
            "description": "Verify release evidence",
            "body": "Inspect production evidence.",
            "source": "plugin",
            "pluginId": "91",
            "publishedByCurrentUser": true,
            "isEnabledForAgent": true,
        ]))
        XCTAssertEqual(skill.pluginId, "91")
        XCTAssertTrue(skill.publishedByCurrentUser)
        XCTAssertFalse(skill.canEdit)
        XCTAssertFalse(skill.canToggle)
        XCTAssertEqual(skill.sourceLabel, "Shared with your team")
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


    func testMarketplacePluginAccountIdentityRequiresCompleteStrictHostIdentity() throws {
        let identity = try XCTUnwrap(MarketplaceModel.marketplacePluginAccountIdentity(from: [
            "protocol": "fabushi.plugin-account-identity.v1",
            "accountScope": String(repeating: "a", count: 64),
            "accountGeneration": String(repeating: "b", count: 64),
        ]))
        XCTAssertEqual(identity.accountScope, String(repeating: "a", count: 64))
        XCTAssertEqual(identity.accountGeneration, String(repeating: "b", count: 64))

        let params = MarketplaceModel.accountBoundParams(
            ["pluginId": "global-dharma"],
            identity: identity
        )
        XCTAssertEqual(params["pluginId"] as? String, "global-dharma")
        XCTAssertEqual(
            params["expectedAccountScope"] as? String,
            String(repeating: "a", count: 64)
        )
        XCTAssertEqual(
            params["expectedAccountGeneration"] as? String,
            String(repeating: "b", count: 64)
        )
        XCTAssertNil(MarketplaceModel.marketplacePluginAccountIdentity(from: [
            "protocol": "fabushi.plugin-account-identity.v1",
            "accountScope": String(repeating: "A", count: 64),
            "accountGeneration": String(repeating: "b", count: 64),
        ]))
        XCTAssertNil(MarketplaceModel.marketplacePluginAccountIdentity(from: [
            "protocol": "fabushi.plugin-account-identity.v1",
            "accountScope": String(repeating: "a", count: 64),
        ]))
    }

    func testMarketplaceInstalledProjectionMergesLocalAndCanonicalAccountTruth() {
        let ids = MarketplaceModel.projectInstalledMiniAppIds(
            localRows: [
                ["pluginId": "local-only"],
                ["pluginId": " shared "],
                ["pluginId": ""],
            ],
            accountRows: [
                ["id": "server-only"],
                ["pluginId": "shared"],
                ["pluginId": "   "],
            ]
        )
        XCTAssertEqual(ids, Set(["local-only", "shared", "server-only"]))
    }

    @MainActor
    func testMarketplaceInstalledProjectionIsAccountScopedUIState() throws {
        let appDataDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("fabushi-marketplace-installed-scope-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(
            at: appDataDirectory,
            withIntermediateDirectories: true
        )
        defer { try? FileManager.default.removeItem(at: appDataDirectory) }
        let main = try IOSMainRuntime(
            appDataDirectory: appDataDirectory,
            featureHostTest: true
        )
        let model = MarketplaceModel(bridge: IOSPreloadBridge(main: main))
        model.installedPluginIds = ["global-dharma"]
        XCTAssertTrue(model.installedPluginIds.contains("global-dharma"))
        model.installedPluginIds.remove("global-dharma")
        XCTAssertTrue(model.installedPluginIds.isEmpty)
    }

    func testMarketplaceYoursShippingSurfaceRefreshesAuthoritativePluginSkillsBeforeHostWorkflowList() throws {
        let testFile = URL(fileURLWithPath: #filePath)
        let repositoryRoot = testFile
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()

        let marketplaceModelSource = try String(
            contentsOf: repositoryRoot
                .appendingPathComponent("mobile/ios/Fabushi/MarketplaceModel.swift"),
            encoding: .utf8
        )
        let marketplaceSurfaceSource = try String(
            contentsOf: repositoryRoot
                .appendingPathComponent("frontend/src/recovered/features/conversation/ContentView+ChatMarketplace.swift"),
            encoding: .utf8
        )
        let coordinatorSource = try String(
            contentsOf: repositoryRoot
                .appendingPathComponent("source/mahayana-agent-coordinator/coordinator-mcp-manager.swift"),
            encoding: .utf8
        )

        XCTAssertTrue(
            marketplaceSurfaceSource.contains("Task { await model.refreshPrivateSkills() }"),
            "The shipping Yours refresh control must keep calling the authoritative skill refresh."
        )
        XCTAssertTrue(
            marketplaceSurfaceSource.contains("await model.refreshPrivateSkills()"),
            "The shipping Yours task/tab lifecycle must keep refreshing installed Marketplace skills."
        )

        let refreshStart = try XCTUnwrap(
            marketplaceModelSource.range(of: "func refreshPrivateSkills() async")
        )
        let refreshTail = marketplaceModelSource[refreshStart.lowerBound...]
        let syncRange = try XCTUnwrap(
            refreshTail.range(of: #"method: "coordinator.skill.pluginFactsSync""#)
        )
        let listRange = try XCTUnwrap(
            refreshTail.range(of: #"type: "workflow.list""#)
        )
        XCTAssertLessThan(
            refreshTail.distance(from: refreshTail.startIndex, to: syncRange.lowerBound),
            refreshTail.distance(from: refreshTail.startIndex, to: listRange.lowerBound),
            "Installed Marketplace plugin facts must be synchronized before the Host workflow list is read."
        )

        XCTAssertTrue(
            coordinatorSource.contains(#"case "coordinator.skill.pluginFactsSync":"#)
        )
        XCTAssertTrue(
            coordinatorSource.contains("refreshAuthoritativePluginFacts(agentId: agentId)")
        )
        XCTAssertTrue(
            coordinatorSource.contains(#"method: "feature.workflow.pluginSkillsSync""#),
            "The shipping Coordinator route must continue handing installed plugin skills to the single Rust Host owner."
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
