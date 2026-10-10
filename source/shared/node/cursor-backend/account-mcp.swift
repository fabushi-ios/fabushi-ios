import Foundation

let ACCOUNT_MCP_RPC_TIMEOUT_MS = 30_000
let ACCOUNT_MCP_MAX_CA_BUNDLE_BYTES = 128 * 1024

struct AccountMcpStdioConfig: Equatable, Sendable {
    var type: String? = nil
    let command: String
    var args: [String] = []
    var env: [String: String]? = nil
    var cwd: String? = nil
}

struct AccountMcpRemoteAuth: Equatable, Sendable {
    let clientID: String
    var clientSecret: String? = nil
    var scopes: [String]? = nil
}

struct AccountMcpTLS: Equatable, Sendable {
    let caBundle: String
}

struct AccountMcpRemoteConfig: Equatable, Sendable {
    var type: String? = nil
    let url: String
    var headers: [String: String]? = nil
    var auth: AccountMcpRemoteAuth? = nil
    var tls: AccountMcpTLS? = nil
}

enum AccountMcpServerConfig: Equatable, Sendable {
    case stdio(AccountMcpStdioConfig)
    case remote(AccountMcpRemoteConfig)

    var isStdio: Bool {
        if case .stdio = self { return true }
        return false
    }
}

struct AccountMcpConfig: Equatable, Sendable {
    var mcpServers: [String: AccountMcpServerConfig]
}

private func accountMcpStringRecord(_ raw: Any?) -> [String: String]? {
    guard let raw = raw as? [String: Any] else { return nil }
    var result: [String: String] = [:]
    for (key, value) in raw {
        guard let value = value as? String else { return nil }
        result[key] = value
    }
    return result
}

private func parseAccountMcpServerConfig(_ raw: Any) -> AccountMcpServerConfig? {
    guard let item = raw as? [String: Any] else { return nil }

    if let command = item["command"] as? String {
        if let type = item["type"] as? String, type != "stdio" { return nil }
        let args: [String]
        if let rawArgs = item["args"] {
            guard let parsed = rawArgs as? [String] else { return nil }
            args = parsed
        } else {
            args = []
        }

        let environment: [String: String]?
        if item["env"] != nil {
            guard let parsed = accountMcpStringRecord(item["env"]) else { return nil }
            environment = parsed
        } else {
            environment = nil
        }

        if item["cwd"] != nil, !(item["cwd"] is String) { return nil }
        return .stdio(.init(
            type: item["type"] as? String,
            command: command,
            args: args,
            env: environment,
            cwd: item["cwd"] as? String
        ))
    }

    guard let url = item["url"] as? String else { return nil }
    if let type = item["type"] as? String, type != "http" && type != "sse" { return nil }

    let headers: [String: String]?
    if item["headers"] != nil {
        guard let parsed = accountMcpStringRecord(item["headers"]) else { return nil }
        headers = parsed
    } else {
        headers = nil
    }

    let auth: AccountMcpRemoteAuth?
    if let rawAuth = item["auth"] {
        guard let source = rawAuth as? [String: Any],
              let clientID = source["CLIENT_ID"] as? String else { return nil }
        if source["CLIENT_SECRET"] != nil, !(source["CLIENT_SECRET"] is String) { return nil }
        let scopes: [String]?
        if let rawScopes = source["scopes"] {
            guard let parsed = rawScopes as? [String] else { return nil }
            scopes = parsed
        } else {
            scopes = nil
        }
        auth = .init(
            clientID: clientID,
            clientSecret: source["CLIENT_SECRET"] as? String,
            scopes: scopes
        )
    } else {
        auth = nil
    }

    let tls: AccountMcpTLS?
    if let rawTLS = item["tls"] {
        guard let source = rawTLS as? [String: Any],
              source.count == 1,
              let rawBundle = source["caBundle"] as? String else { return nil }
        let bundle = rawBundle.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !bundle.isEmpty,
              bundle.utf8.count <= ACCOUNT_MCP_MAX_CA_BUNDLE_BYTES else { return nil }
        tls = .init(caBundle: bundle)
    } else {
        tls = nil
    }

    return .remote(.init(
        type: item["type"] as? String,
        url: url,
        headers: headers,
        auth: auth,
        tls: tls
    ))
}

