import XCTest
@testable import Fabushi

@MainActor
private final class CoordinatorMcpFakeHost: MahayanaHostRequesting, @unchecked Sendable {
    private var disabled = Set(["search"])
    private(set) var toolCalls: [[String: Any]] = []

    func request(
        method: String,
        params: [String: Any]
    ) async throws -> MahayanaHostJSONResult {
        switch method {
        case "feature.mcp.servers":
            return .init(value: [
                "servers": [
                    [
                        "name": "github",
                        "authStatus": "oAuth",
                        "tools": [
                            "search": [
                                "name": "search",
                                "description": "Search repositories",
                                "inputSchema": ["type": "object"],
                            ],
                        ],
                        "fabushiConfig": [
                            "url": "https://mcp.example.test",
                            "disabled_tools": Array(disabled).sorted(),
                        ],
                    ],
                ],
            ])

        case "feature.mcp.setToolDisabled":
            guard let tool = params["tool"] as? String,
                  let shouldDisable = params["disabled"] as? Bool
            else {
                throw MahayanaHostRuntime.HostError.invalidResponse
            }
            if shouldDisable { disabled.insert(tool) }
            else { disabled.remove(tool) }
            return .init(value: [
                "server": params["server"] as? String ?? "",
                "disabledTools": Array(disabled).sorted(),
            ])

        case "feature.mcp.toolCall":
            toolCalls.append(params)
            return .init(value: [
                "content": [
                    ["type": "text", "text": "ok"],
                ],
                "structuredContent": ["ok": true],
            ])

        default:
            return .init(value: NSNull())
        }
    }
}

@MainActor
final class CoordinatorMcpSurfaceTests: XCTestCase {
    func testCoordinatorOwnsAccountScopedServerToolsAndCanonicalToggle() async throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("fabushi-coordinator-mcp-\(UUID().uuidString)")
        try FileManager.default.createDirectory(
            at: root,
            withIntermediateDirectories: true
        )
        defer { try? FileManager.default.removeItem(at: root) }

        let host = CoordinatorMcpFakeHost()
        let supervisor = MahayanaLocalHostSupervisor(
            host: host,
            factory: { host }
        )
        let settings = SandSettingsStore(
            settingsPath: root.appendingPathComponent("settings.json").path
        )
        let coordinator = MahayanaCoordinator(
            hostSupervisor: supervisor,
            settingsStore: settings
        )
        coordinator.updateAccountSettingsScope("account-a")

        let serverResult = try await coordinator.request(
            method: "coordinator.mcp.servers"
        )
        let serverObject = try XCTUnwrap(serverResult.value as? [String: Any])
        let servers = try XCTUnwrap(serverObject["servers"] as? [[String: Any]])
        let server = try XCTUnwrap(servers.first)
        let serverId = try XCTUnwrap(server["id"] as? String)
        XCTAssertEqual(server["serverIdentifier"] as? String, "github")
        XCTAssertEqual(server["accountKey"] as? String, DEFAULT_MCP_ACCOUNT_KEY)
        XCTAssertEqual((server["disabledToolCount"] as? NSNumber)?.intValue, 1)

        let toolsResult = try await coordinator.request(
            method: "coordinator.mcp.tools",
            params: ["serverId": serverId]
        )
        let toolsObject = try XCTUnwrap(toolsResult.value as? [String: Any])
        let tools = try XCTUnwrap(toolsObject["tools"] as? [[String: Any]])
        XCTAssertEqual(tools.first?["name"] as? String, "search")
        XCTAssertEqual(tools.first?["isDisabled"] as? Bool, true)

        let toggle = try await coordinator.request(
            method: "coordinator.mcp.setToolDisabled",
            params: [
                "serverId": serverId,
                "tool": "search",
                "disabled": false,
            ]
        )
        let toggledObject = try XCTUnwrap(toggle.value as? [String: Any])
        let toggledTools = try XCTUnwrap(toggledObject["tools"] as? [[String: Any]])
        XCTAssertEqual(toggledTools.first?["isDisabled"] as? Bool, false)
        XCTAssertFalse(
            settings.getMcpDisabledToolsByServerId()[serverId]?.contains("search") == true
        )

        let instructions = try await coordinator.request(
            method: "coordinator.mcp.setInstructions",
            params: [
                "serverId": serverId,
                "instructions": "Use read-only repository tools.",
            ]
        )
        XCTAssertNotNil((instructions.value as? [String: Any])?["servers"])
        XCTAssertEqual(
            settings.getMcpCustomInstructionsByServerId()[serverId],
            "Use read-only repository tools."
        )

        let reloaded = try await coordinator.request(
            method: "coordinator.mcp.reloadServers"
        )
        XCTAssertNotNil((reloaded.value as? [String: Any])?["servers"])

