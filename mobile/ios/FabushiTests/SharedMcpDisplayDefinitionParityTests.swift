import XCTest
@testable import Fabushi

private final class TestMcpSettingsStore: McpSettingsPort {
    var disabled: [String:[String]] = [:]
    var legacy: [String:String] = [:]
    var byId: [String:String] = [:]
    var migrations: [(String,String)] = []
    var writes: [(String,String,String,Bool)] = []

    func migrateMcpCustomInstructionToServerId(serverId: String, displayName: String) {
        migrations.append((serverId,displayName))
    }
    func setMcpCustomInstructionByServerId(serverId: String, displayName: String, value: String, mirrorLegacyName: Bool) {
        byId[serverId]=value
        if mirrorLegacyName { legacy[displayName]=value }
        writes.append((serverId,displayName,value,mirrorLegacyName))
    }
    func getMcpDisabledToolsByServerId() -> [String : [String]] { disabled }
    func setMcpDisabledToolsByServerId(_ value: [String : [String]]) { disabled=value }
    func getMcpCustomInstructions() -> [String : String] { legacy }
    func getMcpCustomInstructionsByServerId() -> [String : String] { byId }
}

final class SharedMcpDisplayDefinitionParityTests: XCTestCase {
    func testDisplayProjectionAndStatuses() throws {
        let server = DisplayServer(
            id:"1",
            name:"GitHub",
            serverIdentifier:"github",
            config:.http(url:"https://mcp.example.com"),
            isTeamServer:false,
            pluginId:"10",
            isRequired:true
        )
        let runtime = runtimeConfigFromDisplay(.init(servers:[server]))
        XCTAssertEqual(runtime?.mcpServers["github"], .http(url:"https://mcp.example.com"))
        XCTAssertEqual(try normalizeAccountKey(" Work "), "work")
        XCTAssertThrowsError(try normalizeAccountKey("   "))
        XCTAssertEqual(try validateMarketplacePluginId(" 123 "), "123")
        XCTAssertThrowsError(try validateMarketplacePluginId("a12"))
        XCTAssertEqual(statusFromBackendListStatus("loading").status, "initializing")
        XCTAssertEqual(statusFromBoxListStatus(nil, unavailable:true).statusDetail, "Fabushi's computer unreachable")
    }

    func testBuiltinUsesRemoteRunnerAndNeverLocalProcess() {
        let none = resolveBoxComputerRuntime(boxMcpActive:true, remoteRunnerMcpUrl:nil)
        XCTAssertTrue(getBuiltinMcpServers(none).isEmpty)
        let remote = resolveBoxComputerRuntime(
            boxMcpActive:true,
            remoteRunnerMcpUrl:"https://runner.example.com/mcp"
        )
        let config = getBuiltinMcpServers(remote)[BOX_COMPUTER_SERVER_NAME]
        XCTAssertEqual(config?.transport, .http)
        XCTAssertEqual(config?.url, "https://runner.example.com/mcp")
        XCTAssertNil(config?.command)
    }

    func testDefinitionSourceFiltersBuiltinsAndProvidesLastKnownFallback() async {
        let config = McpRuntimeConfig(mcpServers:[
            "account":.http(url:"https://account.example/mcp"),
            BOX_COMPUTER_SERVER_NAME:.http(url:"https://wrong-account.example/mcp"),
            "local":.stdio(command:"tool")
        ])
        let source = SandMcpDefinitionSource(
            includeBuiltins:true,
            provider:{ config },
            builtinProvider:{
                [BOX_COMPUTER_SERVER_NAME:.http(url:"https://runner.example/mcp")]
            }
        )
        await source.ensureConfigLoaded()
        let users = await source.getUserServerConfigs()
        XCTAssertNotNil(users["account"])
        XCTAssertNil(users[BOX_COMPUTER_SERVER_NAME])
        let stdioServers = await source.getStdioServerConfigs()
        XCTAssertEqual(stdioServers["local"]?.command, "tool")
        let definitions = await source.getDefinitions()
        XCTAssertEqual(definitions.map(\.identifier), [BOX_COMPUTER_SERVER_NAME,"account"])
    }

    func testInstructionToggleAndListingSummaryBehavior() async throws {
        let settings = TestMcpSettingsStore()
        settings.byId["1"]="Prefer raw data"
        let server = DisplayServer(
            id:"1",
            name:"GitHub",
            serverIdentifier:"github",
            config:.http(url:"https://example.com"),
            isTeamServer:false,
            accounts:[.init(accountKey:"default",hasToken:true,serverIdentifier:"github")]
        )
        var display = AccountDisplayConfig(servers:[server])
        let toggles = SandMcpInstructionsAndToggles(
            settingsStore:{settings},
            resolveDisplayServer:{ id in id=="1" ? server:nil },
            listServers:{ "listed" },
            lastAccountDisplayConfig:{display},
            getToolsRaw:{
                [
                    .init(providerIdentifier:"github",toolName:"search"),
                    .init(providerIdentifier:"github",toolName:"search"),
                    .init(providerIdentifier:"other",toolName:"skip"),
                ]
            }
        )
        let tools = try await toggles.listServerTools("1")
        XCTAssertEqual(tools.map(\.name),["search"])
        let disabled = try await toggles.toggleMcpToolDisabled(serverId:"1",toolName:"search")
        XCTAssertEqual(disabled.first?.isDisabled,true)
        _ = try await toggles.setServerCustomInstructions(serverId:"1",instructions:"raw only")
        XCTAssertEqual(settings.byId["1"],"raw only")
        XCTAssertEqual(toggles.getMcpCustomInstructions()["github"],"raw only")

        let summaries = SandMcpListingSummaries(
            settingsStore:{settings},
            isRemoteRunnerExecWired:{false}
        )
        let backend = summaries.createBackendServerSummaries(
            server:server,
            entries:[.init(accountLabel:"default",status:"connected",tools:[.init(toolName:"search")])]
        )
        XCTAssertEqual(backend.first?.disabledToolCount,1)
        XCTAssertEqual(backend.first?.toolCount,0)
        let box = summaries.createBoxServerSummary(server:server,box:nil,unavailable:false)
        XCTAssertEqual(box.status,"disconnected")
        XCTAssertEqual(box.statusDetail,"Runs on a Remote Runner")
        display = .init(servers:[])
    }

    func testRowOwnershipIncludesAccountSlots() {
        XCTAssertTrue(backendEntryBelongsToRow(rowServerIdentifier:"row",rowIdentifier:"row"))
        XCTAssertTrue(displayRowOwnsIdentifier(
            "slot-1",
            rowIdentifier:"row",
            slots:[.init(accountKey:"a",hasToken:true,serverIdentifier:"slot-1")]
        ))
        XCTAssertFalse(displayRowOwnsIdentifier("other",rowIdentifier:"row"))
    }
}
