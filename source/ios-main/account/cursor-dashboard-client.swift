import Foundation

struct IOSCursorDashboardError: Error, LocalizedError, Sendable {
    let message: String
    var errorDescription: String? { message }
}

final class IOSCursorDashboardClient: @unchecked Sendable, AccountMcpClient, DashboardMcpExecClient {
    private let credentials: AccountMcpCredentials
    private let backendURL: URL
    private let session: URLSession

    init(
        credentials: AccountMcpCredentials,
        backendURL: URL = URL(string: getConfiguredBackendUrl())!,
        session: URLSession = .shared
    ) {
        self.credentials = credentials
        self.backendURL = backendURL
        self.session = session
    }

    private func rpc(
        _ method: String,
        body: [String: Any],
        timeoutMs: Int
    ) async throws -> [String: Any] {
        let backend = backendURL.absoluteString
        let headers = try await createSandInferenceHeaders(
            backendUrl: backend,
            getAccessToken: { value in
                try await self.credentials.getAccessToken(value)
            },
            getMachineId: credentials.getMachineId,
            resolveGhostMode: { _ in "true" }
        )
        let url = backendURL
            .appendingPathComponent("aiserver.v1.DashboardService")
            .appendingPathComponent(method)
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.timeoutInterval = TimeInterval(max(1, timeoutMs)) / 1_000
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.setValue("1", forHTTPHeaderField: "Connect-Protocol-Version")
        for (name, value) in headers.headers {
            request.setValue(value, forHTTPHeaderField: name)
        }
        request.httpBody = try JSONSerialization.data(withJSONObject: body)
        let (data, response) = try await session.data(for: request)
        guard let http = response as? HTTPURLResponse else {
            throw IOSCursorDashboardError(message: "Dashboard RPC returned no HTTP response.")
        }
        guard (200..<300).contains(http.statusCode) else {
            let errorBody = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
            let detail = errorBody?["message"] as? String
            throw IOSCursorDashboardError(
                message: detail?.isEmpty == false
                    ? detail!
                    : "Dashboard RPC \(method) failed with HTTP \(http.statusCode)."
            )
        }
        if data.isEmpty { return [:] }
        guard let object = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            throw IOSCursorDashboardError(message: "Dashboard RPC \(method) returned invalid JSON.")
        }
        return object
    }

    private func uint64(_ value: Any?) -> UInt64? {
        if let value = value as? String { return UInt64(value) }
        if let value = value as? NSNumber { return value.uint64Value }
        return nil
    }

    private func stringArray(_ value: Any?) -> [String] {
        (value as? [Any] ?? []).compactMap { $0 as? String }
    }

    private func jsonValue(_ value: McpJSONValue) -> Any {
        switch value {
        case .null: NSNull()
        case .bool(let value): value
        case .number(let value): value
        case .string(let value): value
        case .array(let values): values.map(jsonValue)
        case .object(let object): object.mapValues(jsonValue)
        }
    }

    func getAvailableMcpServers(timeoutMs: Int) async throws -> [AvailableMcpServer] {
        let response = try await rpc("GetAvailableMcpServers", body: [:], timeoutMs: timeoutMs)
        return (response["servers"] as? [[String: Any]] ?? []).compactMap { row in
            guard let id = uint64(row["id"]), id != 0,
                  let name = row["name"] as? String,
                  let identifier = row["serverIdentifier"] as? String,
                  let type = row["type"] as? String
            else { return nil }
            let accounts = (row["accounts"] as? [[String: Any]] ?? []).compactMap { account -> AvailableMcpAccount? in
                guard let key = account["accountKey"] as? String else { return nil }
                return .init(
                    accountKey: key,
                    serverIdentifier: account["serverIdentifier"] as? String ?? "",
                    userHasAccessToken: account["userHasAccessToken"] as? Bool ?? false
                )
            }
            return .init(
                id: id,
                name: name,
                serverIdentifier: identifier,
                type: type,
                url: row["url"] as? String,
                command: row["command"] as? String,
                args: stringArray(row["args"]),
                enabled: row["enabled"] as? Bool ?? false,
                isTeamServer: row["isTeamServer"] as? Bool ?? false,
                owningTeamId: uint64(row["owningTeamId"]),
                disabledByTeamAdminPolicy: row["disabledByTeamAdminPolicy"] as? Bool ?? false,
                pluginId: uint64(row["pluginId"]),
                isRequired: row["isRequired"] as? Bool ?? false,
                managedByTeamPluginPolicy: row["managedByTeamPluginPolicy"] as? Bool ?? false,
                accounts: accounts.isEmpty ? nil : accounts
            )
        }
    }

    func getMcpConfig(
        teamScope: Bool,
        redactSecrets: Bool,
        teamId: UInt64?,
        timeoutMs: Int?
    ) async throws -> AccountMcpConfigResponse {
        var body: [String: Any] = [
            "teamScope": teamScope,
            "redactSecrets": redactSecrets,
        ]
        if let teamId { body["teamId"] = String(teamId) }
        let response = try await rpc(
            "GetMcpConfig",
            body: body,
            timeoutMs: timeoutMs ?? ACCOUNT_MCP_RPC_TIMEOUT_MS
        )
        let metadata = (response["serverMetadataByName"] as? [String: Any] ?? [:]).reduce(
            into: [String: AccountMcpServerMetadata]()
        ) { result, pair in
            guard let object = pair.value as? [String: Any] else { return }
            result[pair.key] = .init(serverId: uint64(object["serverId"]))
        }
        return .init(
            configJson: response["configJson"] as? String ?? #"{"mcpServers":{}}"#,
            serverMetadataByName: metadata
        )
    }

    func getEffectiveUserPlugins(excludeConfiguredVariables: Bool) async throws -> [EffectivePluginWire] {
        let response = try await rpc(
            "GetEffectiveUserPlugins",
            body: ["excludeConfiguredVariables": excludeConfiguredVariables],
            timeoutMs: ACCOUNT_MCP_RPC_TIMEOUT_MS
        )
        return (response["plugins"] as? [[String: Any]] ?? []).map { row in
            let pluginObject = row["plugin"] as? [String: Any]
            let plugin: EffectivePluginWire.Plugin?
            if let pluginObject,
               let id = uint64(pluginObject["id"]), id != 0,
               let name = pluginObject["name"] as? String {
                plugin = .init(
                    id: id,
                    name: name,
                    displayName: pluginObject["displayName"] as? String ?? ""
                )
            } else {
                plugin = nil
            }
            return .init(
                plugin: plugin,
                installMode: (row["installMode"] as? NSNumber)?.intValue ?? 0,
                isTeamRequired: row["isTeamRequired"] as? Bool ?? false,
                isEnabled: row["isEnabled"] as? Bool ?? false,
                hasTeamConfiguredVariables: row["hasTeamConfiguredVariables"] as? Bool ?? false
            )
        }
    }

    func setMcpConfig(configJson: String, serverIdsByName: [String: UInt64]) async throws {
        _ = try await rpc(
            "SetMcpConfig",
            body: [
                "teamScope": false,
                "configJson": configJson,
                "serverIdsByName": serverIdsByName.mapValues { String($0) },
            ],
            timeoutMs: ACCOUNT_MCP_RPC_TIMEOUT_MS
        )
    }

    func installUserPlugin(pluginId: UInt64, variables: [String: String]?) async throws {
        var body: [String: Any] = ["pluginId": String(pluginId)]
        if let variables, !variables.isEmpty { body["variables"] = variables }
        _ = try await rpc("InstallUserPlugin", body: body, timeoutMs: ACCOUNT_MCP_RPC_TIMEOUT_MS)
    }

    func uninstallUserPlugin(pluginId: UInt64) async throws {
        _ = try await rpc(
            "UninstallUserPlugin",
            body: ["pluginId": String(pluginId)],
            timeoutMs: ACCOUNT_MCP_RPC_TIMEOUT_MS
        )
    }

    func updateUserPluginInstall(pluginId: UInt64, variables: [String: String]) async throws {
        _ = try await rpc(
            "UpdateUserPluginInstall",
            body: ["pluginId": String(pluginId), "variables": variables],
            timeoutMs: ACCOUNT_MCP_RPC_TIMEOUT_MS
        )
    }

    func listSandMcpTools(
        serverIdentifiers: [String],
        timeoutMs: Int
    ) async throws -> [BackendMcpToolServerWire] {
        let response = try await rpc(
            "ListSandMcpTools",
            body: ["serverIdentifiers": serverIdentifiers],
            timeoutMs: timeoutMs
        )
        return (response["servers"] as? [[String: Any]] ?? []).compactMap { row in
            guard let identifier = row["serverIdentifier"] as? String else { return nil }
            let status: String
            if let string = row["status"] as? String {
                status = string
            } else if let number = row["status"] as? NSNumber {
                status = number.stringValue
            } else {
                status = ""
            }
            let tools = (row["tools"] as? [[String: Any]] ?? []).compactMap { tool -> BackendToolWire? in
                guard let name = tool["name"] as? String,
                      let provider = tool["providerIdentifier"] as? String,
                      let toolName = tool["toolName"] as? String
                else { return nil }
                return .init(
                    name: name,
                    providerIdentifier: provider,
                    toolName: toolName,
                    description: tool["description"] as? String ?? "",
                    inputSchema: tool["inputSchema"].map(McpJSONValue.from)
                )
            }
            return .init(
                serverIdentifier: identifier,
                status: status,
                tools: tools,
                accountLabel: row["accountLabel"] as? String,
                rowServerIdentifier: row["rowServerIdentifier"] as? String
            )
        }
    }

    func executeSandMcpTool(
        serverIdentifier: String,
        toolName: String,
        args: McpJSONValue,
        toolCallId: String,
        agentId: String,
        timeoutMs: Int
    ) async throws -> McpExecResult? {
        let response = try await rpc(
            "ExecuteSandMcpTool",
            body: [
                "serverIdentifier": serverIdentifier,
                "toolName": toolName,
                "args": jsonValue(args),
                "toolCallId": toolCallId,
                "agentId": agentId,
            ],
            timeoutMs: timeoutMs
        )
        guard let result = response["result"] as? [String: Any],
              let pair = result.first else { return nil }
        return .init(caseName: pair.key, value: McpJSONValue.from(pair.value))
    }

    func checkHttpMcpStatus(
        serverIds: [String],
        oauthRedirectUri: String,
        forceReauth: Bool,
        accountKey: String,
        timeoutMs: Int
    ) async throws -> [BackendMcpAuthStatusWire] {
        let response = try await rpc(
            "CheckHttpMcpStatus",
            body: [
                "serverIds": serverIds,
                "oauthRedirectUri": oauthRedirectUri,
                "forceReauth": forceReauth,
                "accountKey": accountKey,
            ],
            timeoutMs: timeoutMs
        )
        return (response["statuses"] as? [[String: Any]] ?? []).compactMap { row in
            guard let id = row["id"] as? String else { return nil }
            return .init(
                id: id,
                isAvailable: row["isAvailable"] as? Bool ?? false,
                requiresAuth: row["requiresAuth"] as? Bool ?? false,
                hasValidToken: row["hasValidToken"] as? Bool ?? false,
                authUrl: row["authUrl"] as? String ?? "",
                error: row["error"] as? String ?? ""
            )
        }
    }

    func completeMcpOAuth(
        stateId: String,
        authorizationCode: String,
        timeoutMs: Int
    ) async throws {
        _ = try await rpc(
            "CompleteMcpOAuth",
            body: ["stateId": stateId, "authorizationCode": authorizationCode],
            timeoutMs: timeoutMs
        )
    }

    func validateMcpOAuthTokens(
        targets: [BackendMcpTokenTarget],
        timeoutMs: Int
    ) async throws -> [BackendMcpTokenValidation] {
        let response = try await rpc(
            "ValidateMcpOAuthTokens",
            body: [
                "targets": targets.map {
                    ["serverUrl": $0.serverUrl, "accountKey": $0.accountKey]
                },
            ],
            timeoutMs: timeoutMs
        )
        return (response["results"] as? [[String: Any]] ?? []).compactMap { row in
            guard let serverURL = row["serverUrl"] as? String else { return nil }
            return .init(
                serverUrl: serverURL,
                accountKey: row["accountKey"] as? String,
                hasValidToken: row["hasValidToken"] as? Bool ?? false
            )
        }
    }

    func deleteMcpOAuthToken(
        serverUrl: String,
        accountKey: String,
        source: String,
        timeoutMs: Int
    ) async throws {
        _ = try await rpc(
            "DeleteMcpOAuthToken",
            body: ["serverUrl": serverUrl, "accountKey": accountKey, "source": source],
            timeoutMs: timeoutMs
        )
    }

    func renameMcpOAuthAccount(
        serverId: String,
        accountKey: String,
        newAccountKey: String,
        timeoutMs: Int
    ) async throws {
        _ = try await rpc(
            "RenameMcpOAuthAccount",
            body: [
                "serverId": serverId,
                "accountKey": accountKey,
                "newAccountKey": newAccountKey,
            ],
            timeoutMs: timeoutMs
        )
    }

    func deleteMcpOAuthAccount(
        serverId: String,
        accountKey: String,
        timeoutMs: Int
    ) async throws {
        _ = try await rpc(
            "DeleteMcpOAuthAccount",
            body: ["serverId": serverId, "accountKey": accountKey],
            timeoutMs: timeoutMs
        )
    }
}