        let routedList = await coordinator.dispatchTransport(
            requestId: "runner-list-1",
            method: "listRoutedMcpTools",
            args: .object([:])
        )
        guard case .ok(let routedListPayload) = routedList,
              let routedObject = routedListPayload.foundationValue as? [String: Any],
              let routedTools = routedObject["tools"] as? [[String: Any]],
              let routedName = routedTools.first?["name"] as? String
        else {
            return XCTFail("routed MCP list did not travel through the shipping coordinator path")
        }
        XCTAssertEqual(routedTools.first?["providerIdentifier"] as? String, "github")

        let routedCall = await coordinator.dispatchTransport(
            requestId: "runner-call-1",
            method: "executeRoutedMcpTool",
            args: .object([
                "name": .string(routedName),
                "toolCallId": .string("tool-call-1"),
                "agentId": .string("agent-1"),
                "arguments": .object(["query": .string("fabushi")]),
            ])
        )
        guard case .ok(let routedCallPayload) = routedCall else {
            return XCTFail("routed MCP execution did not travel through the shipping coordinator path")
        }
        XCTAssertEqual(
            (routedCallPayload.foundationValue as? [String: Any])?["isError"] as? Bool,
            false
        )
        XCTAssertEqual(host.toolCalls.last?["toolCallId"] as? String, "tool-call-1")
        XCTAssertEqual(host.toolCalls.last?["agentId"] as? String, "agent-1")

        settings.setMcpCustomInstructionsByServerId([serverId: "private"])
        coordinator.updateAccountSettingsScope(nil)
        XCTAssertTrue(settings.getMcpCustomInstructionsByServerId().isEmpty)

        let loggedOut = try await coordinator.request(
            method: "coordinator.mcp.servers"
        )
        let loggedOutObject = try XCTUnwrap(loggedOut.value as? [String: Any])
        XCTAssertTrue((loggedOutObject["servers"] as? [[String: Any]])?.isEmpty == true)
    }
}


final class CoordinatorSkillPublishTargetProjectionTests: XCTestCase {
    func testTargetsKeepOnlyDirectPositiveTeamsAndSortStably() {
        let rows = projectSkillPublishTargets([
            .init(teamId: 9, name: "Zulu", isDirectMember: true),
            .init(teamId: 7, name: "Alpha", isDirectMember: true),
            .init(teamId: 3, name: "Alpha", isDirectMember: true),
            .init(teamId: 4, name: "Indirect", isDirectMember: false),
            .init(teamId: 0, name: "Invalid", isDirectMember: true),
        ])

        XCTAssertEqual(rows.count, 3)
        XCTAssertEqual(rows.map { $0["name"] as? String }, ["Alpha", "Alpha", "Zulu"])
        XCTAssertEqual(
            rows.compactMap { ($0["teamId"] as? NSNumber)?.intValue },
            [3, 7, 9]
        )
    }
}


final class CoordinatorSkillPublishAuthoritativeProjectionTests: XCTestCase {
    func testAuthoritativeProjectionRequiresExactCommitAndPreservesPublisherTeamFacts() {
        let plugins = [
            EffectiveUserPlugin(
                pluginId: "41",
                name: "release-check",
                displayName: "Release check",
                installMode: .user,
                isEnabled: true,
                versionRef: "abcdef0123456789abcdef0123456789abcdef01",
                publisherUserId: 99,
                marketplaceTeamId: 7
            ),
            EffectiveUserPlugin(
                pluginId: "42",
                name: "branch-ref",
                displayName: "Branch ref",
                installMode: .user,
                isEnabled: true,
                versionRef: "main",
                publisherUserId: 99,
                marketplaceTeamId: 7
            ),
        ]
        let rows = projectAuthoritativePublishedPlugins(plugins, currentUserId: 99)
        XCTAssertEqual(rows.count, 1)
        XCTAssertEqual(rows[0]["pluginId"] as? String, "41")
        XCTAssertEqual(
            rows[0]["pluginVersion"] as? String,
            "abcdef0123456789abcdef0123456789abcdef01"
        )
        XCTAssertEqual(rows[0]["publishedByCurrentUser"] as? Bool, true)
        XCTAssertEqual((rows[0]["marketplaceTeamId"] as? NSNumber)?.intValue, 7)
    }

    func testVersionNormalizerRejectsNonAuthoritativeRefs() {
        XCTAssertNil(normalizedSkillPublishVersion("main"))
        XCTAssertNil(normalizedSkillPublishVersion("abc123"))
        XCTAssertEqual(
            normalizedSkillPublishVersion("ABCDEF0123456789ABCDEF0123456789ABCDEF01"),
            "abcdef0123456789abcdef0123456789abcdef01"
        )
    }

