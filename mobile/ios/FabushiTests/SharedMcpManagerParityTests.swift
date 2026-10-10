import XCTest
@testable import Fabushi

private final class ManagerDashboardClient: @unchecked Sendable, DashboardMcpExecClient {
    var listToolsResponse: [BackendMcpToolServerWire] = []
    var authStatusCalls = 0

    func listSandMcpTools(
        serverIdentifiers: [String],
        timeoutMs: Int
    ) async throws -> [BackendMcpToolServerWire] {
        listToolsResponse.filter { serverIdentifiers.contains($0.serverIdentifier) }
    }

    func executeSandMcpTool(
        serverIdentifier: String,
        toolName: String,
        args: McpJSONValue,
        toolCallId: String,
        agentId: String,
        timeoutMs: Int
    ) async throws -> McpExecResult? {
        .init(
            caseName: "success",
            value: .object([
                "content": .array([
                    .object([
                        "type": .string("text"),
                        "text": .string("ok"),
                    ]),
                ]),
            ])
        )
    }

    func checkHttpMcpStatus(
        serverIds: [String],
        oauthRedirectUri: String,
        forceReauth: Bool,
        accountKey: String,
        timeoutMs: Int
    ) async throws -> [BackendMcpAuthStatusWire] {
        authStatusCalls += 1
        return []
    }

    func completeMcpOAuth(
        stateId: String,
        authorizationCode: String,
        timeoutMs: Int
    ) async throws {}

    func validateMcpOAuthTokens(
        targets: [BackendMcpTokenTarget],
        timeoutMs: Int
    ) async throws -> [BackendMcpTokenValidation] {
        []
    }

    func deleteMcpOAuthToken(
        serverUrl: String,
        accountKey: String,
        source: String,
        timeoutMs: Int
    ) async throws {}

    func renameMcpOAuthAccount(
        serverId: String,
        accountKey: String,
        newAccountKey: String,
        timeoutMs: Int
    ) async throws {}

    func deleteMcpOAuthAccount(
        serverId: String,
        accountKey: String,
        timeoutMs: Int
    ) async throws {}
}

private actor ManagerDisplayProvider {
    private var display: AccountDisplayConfig

    init(_ display: AccountDisplayConfig) {
        self.display = display
    }

    func get() -> AccountDisplayConfig { display }
    func set(_ next: AccountDisplayConfig) { display = next }
}

private struct ManagerFixture {
    let manager: SandMcpManager
    let settings: SandSettingsStore
    let client: ManagerDashboardClient
    let root: URL
}

private func makeManagerFixture(
    provider: ManagerDisplayProvider,
    client: ManagerDashboardClient,
    setToolDisabled: (@Sendable (String, String, Bool) async throws -> Void)? = nil
) -> ManagerFixture {
    let root = FileManager.default.temporaryDirectory
        .appendingPathComponent("fabushi-mcp-manager-\(UUID().uuidString)")
    try? FileManager.default.createDirectory(
        at: root,
        withIntermediateDirectories: true
    )
    let settings = SandSettingsStore(
        settingsPath: root.appendingPathComponent("settings.json").path
    )
    let backend = DashboardSandBackendMcpExec(
        deps: .init(client: client)
    )
    let definitionSource = SandMcpDefinitionSource(
        includeBuiltins: false,
        provider: {
            runtimeConfigFromDisplay(await provider.get())
        }
    )
    let discovery = SandMcpToolsDiscovery(deps: .init(
        definitionSource: definitionSource,
        settingsStore: settings,
        backendListTools: { names in
            try await backend.listTools(serverIdentifiers: names)
        },
        backendExecuteTool: { identifier, tool, args, callId, agentId in
            sandMcpResultFromBackend(
                await backend.executeTool(
                    serverIdentifier: identifier,
                    toolName: tool,
                    args: args,
                    toolCallId: callId,
                    agentId: agentId
                )
            )
        }
    ))
    var dependencies = SandMcpManagerDependencies(
        settingsStore: settings,
        backendMcpExec: backend,
        definitionSource: definitionSource,
        toolsDiscovery: discovery,
        accountDisplayConfigProvider: { _ in await provider.get() },
        autoPollEnabled: false
    )
    dependencies.setToolDisabled = setToolDisabled
    let manager = SandMcpManager(deps: dependencies)
    return .init(
        manager: manager,
        settings: settings,
        client: client,
        root: root
    )
}