func parseAccountMcpConfigJson(_ json: String) -> AccountMcpConfig? {
    guard !json.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
          let data = json.data(using: .utf8),
          let root = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
          let rawServers = root["mcpServers"] as? [String: Any] else {
        return nil
    }
    var servers: [String: AccountMcpServerConfig] = [:]
    for (name, raw) in rawServers {
        guard let parsed = parseAccountMcpServerConfig(raw) else { return nil }
        servers[name] = parsed
    }
    return .init(mcpServers: servers)
}

private func accountMcpJSONObject(_ config: AccountMcpServerConfig) -> [String: Any] {
    switch config {
    case .stdio(let value):
        var object: [String: Any] = ["command": value.command]
        if let type = value.type { object["type"] = type }
        if !value.args.isEmpty { object["args"] = value.args }
        if let env = value.env { object["env"] = env }
        if let cwd = value.cwd { object["cwd"] = cwd }
        return object
    case .remote(let value):
        var object: [String: Any] = ["url": value.url]
        if let type = value.type { object["type"] = type }
        if let headers = value.headers { object["headers"] = headers }
        if let auth = value.auth {
            var authObject: [String: Any] = ["CLIENT_ID": auth.clientID]
            if let secret = auth.clientSecret { authObject["CLIENT_SECRET"] = secret }
            if let scopes = auth.scopes { authObject["scopes"] = scopes }
            object["auth"] = authObject
        }
        if let tls = value.tls { object["tls"] = ["caBundle": tls.caBundle] }
        return object
    }
}

func accountMcpConfigJson(_ config: AccountMcpConfig) throws -> String {
    let object: [String: Any] = [
        "mcpServers": config.mcpServers.mapValues(accountMcpJSONObject),
    ]
    let data = try JSONSerialization.data(withJSONObject: object, options: [.sortedKeys])
    guard let string = String(data: data, encoding: .utf8) else {
        throw CocoaError(.fileWriteInapplicableStringEncoding)
    }
    return string
}

// Reuse the canonical shared MCP account-label normalizer from source/shared/mcp.swift.

func teamServerTransport(_ type: String?) -> String {
    type?.lowercased() == "sse" ? "sse" : "http"
}

struct AccountMcpServerMetadata: Equatable, Sendable {
    var serverId: UInt64? = nil
}

func serverIdsByNameFromMetadata(
    _ metadata: [String: AccountMcpServerMetadata]
) -> [String: UInt64] {
    metadata.reduce(into: [:]) { result, pair in
        if let id = pair.value.serverId, id != 0 {
            result[pair.key] = id
        }
    }
}

struct AvailableMcpAccount: Equatable, Sendable {
    let accountKey: String
    let serverIdentifier: String
    let userHasAccessToken: Bool
}

struct AvailableMcpServer: Equatable, Sendable {
    let id: UInt64
    let name: String
    let serverIdentifier: String
    let type: String
    var url: String? = nil
    var command: String? = nil
    var args: [String] = []
    let enabled: Bool
    let isTeamServer: Bool
    var owningTeamId: UInt64? = nil
    let disabledByTeamAdminPolicy: Bool
    var pluginId: UInt64? = nil
    var isRequired: Bool = false
    var managedByTeamPluginPolicy: Bool = false
    var accounts: [AvailableMcpAccount]? = nil
}

struct AccountMcpAccountSlot: Equatable, Sendable {
    let accountKey: String
    var serverIdentifier: String? = nil
    let hasToken: Bool
}

struct AccountMcpServer: Equatable, Sendable {
    let id: String
    let name: String
    let serverIdentifier: String
    let config: AccountMcpServerConfig
    let isTeamServer: Bool
    let disabledByTeamAdminPolicy: Bool
    var pluginId: String? = nil
    var isRequired: Bool = false
    var managedByTeamPluginPolicy: Bool = false
    var accounts: [AccountMcpAccountSlot]? = nil
}

