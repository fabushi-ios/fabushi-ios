import XCTest
@testable import Fabushi

final class MarketplaceModelLifecycleTests: XCTestCase {
    private func trustedGlobalDharmaReleaseManifest() -> [String: Any] {
        [
            "schemaVersion": 1,
            "protocol": "mahayana.external-release.v1",
            "pluginId": "global-dharma",
            "version": "1.1.0",
            "installMode": "package",
            "permissions": [
                "offline.capabilities",
                "offline.state.read",
                "offline.practice.record",
                "local.prayer-wheel.start",
            ],
            "source": [
                "provider": "github",
                "repository": "https://github.com/bhrumom/fabushi-marketplace",
                "sourceRef": String(repeating: "b", count: 40),
            ],
            "commands": [
                [
                    "tool": "offline.capabilities",
                    "description": "Read capabilities",
                    "approval": "none",
                    "inputSchema": ["type": "object", "properties": [:], "additionalProperties": false],
                ],
                [
                    "tool": "offline.state.read",
                    "description": "Read state",
                    "approval": "none",
                    "inputSchema": ["type": "object", "properties": [:], "additionalProperties": false],
                ],
                [
                    "tool": "offline.practice.record",
                    "description": "Record practice",
                    "approval": "required",
                    "inputSchema": ["type": "object", "properties": ["label": ["type": "string"]], "additionalProperties": false],
                ],
                [
                    "tool": "local.prayer-wheel.start",
                    "description": "Start prayer wheel",
                    "approval": "required",
                    "inputSchema": ["type": "object", "properties": [:], "additionalProperties": false],
                ],
            ],
            "artifacts": [[
                "id": "global-dharma-ios-fabapp",
                "runtime": "web",
                "platforms": ["ios"],
                "source": [
                    "type": "https",
                    "url": "https://github.com/bhrumom/fabushi-marketplace/releases/download/global-dharma-v1.1.0/global-dharma-1.1.0.fabapp",
                ],
                "sha256": String(repeating: "c", count: 64),
                "size": 8192,
                "format": "zip",
                "entry": "index.html",
            ]],
            "fabappTrust": [
                "publisherId": "fabushi-official",
                "appId": "global-dharma",
                "artifactId": "global-dharma-ios-fabapp",
                "keyId": "fabushi-marketplace-prod-v1",
                "rawPublicKeyBase64Url": String(repeating: "A", count: 43),
            ],
        ]
    }

    @MainActor
    func testGlobalDharmaIOSMarketplaceFailsClosedUntilSignedFabappIsPresent() throws {
        let legacy = [
            "pluginId": "global-dharma",
            "displayName": "全球法布施",
            "latestVersion": "1.0.0",
            "releaseManifest": [
                "pluginId": "global-dharma",
                "version": "1.0.0",
                "installMode": "package",
            ],
        ] as [String: Any]
        XCTAssertNil(MarketplaceModel.marketplacePlugin(from: legacy))

        var unsigned = trustedGlobalDharmaReleaseManifest()
        unsigned.removeValue(forKey: "fabappTrust")
        XCTAssertNil(MarketplaceModel.marketplacePlugin(from: [
            "pluginId": "global-dharma",
            "displayName": "全球法布施",
            "latestVersion": "1.1.0",
            "releaseManifest": unsigned,
        ]))

        let trusted = try XCTUnwrap(MarketplaceModel.marketplacePlugin(from: [
            "pluginId": "global-dharma",
            "displayName": "全球法布施",
            "latestVersion": "1.1.0",
            "releaseManifest": trustedGlobalDharmaReleaseManifest(),
        ]))
        XCTAssertEqual(trusted.latestVersion, "1.1.0")
        XCTAssertEqual(
            trusted.tools.map(\.name),
            [
                "offline.capabilities",
                "offline.state.read",
                "offline.practice.record",
                "local.prayer-wheel.start",
            ]
        )
    }