final class SharedMcpManagerParityTests: XCTestCase {
    func testManagerListsBackendAndRemoteRunnerOnlyStdioRows() async throws {
        let display = AccountDisplayConfig(
            servers: [
                .init(
                    id: "1",
                    name: "Remote",
                    serverIdentifier: "remote",
                    config: .http(url: "https://mcp.example.test"),
                    isTeamServer: false
                ),
                .init(
                    id: "2",
                    name: "Local Process",
                    serverIdentifier: "local",
                    config: .stdio(command: "node", args: ["server.js"]),
                    isTeamServer: false
                ),
            ],
            cacheScope: "account-a"
        )
        let provider = ManagerDisplayProvider(display)
        let client = ManagerDashboardClient()
        client.listToolsResponse = [
            .init(
                serverIdentifier: "remote",
                status: "connected",
                tools: [
                    .init(
                        name: "search",
                        providerIdentifier: "remote",
                        toolName: "search",
                        description: "Search",
                        inputSchema: nil
                    ),
                ],
                accountLabel: "default",
                rowServerIdentifier: "remote"
            ),
        ]
        let fixture = makeManagerFixture(provider: provider, client: client)
        defer { try? FileManager.default.removeItem(at: fixture.root) }

        let state = try await fixture.manager.listServers()
        let remote = try XCTUnwrap(state.servers.first { $0.id == "1" })
        let stdio = try XCTUnwrap(state.servers.first { $0.id == "2" })

        XCTAssertEqual(remote.status, "connected")
        XCTAssertEqual(remote.toolCount, 1)
        XCTAssertEqual(stdio.status, "disconnected")
        XCTAssertEqual(stdio.statusDetail, "Runs on a Remote Runner")

        let connected = try await fixture.manager.listConnectedBackendTools()
        XCTAssertEqual(connected.map(\.toolName), ["search"])
    }

    func testManagerNeverAuthenticatesOrExecutesStdioLocallyWithoutRunner() async throws {
        let display = AccountDisplayConfig(
            servers: [
                .init(
                    id: "2",
                    name: "Local Process",
                    serverIdentifier: "local",
                    config: .stdio(command: "node", args: ["server.js"]),
                    isTeamServer: false
                ),
            ],
            cacheScope: "account-a"
        )
        let provider = ManagerDisplayProvider(display)
        let client = ManagerDashboardClient()
        let fixture = makeManagerFixture(provider: provider, client: client)
        defer { try? FileManager.default.removeItem(at: fixture.root) }

        let auth = try await fixture.manager.authenticateServer("2")
        XCTAssertEqual(auth.status, .notSupported)
        XCTAssertTrue(auth.message?.contains("Runner-managed stdio") == true)
        XCTAssertEqual(client.authStatusCalls, 0)

        let result = await fixture.manager.executeTool(.init(
            providerIdentifier: "local",
            name: "run",
            args: .object([:]),
            toolCallId: "call-1"
        ))
        guard case .error(let message) = result.result else {
            return XCTFail("stdio without a Runner must fail closed")
        }
        XCTAssertTrue(message.contains("Remote Runner"))
    }

    func testAccountScopeChangeClearsAccountScopedMcpSettings() async throws {
        let first = AccountDisplayConfig(
            servers: [
                .init(
                    id: "1",
                    name: "First",
                    serverIdentifier: "first",
                    config: .http(url: "https://first.example.test"),
                    isTeamServer: false
                ),
            ],
            cacheScope: "account-a"
        )
        let provider = ManagerDisplayProvider(first)
        let client = ManagerDashboardClient()
        let fixture = makeManagerFixture(provider: provider, client: client)
        defer { try? FileManager.default.removeItem(at: fixture.root) }

        _ = try await fixture.manager.listServers()
        fixture.settings.setMcpCustomInstructionsByServerId(["1": "private instruction"])
        fixture.settings.setMcpDisabledToolsByServerId(["1": ["search"]])

        await provider.set(.init(
            servers: [
                .init(
                    id: "2",
                    name: "Second",
                    serverIdentifier: "second",
                    config: .http(url: "https://second.example.test"),
                    isTeamServer: false
                ),
            ],
            cacheScope: "account-b"
        ))
        _ = try await fixture.manager.listServers()

        XCTAssertTrue(fixture.settings.getMcpCustomInstructionsByServerId().isEmpty)
        XCTAssertTrue(fixture.settings.getMcpDisabledToolsByServerId().isEmpty)
        let lastDisplayConfig = await fixture.manager.lastAccountDisplayConfigView()
        XCTAssertEqual(lastDisplayConfig?.cacheScope, "account-b")
    }

