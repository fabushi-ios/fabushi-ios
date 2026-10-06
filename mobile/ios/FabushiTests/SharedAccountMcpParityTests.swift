import XCTest
@testable import Fabushi

private enum AccountMcpTestError: Error {
    case unavailable
}

private final class FakeAccountMcpClient: @unchecked Sendable, AccountMcpClient {
    var available: [AvailableMcpServer] = []
    var configResponse = AccountMcpConfigResponse(configJson: #"{"mcpServers":{}}"#, serverMetadataByName: [:])
    var effective: [EffectivePluginWire] = []
    var failAvailable = false
    var installed: [UInt64] = []
    var uninstalled: [UInt64] = []
    var updated: [(UInt64, [String: String])] = []
    var writtenConfig: String?
    var writtenServerIds: [String: UInt64] = [:]

    func getAvailableMcpServers(timeoutMs: Int) async throws -> [AvailableMcpServer] {
        if failAvailable { throw AccountMcpTestError.unavailable }
        return available
    }

    func getMcpConfig(
        teamScope: Bool,
        redactSecrets: Bool,
        teamId: UInt64?,
        timeoutMs: Int?
    ) async throws -> AccountMcpConfigResponse {
        configResponse
    }

    func getEffectiveUserPlugins(excludeConfiguredVariables: Bool) async throws -> [EffectivePluginWire] {
        effective
    }

    func setMcpConfig(configJson: String, serverIdsByName: [String: UInt64]) async throws {
        writtenConfig = configJson
        writtenServerIds = serverIdsByName
    }

    func installUserPlugin(pluginId: UInt64, variables: [String: String]?) async throws {
        installed.append(pluginId)
    }

    func uninstallUserPlugin(pluginId: UInt64) async throws {
        uninstalled.append(pluginId)
    }

    func updateUserPluginInstall(pluginId: UInt64, variables: [String: String]) async throws {
        updated.append((pluginId, variables))
    }
}

final class SharedAccountMcpParityTests: XCTestCase {
    func testConfigParserPreservesStdioRemoteAuthAndTls() throws {
        let parsed = try XCTUnwrap(parseAccountMcpConfigJson(#"""
        {
          "mcpServers": {
            "local": {
              "type": "stdio",
              "command": "node",
              "args": ["server.js"],
              "env": {"A":"B"},
              "cwd": "/workspace"
            },
            "remote": {
              "type": "sse",
              "url": "https://mcp.example.test",
              "headers": {"X-Test":"1"},
              "auth": {
                "CLIENT_ID": "client",
                "CLIENT_SECRET": "secret",
                "scopes": ["read"]
              },
              "tls": {"caBundle":"  CERT  "}
            }
          }
        }
        """#))

        guard case .stdio(let local) = parsed.mcpServers["local"] else {
            return XCTFail("expected stdio config")
        }
        XCTAssertEqual(local.command, "node")
        XCTAssertEqual(local.args, ["server.js"])
        XCTAssertEqual(local.env, ["A": "B"])
        XCTAssertEqual(local.cwd, "/workspace")

        guard case .remote(let remote) = parsed.mcpServers["remote"] else {
            return XCTFail("expected remote config")
        }
        XCTAssertEqual(remote.type, "sse")
        XCTAssertEqual(remote.auth?.clientID, "client")
        XCTAssertEqual(remote.auth?.scopes, ["read"])
        XCTAssertEqual(remote.tls?.caBundle, "CERT")

        let encoded = try accountMcpConfigJson(parsed)
        XCTAssertNotNil(parseAccountMcpConfigJson(encoded))
    }

    func testParserRejectsUnsafeOrMalformedConfiguration() {
        XCTAssertNil(parseAccountMcpConfigJson(""))
        XCTAssertNil(parseAccountMcpConfigJson(#"{"mcpServers":{"bad":{"type":"http","command":"sh"}}}"#))
        XCTAssertNil(parseAccountMcpConfigJson(#"{"mcpServers":{"bad":{"url":"https://x","headers":{"A":1}}}}"#))
        XCTAssertNil(parseAccountMcpConfigJson(#"{"mcpServers":{"bad":{"url":"https://x","tls":{"caBundle":"CERT","extra":"x"}}}}"#))
    }

    func testMetadataHelpersMatchReferenceSemantics() {
        XCTAssertEqual(normalizeMcpAccountLabel("  Work@Example.COM "), "work@example.com")
        XCTAssertEqual(teamServerTransport("SSE"), "sse")
        XCTAssertEqual(teamServerTransport("websocket"), "http")
        XCTAssertEqual(
            serverIdsByNameFromMetadata([
                "keep": .init(serverId: 12),
                "drop": .init(serverId: 0),
                "missing": .init(),
            ]),
            ["keep": 12]
        )
        XCTAssertEqual(toEffectivePluginInstallMode(1), .user)
        XCTAssertEqual(toEffectivePluginInstallMode(3), .teamRequired)
        XCTAssertEqual(toEffectivePluginInstallMode(99), .unknown)
    }

    func testFetchCombinesBackendMetadataWithoutRunningLocalProcesses() async throws {
        let client = FakeAccountMcpClient()
        client.available = [
            .init(
                id: 1, name: "stdio", serverIdentifier: "stdio-id", type: "stdio",
                enabled: true, isTeamServer: false, disabledByTeamAdminPolicy: false,
                pluginId: 100,
                accounts: [.init(accountKey: " USER@EXAMPLE.COM ", serverIdentifier: "slot", userHasAccessToken: true)]
            ),
            .init(
                id: 2, name: "remote", serverIdentifier: "remote-id", type: "http",
                url: "https://mcp.example.test", enabled: true, isTeamServer: false,
                disabledByTeamAdminPolicy: false
            ),
            .init(
                id: 3, name: "blocked", serverIdentifier: "blocked-id", type: "stdio",
                command: "blocked-command", args: ["--safe-metadata-only"],
                enabled: false, isTeamServer: false, disabledByTeamAdminPolicy: true
            ),
            .init(
                id: 4, name: "unresolved", serverIdentifier: "unresolved-id", type: "stdio",
                enabled: true, isTeamServer: false, disabledByTeamAdminPolicy: false
            ),
        ]
        client.configResponse = .init(
            configJson: #"{"mcpServers":{"stdio":{"command":"npx","args":["-y","server"]}}}"#,
            serverMetadataByName: ["stdio": .init(serverId: 1)]
        )

        let deps = AccountMcpDependencies(
            getAccessToken: { _ in "token-a" },
            getMachineId: { "machine" },
            getBackendUrl: { "https://api.example.test" },
            createClient: { _ in client }
        )
        let fetched = await fetchAccountMcpServers(deps)
        let result = try XCTUnwrap(fetched)

        XCTAssertEqual(result.cacheScope, accountCacheScope("token-a"))
        XCTAssertEqual(result.servers.map(\.id), ["1", "2", "3"])
        XCTAssertEqual(result.unresolvedServerIds, ["4"])
        XCTAssertFalse(result.unavailable)

        let stdio = try XCTUnwrap(result.servers.first(where: { $0.id == "1" }))
        guard case .stdio(let config) = stdio.config else {
            return XCTFail("expected metadata-only stdio config")
        }
        XCTAssertEqual(config.command, "npx")
        XCTAssertEqual(stdio.accounts?.first?.accountKey, "user@example.com")
        XCTAssertEqual(stdio.pluginId, "100")
    }

    func testFetchDistinguishesAuthenticationFailureFromBackendUnavailability() async {
        let client = FakeAccountMcpClient()
        client.failAvailable = true

        let backendDown = await fetchAccountMcpServers(.init(
            getAccessToken: { _ in "token-a" },
            getMachineId: { "machine" },
            getBackendUrl: { "https://api.example.test" },
            createClient: { _ in client }
        ))
        XCTAssertEqual(backendDown?.unavailable, true)
        XCTAssertEqual(backendDown?.cacheScope, accountCacheScope("token-a"))

        let noIdentity = await fetchAccountMcpServers(.init(
            getAccessToken: { _ in throw AccountMcpTestError.unavailable },
            getMachineId: { "machine" },
            getBackendUrl: { "https://api.example.test" },
            createClient: { _ in client }
        ))
        XCTAssertNil(noIdentity)
    }

    func testEffectivePluginsBackfillAndWriterPreserveBackendContracts() async throws {
        let client = FakeAccountMcpClient()
        client.available = [
            .init(
                id: 7, name: "plugin-server", serverIdentifier: "plugin",
                type: "http", url: "https://plugin.example.test",
                enabled: true, isTeamServer: false, disabledByTeamAdminPolicy: false,
                pluginId: 55
            ),
        ]
        client.effective = [
            .init(
                plugin: .init(id: 44, name: "required", displayName: ""),
                installMode: 0,
                isTeamRequired: true,
                isEnabled: true
            ),
        ]
        client.configResponse = .init(
            configJson: #"{"mcpServers":{"remote":{"url":"https://mcp.example.test"}}}"#,
            serverMetadataByName: ["remote": .init(serverId: 9)]
        )

        let deps = AccountMcpDependencies(
            getAccessToken: { _ in "token" },
            getMachineId: { "machine" },
            getBackendUrl: { "https://api.example.test" },
            createClient: { _ in client }
        )

        let effective = try await fetchEffectiveUserPlugins(deps)
        XCTAssertEqual(effective.first?.installMode, .teamRequired)
        XCTAssertEqual(effective.first?.displayName, "required")

        let backfilled = await backfillUserPluginInstalls(deps)
        XCTAssertEqual(backfilled, ["55"])
        XCTAssertEqual(client.installed, [55])

        let writer = createAccountMcpWriter(deps)
        let edit = try await writer.getConfigForEdit()
        XCTAssertEqual(edit.serverIdsByName, ["remote": 9])
        try await writer.setConfig(edit.config, serverIdsByName: edit.serverIdsByName)
        XCTAssertNotNil(client.writtenConfig)
        XCTAssertEqual(client.writtenServerIds, ["remote": 9])

        try await writer.uninstallPlugin(pluginId: 55)
        try await writer.updatePluginInstall(pluginId: 44, variables: ["TOKEN": "value"])
        XCTAssertEqual(client.uninstalled, [55])
        XCTAssertEqual(client.updated.first?.0, 44)
        XCTAssertEqual(client.updated.first?.1, ["TOKEN": "value"])
    }
}


private actor CursorDashboardProtoRequestRecorder {
    private var requests: [URLRequest] = []

    func record(_ request: URLRequest) -> Data {
        requests.append(request)
        switch request.url?.path {
        case "/aiserver.v1.DashboardService/GetTeams":
            return Data([
                0x0a, 0x0b,
                0x0a, 0x04, 0x43, 0x6f, 0x72, 0x65,
                0x10, 0x07,
                0xa0, 0x02, 0x01,
            ])
        case "/aiserver.v1.DashboardService/PublishPlugin":
            return Data([
                0x08, 0x2a,
                0x10, 0x0b,
                0x1a, 0x06, 0x61, 0x62, 0x63, 0x31, 0x32, 0x33,
            ])
        case "/aiserver.v1.DashboardService/UnpublishPlugin":
            return Data([0x0a, 0x03, 0x6f, 0x6b, 0x31])
        default:
            return Data()
        }
    }

    func snapshot() -> [URLRequest] {
        requests
    }
}

final class CursorDashboardProtoConnectParityTests: XCTestCase {
    func testSkillPublishingUsesDesktopTypedBinaryConnectContract() async throws {
        let recorder = CursorDashboardProtoRequestRecorder()
        let credentials = AccountMcpCredentials(
            getAccessToken: { _ in "cursor-token" },
            getMachineId: { "machine-1" }
        )
        let client = IOSCursorDashboardClient(
            credentials: credentials,
            backendURL: URL(string: "https://api.example.test")!,
            requestExecutor: { request in
                let body = await recorder.record(request)
                let response = HTTPURLResponse(
                    url: request.url!,
                    statusCode: 200,
                    httpVersion: "HTTP/2",
                    headerFields: ["Content-Type": "application/proto"]
                )!
                return (body, response)
            }
        )

        let teams = try await client.getSkillPublishTeams()
        XCTAssertEqual(teams, [
            .init(teamId: 7, name: "Core", isDirectMember: true),
        ])

        let published = try await client.publishSkillPlugin(
            teamId: 7,
            name: "skill-a",
            displayName: "Skill A",
            description: "Desc",
            pluginTarGz: Data([0x01, 0x02, 0x03])
        )
        XCTAssertEqual(published, .init(pluginId: "42", commitSha: "abc123"))

        try await client.unpublishSkillPlugin(pluginId: "42", teamId: 7)

        let requests = await recorder.snapshot()
        XCTAssertEqual(requests.count, 3)
        XCTAssertEqual(requests.map { $0.httpMethod }, ["POST", "POST", "POST"])
        XCTAssertEqual(
            requests.map { $0.url?.path },
            [
                "/aiserver.v1.DashboardService/GetTeams",
                "/aiserver.v1.DashboardService/PublishPlugin",
                "/aiserver.v1.DashboardService/UnpublishPlugin",
            ]
        )
        for request in requests {
            XCTAssertEqual(request.value(forHTTPHeaderField: "Content-Type"), "application/proto")
            XCTAssertEqual(request.value(forHTTPHeaderField: "Connect-Protocol-Version"), "1")
            XCTAssertEqual(request.value(forHTTPHeaderField: "Authorization"), "Bearer cursor-token")
            XCTAssertTrue(
                request.value(forHTTPHeaderField: "x-cursor-checksum")?.hasSuffix("machine-1") == true
            )
        }
        XCTAssertEqual(requests[0].httpBody, Data([0x08, 0x01]))
        XCTAssertEqual(
            requests[1].httpBody,
            Data([
                0x08, 0x07,
                0x12, 0x07, 0x73, 0x6b, 0x69, 0x6c, 0x6c, 0x2d, 0x61,
                0x1a, 0x07, 0x53, 0x6b, 0x69, 0x6c, 0x6c, 0x20, 0x41,
                0x22, 0x04, 0x44, 0x65, 0x73, 0x63,
                0x2a, 0x03, 0x01, 0x02, 0x03,
            ])
        )
        XCTAssertEqual(requests[2].httpBody, Data([0x08, 0x2a, 0x10, 0x07]))
    }
}
