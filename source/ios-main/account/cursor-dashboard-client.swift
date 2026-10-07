import Foundation

struct IOSCursorDashboardError: Error, LocalizedError, Sendable {
    let message: String
    var errorDescription: String? { message }
}


private enum IOSCursorDashboardProto {
    private enum WireValue {
        case varint(UInt64)
        case bytes(Data)
    }

    private struct Reader {
        private let bytes: [UInt8]
        private var index = 0

        init(_ data: Data) {
            bytes = Array(data)
        }

        mutating func readFields() throws -> [(Int, WireValue)] {
            var fields: [(Int, WireValue)] = []
            while index < bytes.count {
                let key = try readVarint()
                let field = Int(key >> 3)
                guard field > 0 else {
                    throw IOSCursorDashboardError(message: "Dashboard protobuf contained field zero.")
                }
                switch Int(key & 0x07) {
                case 0:
                    fields.append((field, .varint(try readVarint())))
                case 1:
                    try skip(8)
                case 2:
                    let length = try readLength()
                    let end = index + length
                    guard end <= bytes.count else {
                        throw IOSCursorDashboardError(message: "Dashboard protobuf length exceeded the response body.")
                    }
                    fields.append((field, .bytes(Data(bytes[index..<end]))))
                    index = end
                case 5:
                    try skip(4)
                default:
                    throw IOSCursorDashboardError(message: "Dashboard protobuf used an unsupported wire type.")
                }
            }
            return fields
        }

        private mutating func readVarint() throws -> UInt64 {
            var value: UInt64 = 0
            var shift: UInt64 = 0
            while index < bytes.count, shift < 64 {
                let byte = bytes[index]
                index += 1
                value |= UInt64(byte & 0x7f) << shift
                if byte & 0x80 == 0 {
                    return value
                }
                shift += 7
            }
            throw IOSCursorDashboardError(message: "Dashboard protobuf contained an invalid varint.")
        }

        private mutating func readLength() throws -> Int {
            let value = try readVarint()
            guard value <= UInt64(Int.max) else {
                throw IOSCursorDashboardError(message: "Dashboard protobuf length overflowed this platform.")
            }
            return Int(value)
        }

        private mutating func skip(_ count: Int) throws {
            guard count >= 0, index + count <= bytes.count else {
                throw IOSCursorDashboardError(message: "Dashboard protobuf ended unexpectedly.")
            }
            index += count
        }
    }

    static func getTeamsRequest(activeOnly: Bool) -> Data {
        var data = Data()
        appendVarintField(1, activeOnly ? 1 : 0, to: &data)
        return data
    }

    static func publishPluginRequest(
        teamId: Int32,
        name: String,
        displayName: String,
        description: String,
        pluginTarGz: Data
    ) throws -> Data {
        guard teamId >= 0 else {
            throw IOSCursorDashboardError(message: "Skill publish team id must be non-negative.")
        }
        var data = Data()
        appendVarintField(1, UInt64(teamId), to: &data)
        appendBytesField(2, Data(name.utf8), to: &data)
        appendBytesField(3, Data(displayName.utf8), to: &data)
        appendBytesField(4, Data(description.utf8), to: &data)
        appendBytesField(5, pluginTarGz, to: &data)
        return data
    }

    static func unpublishPluginRequest(pluginId: Int64, teamId: Int32) throws -> Data {
        guard pluginId >= 0, teamId >= 0 else {
            throw IOSCursorDashboardError(message: "Skill unpublish ids must be non-negative.")
        }
        var data = Data()
        appendVarintField(1, UInt64(pluginId), to: &data)
        appendVarintField(2, UInt64(teamId), to: &data)
        return data
    }

    static func getBackgroundComposerInfoRequest(bcId: String) throws -> Data {
        let bcId = bcId.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !bcId.isEmpty else {
            throw IOSCursorDashboardError(message: "Background composer id is required.")
        }
        var data = Data()
        appendBytesField(1, Data(bcId.utf8), to: &data)
        appendVarintField(2, 0, to: &data)
        appendVarintField(3, 1, to: &data)
        return data
    }