    func testFreshResolutionDoesNotFallBackToStaleUnavailableDisplay() async throws {
        let first = AccountDisplayConfig(
            servers: [
                .init(
                    id: "1",
                    name: "First",
                    serverIdentifier: "first",
                    config: .http(url: "https://first.example.test"),
                    isTeamServer: false
                ),
            ],
            cacheScope: "account-a"
        )
        let provider = ManagerDisplayProvider(first)
        let client = ManagerDashboardClient()
        let fixture = makeManagerFixture(provider: provider, client: client)
        defer { try? FileManager.default.removeItem(at: fixture.root) }

        _ = try await fixture.manager.listServers()
        await provider.set(.init(
            servers: [],
            cacheScope: "account-a",
            unavailable: true
        ))

        do {
            _ = try await fixture.manager.resolveDisplayServer(
                "1",
                requireFreshRead: true
            )
            XCTFail("fresh policy/auth reads must fail closed when unavailable")
        } catch let error as SandMcpConfigError {
            XCTAssertTrue(error.message.contains("unavailable"))
        }
    }

    func testLogoutScopeClearsAccountScopedStateAndCachedDisplay() async throws {
        let provider = ManagerDisplayProvider(.init(
            servers: [
                .init(
                    id: "1",
                    name: "Remote",
                    serverIdentifier: "remote",
                    config: .http(url: "https://mcp.example.test"),
                    isTeamServer: false
                ),
            ],
            cacheScope: "account-a"
        ))
        let client = ManagerDashboardClient()
        let fixture = makeManagerFixture(provider: provider, client: client)
        defer { try? FileManager.default.removeItem(at: fixture.root) }

        _ = try await fixture.manager.listServers()
        fixture.settings.setMcpCustomInstructionsByServerId(["1": "private"])
        fixture.settings.setMcpDisabledToolsByServerId(["1": ["search"]])

        await provider.set(.init(servers: [], cacheScope: nil))
        let loggedOut = try await fixture.manager.listServers()

        XCTAssertTrue(loggedOut.servers.isEmpty)
        XCTAssertTrue(fixture.settings.getMcpCustomInstructionsByServerId().isEmpty)
        XCTAssertTrue(fixture.settings.getMcpDisabledToolsByServerId().isEmpty)
        let lastDisplay = await fixture.manager.lastAccountDisplayConfigView()
        XCTAssertNil(lastDisplay?.cacheScope)
        XCTAssertTrue(lastDisplay?.servers.isEmpty == true)
    }

    func testRemoteToolToggleFailureLeavesLocalDisabledStateUnchanged() async throws {
        let provider = ManagerDisplayProvider(.init(
            servers: [
                .init(
                    id: "1",
                    name: "Remote",
                    serverIdentifier: "remote",
                    config: .http(url: "https://mcp.example.test"),
                    isTeamServer: false
                ),
            ],
            cacheScope: "account-a"
        ))
        let client = ManagerDashboardClient()
        client.listToolsResponse = [
            .init(
                serverIdentifier: "remote",
                status: "connected",
                tools: [
                    .init(
                        name: "remote.search",
                        providerIdentifier: "remote",
                        toolName: "search",
                        description: "Search",
                        inputSchema: nil
                    ),
                ],
                accountLabel: "default",
                rowServerIdentifier: "remote"
            ),
        ]
        let fixture = makeManagerFixture(
            provider: provider,
            client: client,
            setToolDisabled: { _, _, _ in
                throw SandMcpConfigError("remote mutation failed")
            }
        )
        defer { try? FileManager.default.removeItem(at: fixture.root) }

        _ = try await fixture.manager.listServers()
        XCTAssertTrue(fixture.settings.getMcpDisabledToolsByServerId().isEmpty)

        do {
            _ = try await fixture.manager.setMcpToolDisabled(
                serverId: "1",
                toolName: "search",
                disabled: true
            )
            XCTFail("remote mutation failure must be surfaced")
        } catch let error as SandMcpConfigError {
            XCTAssertTrue(error.message.contains("remote mutation failed"))
        }

        XCTAssertTrue(fixture.settings.getMcpDisabledToolsByServerId().isEmpty)
    }

}