    @MainActor
    func testGlobalDharmaActivePointerCannotRollBackToLegacyPackage() {
        let legacy = MarketplacePlugin(
            pluginId: "global-dharma",
            displayName: "全球法布施",
            description: "",
            latestVersion: "1.0.0",
            tools: []
        )
        XCTAssertFalse(MarketplaceModel.activeLocalInstallSatisfies(
            plugin: legacy,
            pointer: ["pluginId": "global-dharma", "version": "1.0.0"]
        ))

        let current = MarketplacePlugin(
            pluginId: "global-dharma",
            displayName: "全球法布施",
            description: "",
            latestVersion: "1.1.0",
            tools: []
        )
        XCTAssertTrue(MarketplaceModel.activeLocalInstallSatisfies(
            plugin: current,
            pointer: ["pluginId": "global-dharma", "version": "1.1.0"]
        ))
        XCTAssertFalse(MarketplaceModel.activeLocalInstallSatisfies(
            plugin: current,
            pointer: ["pluginId": "global-dharma", "version": "1.0.0"]
        ))

        let generic = MarketplacePlugin(
            pluginId: "other-miniapp",
            displayName: "Other",
            description: "",
            latestVersion: "1.0.0",
            tools: []
        )
        XCTAssertTrue(MarketplaceModel.activeLocalInstallSatisfies(
            plugin: generic,
            pointer: ["pluginId": "other-miniapp", "version": "1.0.0"]
        ))
    }

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