    static func decodeBackgroundComposerInfo(_ data: Data) throws -> IOSCloudAgentComposerInfo {
        var response = Reader(data)
        var detailedMessage: Data?
        for (field, value) in try response.readFields() where field == 1 {
            if case .bytes(let bytes) = value {
                detailedMessage = bytes
                break
            }
        }
        guard let detailedMessage else {
            throw IOSCursorDashboardError(message: "Background composer response was missing composer details.")
        }

        var detailed = Reader(detailedMessage)
        var composerMessage: Data?
        var detailedStatus: Int32?
        var summary: String?
        var permanentErrorMessage: Data?
        for (field, value) in try detailed.readFields() {
            switch (field, value) {
            case (1, .bytes(let bytes)):
                composerMessage = bytes
            case (5, .varint(let value)):
                detailedStatus = Int32(truncatingIfNeeded: value)
            case (10, .bytes(let bytes)):
                summary = String(data: bytes, encoding: .utf8)
            case (16, .bytes(let bytes)):
                permanentErrorMessage = bytes
            default:
                break
            }
        }

        var composerStatus: Int32?
        if let composerMessage {
            var composer = Reader(composerMessage)
            for (field, value) in try composer.readFields() where field == 12 {
                if case .varint(let value) = value {
                    composerStatus = Int32(truncatingIfNeeded: value)
                    break
                }
            }
        }

        var permanentError: String?
        if let permanentErrorMessage {
            var errorEnvelope = Reader(permanentErrorMessage)
            var customErrorMessage: Data?
            for (field, value) in try errorEnvelope.readFields() where field == 2 {
                if case .bytes(let bytes) = value {
                    customErrorMessage = bytes
                    break
                }
            }
            if let customErrorMessage {
                var customError = Reader(customErrorMessage)
                var title: String?
                var detail: String?
                for (field, value) in try customError.readFields() {
                    switch (field, value) {
                    case (1, .bytes(let bytes)):
                        title = String(data: bytes, encoding: .utf8)
                    case (2, .bytes(let bytes)):
                        detail = String(data: bytes, encoding: .utf8)
                    default:
                        break
                    }
                }
                let joined = [title, detail]
                    .compactMap { $0?.trimmingCharacters(in: .whitespacesAndNewlines) }
                    .filter { !$0.isEmpty }
                    .joined(separator: ": ")
                permanentError = joined.isEmpty ? nil : joined
            }
        }

        guard let status = composerStatus ?? detailedStatus else {
            throw IOSCursorDashboardError(message: "Background composer response was missing status.")
        }
        return .init(status: status, summary: summary, permanentError: permanentError)
    }

    static func decodeTeams(_ data: Data) throws -> [IOSCursorSkillPublishTeam] {
        var reader = Reader(data)
        var teams: [IOSCursorSkillPublishTeam] = []
        for (field, value) in try reader.readFields() where field == 1 {
            guard case .bytes(let message) = value else { continue }
            var nested = Reader(message)
            var name = ""
            var id: Int32 = 0
            var isDirectMember = false
            for (nestedField, nestedValue) in try nested.readFields() {
                switch (nestedField, nestedValue) {
                case (1, .bytes(let bytes)):
                    name = String(data: bytes, encoding: .utf8) ?? ""
                case (2, .varint(let value)):
                    id = Int32(truncatingIfNeeded: value)
                case (36, .varint(let value)):
                    isDirectMember = value != 0
                default:
                    break
                }
            }
            teams.append(.init(teamId: id, name: name, isDirectMember: isDirectMember))
        }
        return teams
    }

    static func decodePublishedSkill(_ data: Data) throws -> IOSCursorPublishedSkill {
        var reader = Reader(data)
        var pluginId: Int64 = 0
        var commitSha = ""
        for (field, value) in try reader.readFields() {
            switch (field, value) {
            case (1, .varint(let value)):
                pluginId = Int64(bitPattern: value)
            case (3, .bytes(let bytes)):
                commitSha = String(data: bytes, encoding: .utf8) ?? ""
            default:
                break
            }
        }
        guard pluginId > 0, !commitSha.isEmpty else {
            throw IOSCursorDashboardError(message: "Dashboard publish response was missing plugin identity or commit SHA.")
        }
        return .init(pluginId: String(pluginId), commitSha: commitSha)
    }

    private static func appendVarintField(_ field: Int, _ value: UInt64, to data: inout Data) {
        appendVarint(UInt64(field << 3), to: &data)
        appendVarint(value, to: &data)
    }

    private static func appendBytesField(_ field: Int, _ value: Data, to data: inout Data) {
        appendVarint(UInt64((field << 3) | 2), to: &data)
        appendVarint(UInt64(value.count), to: &data)
        data.append(value)
    }

    private static func appendVarint(_ value: UInt64, to data: inout Data) {
        var value = value
        repeat {
            var byte = UInt8(value & 0x7f)
            value >>= 7
            if value != 0 { byte |= 0x80 }
            data.append(byte)
        } while value != 0
    }
}

struct IOSCursorSkillPublishTeam: Equatable, Sendable {
    let teamId: Int32
    let name: String
    let isDirectMember: Bool
}

struct IOSCursorPublishedSkill: Equatable, Sendable {
    let pluginId: String
    let commitSha: String
}

struct IOSCloudAgentComposerInfo: Equatable, Sendable {
    let status: Int32
    let summary: String?
    let permanentError: String?

    var isActive: Bool { status == 1 || status == 4 }
    var isError: Bool { status == 3 || status == 5 || permanentError != nil }
}