struct EffectivePluginWire: Equatable, Sendable {
    struct Plugin: Equatable, Sendable {
        let id: UInt64
        let name: String
        let displayName: String
        var gitRef: String? = nil
        var publisherUserId: UInt64? = nil
        var marketplaceTeamId: UInt64? = nil
    }
    var plugin: Plugin? = nil
    let installMode: Int
    let isTeamRequired: Bool
    let isEnabled: Bool
    var pinnedGitRef: String? = nil
    var hasTeamConfiguredVariables: Bool = false
}

struct EffectiveUserPlugin: Equatable, Sendable {
    let pluginId: String
    let name: String
    let displayName: String
    let installMode: EffectivePluginInstallMode
    let isEnabled: Bool
    var versionRef: String? = nil
    var publisherUserId: UInt64? = nil
    var marketplaceTeamId: UInt64? = nil
    var hasTeamConfiguredVariables: Bool = false
}

enum EffectivePluginInstallMode: String, Equatable, Sendable {
    case user
    case teamDefault = "team-default"
    case teamRequired = "team-required"
    case unknown
}

func toEffectivePluginInstallMode(_ mode: Int) -> EffectivePluginInstallMode {
    switch mode {
    case 1: return .user
    case 2: return .teamDefault
    case 3: return .teamRequired
    default: return .unknown
    }
}

struct AccountMcpConfigResponse: Sendable {
    let configJson: String
    let serverMetadataByName: [String: AccountMcpServerMetadata]
}

protocol AccountMcpClient: Sendable {
    func getAvailableMcpServers(timeoutMs: Int) async throws -> [AvailableMcpServer]
    func getMcpConfig(
        teamScope: Bool,
        redactSecrets: Bool,
        teamId: UInt64?,
        timeoutMs: Int?
    ) async throws -> AccountMcpConfigResponse
    func getEffectiveUserPlugins(excludeConfiguredVariables: Bool) async throws -> [EffectivePluginWire]
    func setMcpConfig(
        configJson: String,
        serverIdsByName: [String: UInt64]
    ) async throws
    func installUserPlugin(pluginId: UInt64, variables: [String: String]?) async throws
    func uninstallUserPlugin(pluginId: UInt64) async throws
    func updateUserPluginInstall(pluginId: UInt64, variables: [String: String]) async throws
}

struct AccountMcpCredentials: Sendable {
    let getAccessToken: @Sendable (String?) async throws -> String
    let getMachineId: @Sendable () async throws -> String
}

struct AccountMcpDependencies: @unchecked Sendable {
    let getAccessToken: @Sendable (String?) async throws -> String
    let getMachineId: @Sendable () async throws -> String
    let getBackendUrl: @Sendable () -> String
    let createClient: @Sendable (AccountMcpCredentials) -> any AccountMcpClient
    var reportFailure: (@Sendable (String, Error) -> Void)? = nil
}

private func accountMcpHTTPConfig(_ server: AvailableMcpServer) -> AccountMcpServerConfig? {
    guard server.type.lowercased() != "stdio",
          let url = server.url,
          !url.isEmpty else { return nil }
    return .remote(.init(type: teamServerTransport(server.type), url: url))
}

private func accountMcpSlots(_ server: AvailableMcpServer) -> [AccountMcpAccountSlot]? {
    let slots = (server.accounts ?? []).compactMap { account -> AccountMcpAccountSlot? in
        let key = normalizeMcpAccountLabel(account.accountKey)
        guard !key.isEmpty else { return nil }
        return .init(
            accountKey: key,
            serverIdentifier: account.serverIdentifier.isEmpty ? nil : account.serverIdentifier,
            hasToken: account.userHasAccessToken
        )
    }
    return slots.isEmpty ? nil : slots
}

private func accountMcpAttribution(
    _ server: AvailableMcpServer,
    into target: inout AccountMcpServer
) {
    if let pluginId = server.pluginId, pluginId != 0 {
        target.pluginId = String(pluginId)
    }
    target.isRequired = server.isRequired
    target.managedByTeamPluginPolicy = server.managedByTeamPluginPolicy
}

struct AccountMcpFetchResult: Equatable, Sendable {
    let servers: [AccountMcpServer]
    let cacheScope: String
    var unresolvedServerIds: [String] = []
    var unavailable: Bool = false
}

