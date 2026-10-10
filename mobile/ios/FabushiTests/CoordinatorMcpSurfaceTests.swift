import XCTest
@testable import Fabushi

@MainActor
private final class CoordinatorMcpFakeHost: MahayanaHostRequesting, @unchecked Sendable {
    private var disabled = Set(["search"])

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
            return .init(value: ["ok": true])

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
}