    func testInstalledMarketplaceSkillSourceFailsClosedUnlessVersionAndSourceAreImmutable() throws {
        let source = try requireInstalledMarketplaceSkillSource(
            pluginId: "41",
            skillName: "release-review",
            versionRef: "ABCDEF0123456789ABCDEF0123456789ABCDEF01",
            sourceUrl: "https://github.com/example/connectors/blob/main/skills/release-review/SKILL.md"
        )
        XCTAssertEqual(source.version, "abcdef0123456789abcdef0123456789abcdef01")
        XCTAssertEqual(
            source.sourceUrl,
            "https://github.com/example/connectors/blob/abcdef0123456789abcdef0123456789abcdef01/skills/release-review/SKILL.md"
        )

        XCTAssertThrowsError(
            try requireInstalledMarketplaceSkillSource(
                pluginId: "41",
                skillName: "release-review",
                versionRef: "main",
                sourceUrl: "https://github.com/example/connectors/blob/main/skills/release-review/SKILL.md"
            )
        )
        XCTAssertThrowsError(
            try requireInstalledMarketplaceSkillSource(
                pluginId: "41",
                skillName: "release-review",
                versionRef: "abcdef0123456789abcdef0123456789abcdef01",
                sourceUrl: "https://raw.githubusercontent.com/example/connectors/main/skills/release-review/SKILL.md"
            )
        )
        XCTAssertThrowsError(
            try requireInstalledMarketplaceSkillSource(
                pluginId: "41",
                skillName: "release-review",
                versionRef: "abcdef0123456789abcdef0123456789abcdef01",
                sourceUrl: nil
            )
        )
    }
}

