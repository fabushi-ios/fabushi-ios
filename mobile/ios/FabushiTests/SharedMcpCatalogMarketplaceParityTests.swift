import XCTest
@testable import Fabushi

private actor MarketplaceParityState {
    var authenticated = false
    var fetches = 0

    func setAuthenticated(_ value: Bool) { authenticated = value }
    func isAuthenticated() -> Bool { authenticated }
    func recordFetch() { fetches += 1 }
    func fetchCount() -> Int { fetches }
}

private actor MarketplaceListingClientStub: SandMarketplaceListingClient {
    let base: [MarketplaceWirePlugin]
    let privateMarketplaces: [MarketplaceWireMarketplace]
    let privatePlugins: [UInt64: [MarketplaceWirePlugin]]

    init(
        base: [MarketplaceWirePlugin],
        privateMarketplaces: [MarketplaceWireMarketplace] = [],
        privatePlugins: [UInt64: [MarketplaceWirePlugin]] = [:]
    ) {
        self.base = base
        self.privateMarketplaces = privateMarketplaces
        self.privatePlugins = privatePlugins
    }

    func listMarketplacePlugins(
        marketplaceId: UInt64?,
        excludeCloudAgentPlugins: Bool,
        timeoutMs: Int
    ) async throws -> MarketplacePluginListing {
        XCTAssertTrue(excludeCloudAgentPlugins)
        if let marketplaceId {
            return .init(plugins: privatePlugins[marketplaceId] ?? [])
        }
        return .init(plugins: base)
    }

    func listMarketplaces(timeoutMs: Int) async throws -> [MarketplaceWireMarketplace] {
        privateMarketplaces
    }

    func getPluginMcpConfig(pluginId: UInt64, timeoutMs: Int) async throws -> String? {
        nil
    }
}

final class SharedMcpCatalogMarketplaceParityTests: XCTestCase {
    private static func plugin(
        id: String = "101",
        displayName: String = "GitHub",
        required: Bool = false
    ) -> SandMarketplacePlugin {
        .init(
            pluginId: id,
            name: "github",
            displayName: displayName,
            description: "Repository tools",
            category: "Developer",
            sourceUrls: [
                "https://github.com/example/connectors/blob/main/github/mcp.json",
            ],
            connectors: [.init(name: "github", description: "GitHub MCP")],
            variableFields: required
                ? [.init(
                    key: "API_TOKEN",
                    label: "API Token",
                    placeholder: "API_TOKEN",
                    isRequired: true,
                    isSecret: true
                )]
                : []
        )
    }

    func testCatalogCacheIsSeparatedByAuthenticationScope() async throws {
        let state = MarketplaceParityState()
        let core = SandMcpCatalogCore(
            isAuthenticated: { await state.isAuthenticated() },
            fetchMarketplace: {
                await state.recordFetch()
                let authenticated = await state.isAuthenticated()
                return .init(
                    plugins: [
                        Self.plugin(
                            id: authenticated ? "202" : "101",
                            displayName: authenticated ? "Private" : "Public"
                        ),
                    ],
                    includesPrivateMarketplaces: authenticated
                )
            },
            requireAccountWriter: {
                throw SandMcpConfigError("writer should not be needed")
            },
            reloadServers: {}
        )
        let catalog = SandMcpCatalogFlow(core: core)

        let publicFirst = try await catalog.getCatalog()
        let publicCached = try await catalog.getCatalog()
        XCTAssertEqual(publicFirst.map(\.displayName), ["Public"])
        XCTAssertEqual(publicCached, publicFirst)
        let publicFetchCount = await state.fetchCount()
        XCTAssertEqual(publicFetchCount, 1)

        await state.setAuthenticated(true)
        let privateView = try await catalog.getCatalog()
        XCTAssertEqual(privateView.map(\.displayName), ["Private"])
        let privateFetchCount = await state.fetchCount()
        XCTAssertEqual(privateFetchCount, 2)
    }

    func testCatalogRequiredVariablesFailBeforeAccountMutation() async throws {
        let catalog = SandMcpCatalogFlow(core: .init(
            isAuthenticated: { false },
            fetchMarketplace: {
                .init(
                    plugins: [Self.plugin(required: true)],
                    includesPrivateMarketplaces: false
                )
            },
            requireAccountWriter: {
                XCTFail("missing required variables must fail before account writer access")
                throw SandMcpConfigError("writer must not be reached")
            },
            reloadServers: {
                XCTFail("missing required variables must not reload")
            }
        ))

        _ = try await catalog.getCatalog()
        do {
            try await catalog.installEntry(entryId: "101")
            XCTFail("required API token must be enforced")
        } catch let error as SandMcpConfigError {
            XCTAssertTrue(error.message.contains("API Token"))
            XCTAssertTrue(error.message.contains("API_TOKEN"))
        }
    }