    func testPluginPermissionRequestRetainsInstallAccountGeneration() {
        let original = MarketplacePluginAccountIdentity(
            accountScope: String(repeating: "a", count: 64),
            accountGeneration: String(repeating: "b", count: 64)
        )
        let request = PluginPermissionRequest(
            pluginId: "global-dharma",
            runtime: "local-web",
            permissions: ["network"],
            accountIdentity: original
        )
        XCTAssertTrue(request.accepts(identity: original))
        XCTAssertFalse(request.accepts(identity: MarketplacePluginAccountIdentity(
            accountScope: original.accountScope,
            accountGeneration: String(repeating: "c", count: 64)
        )))
        XCTAssertFalse(request.accepts(identity: MarketplacePluginAccountIdentity(
            accountScope: String(repeating: "d", count: 64),
            accountGeneration: original.accountGeneration
        )))
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

    func testPluginCodeRemovalRequestUsesCodeOnlyHostRouteAndExactGeneration() {
        let identity = MarketplacePluginAccountIdentity(
            accountScope: String(repeating: "a", count: 64),
            accountGeneration: String(repeating: "b", count: 64)
        )
        let request = MarketplaceModel.pluginCodeRemovalRequest(
            pluginId: "global-dharma",
            identity: identity
        )

        XCTAssertEqual(request.method, "feature.plugin.removeCode")
        XCTAssertEqual(request.params["pluginId"] as? String, "global-dharma")
        XCTAssertEqual(
            request.params["expectedAccountScope"] as? String,
            identity.accountScope
        )
        XCTAssertEqual(
            request.params["expectedAccountGeneration"] as? String,
            identity.accountGeneration
        )
    }

    func testMarketplaceRemoveClosesWebMcpBeforeCodeOnlyDeletionAndKeepsFullUninstallOutOfPath() throws {
        let testFile = URL(fileURLWithPath: #filePath)
        let repositoryRoot = testFile
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let source = try String(
            contentsOf: repositoryRoot.appendingPathComponent(
                "mobile/ios/Fabushi/MarketplaceModel.swift"
            ),
            encoding: .utf8
        )
        let start = try XCTUnwrap(
            source.range(of: "    func remove(_ plugin: MarketplacePlugin) async {")
        )
        let tail = source[start.lowerBound...]
        let end = try XCTUnwrap(
            tail.range(of: "    func approvePermissions() async {")
        )
        let block = String(tail[..<end.lowerBound])
        let revoke = try XCTUnwrap(
            block.range(of: "ActiveMiniAppWebMcpSessionRegistry.shared.revokeSession")
        )
        let codeRemoval = try XCTUnwrap(
            block.range(of: "let codeRemoval = try await bridge.request")
        )
        let accountRemoval = try XCTUnwrap(
            block.range(of: #"method: "feature.marketplace.remove""#)
        )
        let finalFence = try XCTUnwrap(
            block.range(of: "try await ensurePluginAccountIdentity(identity)", options: .backwards)
        )
        let uiCommit = try XCTUnwrap(
            block.range(of: "installedPluginIds.remove(pluginId)")
        )

        XCTAssertLessThan(
            block.distance(from: block.startIndex, to: revoke.lowerBound),
            block.distance(from: block.startIndex, to: codeRemoval.lowerBound)
        )
        XCTAssertLessThan(
            block.distance(from: block.startIndex, to: codeRemoval.lowerBound),
            block.distance(from: block.startIndex, to: accountRemoval.lowerBound)
        )
        XCTAssertLessThan(
            block.distance(from: block.startIndex, to: finalFence.lowerBound),
            block.distance(from: block.startIndex, to: uiCommit.lowerBound)
        )
        XCTAssertTrue(block.contains("Self.pluginCodeRemovalRequest"))
        XCTAssertFalse(block.contains("feature.plugin.uninstall"))
        XCTAssertFalse(block.contains("plugin.permission."))
        XCTAssertFalse(block.contains("removeData(ofTypes:"))
        XCTAssertFalse(block.contains("removeDataStore"))
        XCTAssertFalse(block.contains("websiteDataStore = .nonPersistent()"))

        let webMcpSource = try String(
            contentsOf: repositoryRoot.appendingPathComponent(
                "mobile/ios/Fabushi/MiniAppWebMcpSurface.swift"
            ),
            encoding: .utf8
        )
        let registerStart = try XCTUnwrap(
            webMcpSource.range(of: "            ActiveMiniAppWebMcpSessionRegistry.shared.register(")
        )
        let registerTail = webMcpSource[registerStart.lowerBound...]
        let registerEnd = try XCTUnwrap(
            registerTail.range(of: #"            status = "本地 WebMCP 已连接"#)
        )
        let registrationBlock = String(registerTail[..<registerEnd.lowerBound])
        XCTAssertTrue(registrationBlock.contains("revoke: {"))
        XCTAssertTrue(registrationBlock.contains("webView.stopLoading()"))
        XCTAssertTrue(registrationBlock.contains("webView.loadHTMLString(\"\", baseURL: nil)"))
        XCTAssertTrue(registrationBlock.contains("self.disposeLocalBridgeSession()"))
        XCTAssertTrue(registrationBlock.contains("self.onRuntimeRevoked()"))
        XCTAssertTrue(webMcpSource.contains("onRuntimeRevoked: {"))
    }

    func testCanonicalMiniAppRosterReadUsesStrictMarketplaceAccountIdentity() {
        let identity = MarketplacePluginAccountIdentity(
            accountScope: String(repeating: "a", count: 64),
            accountGeneration: String(repeating: "b", count: 64)
        )
        let request = GlobalDharmaMiniAppBridge.installedMiniAppRosterRequest(
            identity: identity
        )

        XCTAssertEqual(request.method, "feature.marketplace.added")
        XCTAssertEqual(
            request.params["expectedAccountScope"] as? String,
            identity.accountScope
        )
        XCTAssertEqual(
            request.params["expectedAccountGeneration"] as? String,
            identity.accountGeneration
        )
        XCTAssertNil(
            request.params["authenticated"],
            "Canonical Mini App roster must not bypass the Host account-generation fence through platform.request."
        )
        XCTAssertNil(request.params["path"])
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

    func testCanonicalInstalledProjectionAdvancesRosterRevisionOnlyWhenAccountTruthChanges() {
        XCTAssertEqual(
            MarketplaceModel.nextAccountRosterRevision(
                currentInstalledPluginIds: ["local-only"],
                nextInstalledPluginIds: ["local-only", "global-dharma"],
                currentRevision: 41
            ),
            42
        )
        XCTAssertEqual(
            MarketplaceModel.nextAccountRosterRevision(
                currentInstalledPluginIds: ["global-dharma"],
                nextInstalledPluginIds: ["global-dharma"],
                currentRevision: 42
            ),
            42,
            "Repeated canonical reads must not create a roster refresh loop."
        )
        XCTAssertEqual(
            MarketplaceModel.nextAccountRosterRevision(
                currentInstalledPluginIds: ["global-dharma"],
                nextInstalledPluginIds: [],
                currentRevision: 9
            ),
            10,
            "Canonical account removal must invalidate the projected Mini App roster."
        )
        XCTAssertEqual(
            MarketplaceModel.nextAccountRosterRevision(
                currentInstalledPluginIds: [],
                nextInstalledPluginIds: ["global-dharma"],
                currentRevision: Int.max
            ),
            1,
            "Roster revision rollover must preserve the existing bounded generation convention."
        )
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


final class MarketplaceMcpPluginUpdateShippingTests: XCTestCase {
    func testCatalogProjectionPreservesEditableAndSecretFieldSemantics() throws {
        let entry = try XCTUnwrap(MarketplaceModel.mcpCatalogEntry(from: [
            "id": "101",
            "name": "github",
            "displayName": "GitHub",
            "connectors": ["github"],
            "iconUrl": "https://example.test/icon.png",
            "fields": [[
                "key": "API_TOKEN",
                "label": "API Token",
                "placeholder": "API_TOKEN",
                "isRequired": true,
                "isSecret": true,
                "defaultValue": NSNull(),
                "hint": "Personal access token",
            ]],
        ]))
        XCTAssertEqual(entry.id, "101")
        XCTAssertEqual(entry.iconURL, "https://example.test/icon.png")
        XCTAssertEqual(entry.fields.count, 1)
        XCTAssertEqual(entry.fields[0].key, "API_TOKEN")
        XCTAssertTrue(entry.fields[0].isRequired)
        XCTAssertTrue(entry.fields[0].isSecret)
        XCTAssertEqual(entry.fields[0].hint, "Personal access token")
    }

    func testShippingMcpPluginConfigUsesCanonicalCoordinatorUpdateRoute() throws {
        let testFile = URL(fileURLWithPath: #filePath)
        let repositoryRoot = testFile
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let modelSource = try String(
            contentsOf: repositoryRoot.appendingPathComponent("mobile/ios/Fabushi/MarketplaceModel.swift"),
            encoding: .utf8
        )
        let surfaceSource = try String(
            contentsOf: repositoryRoot.appendingPathComponent("frontend/src/recovered/features/conversation/ContentView+ChatMarketplace.swift"),
            encoding: .utf8
        )
        let coordinatorSource = try String(
            contentsOf: repositoryRoot.appendingPathComponent("source/mahayana-agent-coordinator/coordinator-mcp-manager.swift"),
            encoding: .utf8
        )
        XCTAssertTrue(coordinatorSource.contains(#"case "coordinator.mcp.updatePluginInstall":"#))
        XCTAssertTrue(coordinatorSource.contains("try await manager.updatePluginInstall"))
        XCTAssertTrue(modelSource.contains(#"method: "coordinator.mcp.updatePluginInstall""#))
        XCTAssertTrue(modelSource.contains("plugin-update:"))
        XCTAssertTrue(modelSource.contains("Task.checkCancellation()"))
        XCTAssertTrue(surfaceSource.contains("SecureField("))
        XCTAssertTrue(surfaceSource.contains("mcp-plugin-config-save-"))
        XCTAssertTrue(surfaceSource.contains("model.updateMcpPluginInstall(pluginId: pluginId)"))
        XCTAssertTrue(surfaceSource.contains("await model.loadConnectorCardData()"))
    }

    func testShippingConnectorCatalogExposesSetupAndPassesDraftValuesIntoCanonicalInstall() throws {
        let entry = MobileConnectorCatalogEntry(
            id: "fabushi-official-github",
            name: "github",
            displayName: "GitHub",
            connectors: ["github"],
            fields: [
                .init(
                    key: "CLIENT_ID",
                    label: "Client ID",
                    placeholder: "CLIENT_ID",
                    isRequired: true,
                    isSecret: false
                ),
            ]
        )
        let params = MarketplaceModel.connectorInstallParams(
            entry: entry,
            values: ["CLIENT_ID": "configured-client"]
        )
        XCTAssertEqual(params["entryId"] as? String, entry.id)
        XCTAssertEqual(params["values"] as? [String: String], ["CLIENT_ID": "configured-client"])

        let testFile = URL(fileURLWithPath: #filePath)
        let repositoryRoot = testFile
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let modelSource = try String(
            contentsOf: repositoryRoot.appendingPathComponent("mobile/ios/Fabushi/MarketplaceModel.swift"),
            encoding: .utf8
        )
        let surfaceSource = try String(
            contentsOf: repositoryRoot.appendingPathComponent("frontend/src/recovered/features/conversation/ContentView+ChatMarketplace.swift"),
            encoding: .utf8
        )
        XCTAssertTrue(modelSource.contains("params: Self.connectorInstallParams("))
        XCTAssertTrue(modelSource.contains("values: mcpPluginDraftValuesByPluginId[entry.id] ?? [:]"))
        XCTAssertTrue(surfaceSource.contains(#"DisclosureGroup("添加 MCP 连接器")"#))
        XCTAssertTrue(surfaceSource.contains("model.connectConnectorCard(connector: entry.id)"))
        XCTAssertTrue(surfaceSource.contains(#""mcp-connector-install-field-\(entry.id)-\(field.key)""#))
        XCTAssertTrue(surfaceSource.contains("if model.loggedIn && model.connectorCatalog.isEmpty"))
    }
}

extension MarketplaceModelLifecycleTests {
    func testMcpBackendStatusUpdateRejectsStaleAccountRequestAndFreshness() {
        XCTAssertTrue(MarketplaceModel.acceptsMcpBackendStatusUpdate(
            requestEpoch: 7, currentEpoch: 7, requestSerial: 11, currentSerial: 11,
            incomingFreshness: 4, currentFreshness: 4
        ))
        XCTAssertFalse(MarketplaceModel.acceptsMcpBackendStatusUpdate(
            requestEpoch: 6, currentEpoch: 7, requestSerial: 11, currentSerial: 11,
            incomingFreshness: 5, currentFreshness: 4
        ))
        XCTAssertFalse(MarketplaceModel.acceptsMcpBackendStatusUpdate(
            requestEpoch: 7, currentEpoch: 7, requestSerial: 10, currentSerial: 11,
            incomingFreshness: 5, currentFreshness: 4
        ))
        XCTAssertFalse(MarketplaceModel.acceptsMcpBackendStatusUpdate(
            requestEpoch: 7, currentEpoch: 7, requestSerial: 11, currentSerial: 11,
            incomingFreshness: 3, currentFreshness: 4
        ))
    }


    func testMarketplaceAuthLogoutRevokesPreviousAccountMiniAppSessions() throws {
        let testFile = URL(fileURLWithPath: #filePath)
        let repositoryRoot = testFile
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let source = try String(
            contentsOf: repositoryRoot.appendingPathComponent(
                "mobile/ios/Fabushi/MarketplaceModel.swift"
            ),
            encoding: .utf8
        )
        let start = try XCTUnwrap(
            source.range(of: "    private func applyAuth(_ object: [String: Any]?, defaultLoggedIn: Bool = false) {")
        )
        let tail = source[start.lowerBound...]
        let end = try XCTUnwrap(
            tail.range(of: "    nonisolated static func normalizedAccountDisplayName")
        )
        let body = String(tail[..<end.lowerBound])

        XCTAssertTrue(body.contains("let previousWebMcpAccountKey = settingsNoticeAccountKey"))
        XCTAssertTrue(body.contains("if previousMcpScope != globalDharmaAccountScope || !loggedIn"))
        XCTAssertTrue(body.contains("accountKey: previousWebMcpAccountKey"))

        let revoke = try XCTUnwrap(body.range(of: "ActiveMiniAppWebMcpSessionRegistry.shared.revokeSessions("))
        let reset = try XCTUnwrap(body.range(of: "resetMcpState()"))
        let clearInstalled = try XCTUnwrap(body.range(of: "installedPluginIds = []"))
        XCTAssertLessThan(
            body.distance(from: body.startIndex, to: revoke.lowerBound),
            body.distance(from: body.startIndex, to: reset.lowerBound)
        )
        XCTAssertLessThan(
            body.distance(from: body.startIndex, to: revoke.lowerBound),
            body.distance(from: body.startIndex, to: clearInstalled.lowerBound)
        )
    }
}


final class AccountDisplayNameMutationShippingTests: XCTestCase {
    func testShippingDisplayNameMutationUsesLatestRequestAndAccountEpochFences() throws {
        let testFile = URL(fileURLWithPath: #filePath)
        let repositoryRoot = testFile
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let source = try String(
            contentsOf: repositoryRoot.appendingPathComponent(
                "mobile/ios/Fabushi/MarketplaceModel.swift"
            ),
            encoding: .utf8
        )
        XCTAssertTrue(source.contains("let requestSerial = accountDisplayNameRequestSerial"))
        XCTAssertTrue(source.contains("let expectedEpoch = mcpAccountEpoch"))
        XCTAssertTrue(source.contains("let expectedScope = globalDharmaAccountScope"))
        XCTAssertTrue(source.contains("requestSerial == accountDisplayNameRequestSerial"))
        XCTAssertTrue(source.contains("expectedEpoch == mcpAccountEpoch"))
        XCTAssertTrue(source.contains("expectedScope == globalDharmaAccountScope"))
        XCTAssertTrue(source.contains("accountName = settledDisplayName"))
    }
}


final class MarketplacePluginGitHubAuthParityTests: XCTestCase {
    func testAuthBlockedProjectionFiltersMalformedRowsAndPreservesMarketplaceName() throws {
        let blocks = MarketplaceModel.pluginAuthBlocks(from: [
            "authBlocked": [
                [
                    "pluginId": "private-a",
                    "pluginName": "Private A",
                    "marketplaceName": "team",
                ],
                [
                    "pluginId": "private-b",
                    "pluginName": "Private B",
                ],
                [
                    "pluginId": "",
                    "pluginName": "invalid",
                ],
                [
                    "pluginName": "missing-id",
                ],
            ],
        ])
        XCTAssertEqual(blocks.count, 2)
        XCTAssertEqual(blocks[0].pluginId, "private-a")
        XCTAssertEqual(blocks[0].marketplaceName, "team")
        XCTAssertEqual(blocks[1].pluginName, "Private B")
        XCTAssertEqual(
            MarketplaceModel.pluginAuthBlockedDetail(blocks),
            "Private A, Private B are installed, but their content can't be fetched until Grok Bot's computer can read the source repository."
        )
    }

    func testAuthBlockedDetailFallsBackToCountBeyondThreeNames() {
        let blocks = (1...4).map {
            MarketplacePluginAuthBlock(
                pluginId: "plugin-\($0)",
                pluginName: "Plugin \($0)",
                marketplaceName: nil
            )
        }
        XCTAssertEqual(
            MarketplaceModel.pluginAuthBlockedDetail(blocks),
            "4 installed plugins can't be fetched until Grok Bot's computer can read their source repository."
        )
    }

    func testShippingMarketplaceWiresPluginAuthBannerRepairRetryAndDisposal() throws {
        let testFile = URL(fileURLWithPath: #filePath)
        let repositoryRoot = testFile
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let modelSource = try String(
            contentsOf: repositoryRoot.appendingPathComponent("mobile/ios/Fabushi/MarketplaceModel.swift"),
            encoding: .utf8
        )
        let surfaceSource = try String(
            contentsOf: repositoryRoot.appendingPathComponent(
                "frontend/src/recovered/features/conversation/ContentView+ChatMarketplace.swift"
            ),
            encoding: .utf8
        )
        let serviceSource = try String(
            contentsOf: repositoryRoot.appendingPathComponent(
                "frontend/src/production/GrokMobileBotService.swift"
            ),
            encoding: .utf8
        )

        XCTAssertTrue(modelSource.contains(#"bridge.request(method: "getPluginSyncStatus")"#))
        XCTAssertTrue(modelSource.contains("pluginAuthScopeGeneration"))
        XCTAssertTrue(modelSource.contains("pluginAuthRequestGeneration"))
        XCTAssertTrue(modelSource.contains("ensurePluginAccountIdentity(identity)"))
        XCTAssertTrue(modelSource.contains("resetPluginAuthState()"))
        XCTAssertTrue(serviceSource.contains(#"method: "createAgent""#))
        XCTAssertTrue(serviceSource.contains(#""purpose": Self.pluginAuthAgentPurpose"#))
        XCTAssertTrue(serviceSource.contains(#"method: "listAgents""#))
        XCTAssertTrue(serviceSource.contains("Host did not persist the Plugin Setup Agent purpose."))
        XCTAssertTrue(serviceSource.contains(#"method: "sendPrompt""#))
        XCTAssertTrue(surfaceSource.contains("plugin-github-auth-banner"))
        XCTAssertTrue(surfaceSource.contains("plugin-github-auth-fix"))
        XCTAssertTrue(surfaceSource.contains("plugin-github-auth-retry"))
        XCTAssertTrue(surfaceSource.contains("model.disposePluginAuthScope()"))
    }
}
