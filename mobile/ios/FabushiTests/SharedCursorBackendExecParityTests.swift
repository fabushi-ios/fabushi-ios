import XCTest
@testable import Fabushi

private enum BackendMcpFakeError: Error {
    case deadline
}

private actor BackendMcpExecFakeClient: DashboardMcpExecClient {
    var executeError: Error?
    var executeResult: McpExecResult? = .init(caseName: "success", value: .object(["ok": .bool(true)]))

    func listSandMcpTools(serverIdentifiers: [String], timeoutMs: Int) async throws -> [BackendMcpToolServerWire] {
        XCTAssertEqual(timeoutMs, LIST_TOOLS_TIMEOUT_MS)
        return [.init(
            serverIdentifier: serverIdentifiers.first ?? "server",
            status: "ready",
            tools: [.init(
                name: "Search",
                providerIdentifier: "provider",
                toolName: "search",
                description: "",
                inputSchema: .object(["type": .string("object")])
            )],
            accountLabel: nil,
            rowServerIdentifier: ""
        )]
    }

    func executeSandMcpTool(
        serverIdentifier: String,
        toolName: String,
        args: McpJSONValue,
        toolCallId: String,
        agentId: String,
        timeoutMs: Int
    ) async throws -> McpExecResult? {
        XCTAssertEqual(timeoutMs, EXECUTE_TOOL_DIAL_DISCOVER_CALL_TIMEOUT_MS)
        if let executeError { throw executeError }
        return executeResult
    }

    func checkHttpMcpStatus(
        serverIds: [String],
        oauthRedirectUri: String,
        forceReauth: Bool,
        accountKey: String,
        timeoutMs: Int
    ) async throws -> [BackendMcpAuthStatusWire] {
        [.init(
            id: serverIds[0],
            isAvailable: true,
            requiresAuth: true,
            hasValidToken: false,
            authUrl: "https://example.test/auth",
            error: ""
        )]
    }

    func completeMcpOAuth(stateId: String, authorizationCode: String, timeoutMs: Int) async throws {}

    func validateMcpOAuthTokens(
        targets: [BackendMcpTokenTarget],
        timeoutMs: Int
    ) async throws -> [BackendMcpTokenValidation] {
        targets.map { .init(serverUrl: $0.serverUrl, accountKey: nil, hasValidToken: true) }
    }

    func deleteMcpOAuthToken(serverUrl: String, accountKey: String, source: String, timeoutMs: Int) async throws {}
    func renameMcpOAuthAccount(serverId: String, accountKey: String, newAccountKey: String, timeoutMs: Int) async throws {}
    func deleteMcpOAuthAccount(serverId: String, accountKey: String, timeoutMs: Int) async throws {}

    func failExecution(_ error: Error?) {
        executeError = error
    }
}

private actor GenerateImageFakeClient: CursorGenerateImageClient {
    var result: GenerateImageWireResult

    init(result: GenerateImageWireResult) {
        self.result = result
    }

    func runGenerateImage(_ request: RunGenerateImageRequest) async throws -> GenerateImageWireResult {
        XCTAssertEqual(request.modelId, "image-model")
        return result
    }
}

final class SharedCursorBackendExecParityTests: XCTestCase {
    func testBackendToolMappingAndDiscoveryFallbacks() async throws {
        let client = BackendMcpExecFakeClient()
        let exec = createDashboardSandBackendMcpExec(.init(client: client))
        let servers = try await exec.listTools(serverIdentifiers: ["server-1"])

        XCTAssertEqual(servers.count, 1)
        XCTAssertEqual(servers[0].accountLabel, "default")
        XCTAssertEqual(servers[0].rowServerIdentifier, "server-1")
        XCTAssertEqual(servers[0].tools[0].clientKey, "provider")
        XCTAssertNil(servers[0].tools[0].description)
    }

    func testBackendExecutionDeadlineReturnsAmbiguousSafetyMessage() async {
        let client = BackendMcpExecFakeClient()
        await client.failExecution(BackendMcpFakeError.deadline)
        let exec = createDashboardSandBackendMcpExec(.init(
            client: client,
            connectErrorCode: { _ in .init(name: "DeadlineExceeded", deadlineExceeded: true) }
        ))

        let result = await exec.executeTool(
            serverIdentifier: "server",
            toolName: "dangerous-write",
            args: .object([:]),
            toolCallId: "call-1"
        )
        guard case .object(let object) = result.value,
              case .string(let message)? = object["error"] else {
            return XCTFail("expected error result")
        }
        XCTAssertTrue(message.contains("may still have applied it"))
        XCTAssertTrue(message.contains("retry only if"))
    }

    func testAuthAndTokenNormalization() async throws {
        let client = BackendMcpExecFakeClient()
        let exec = createDashboardSandBackendMcpExec(.init(client: client))
        let auth = try await exec.checkAuthStatus(
            serverId: "server",
            oauthRedirectUri: "fabushi://oauth",
            accountKey: "work"
        )
        XCTAssertTrue(auth.requiresAuth)

        let tokens = await exec.validateTokens([
            .init(serverUrl: "  https://mcp.example.test  ", accountKey: "")
        ])
        XCTAssertEqual(tokens, [
            .init(serverUrl: "https://mcp.example.test", accountKey: "default", hasValidToken: true)
        ])
    }

    func testGenerateImageSuccessAndRestrictedErrors() async throws {
        let successClient = GenerateImageFakeClient(result: .success(.init(
            imageData: "base64",
            mimeType: "image/png"
        )))
        let service = createCursorGenerateImageService(
            client: successClient,
            modelId: "image-model",
            maxMode: true
        )
        let output = try await service.generate(description: "lotus")
        XCTAssertEqual(output.mimeType, "image/png")

        let restrictedClient = GenerateImageFakeClient(result: .error(
            message: "restricted",
            modelRestricted: true
        ))
        let restricted = createCursorGenerateImageService(
            client: restrictedClient,
            modelId: "image-model"
        )
        do {
            _ = try await restricted.generate(description: "lotus")
            XCTFail("expected restricted error")
        } catch let error as SandGenerateImageModelRestrictedError {
            XCTAssertEqual(error.message, "restricted")
        } catch {
            XCTFail("unexpected error \(error)")
        }
    }

    func testGenerateImageNoResultFailsExplicitly() async {
        let client = GenerateImageFakeClient(result: .none)
        let service = createCursorGenerateImageService(client: client, modelId: "image-model")
        do {
            _ = try await service.generate(description: "lotus")
            XCTFail("expected no-result error")
        } catch let error as SandGenerateImageError {
            XCTAssertEqual(error.message, "Image generation returned no result.")
        } catch {
            XCTFail("unexpected error \(error)")
        }
    }
}