    func testMarketplaceHelpersPreserveGrokTransportAndSourceSemantics() throws {
        XCTAssertEqual(
            toRawGithubUrl(
                "https://github.com/example/connectors/blob/main/github/mcp.json"
            ),
            "https://raw.githubusercontent.com/example/connectors/main/github/mcp.json"
        )
        XCTAssertNil(toRawGithubUrl("http://github.com/example/repo/blob/main/mcp.json"))
        XCTAssertNil(toRawGithubUrl("https://example.com/example/repo/blob/main/mcp.json"))

        let raw: [String: Any] = [
            "mcpServers": [
                "github": [
                    "transport": "streamableHttp",
                    "url": "https://mcp.example.test",
                ],
                "local": [
                    "transport": "stdio",
                    "command": "node",
                ],
            ],
        ]
        let parsed = normalizePluginConfig(raw) { value in
            guard let object = value as? [String: Any] else { return nil }
            if let url = object["url"] as? String {
                let type = object["type"] as? String
                return type == "sse" ? .sse(url: url) : .http(url: url)
            }
            if let command = object["command"] as? String {
                return .stdio(command: command)
            }
            return nil
        }
        XCTAssertEqual(parsed["github"]?.transport, .http)
        XCTAssertEqual(parsed["github"]?.url, "https://mcp.example.test")
        XCTAssertEqual(parsed["local"]?.transport, .stdio)
        XCTAssertEqual(parsed["local"]?.command, "node")
    }

    func testAuthenticatedMarketplaceAddsPrivateListingsAndDeduplicatesByPluginId() async throws {
        let team = MarketplaceWireMarketplace(
            id: 9,
            name: "team",
            displayName: "Team Marketplace",
            teamId: 44
        )
        let base = MarketplaceWirePlugin(
            id: 101,
            name: "public",
            displayName: "Public",
            mcpServers: [.init(name: "public")]
        )
        let privateReplacement = MarketplaceWirePlugin(
            id: 101,
            name: "private-replacement",
            displayName: "Private Replacement",
            mcpServers: [.init(name: "private-replacement")],
            marketplace: team
        )
        let privateOnly = MarketplaceWirePlugin(
            id: 202,
            name: "private-only",
            displayName: "Private Only",
            skills: [.init(name: "skill")]
        )
        let client = MarketplaceListingClientStub(
            base: [base],
            privateMarketplaces: [team],
            privatePlugins: [9: [privateReplacement, privateOnly]]
        )

        let result = try await fetchMarketplaceMcpPlugins(deps: .init(
            bestEffortToken: { "token" },
            createClient: { client },
            rememberPluginLogoUrl: { _ in }
        ))

        XCTAssertTrue(result.includesPrivateMarketplaces)
        XCTAssertEqual(result.plugins.map(\.pluginId), ["202", "101"])
        XCTAssertEqual(
            result.plugins.first { $0.pluginId == "101" }?.displayName,
            "Private Replacement"
        )
        XCTAssertEqual(
            result.plugins.first { $0.pluginId == "101" }?.marketplace?.ownership,
            .team
        )
    }

    func testAnonymousMarketplaceDoesNotEnumeratePrivateMarketplaces() async throws {
        let client = MarketplaceListingClientStub(
            base: [
                .init(
                    id: 1,
                    name: "public",
                    displayName: "Public",
                    mcpServers: [.init(name: "public")]
                ),
            ],
            privateMarketplaces: [
                .init(id: 9, name: "team", displayName: "Team", teamId: 44),
            ],
            privatePlugins: [
                9: [
                    .init(
                        id: 2,
                        name: "private",
                        displayName: "Private",
                        mcpServers: [.init(name: "private")]
                    ),
                ],
            ]
        )

        let result = try await fetchMarketplaceMcpPlugins(deps: .init(
            bestEffortToken: { nil },
            createClient: { client },
            rememberPluginLogoUrl: { _ in }
        ))

        XCTAssertFalse(result.includesPrivateMarketplaces)
        XCTAssertEqual(result.plugins.map(\.pluginId), ["1"])
    }
    func testMarketplaceLogoRegistryPreservesExactStringMembership() {
        CursorMarketplaceLogoRegistry.shared.resetForTesting()
        defer { CursorMarketplaceLogoRegistry.shared.resetForTesting() }

        rememberPluginLogoUrl("")
        rememberPluginLogoUrl("https://cdn.example.test/logo.png")

        XCTAssertTrue(isKnownPluginLogoUrl(""))
        XCTAssertTrue(isKnownPluginLogoUrl("https://cdn.example.test/logo.png"))
        XCTAssertFalse(isKnownPluginLogoUrl("https://cdn.example.test/other.png"))
    }


}
