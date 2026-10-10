import Foundation

let LIST_TOOLS_TIMEOUT_MS = 60_000
let CONTROL_RPC_TIMEOUT_MS = 30_000
let MCP_SDK_REQUEST_TIMEOUT_MS = 60_000
let EXECUTE_TOOL_DIAL_DISCOVER_CALL_TIMEOUT_MS = 3 * MCP_SDK_REQUEST_TIMEOUT_MS

struct SandBackendMcpExecError: Error, LocalizedError, Equatable, Sendable {
    let message: String
    var errorDescription: String? { message }
}

indirect enum McpJSONValue: Equatable, Sendable {
    case null
    case bool(Bool)
    case number(Double)
    case string(String)
    case array([McpJSONValue])
    case object([String: McpJSONValue])

    static func from(_ value: Any?) -> McpJSONValue {
        guard let value else { return .null }
        switch value {
        case is NSNull:
            return .null
        case let value as Bool:
            return .bool(value)
        case let value as NSNumber:
            return .number(value.doubleValue)
        case let value as String:
            return .string(value)
        case let value as [Any]:
            return .array(value.map(McpJSONValue.from))
        case let value as [String: Any]:
            return .object(value.mapValues(McpJSONValue.from))
        default:
            return .string(String(describing: value))
        }
    }
}

struct BackendToolWire: Equatable, Sendable {
    let name: String
    let providerIdentifier: String
    let toolName: String
    let description: String
    let inputSchema: McpJSONValue?
}

struct NamedBackendTool: Equatable, Sendable {
    let name: String
    let providerIdentifier: String
    let toolName: String
    let clientKey: String
    let description: String?
    let inputSchema: McpJSONValue?
}

struct McpExecResult: Equatable, Sendable {
    let caseName: String
    let value: McpJSONValue
}

struct BackendMcpToolServerWire: Equatable, Sendable {
    let serverIdentifier: String
    let status: String
    let tools: [BackendToolWire]
    let accountLabel: String?
    let rowServerIdentifier: String?
}

struct BackendMcpToolServer: Equatable, Sendable {
    let serverIdentifier: String
    let status: String
    let tools: [NamedBackendTool]
    let accountLabel: String
    let rowServerIdentifier: String
}

struct BackendMcpAuthStatusWire: Equatable, Sendable {
    let id: String
    let isAvailable: Bool
    let requiresAuth: Bool
    let hasValidToken: Bool
    let authUrl: String
    let error: String
}

struct BackendMcpAuthStatus: Equatable, Sendable {
    let isAvailable: Bool
    let requiresAuth: Bool
    let hasValidToken: Bool
    let authUrl: String
    let error: String
}

struct BackendMcpTokenTarget: Equatable, Sendable {
    let serverUrl: String
    let accountKey: String
}

struct BackendMcpTokenValidation: Equatable, Sendable {
    let serverUrl: String
    let accountKey: String?
    let hasValidToken: Bool
}

struct ConnectErrorInfo: Equatable, Sendable {
    let name: String
    let deadlineExceeded: Bool
}

protocol DashboardMcpExecClient: Sendable {
    func listSandMcpTools(serverIdentifiers: [String], timeoutMs: Int) async throws -> [BackendMcpToolServerWire]
    func executeSandMcpTool(
        serverIdentifier: String,
        toolName: String,
        args: McpJSONValue,
        toolCallId: String,
        agentId: String,
        timeoutMs: Int
    ) async throws -> McpExecResult?
    func checkHttpMcpStatus(
        serverIds: [String],
        oauthRedirectUri: String,
        forceReauth: Bool,
        accountKey: String,
        timeoutMs: Int
    ) async throws -> [BackendMcpAuthStatusWire]
    func completeMcpOAuth(stateId: String, authorizationCode: String, timeoutMs: Int) async throws
    func validateMcpOAuthTokens(
        targets: [BackendMcpTokenTarget],
        timeoutMs: Int
    ) async throws -> [BackendMcpTokenValidation]
    func deleteMcpOAuthToken(serverUrl: String, accountKey: String, source: String, timeoutMs: Int) async throws
    func renameMcpOAuthAccount(serverId: String, accountKey: String, newAccountKey: String, timeoutMs: Int) async throws
    func deleteMcpOAuthAccount(serverId: String, accountKey: String, timeoutMs: Int) async throws
}

struct DashboardMcpExecDependencies: Sendable {
    let client: any DashboardMcpExecClient
    let reportFailure: (@Sendable (String, Error) -> Void)?
    let recordExecError: (@Sendable (String, Error) -> Void)?
    let connectErrorCode: (@Sendable (Error) -> ConnectErrorInfo)?

    init(
        client: any DashboardMcpExecClient,
        reportFailure: (@Sendable (String, Error) -> Void)? = nil,
        recordExecError: (@Sendable (String, Error) -> Void)? = nil,
        connectErrorCode: (@Sendable (Error) -> ConnectErrorInfo)? = nil
    ) {
        self.client = client
        self.reportFailure = reportFailure
        self.recordExecError = recordExecError
        self.connectErrorCode = connectErrorCode
    }
}

func backendToolToNamed(_ tool: BackendToolWire) -> NamedBackendTool {
    .init(
        name: tool.name,
        providerIdentifier: tool.providerIdentifier,
        toolName: tool.toolName,
        clientKey: tool.providerIdentifier,
        description: tool.description.isEmpty ? nil : tool.description,
        inputSchema: tool.inputSchema
    )
}

func errorResult(_ message: String) -> McpExecResult {
    .init(caseName: "error", value: .object(["error": .string(message)]))
}

func normalizeAccountLabel(_ label: String?) -> String {
    guard let label, !label.isEmpty else { return "default" }
    return label
}