func fetchAccountMcpServers(
    _ deps: AccountMcpDependencies
) async -> AccountMcpFetchResult? {
    var cacheScope: String?
    do {
        let backendUrl = deps.getBackendUrl()
        let accessToken = try await deps.getAccessToken(backendUrl)
        let resolvedScope = accountCacheScope(accessToken)
        cacheScope = resolvedScope
        let client = deps.createClient(.init(
            getAccessToken: { _ in accessToken },
            getMachineId: deps.getMachineId
        ))
        let available = try await client.getAvailableMcpServers(
            timeoutMs: ACCOUNT_MCP_RPC_TIMEOUT_MS
        )

        let hasUserStdio = available.contains {
            $0.enabled && !$0.isTeamServer && $0.type.lowercased() == "stdio"
        }
        let teamIds = Set(available.compactMap {
            $0.enabled && $0.isTeamServer && $0.type.lowercased() == "stdio"
                ? $0.owningTeamId : nil
        })

        var configResponses: [AccountMcpConfigResponse] = []
        if hasUserStdio {
            do {
                configResponses.append(try await client.getMcpConfig(
                    teamScope: false,
                    redactSecrets: false,
                    teamId: nil,
                    timeoutMs: ACCOUNT_MCP_RPC_TIMEOUT_MS
                ))
            } catch {
                deps.reportFailure?("account-config-fetch", error)
            }
        }
        for teamId in teamIds {
            do {
                configResponses.append(try await client.getMcpConfig(
                    teamScope: true,
                    redactSecrets: false,
                    teamId: teamId,
                    timeoutMs: ACCOUNT_MCP_RPC_TIMEOUT_MS
                ))
            } catch {
                deps.reportFailure?("account-config-fetch", error)
            }
        }

        var stdioConfigById: [String: AccountMcpServerConfig] = [:]
        for response in configResponses {
            guard let config = parseAccountMcpConfigJson(response.configJson) else { continue }
            let ids = serverIdsByNameFromMetadata(response.serverMetadataByName)
            for (name, config) in config.mcpServers {
                if let id = ids[name] {
                    stdioConfigById[String(id)] = config
                }
            }
        }

        var servers: [AccountMcpServer] = []
        var unresolved: [String] = []
        for server in available {
            let id = String(server.id)
            let isStdio = server.type.lowercased() == "stdio"

            if !server.enabled && server.disabledByTeamAdminPolicy && !server.isTeamServer {
                let config: AccountMcpServerConfig?
                if isStdio, let command = server.command, !command.isEmpty {
                    config = .stdio(.init(command: command, args: server.args))
                } else {
                    config = accountMcpHTTPConfig(server)
                }
                if let config {
                    var value = AccountMcpServer(
                        id: id,
                        name: server.name,
                        serverIdentifier: server.serverIdentifier,
                        config: config,
                        isTeamServer: false,
                        disabledByTeamAdminPolicy: true,
                        accounts: accountMcpSlots(server)
                    )
                    accountMcpAttribution(server, into: &value)
                    servers.append(value)
                }
                continue
            }

            guard server.enabled else { continue }
            let config = isStdio ? stdioConfigById[id] : accountMcpHTTPConfig(server)
            guard let config else {
                if isStdio { unresolved.append(id) }
                continue
            }
            guard isStdio == config.isStdio else {
                if isStdio { unresolved.append(id) }
                continue
            }

            var value = AccountMcpServer(
                id: id,
                name: server.name,
                serverIdentifier: server.serverIdentifier,
                config: config,
                isTeamServer: server.isTeamServer,
                disabledByTeamAdminPolicy: server.disabledByTeamAdminPolicy,
                accounts: accountMcpSlots(server)
            )
            accountMcpAttribution(server, into: &value)
            servers.append(value)
        }

        return .init(
            servers: servers,
            cacheScope: resolvedScope,
            unresolvedServerIds: unresolved
        )
    } catch {
        guard let cacheScope else { return nil }
        return .init(servers: [], cacheScope: cacheScope, unavailable: true)
    }
}

