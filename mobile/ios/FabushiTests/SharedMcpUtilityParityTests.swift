import XCTest
@testable import Fabushi

private struct McpTestConnectError: ConnectErrorLike {
    let connectCode: Int
    let connectMetadata: [String : String] = [:]
}

private struct McpTestJson: McpJsonConvertible {
    let value: String
    func toJson() -> Any { ["value": value] }
}

private struct McpTestServer: Equatable {
    let id: String
    let value: String
}

final class SharedMcpUtilityParityTests: XCTestCase {
    func testServerIdAndNameValidation() throws {
        XCTAssertTrue(isMcpServerId(" 42 "))
        XCTAssertFalse(isMcpServerId("0"))
        XCTAssertFalse(isMcpServerId("-1"))
        XCTAssertEqual(try parseInt32McpServerId("2147483647"), Int32.max)
        XCTAssertThrowsError(try parseInt32McpServerId("2147483648"))

        XCTAssertEqual(try validateServerName(" github "), "github")
        XCTAssertThrowsError(try validateServerName("__proto__"))
        XCTAssertThrowsError(try validateServerName("a/b"))
        XCTAssertThrowsError(try validateServerName("a--b"))
    }

    func testTransportCommandJsonParseAndProjection() throws {
        let stdio = McpServerConfig.stdio(command: "node", args: ["server.js","--x"])
        XCTAssertEqual(getTransport(stdio), .stdio)
        XCTAssertEqual(getCommand(stdio), "node server.js --x")
        XCTAssertEqual(getTransport(.sse(url: "https://example.com/sse")), .sse)
        XCTAssertNil(getCommand(.http(url: "https://example.com/mcp")))

        let parsed = try parseServerConfig(#"{"url":"https://example.com"}"#) { value in
            guard let object = value as? [String: Any], let url = object["url"] as? String else {
                throw SandMcpConfigError("bad")
            }
            return .http(url: url)
        }
        XCTAssertEqual(parsed, .http(url: "https://example.com"))

        let projected = toJsonArgs(["plain":"x","wrapped":McpTestJson(value:"y")])
        XCTAssertEqual(projected["plain"] as? String, "x")
        XCTAssertEqual((projected["wrapped"] as? [String:String])?["value"], "y")
    }

    func testPluginVariableFieldsInferSecretsDefaultsAndRequirements() {
        let fields = pluginVariablesSchemaToFields([
            "required":["API_TOKEN","BASE_URL"],
            "properties":[
                "API_TOKEN":["description":"secret"],
                "BASE_URL":["default":"https://example.com"],
                "user_id":["title":"User","writeOnly":true],
            ],
        ])
        XCTAssertEqual(fields.first(where: {$0.key=="API_TOKEN"})?.isSecret, true)
        XCTAssertEqual(fields.first(where: {$0.key=="BASE_URL"})?.label, "Base URL")
        XCTAssertEqual(fields.first(where: {$0.key=="user_id"})?.isSecret, true)
        let missing = findMissingRequiredCatalogFields(fields, values: ["API_TOKEN":"   "])
        XCTAssertEqual(missing.map(\.key), ["API_TOKEN"])
    }

    func testDiagnosticsBufferConnectClassificationAndTakeOnce() {
        pinMcpDiagnosticsReporter(nil)
        reportMcpHostEdgeDegraded("catalog", errorClass: "degraded")
        var received: [McpDiagnostic] = []
        pinMcpDiagnosticsReporter { received.append($0) }
        XCTAssertTrue(received.contains(.init(leg:"catalog", errorClass:"degraded")))

        let error = McpTestConnectError(connectCode: CONNECT_CODE_UNAVAILABLE)
        XCTAssertEqual(mcpErrorClassOf(error), "ConnectError.Unavailable")
        recordMcpExecErrorClass("tool-1", error: error)
        XCTAssertEqual(takeMcpExecErrorClass("tool-1"), "ConnectError.Unavailable")
        XCTAssertEqual(takeMcpExecErrorClass("tool-1"), MCP_ERROR_RESULT_CLASS)
        pinMcpDiagnosticsReporter(nil)
    }

    func testUnresolvedAccountMergeKeepsCachedRowsAndFreshReplacementOrder() {
        let cached = AccountDisplay(servers:[
            McpTestServer(id:"a",value:"old-a"),
            McpTestServer(id:"b",value:"old-b"),
        ])
        let fresh = AccountDisplay(
            servers:[
                McpTestServer(id:"a",value:"new-a"),
                McpTestServer(id:"c",value:"new-c"),
            ],
            unresolvedServerIds:["b"]
        )
        let merged = mergeUnresolvedAccountServers(display:fresh,cached:cached,id:{$0.id})
        XCTAssertEqual(merged.servers,[
            .init(id:"a",value:"new-a"),
            .init(id:"b",value:"old-b"),
            .init(id:"c",value:"new-c"),
        ])
    }
}
