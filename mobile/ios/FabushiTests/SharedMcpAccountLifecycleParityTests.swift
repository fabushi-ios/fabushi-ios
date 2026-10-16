import XCTest
@testable import Fabushi

private final class TestMcpAccountBackend: McpAccountBackend {
    var logouts: [(String,String)] = []
    var renames: [(Int32,String,String)] = []
    var deletes: [(Int32,String)] = []

    func logoutAccount(serverUrl: String, accountKey: String) async throws {
        logouts.append((serverUrl,accountKey))
    }
    func renameAccount(serverId: Int32, accountKey: String, newAccountKey: String) async throws {
        renames.append((serverId,accountKey,newAccountKey))
    }
    func deleteAccount(serverId: Int32, accountKey: String) async throws {
        deletes.append((serverId,accountKey))
    }
}

final class SharedMcpAccountLifecycleParityTests: XCTestCase {
    func testAuthWatchKeyCatalogAuthorizationAndLegacyCleanup() async throws {
        XCTAssertEqual(authWatchKey("1","work"),"1::work")
        XCTAssertNotNil(validateAuthorizationUrl("https://auth.example.com/start"))
        XCTAssertNil(validateAuthorizationUrl("http://auth.example.com/start"))
        XCTAssertNotNil(validateAuthorizationUrl(
            "http://localhost:9000/auth",
            serverUrl:"http://127.0.0.1:3000/mcp"
        ))
        XCTAssertNil(validateAuthorizationUrl(
            "http://localhost:9000/auth",
            serverUrl:"https://remote.example/mcp"
        ))

        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at:root,withIntermediateDirectories:true)
        try Data("x".utf8).write(to:root.appendingPathComponent("mcp-auth.json"))
        try Data("y".utf8).write(to:root.appendingPathComponent("mcp-auth.json.old"))
        try Data("z".utf8).write(to:root.appendingPathComponent("keep.json"))
        let result = await cleanupLegacyMcpAuthCredentials(root.path)
        XCTAssertEqual(result,.init(outcome:"deleted",removedCount:2))
        XCTAssertTrue(FileManager.default.fileExists(atPath:root.appendingPathComponent("keep.json").path))
        try? FileManager.default.removeItem(at:root)
    }

    func testAccountRenamePatchesDisplayAndSummaryWithoutReload() async throws {
        let backend = TestMcpAccountBackend()
        let server = DisplayServer(
            id:"12",
            name:"GitHub",
            serverIdentifier:"row-12",
            config:.http(url:"https://mcp.example"),
            isTeamServer:false,
            accounts:[
                .init(accountKey:"work",hasToken:true,serverIdentifier:"slot-work"),
                .init(accountKey:"personal",hasToken:true,serverIdentifier:"slot-personal"),
            ]
        )
        var display: AccountDisplayConfig? = .init(servers:[server])
        let summary = McpServerSummary(
            id:"12",name:"GitHub",serverIdentifier:"slot-work",accountKey:"work",
            rowServerIdentifier:"row-12",transport:.http,toolCount:1,
            customInstructions:"",isTeamServer:false,attribution:.init(),status:"connected"
        )
        var state: McpListedState? = .init(servers:[summary])
        var reloads = 0
        var invalidations = 0
        var resets = 0
        var adopted: McpRuntimeConfig?
        let lifecycle = SandMcpAccountSlotLifecycle(
            backend:backend,
            resolveDisplayServer:{ _ in server },
            reloadServers:{ reloads += 1; return state! },
            clearPendingAuthWatch:{ _,_ in .init(serverId:"12",accountKey:"work") },
            notifyWatchCancelled:{ _ in },
            getDisplay:{display},
            setDisplay:{display=$0},
            getListedState:{state},
            setListedState:{state=$0},
            adoptAccountConfig:{adopted=$0},
            invalidateToolsCache:{invalidations += 1},
            resetPushState:{resets += 1}
        )
        let result = try await lifecycle.renameAccount("12",key:"work",next:"office")
        XCTAssertEqual(backend.renames.first?.1,"work")
        XCTAssertEqual(display?.servers[0].accounts[0].accountKey,"office")
        XCTAssertEqual(result.servers[0].accountKey,"office")
        XCTAssertEqual(result.servers[0].serverIdentifier,"row-12--office")
        XCTAssertEqual(adopted?.mcpServers["row-12"]?.url,"https://mcp.example")
        XCTAssertEqual(invalidations,1)
        XCTAssertEqual(resets,1)
        XCTAssertEqual(reloads,0)
    }

    func testRemovingLastAccountFallsBackToReload() async throws {
        let backend = TestMcpAccountBackend()
        let server = DisplayServer(
            id:"7",name:"Only",serverIdentifier:"row-7",
            config:.http(url:"https://only.example"),isTeamServer:false,
            accounts:[.init(accountKey:"default",hasToken:true)]
        )
        var display: AccountDisplayConfig? = .init(servers:[server])
        var state: McpListedState? = .init(servers:[])
        var reloads = 0
        let lifecycle = SandMcpAccountSlotLifecycle(
            backend:backend,
            resolveDisplayServer:{ _ in server },
            reloadServers:{ reloads += 1; return .init(servers:[]) },
            clearPendingAuthWatch:{ _,_ in nil },
            notifyWatchCancelled:{ _ in },
            getDisplay:{display},
            setDisplay:{display=$0},
            getListedState:{state},
            setListedState:{state=$0},
            adoptAccountConfig:{ _ in },
            invalidateToolsCache:{},
            resetPushState:{}
        )
        _ = try await lifecycle.removeAccount("7",key:"default")
        XCTAssertEqual(backend.deletes.first?.0,7)
        XCTAssertEqual(reloads,1)
    }

    func testStdioAccountMutationIsRefused() async {
        let backend = TestMcpAccountBackend()
        let server = DisplayServer(
            id:"3",name:"Local",config:.stdio(command:"tool"),isTeamServer:false
        )
        let lifecycle = SandMcpAccountSlotLifecycle(
            backend:backend,
            resolveDisplayServer:{ _ in server },
            reloadServers:{.init(servers:[])},
            clearPendingAuthWatch:{_,_ in nil},
            notifyWatchCancelled:{_ in},
            getDisplay:{nil},
            setDisplay:{_ in},
            getListedState:{nil},
            setListedState:{_ in},
            adoptAccountConfig:{_ in},
            invalidateToolsCache:{},
            resetPushState:{}
        )
        do {
            _ = try await lifecycle.logoutAccount("3",key:"default")
            XCTFail("stdio must not enter OAuth account lifecycle")
        } catch let error as SandMcpConfigError {
            XCTAssertTrue(error.message.contains("no OAuth accounts"))
        } catch {
            XCTFail("unexpected error \(error)")
        }
    }
}