extension CoordinatorMcpSurfaceTests {
    func testCursorAccountEdgeShippingWiringRefreshesServersAndBoundsName() throws {
        let testFile = URL(fileURLWithPath: #filePath)
        let repositoryRoot = testFile
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let source = try String(
            contentsOf: repositoryRoot.appendingPathComponent(
                "source/mahayana-agent-coordinator/coordinator-mcp-manager.swift"
            ),
            encoding: .utf8
        )
        XCTAssertTrue(source.contains(
            #"if let status = try await cursorAuth.pollLogin(attemptId: attemptId) {"#
        ))
        XCTAssertTrue(source.contains("_ = try await manager.reloadServers()"))
        XCTAssertTrue(source.contains(
            #"guard let name = params["name"] as? String, name.count <= 200 else {"#
        ))
        XCTAssertTrue(source.contains("cursorAuth.updateDisplayName(name)"))
        XCTAssertTrue(source.contains("cursorAuth.setProfileFetcher"))
        XCTAssertTrue(source.contains(#""profilePictureUrl": status.profilePictureUrl ?? NSNull()"#))
        XCTAssertTrue(source.contains(#""isAnysphereUser": status.isAnysphereUser.map"#))
        XCTAssertTrue(source.contains(#""displayName": status.displayName ?? NSNull()"#))
    }
}


@MainActor
final class IOSCursorAuthMainRpcCompatibilityTests: XCTestCase {
    func testStatusProjectsPendingLoginAsDesktopLoggingIn() async {
        var pending = true
        let runtime = IOSCursorAuthMainRpcCompatibility(
            dependencies: .init(
                readStatus: { .init(loggedIn: false, freshness: 7) },
                isLoginPending: { pending },
                beginLogin: { .init(attemptId: "a", loginURL: URL(string: "https://cursor.com/login")!) },
                pollLogin: { _ in nil },
                cancelPendingLogins: { pending = false },
                logout: {},
                openExternal: { _ in },
                reloadAuthoritativeMcp: {},
                invalidateMcp: {},
                sleepBeforePoll: {}
            )
        )
        let value = await runtime.getStatus()
        XCTAssertEqual(value["kind"] as? String, "logging-in")
        XCTAssertEqual((value["freshness"] as? NSNumber)?.uint64Value, 7)
    }

    func testLoginPollsThenReloadsAuthoritativeMcpBeforeReturning() async throws {
        var pending = false
        var polls = 0
        var events: [String] = []
        let runtime = IOSCursorAuthMainRpcCompatibility(
            dependencies: .init(
                readStatus: { .init(loggedIn: false, freshness: 3) },
                isLoginPending: { pending },
                beginLogin: {
                    pending = true
                    return .init(attemptId: "attempt-1", loginURL: URL(string: "https://cursor.com/login")!)
                },
                pollLogin: { attemptId in
                    XCTAssertEqual(attemptId, "attempt-1")
                    polls += 1
                    if polls == 1 { return nil }
                    pending = false
                    return .init(loggedIn: true, authId: "auth-1", email: "person@example.test", expiresAtMs: 9_999, freshness: 4)
                },
                cancelPendingLogins: { pending = false; events.append("cancel") },
                logout: { events.append("logout") },
                openExternal: { _ in events.append("open") },
                reloadAuthoritativeMcp: { events.append("reload-authoritative") },
                invalidateMcp: { events.append("invalidate") },
                sleepBeforePoll: { events.append("sleep") }
            )
        )
        let value = try await runtime.login()
        XCTAssertEqual(value["kind"] as? String, "logged-in")
        XCTAssertEqual(value["authId"] as? String, "auth-1")
        XCTAssertEqual((value["expiresAt"] as? NSNumber)?.int64Value, 9_999)
        XCTAssertEqual(events, ["open", "sleep", "reload-authoritative"])
    }

    func testCancelLoginRevokesCanonicalCredentialsAndInvalidatesMcp() async throws {
        var pending = true
        var loggedIn = true
        var events: [String] = []
        let runtime = IOSCursorAuthMainRpcCompatibility(
            dependencies: .init(
                readStatus: { .init(loggedIn: loggedIn, freshness: 5) },
                isLoginPending: { pending },
                beginLogin: { .init(attemptId: "a", loginURL: URL(string: "https://cursor.com/login")!) },
                pollLogin: { _ in nil },
                cancelPendingLogins: { pending = false; events.append("cancel") },
                logout: { loggedIn = false; events.append("logout") },
                openExternal: { _ in },
                reloadAuthoritativeMcp: { events.append("reload-authoritative") },
                invalidateMcp: { events.append("invalidate") },
                sleepBeforePoll: {}
            )
        )
        let value = try await runtime.cancelLogin()
        XCTAssertEqual(value["kind"] as? String, "logged-out")
        XCTAssertEqual(events, ["cancel", "logout", "invalidate"])
    }
}


extension CoordinatorMcpSurfaceTests {
    func testComputerMigrationStallBudgetMatchesDesktopWatchdogLimit() {
        var budget = IOSComputerMigrationStallBudget()
        XCTAssertEqual(budget.remaining, IOS_COMPUTER_MIGRATION_MAX_STALL_REATTACHES)

        for expected in stride(
            from: IOS_COMPUTER_MIGRATION_MAX_STALL_REATTACHES - 1,
            through: 0,
            by: -1
        ) {
            XCTAssertTrue(budget.consumeReattach())
            XCTAssertEqual(budget.remaining, expected)
        }
        XCTAssertFalse(budget.consumeReattach())
        budget.reset()
        XCTAssertEqual(budget.remaining, IOS_COMPUTER_MIGRATION_MAX_STALL_REATTACHES)
    }


    func testAutomationRelayAcksOnlyAfterSuccessfulSinkAndDeduplicatesReturnedEvent() async {
        var now: Int64 = 1_000
        var ackRequests = [[String]]()
        var sinkAttempts = 0
        let wire: [String: Any] = [
            "id": "event-1",
            "source": "github",
            "repo": "owner/repo",
            "kind": "pr-opened",
            "title": "PR",
            "actor": "octocat",
            "timestampMs": NSNumber(value: now),
        ]
        let relay = IOSAutomationRelayLifecycle(
            loadSubscription: {
                IOSAutomationRelaySubscription(
                    slackChannels: [],
                    githubRepos: ["owner/repo"],
                    githubKinds: ["pr-opened"]
                )
            },
            register: { _ in ["github": ["status": "listening"]] },
            drain: { ackIds in
                ackRequests.append(ackIds)
                return ["events": [wire]]
            },
            sink: { _ in
                sinkAttempts += 1
                return sinkAttempts > 1
            },
            pollReconnects: {},
            nowMilliseconds: { now },
            launchBackgroundTasks: false
        )
        relay.updateAccountScope("account-a")
        relay.resume()

        await relay.runOneCycleForTesting()
        XCTAssertTrue(relay.pendingAckIds.isEmpty)

        now += 4_000
        await relay.runOneCycleForTesting()
        XCTAssertEqual(relay.pendingAckIds, Set(["event-1"]))

        now += 4_000
        await relay.runOneCycleForTesting()
        XCTAssertEqual(sinkAttempts, 2)
        XCTAssertEqual(ackRequests, [[], [], ["event-1"]])
    }

    func testAutomationRelayUsesThirtySecondErrorBackoffAndRecoversRegistration() async {
        var now: Int64 = 10_000
        var registrations = 0
        let relay = IOSAutomationRelayLifecycle(
            loadSubscription: {
                IOSAutomationRelaySubscription(
                    slackChannels: ["C123"],
                    githubRepos: [],
                    githubKinds: []
                )
            },
            register: { _ in
                registrations += 1
                if registrations == 1 {
                    throw SandMcpConfigError("temporary")
                }
                return ["slack": ["status": "listening"]]
            },
            drain: { _ in ["events": []] },
            sink: { _ in true },
            pollReconnects: {},
            nowMilliseconds: { now },
            launchBackgroundTasks: false
        )
        relay.updateAccountScope("account-a")
        relay.resume()

        await relay.runOneCycleForTesting()
        XCTAssertEqual(relay.backoffUntilMs, now + IOS_AUTOMATION_RELAY_ERROR_BACKOFF_MS)
        XCTAssertEqual(registrations, 1)

        now += IOS_AUTOMATION_RELAY_ERROR_BACKOFF_MS - 1
        await relay.runOneCycleForTesting()
        XCTAssertEqual(registrations, 1)

        now += 1
        await relay.runOneCycleForTesting()
        XCTAssertEqual(registrations, 2)
        XCTAssertFalse(relay.registeredKey.isEmpty)
    }

    func testAutomationRelayAccountSwitchFencesReconnectWatcher() async {
        var reconnectPolls = 0
        let relay = IOSAutomationRelayLifecycle(
            loadSubscription: {
                IOSAutomationRelaySubscription(
                    slackChannels: [],
                    githubRepos: [],
                    githubKinds: []
                )
            },
            register: { _ in [:] },
            drain: { _ in ["events": []] },
            sink: { _ in true },
            pollReconnects: { reconnectPolls += 1 },
            launchBackgroundTasks: false
        )
        relay.updateAccountScope("account-a")
        relay.resume()
        await relay.runReconnectPollForTesting()
        XCTAssertEqual(reconnectPolls, 1)

        let oldGeneration = relay.generation
        relay.updateAccountScope("account-b")
        XCTAssertNotEqual(relay.generation, oldGeneration)
        XCTAssertTrue(relay.pendingAckIds.isEmpty)
        await relay.runReconnectPollForTesting()
        XCTAssertEqual(reconnectPolls, 2)

        relay.suspend()
        await relay.runReconnectPollForTesting()
        XCTAssertEqual(reconnectPolls, 2)
    }

    func testAutomationRelayMapsCanonicalSlackAndGithubEventCards() {
        let slack = IOSAutomationRelayLifecycle.mapRelayWireEvent(
            [
                "id": "s1",
                "source": "slack",
                "channelId": "C123",
                "channelName": "alerts",
                "kind": "message",
                "text": "hello",
                "senderSlackUserId": "U1",
                "isMention": true,
            ],
            nowMilliseconds: 42
        )
        XCTAssertEqual(slack?["source"] as? String, "slack")
        XCTAssertEqual(slack?["event"] as? String, "mention")
        XCTAssertEqual((slack?["occurredAtMs"] as? NSNumber)?.int64Value, 42)

        let github = IOSAutomationRelayLifecycle.mapRelayWireEvent(
            [
                "id": "g1",
                "source": "github",
                "repo": "owner/repo",
                "kind": "ci-failed",
                "title": "CI failed",
                "actor": "bot",
                "branch": "main",
            ],
            nowMilliseconds: 84
        )
        XCTAssertEqual(github?["source"] as? String, "github")
        XCTAssertEqual(github?["event"] as? String, "ci-failed")
        XCTAssertEqual((github?["occurredAtMs"] as? NSNumber)?.int64Value, 84)
    }

    func testAutomationRelayProductionWiringUsesCanonicalHostAndFiveSecondReconnectWatcher() throws {
        let testFile = URL(fileURLWithPath: #filePath)
        let repositoryRoot = testFile
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let source = try String(
            contentsOf: repositoryRoot.appendingPathComponent(
                "source/mahayana-agent-coordinator/coordinator-mcp-manager.swift"
            ),
            encoding: .utf8
        )
        XCTAssertTrue(source.contains("registerAutomationRelay("))
        XCTAssertTrue(source.contains("drainAutomationRelayEvents(ackIds:"))
        XCTAssertTrue(source.contains("feature.automation.ingestListenerEvent"))
        XCTAssertTrue(source.contains("feature.automation.pollListenerReconnects"))
        XCTAssertTrue(source.contains("IOS_AUTOMATION_LISTENER_RECONNECT_INTERVAL_NS: UInt64 = 5_000_000_000"))
        XCTAssertTrue(source.contains("IOS_AUTOMATION_RELAY_ERROR_BACKOFF_MS: Int64 = 30_000"))
        XCTAssertTrue(source.contains("relayWakeRequested = true"))
        XCTAssertTrue(source.contains("relaySleepTask?.cancel()"))
        XCTAssertTrue(source.contains("wakeRequested = true"))
        XCTAssertTrue(source.contains("sleepTask?.cancel()"))

        let runtimeSource = try String(
            contentsOf: repositoryRoot.appendingPathComponent(
                "source/ios-main/FabushiRuntime.swift"
            ),
            encoding: .utf8
        )
        XCTAssertTrue(runtimeSource.contains("main.coordinator.startAutomationRelay()"))
        XCTAssertTrue(runtimeSource.contains("main.coordinator.reconcileAutomationRelay()"))
        XCTAssertTrue(runtimeSource.contains("main.coordinator.stopAutomationRelay()"))
    }


    func testAutomationFireCompletionRetriesThenBecomesAckableReported() async {
        var pollCount = 0
        var ackRequests = [[String]]()
        var completionAttempts = 0
        let runUuid = "11111111-1111-4111-8111-111111111111"
        let fire: [String: Any] = [
            "id": runUuid,
            "sandAgentId": "mahayana-assistant",
            "automationId": "stable-id",
            "timestampMs": 10,
        ]
        let lifecycle = IOSAutomationFireLifecycle(
            pollState: { ["isReady": true, "hasServerSchedulable": true] },
            poll: { ackRunUuids in
                ackRequests.append(ackRunUuids)
                pollCount += 1
                return ["events": pollCount <= 2 ? [fire] : []]
            },
            complete: { _, _, _ in
                completionAttempts += 1
                if completionAttempts == 1 {
                    throw SandMcpConfigError("temporary completion failure")
                }
            },
            admit: { _ in
                [
                    "disposition": "complete",
                    "status": "failed",
                    "errorMessage": "expected failure",
                ]
            },
            status: { _ in ["state": "completed", "status": "failed"] },
            launchBackgroundTask: false
        )
        lifecycle.updateAccountScope("account-a")
        lifecycle.resume()

        await lifecycle.runOneCycleForTesting()
        XCTAssertEqual(lifecycle.phaseForTesting(runUuid), "completed")
        XCTAssertEqual(completionAttempts, 1)

        await lifecycle.runOneCycleForTesting()
        XCTAssertEqual(lifecycle.phaseForTesting(runUuid), "reported")
        XCTAssertEqual(completionAttempts, 2)

        await lifecycle.runOneCycleForTesting()
        XCTAssertEqual(ackRequests.last, [runUuid])
        XCTAssertNil(lifecycle.phaseForTesting(runUuid))
    }

    func testAutomationFirePollErrorBackoffAndSuspendResumeFencing() async {
        var now: Int64 = 1_000
        var polls = 0
        let lifecycle = IOSAutomationFireLifecycle(
            pollState: { ["isReady": true, "hasServerSchedulable": true] },
            poll: { _ in
                polls += 1
                if polls == 1 {
                    throw SandMcpConfigError("poll unavailable")
                }
                return ["events": []]
            },
            complete: { _, _, _ in },
            admit: { _ in ["disposition": "abandon"] },
            status: { _ in ["state": "missing"] },
            nowMilliseconds: { now },
            launchBackgroundTask: false
        )
        lifecycle.updateAccountScope("account-a")
        lifecycle.resume()
        await lifecycle.runOneCycleForTesting()
        XCTAssertEqual(lifecycle.backoffUntilMs, now + IOS_AUTOMATION_RELAY_ERROR_BACKOFF_MS)

        now += IOS_AUTOMATION_RELAY_ERROR_BACKOFF_MS - 1
        await lifecycle.runOneCycleForTesting()
        XCTAssertEqual(polls, 1)

        lifecycle.suspend()
        now += 1
        await lifecycle.runOneCycleForTesting()
        XCTAssertEqual(polls, 1)

        lifecycle.resume()
        await lifecycle.runOneCycleForTesting()
        XCTAssertEqual(polls, 2)
    }


    func testAutomationFireUnschedulableDrainsOnceUntilReconcile() async {
        var polls = 0
        let lifecycle = IOSAutomationFireLifecycle(
            pollState: { ["isReady": true, "hasServerSchedulable": false] },
            poll: { _ in
                polls += 1
                return ["events": []]
            },
            complete: { _, _, _ in },
            admit: { _ in ["disposition": "abandon"] },
            status: { _ in ["state": "missing"] },
            launchBackgroundTask: false
        )
        lifecycle.updateAccountScope("account-a")
        lifecycle.resume()
        await lifecycle.runOneCycleForTesting()
        await lifecycle.runOneCycleForTesting()
        XCTAssertEqual(polls, 1)
        XCTAssertTrue(lifecycle.drainedWhileUnschedulable)

        lifecycle.reconcile()
        await lifecycle.runOneCycleForTesting()
        XCTAssertEqual(polls, 2)
    }

    func testAutomationFireProductionWiringUsesBackendPollCompletionAndCanonicalHostAdmission() throws {
        let testFile = URL(fileURLWithPath: #filePath)
        let repositoryRoot = testFile
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let coordinatorSource = try String(
            contentsOf: repositoryRoot.appendingPathComponent(
                "source/mahayana-agent-coordinator/coordinator-mcp-manager.swift"
            ),
            encoding: .utf8
        )
        let dashboardSource = try String(
            contentsOf: repositoryRoot.appendingPathComponent(
                "source/ios-main/account/cursor-dashboard-client.swift"
            ),
            encoding: .utf8
        )
        XCTAssertTrue(coordinatorSource.contains("feature.automation.admitBackendFire"))
        XCTAssertTrue(coordinatorSource.contains("feature.automation.backendFireStatus"))
        XCTAssertTrue(coordinatorSource.contains("IOSAutomationFireLifecycle"))
        XCTAssertTrue(dashboardSource.contains("/sand/automation-events/poll"))
        XCTAssertTrue(dashboardSource.contains("/sand/automation-runs/complete"))
        XCTAssertTrue(dashboardSource.contains("ackRunUuids"))
    }

    func testComputerMigrationWatchProductionContainsIdleReattachTelemetryAndDepartureCleanup() throws {
        let testFile = URL(fileURLWithPath: #filePath)
        let repositoryRoot = testFile
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let source = try String(
            contentsOf: repositoryRoot.appendingPathComponent(
                "source/mahayana-agent-coordinator/coordinator-mcp-manager.swift"
            ),
            encoding: .utf8
        )

        XCTAssertTrue(source.contains("IOS_COMPUTER_MIGRATION_STALL_SECONDS = 30"))
        XCTAssertTrue(source.contains("IOS_COMPUTER_MIGRATION_MAX_STALL_REATTACHES = 20"))
        XCTAssertTrue(source.contains("reattachComputerMigrationWatchAfterStall"))
        XCTAssertTrue(source.contains("computerMigrationStallBudget.consumeReattach()"))
        XCTAssertTrue(source.contains("\"watch_exhausted\""))
        XCTAssertTrue(source.contains("\"dropped\""))
        XCTAssertTrue(source.contains("\"attached\""))
        XCTAssertTrue(source.contains("stopComputerMigrationWatch(clearState: true)"))
        XCTAssertFalse(source.contains("await gatewayDescriptorCache.clear()"))
        XCTAssertTrue(source.contains("await manager.reload()"))
    }
}


extension IOSCursorAuthMainRpcCompatibilityTests {
    func testLoggedOutProjectionCarriesCanonicalRefusalError() async {
        let runtime = IOSCursorAuthMainRpcCompatibility(
            dependencies: .init(
                readStatus: {
                    .init(
                        loggedIn: false,
                        errorMessage: IOS_CURSOR_ACCOUNT_REFUSED_MESSAGE,
                        freshness: 11
                    )
                },
                isLoginPending: { false },
                beginLogin: {
                    .init(
                        attemptId: "unused",
                        loginURL: URL(string: "https://cursor.com/login")!
                    )
                },
                pollLogin: { _ in nil },
                cancelPendingLogins: {},
                logout: {},
                openExternal: { _ in },
                reloadAuthoritativeMcp: {},
                invalidateMcp: {},
                sleepBeforePoll: {}
            )
        )

        let value = await runtime.getStatus()
        XCTAssertEqual(value["kind"] as? String, "logged-out")
        XCTAssertEqual(value["errorMessage"] as? String, IOS_CURSOR_ACCOUNT_REFUSED_MESSAGE)
        XCTAssertEqual((value["freshness"] as? NSNumber)?.uint64Value, 11)
    }

    func testAccountRefusalProductionWiringUsesCanonicalCursorAuthOwner() throws {
        let testFile = URL(fileURLWithPath: #filePath)
        let repositoryRoot = testFile
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let surfaceSource = try String(
            contentsOf: repositoryRoot.appendingPathComponent(
                "source/mahayana-agent-coordinator/coordinator-mcp-manager.swift"
            ),
            encoding: .utf8
        )
        let runtimeSource = try String(
            contentsOf: repositoryRoot.appendingPathComponent(
                "source/ios-main/IOSMainRuntime.swift"
            ),
            encoding: .utf8
        )

        XCTAssertTrue(surfaceSource.contains("cursorAuth.revokeForAccountRefusal()"))
        XCTAssertTrue(surfaceSource.contains("authorizeGatewayDescriptorAccount(_ accountScope: String)"))
        XCTAssertTrue(surfaceSource.contains("authorizeExistingDescriptor(accountScope: accountScope)"))
        XCTAssertFalse(surfaceSource.contains("await gatewayDescriptorCache.clear()"))
        XCTAssertTrue(surfaceSource.contains("await manager.reload()"))
        XCTAssertTrue(runtimeSource.contains("ProductionCoordinatorAccountRevoker("))
        XCTAssertTrue(runtimeSource.contains("coordinator.revokeCursorAuthForAccountRefusal()"))
    }
}


extension CoordinatorMcpSurfaceTests {
    func testAutomationCloudSyncLifecycleConvergesMutationsAndReadback() async {
        var calls = [String]()
        var commits = 0
        var failures = [String]()
        let lifecycle = IOSAutomationCloudSyncLifecycle(
            snapshot: {
                [
                    "agents": [[
                        "agentId": "mahayana-assistant",
                        "listRequestBase64": "AQ==",
                    ]],
                ]
            },
            plan: { agentId, response in
                XCTAssertEqual(agentId, "mahayana-assistant")
                XCTAssertEqual(response, "Ag==")
                return [
                    "converged": false,
                    "mutations": [[
                        "method": "CreateSandAutomation",
                        "bodyBase64": "Aw==",
                    ]],
                ]
            },
            commit: { agentId, response in
                XCTAssertEqual(agentId, "mahayana-assistant")
                XCTAssertEqual(response, "Ag==")
                commits += 1
                return ["converged": true]
            },
            markFailed: { agentId in failures.append(agentId) },
            unary: { method, body in
                calls.append("\(method):\(body)")
                if method == "ListSandAutomations" {
                    return "Ag=="
                }
                return "BA=="
            },
            launchBackgroundTask: false
        )
        lifecycle.updateAccountScope("account-a")
        lifecycle.resume()
        await lifecycle.runOneCycleForTesting()

        XCTAssertEqual(
            calls,
            [
                "ListSandAutomations:AQ==",
                "CreateSandAutomation:Aw==",
                "ListSandAutomations:AQ==",
            ]
        )
        XCTAssertEqual(commits, 1)
        XCTAssertTrue(failures.isEmpty)
        XCTAssertEqual(lifecycle.backoffUntilMs, 0)
    }

    func testAutomationCloudSyncLifecycleBacksOffAndMarksAgentFailed() async {
        var now: Int64 = 20_000
        var snapshots = 0
        var failures = [String]()
        let lifecycle = IOSAutomationCloudSyncLifecycle(
            snapshot: {
                snapshots += 1
                return [
                    "agents": [[
                        "agentId": "agent-a",
                        "listRequestBase64": "AQ==",
                    ]],
                ]
            },
            plan: { _, _ in ["converged": true] },
            commit: { _, _ in ["converged": true] },
            markFailed: { failures.append($0) },
            unary: { _, _ in throw SandMcpConfigError("temporary") },
            nowMilliseconds: { now },
            launchBackgroundTask: false
        )
        lifecycle.updateAccountScope("account-a")
        lifecycle.resume()

        await lifecycle.runOneCycleForTesting()
        XCTAssertEqual(failures, ["agent-a"])
        XCTAssertEqual(
            lifecycle.backoffUntilMs,
            now + IOS_AUTOMATION_RELAY_ERROR_BACKOFF_MS
        )

        now += IOS_AUTOMATION_RELAY_ERROR_BACKOFF_MS - 1
        await lifecycle.runOneCycleForTesting()
        XCTAssertEqual(snapshots, 1)

        lifecycle.suspend()
        now += 1
        await lifecycle.runOneCycleForTesting()
        XCTAssertEqual(snapshots, 1)

        lifecycle.resume()
        await lifecycle.runOneCycleForTesting()
        XCTAssertEqual(snapshots, 2)
    }

    func testAutomationCloudSyncProductionWiringKeepsDefinitionInFeatureHost() throws {
        let testFile = URL(fileURLWithPath: #filePath)
        let repositoryRoot = testFile
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let coordinator = try String(
            contentsOf: repositoryRoot.appendingPathComponent(
                "source/mahayana-agent-coordinator/coordinator-mcp-manager.swift"
            ),
            encoding: .utf8
        )
        let dashboard = try String(
            contentsOf: repositoryRoot.appendingPathComponent(
                "source/ios-main/account/cursor-dashboard-client.swift"
            ),
            encoding: .utf8
        )
        XCTAssertTrue(coordinator.contains("feature.automation.cloudSyncSnapshot"))
        XCTAssertTrue(coordinator.contains("feature.automation.cloudSyncPlan"))
        XCTAssertTrue(coordinator.contains("feature.automation.cloudSyncCommit"))
        XCTAssertTrue(coordinator.contains("feature.automation.cloudSyncMarkFailed"))
        XCTAssertTrue(coordinator.contains("IOSAutomationCloudSyncLifecycle"))
        XCTAssertTrue(coordinator.contains("automationCloudSync.updateAccountScope(next)"))
        XCTAssertTrue(coordinator.contains("automationCloudSync.requestSync()"))
        XCTAssertTrue(dashboard.contains("service: \"AutomationsService\""))
        XCTAssertFalse(coordinator.contains("sand-shadow:"))

        let coordinatorRuntime = try String(
            contentsOf: repositoryRoot.appendingPathComponent(
                "source/mahayana-agent-coordinator/MahayanaCoordinator.swift"
            ),
            encoding: .utf8
        )
        XCTAssertTrue(coordinatorRuntime.contains("if channel == \"automation-lifecycle\""))
        XCTAssertTrue(coordinatorRuntime.contains("mcpSurface?.requestAutomationRelayDrain()"))
    }
}

extension CoordinatorMcpSurfaceTests {

    func testCloudAgentLaunchErrorPreservesTypedIdentityAndMessage() {
        let error = SandCloudAgentLaunchError("launch failed")
        XCTAssertEqual(SandCloudAgentLaunchError.name, "SandCloudAgentLaunchError")
        XCTAssertEqual(error.errorDescription, "launch failed")
    }

    func testCloudAgentLaunchShippingCallerWrapsConfigurationAndTransportErrors() throws {
        let testFile = URL(fileURLWithPath: #filePath)
        let repositoryRoot = testFile
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let source = try String(
            contentsOf: repositoryRoot.appendingPathComponent(
                "source/mahayana-agent-coordinator/coordinator-mcp-manager.swift"
            ),
            encoding: .utf8
        )
        XCTAssertTrue(source.contains("case \"launch\":\n            do {"))
        XCTAssertTrue(source.contains("throw SandCloudAgentLaunchError(wrapping: error)"))
        XCTAssertTrue(source.contains("dashboard.startBackgroundComposer"))
        XCTAssertTrue(source.contains("catch let error as CancellationError"))
    }
}