struct DashboardSandBackendMcpExec: Sendable {
    private let deps: DashboardMcpExecDependencies

    init(deps: DashboardMcpExecDependencies) {
        self.deps = deps
    }

    func listTools(serverIdentifiers: [String]) async throws -> [BackendMcpToolServer] {
        do {
            return try await deps.client.listSandMcpTools(
                serverIdentifiers: serverIdentifiers,
                timeoutMs: LIST_TOOLS_TIMEOUT_MS
            ).map { server in
                .init(
                    serverIdentifier: server.serverIdentifier,
                    status: server.status,
                    tools: server.tools.map(backendToolToNamed),
                    accountLabel: normalizeAccountLabel(server.accountLabel),
                    rowServerIdentifier: {
                        guard let row = server.rowServerIdentifier, !row.isEmpty else {
                            return server.serverIdentifier
                        }
                        return row
                    }()
                )
            }
        } catch {
            deps.reportFailure?("backend-list-tools", error)
            throw SandBackendMcpExecError(
                message: "Backend MCP tool discovery failed: \(errorLabel(error))"
            )
        }
    }

    func executeTool(
        serverIdentifier: String,
        toolName: String,
        args: McpJSONValue,
        toolCallId: String,
        agentId: String? = nil
    ) async -> McpExecResult {
        do {
            return try await deps.client.executeSandMcpTool(
                serverIdentifier: serverIdentifier,
                toolName: toolName,
                args: args,
                toolCallId: toolCallId,
                agentId: agentId ?? "",
                timeoutMs: EXECUTE_TOOL_DIAL_DISCOVER_CALL_TIMEOUT_MS
            ) ?? errorResult("Backend MCP execution returned no result for \"\(toolName)\".")
        } catch {
            deps.recordExecError?(toolCallId, error)
            if deps.connectErrorCode?(error).deadlineExceeded == true {
                return errorResult(
                    "Backend MCP execution for \"\(toolName)\" timed out after \(EXECUTE_TOOL_DIAL_DISCOVER_CALL_TIMEOUT_MS / 1_000)s. The connector may still have applied it, so retry only if repeating the call is safe."
                )
            }
            return errorResult("Backend MCP execution failed for \"\(toolName)\": \(errorLabel(error))")
        }
    }

    func checkAuthStatus(
        serverId: String,
        oauthRedirectUri: String,
        forceReauth: Bool = false,
        accountKey: String
    ) async throws -> BackendMcpAuthStatus {
        do {
            let statuses = try await deps.client.checkHttpMcpStatus(
                serverIds: [serverId],
                oauthRedirectUri: oauthRedirectUri,
                forceReauth: forceReauth,
                accountKey: accountKey,
                timeoutMs: CONTROL_RPC_TIMEOUT_MS
            )
            guard let status = statuses.first(where: { $0.id == serverId }) else {
                throw SandBackendMcpExecError(
                    message: "The backend did not report OAuth status for this connector."
                )
            }
            return .init(
                isAvailable: status.isAvailable,
                requiresAuth: status.requiresAuth,
                hasValidToken: status.hasValidToken,
                authUrl: status.authUrl,
                error: status.error
            )
        } catch {
            deps.reportFailure?("backend-check-auth-status", error)
            if let typed = error as? SandBackendMcpExecError { throw typed }
            throw SandBackendMcpExecError(
                message: "Backend MCP OAuth status check failed: \(errorLabel(error))"
            )
        }
    }

    func completeOAuth(stateId: String, code: String) async throws {
        try await deps.client.completeMcpOAuth(
            stateId: stateId,
            authorizationCode: code,
            timeoutMs: CONTROL_RPC_TIMEOUT_MS
        )
    }

    func validateTokens(_ targets: [BackendMcpTokenTarget]) async -> [BackendMcpTokenValidation] {
        let cleaned = targets.compactMap { target -> BackendMcpTokenTarget? in
            let url = target.serverUrl.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !url.isEmpty else { return nil }
            return .init(serverUrl: url, accountKey: target.accountKey)
        }
        guard !cleaned.isEmpty else { return [] }
        do {
            return try await deps.client.validateMcpOAuthTokens(
                targets: cleaned,
                timeoutMs: CONTROL_RPC_TIMEOUT_MS
            ).map {
                .init(
                    serverUrl: $0.serverUrl,
                    accountKey: normalizeAccountLabel($0.accountKey),
                    hasValidToken: $0.hasValidToken
                )
            }
        } catch {
            deps.reportFailure?("backend-validate-tokens", error)
            return []
        }
    }

    func logoutAccount(serverUrl: String, accountKey: String) async throws {
        try await deps.client.deleteMcpOAuthToken(
            serverUrl: serverUrl,
            accountKey: accountKey,
            source: "sand",
            timeoutMs: CONTROL_RPC_TIMEOUT_MS
        )
    }

    func renameAccount(serverId: String, accountKey: String, newAccountKey: String) async throws {
        try await deps.client.renameMcpOAuthAccount(
            serverId: serverId,
            accountKey: accountKey,
            newAccountKey: newAccountKey,
            timeoutMs: CONTROL_RPC_TIMEOUT_MS
        )
    }

    func deleteAccount(serverId: String, accountKey: String) async throws {
        try await deps.client.deleteMcpOAuthAccount(
            serverId: serverId,
            accountKey: accountKey,
            timeoutMs: CONTROL_RPC_TIMEOUT_MS
        )
    }

    private func errorLabel(_ error: Error) -> String {
        if let code = deps.connectErrorCode?(error) { return code.name }
        let name = String(describing: type(of: error))
        return name.isEmpty ? "Error" : name
    }
}

func createDashboardSandBackendMcpExec(
    _ deps: DashboardMcpExecDependencies
) -> DashboardSandBackendMcpExec {
    .init(deps: deps)
}