func fetchEffectiveUserPlugins(
    _ deps: AccountMcpDependencies
) async throws -> [EffectiveUserPlugin] {
    let client = deps.createClient(.init(
        getAccessToken: deps.getAccessToken,
        getMachineId: deps.getMachineId
    ))
    let response = try await client.getEffectiveUserPlugins(excludeConfiguredVariables: true)
    return response.compactMap { effective in
        guard let plugin = effective.plugin, plugin.id != 0 else { return nil }
        let mapped = toEffectivePluginInstallMode(effective.installMode)
        return .init(
            pluginId: String(plugin.id),
            name: plugin.name,
            displayName: plugin.displayName.isEmpty ? plugin.name : plugin.displayName,
            installMode: mapped == .unknown && effective.isTeamRequired ? .teamRequired : mapped,
            isEnabled: effective.isEnabled,
            versionRef: effective.pinnedGitRef ?? plugin.gitRef,
            publisherUserId: plugin.publisherUserId,
            marketplaceTeamId: plugin.marketplaceTeamId,
            hasTeamConfiguredVariables: effective.hasTeamConfiguredVariables
        )
    }
}

func backfillUserPluginInstalls(
    _ deps: AccountMcpDependencies
) async -> [String] {
    let client = deps.createClient(.init(
        getAccessToken: deps.getAccessToken,
        getMachineId: deps.getMachineId
    ))
    do {
        let available = try await client.getAvailableMcpServers(timeoutMs: ACCOUNT_MCP_RPC_TIMEOUT_MS)
        let effective = try await fetchEffectiveUserPlugins(deps)
        let known = Set(effective.map(\.pluginId))
        let missing = Set(available.compactMap { server -> String? in
            guard !server.isTeamServer,
                  !server.managedByTeamPluginPolicy,
                  let pluginId = server.pluginId,
                  pluginId != 0,
                  !known.contains(String(pluginId)) else { return nil }
            return String(pluginId)
        })

        var backfilled: [String] = []
        for pluginId in missing.sorted() {
            guard let id = UInt64(pluginId) else { continue }
            do {
                try await client.installUserPlugin(pluginId: id, variables: nil)
                backfilled.append(pluginId)
            } catch {
                deps.reportFailure?("account-install-backfill", error)
            }
        }
        return backfilled
    } catch {
        deps.reportFailure?("account-install-backfill", error)
        return []
    }
}

struct AccountMcpWriter: Sendable {
    let dependencies: AccountMcpDependencies

    private func client() -> any AccountMcpClient {
        dependencies.createClient(.init(
            getAccessToken: dependencies.getAccessToken,
            getMachineId: dependencies.getMachineId
        ))
    }

    func getConfigForEdit() async throws -> (
        config: AccountMcpConfig,
        serverIdsByName: [String: UInt64]
    ) {
        let response = try await client().getMcpConfig(
            teamScope: false,
            redactSecrets: true,
            teamId: nil,
            timeoutMs: nil
        )
        return (
            parseAccountMcpConfigJson(response.configJson) ?? .init(mcpServers: [:]),
            serverIdsByNameFromMetadata(response.serverMetadataByName)
        )
    }

    func setConfig(
        _ config: AccountMcpConfig,
        serverIdsByName: [String: UInt64]
    ) async throws {
        try await client().setMcpConfig(
            configJson: try accountMcpConfigJson(config),
            serverIdsByName: serverIdsByName
        )
    }

    func installPlugin(pluginId: UInt64, variables: [String: String]? = nil) async throws {
        try await client().installUserPlugin(
            pluginId: pluginId,
            variables: variables?.isEmpty == true ? nil : variables
        )
    }

    func uninstallPlugin(pluginId: UInt64) async throws {
        try await client().uninstallUserPlugin(pluginId: pluginId)
    }

    func updatePluginInstall(
        pluginId: UInt64,
        variables: [String: String]
    ) async throws {
        try await client().updateUserPluginInstall(
            pluginId: pluginId,
            variables: variables
        )
    }
}

func createAccountMcpWriter(
    _ deps: AccountMcpDependencies
) -> AccountMcpWriter {
    .init(dependencies: deps)
}