final class IOSCursorDashboardClient: @unchecked Sendable, AccountMcpClient, DashboardMcpExecClient {
    typealias RequestExecutor = @Sendable (URLRequest) async throws -> (Data, URLResponse)

    private let credentials: AccountMcpCredentials
    private let backendURL: URL
    private let session: URLSession
    private let requestExecutor: RequestExecutor?

    init(
        credentials: AccountMcpCredentials,
        backendURL: URL = URL(string: getConfiguredBackendUrl())!,
        session: URLSession = .shared,
        requestExecutor: RequestExecutor? = nil
    ) {
        self.credentials = credentials
        self.backendURL = backendURL
        self.session = session
        self.requestExecutor = requestExecutor
    }

    private func perform(_ request: URLRequest) async throws -> (Data, URLResponse) {
        if let requestExecutor {
            return try await requestExecutor(request)
        }
        return try await session.data(for: request)
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
        let (data, response) = try await perform(request)
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


    private func protoRPC(
        _ method: String,
        body: Data,
        timeoutMs: Int
    ) async throws -> Data {
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
        request.setValue("application/proto", forHTTPHeaderField: "Content-Type")
        request.setValue("1", forHTTPHeaderField: "Connect-Protocol-Version")
        for (name, value) in headers.headers {
            request.setValue(value, forHTTPHeaderField: name)
        }
        request.httpBody = body
        let (data, response) = try await perform(request)
        guard let http = response as? HTTPURLResponse else {
            throw IOSCursorDashboardError(message: "Dashboard Connect RPC returned no HTTP response.")
        }
        guard (200..<300).contains(http.statusCode) else {
            let detail = String(data: data, encoding: .utf8)?
                .trimmingCharacters(in: .whitespacesAndNewlines)
                .prefix(512)
            let suffix = detail.map { $0.isEmpty ? "" : ": \($0)" } ?? ""
            throw IOSCursorDashboardError(
                message: "Dashboard Connect RPC \(method) failed with HTTP \(http.statusCode)\(suffix)"
            )
        }
        return data
    }

    func getSkillPublishTeams(timeoutMs: Int = 10_000) async throws -> [IOSCursorSkillPublishTeam] {
        let response = try await protoRPC(
            "GetTeams",
            body: IOSCursorDashboardProto.getTeamsRequest(activeOnly: true),
            timeoutMs: timeoutMs
        )
        return try IOSCursorDashboardProto.decodeTeams(response)
    }

    func publishSkillPlugin(
        teamId: Int32,
        name: String,
        displayName: String,
        description: String,
        pluginTarGz: Data,
        timeoutMs: Int = 60_000
    ) async throws -> IOSCursorPublishedSkill {
        let body = try IOSCursorDashboardProto.publishPluginRequest(
            teamId: teamId,
            name: name,
            displayName: displayName,
            description: description,
            pluginTarGz: pluginTarGz
        )
        let response = try await protoRPC("PublishPlugin", body: body, timeoutMs: timeoutMs)
        return try IOSCursorDashboardProto.decodePublishedSkill(response)
    }

    func unpublishSkillPlugin(
        pluginId: String,
        teamId: Int32,
        timeoutMs: Int = 60_000
    ) async throws {
        guard let parsedPluginId = Int64(pluginId) else {
            throw IOSCursorDashboardError(message: "Published plugin id is not an int64.")
        }
        let body = try IOSCursorDashboardProto.unpublishPluginRequest(
            pluginId: parsedPluginId,
            teamId: teamId
        )
        _ = try await protoRPC("UnpublishPlugin", body: body, timeoutMs: timeoutMs)
    }

    func getCurrentUserId(timeoutMs: Int = 10_000) async throws -> UInt64 {
        let response = try await rpc("GetMe", body: [:], timeoutMs: timeoutMs)
        guard let userId = uint64(response["userId"]), userId != 0 else {
            throw IOSCursorDashboardError(message: "Dashboard GetMe response was missing the signed-in user id.")
        }
        return userId
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
                let publisher = pluginObject["publisher"] as? [String: Any]
                let marketplace = pluginObject["marketplace"] as? [String: Any]
                plugin = .init(
                    id: id,
                    name: name,
                    displayName: pluginObject["displayName"] as? String ?? "",
                    gitRef: pluginObject["gitRef"] as? String,
                    publisherUserId: uint64(publisher?["ownerUserId"]),
                    marketplaceTeamId: uint64(marketplace?["teamId"])
                )
            } else {
                plugin = nil
            }
            return .init(
                plugin: plugin,
                installMode: (row["installMode"] as? NSNumber)?.intValue ?? 0,
                isTeamRequired: row["isTeamRequired"] as? Bool ?? false,
                isEnabled: row["isEnabled"] as? Bool ?? false,
                pinnedGitRef: row["pinnedGitRef"] as? String,
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
