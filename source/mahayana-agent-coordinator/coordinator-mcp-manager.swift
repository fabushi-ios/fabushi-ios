
private func accountDisplayConfig(from fetched: AccountMcpFetchResult) -> AccountDisplayConfig {
    let servers = fetched.servers.map { server -> DisplayServer in
        let config: McpServerConfig
        switch server.config {
        case .stdio(let value):
            config = .stdio(
                command: value.command,
                args: value.args,
                env: value.env ?? [:]
            )
        case .remote(let value):
            if value.type?.lowercased() == "sse" {
                config = .sse(url: value.url, headers: value.headers ?? [:])
            } else {
                config = .http(url: value.url, headers: value.headers ?? [:])
            }
        }
        return .init(
            id: server.id,
            name: server.name,
            serverIdentifier: server.serverIdentifier,
            config: config,
            isTeamServer: server.isTeamServer,
            disabledByTeamAdminPolicy: server.disabledByTeamAdminPolicy,
            pluginId: server.pluginId,
            isRequired: server.isRequired,
            managedByTeamPluginPolicy: server.managedByTeamPluginPolicy,
            accounts: (server.accounts ?? []).map {
                .init(
                    accountKey: $0.accountKey,
                    hasToken: $0.hasToken,
                    serverIdentifier: $0.serverIdentifier
                )
            }
        )
    }
    return .init(
        servers: servers,
        cacheScope: fetched.cacheScope,
        unavailable: fetched.unavailable,
        unresolvedServerIds: fetched.unresolvedServerIds
    )
}

import Foundation
#if canImport(UIKit)
import UIKit
#endif

func projectSkillPublishTargets(_ teams: [IOSCursorSkillPublishTeam]) -> [[String: Any]] {
    teams
        .filter { $0.isDirectMember && $0.teamId > 0 }
        .sorted {
            if $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedSame {
                return $0.teamId < $1.teamId
            }
            return $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending
        }
        .map {
            [
                "teamId": NSNumber(value: $0.teamId),
                "name": $0.name,
            ]
        }
}

let SKILL_PUBLISH_CONFIRM_MAX_ATTEMPTS = 5

func normalizedSkillPublishVersion(_ raw: String?) -> String? {
    guard let raw else { return nil }
    let value = raw.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
    guard value.count == 40, value.utf8.allSatisfy({
        ($0 >= 48 && $0 <= 57) || ($0 >= 97 && $0 <= 102)
    }) else { return nil }
    return value
}

struct InstalledMarketplaceSkillSource: Equatable, Sendable {
    let version: String
    let sourceUrl: String
}

func requireInstalledMarketplaceSkillSource(
    pluginId: String,
    skillName: String,
    versionRef: String?,
    sourceUrl: String?
) throws -> InstalledMarketplaceSkillSource {
    guard let version = normalizedSkillPublishVersion(versionRef) else {
        throw SandMcpConfigError(
            "Installed plugin \(pluginId) skill \(skillName) is not pinned to an immutable commit."
        )
    }
    guard let sourceUrl,
          let pinnedSourceUrl = pinnedMarketplaceSkillSourceUrl(
              sourceUrl,
              versionRef: version
          )
    else {
        throw SandMcpConfigError(
            "Installed plugin \(pluginId) skill \(skillName) does not expose a supported immutable source."
        )
    }
    return .init(version: version, sourceUrl: pinnedSourceUrl)
}

func projectAuthoritativePublishedPlugins(
    _ plugins: [EffectiveUserPlugin],
    currentUserId: UInt64
) -> [[String: Any]] {
    plugins.compactMap { plugin in
        guard let version = normalizedSkillPublishVersion(plugin.versionRef) else { return nil }
        var row: [String: Any] = [
            "pluginId": plugin.pluginId,
            "pluginVersion": version,
            "name": plugin.name,
            "displayName": plugin.displayName,
            "publishedByCurrentUser": plugin.publisherUserId == currentUserId,
            "isEnabledForAgent": plugin.isEnabled,
        ]
        if let teamId = plugin.marketplaceTeamId {
            row["marketplaceTeamId"] = NSNumber(value: teamId)
        }
        return row
    }
}

private struct CoordinatorMcpSnapshot: Sendable {
    let display: AccountDisplayConfig
    let backendServers: [BackendMcpToolServerWire]
    let disabledToolsByServerId: [String: [String]]
    let scopeGeneration: UInt64
}

@MainActor
private final class CoordinatorMcpHostPort {
    private let hostSupervisor: MahayanaLocalHostSupervisor
    private let settingsStore: SandSettingsStore
    private var accountScope: String?
    private var scopeGeneration: UInt64 = 0
    private var lastSnapshot: CoordinatorMcpSnapshot?
    private var dashboardServerIdentifiers = Set<String>()

    init(
        hostSupervisor: MahayanaLocalHostSupervisor,
        settingsStore: SandSettingsStore
    ) {
        self.hostSupervisor = hostSupervisor
        self.settingsStore = settingsStore
    }

    func updateDashboardServerIdentifiers(_ identifiers: Set<String>) {
        dashboardServerIdentifiers = identifiers
    }

    func usesDashboardServer(_ identifier: String) -> Bool {
        dashboardServerIdentifiers.contains(identifier)
    }

    func updateAccountScope(_ rawScope: String?) {
        let trimmed = rawScope?.trimmingCharacters(in: .whitespacesAndNewlines)
        let next = trimmed?.isEmpty == false ? trimmed : nil
        guard next != accountScope else { return }
        accountScope = next
        scopeGeneration = scopeGeneration == UInt64.max ? 1 : scopeGeneration + 1
        lastSnapshot = nil
    }

    func automationRelaySubscription() async throws -> IOSAutomationRelaySubscription {
        let response = try await hostSupervisor.request(
            method: "feature.automation.relaySubscription",
            params: [:]
        )
        guard let object = response.value as? [String: Any] else {
            throw SandMcpConfigError("Automation relay subscription response is invalid.")
        }
        return IOSAutomationRelaySubscription(
            slackChannels: object["slackChannels"] as? [String] ?? [],
            githubRepos: object["githubRepos"] as? [String] ?? [],
            githubKinds: object["githubKinds"] as? [String] ?? []
        )
    }

    func ingestAutomationRelayEvent(_ event: [String: Any]) async throws -> Bool {
        let response = try await hostSupervisor.request(
            method: "feature.automation.ingestListenerEvent",
            params: ["event": event]
        )
        guard let object = response.value as? [String: Any],
              object["matchedAutomationCount"] is NSNumber
        else {
            throw SandMcpConfigError("Automation relay sink response is invalid.")
        }
        return true
    }

    func pollAutomationListenerReconnects() async throws {
        _ = try await hostSupervisor.request(
            method: "feature.automation.pollListenerReconnects",
            params: [:]
        )
    }

    func hydrateAgentStore() async throws -> [String: Any] {
        let response = try await hostSupervisor.request(
            method: "feature.agentStore.hydrateAll",
            params: [:]
        )
        guard let object = response.value as? [String: Any] else {
            throw SandMcpConfigError("Agent Store hydrate response is invalid.")
        }
        return object
    }

    func snapshotAgentStore() async throws -> [String: Any] {
        let response = try await hostSupervisor.request(
            method: "feature.agentStore.snapshotAll",
            params: [:]
        )
        guard let object = response.value as? [String: Any] else {
            throw SandMcpConfigError("Agent Store snapshot response is invalid.")
        }
        return object
    }

    func admitBackendAutomationFire(_ fire: [String: Any]) async throws -> [String: Any] {
        let response = try await hostSupervisor.request(
            method: "feature.automation.admitBackendFire",
            params: ["fire": fire]
        )
        guard let object = response.value as? [String: Any] else {
            throw SandMcpConfigError("Backend automation fire admission response is invalid.")
        }
        return object
    }

    func backendAutomationFireStatus(_ runUuid: String) async throws -> [String: Any] {
        let response = try await hostSupervisor.request(
            method: "feature.automation.backendFireStatus",
            params: ["runUuid": runUuid]
        )
        guard let object = response.value as? [String: Any] else {
            throw SandMcpConfigError("Backend automation fire status response is invalid.")
        }
        return object
    }

    func backendAutomationFirePollState() async throws -> [String: Any] {
        let response = try await hostSupervisor.request(
            method: "feature.automation.backendFirePollState",
            params: [:]
        )
        guard let object = response.value as? [String: Any] else {
            throw SandMcpConfigError("Backend automation fire poll state response is invalid.")
        }
        return object
    }

    func automationCloudSyncSnapshot() async throws -> [String: Any] {
        let response = try await hostSupervisor.request(
            method: "feature.automation.cloudSyncSnapshot",
            params: [:]
        )
        guard let object = response.value as? [String: Any] else {
            throw SandMcpConfigError("Automation cloud sync snapshot response is invalid.")
        }
        return object
    }

    func automationCloudSyncPlan(
        agentId: String,
        responseBase64: String
    ) async throws -> [String: Any] {
        let response = try await hostSupervisor.request(
            method: "feature.automation.cloudSyncPlan",
            params: [
                "agentId": agentId,
                "responseBase64": responseBase64,
            ]
        )
        guard let object = response.value as? [String: Any] else {
            throw SandMcpConfigError("Automation cloud sync plan response is invalid.")
        }
        return object
    }

    func automationCloudSyncCommit(
        agentId: String,
        responseBase64: String
    ) async throws -> [String: Any] {
        let response = try await hostSupervisor.request(
            method: "feature.automation.cloudSyncCommit",
            params: [
                "agentId": agentId,
                "responseBase64": responseBase64,
            ]
        )
        guard let object = response.value as? [String: Any] else {
            throw SandMcpConfigError("Automation cloud sync commit response is invalid.")
        }
        return object
    }

    func markAutomationCloudSyncFailed(agentId: String) async throws {
        _ = try await hostSupervisor.request(
            method: "feature.automation.cloudSyncMarkFailed",
            params: ["agentId": agentId]
        )
    }

    func loadAccountDisplay(forceFresh: Bool) async throws -> AccountDisplayConfig {
        let snapshot = try await loadSnapshot(forceFresh: forceFresh)
        settingsStore.setMcpDisabledToolsByServerId(snapshot.disabledToolsByServerId)
        return snapshot.display
    }

    func listBackendServers(
        serverIdentifiers: [String]
    ) async throws -> [BackendMcpToolServerWire] {
        let requested = Set(serverIdentifiers)
        return try await loadSnapshot(forceFresh: false).backendServers.filter {
            requested.contains($0.serverIdentifier)
        }
    }

    func setToolDisabled(
        serverIdentifier: String,
        toolName: String,
        disabled: Bool
    ) async throws {
        guard let scope = accountScope else {
            throw SandMcpConfigError("MCP tools require an authenticated account.")
        }
        let generation = scopeGeneration
        _ = try await hostSupervisor.request(
            method: "feature.mcp.setToolDisabled",
            params: [
                "server": serverIdentifier,
                "tool": toolName,
                "disabled": disabled,
            ]
        )
        guard generation == scopeGeneration, scope == accountScope else {
            throw SandMcpConfigError(
                "MCP tool update became stale because the account changed."
            )
        }
        lastSnapshot = nil
    }

    func exportWorkflowPublishPackage(
        agentId: String,
        workflowId: String
    ) async throws -> [String: Any] {
        let response = try await hostSupervisor.request(
            method: "feature.workflow.publishPackage",
            params: [
                "agentId": agentId,
                "workflowId": workflowId,
            ]
        )
        guard let package = response.value as? [String: Any] else {
            throw SandMcpConfigError("Skill publish package response is invalid.")
        }
        return package
    }

    func syncWorkflowPluginFacts(
        agentId: String,
        plugins: [[String: Any]]
    ) async throws -> [String: Any] {
        let response = try await hostSupervisor.request(
            method: "feature.workflow.pluginFactsSync",
            params: [
                "agentId": agentId,
                "plugins": plugins,
            ]
        )
        guard let value = response.value as? [String: Any] else {
            throw SandMcpConfigError("Published skill projection response is invalid.")
        }
        return value
    }

    func syncInstalledPluginSkills(
        agentId: String,
        skills: [[String: Any]]
    ) async throws -> [String: Any] {
        let response = try await hostSupervisor.request(
            method: "feature.workflow.pluginSkillsSync",
            params: [
                "agentId": agentId,
                "skills": skills,
            ]
        )
        guard let value = response.value as? [String: Any] else {
            throw SandMcpConfigError("Installed plugin skill projection response is invalid.")
        }
        return value
    }

    func confirmWorkflowPublish(
        agentId: String,
        workflowId: String,
        pluginId: String,
        commitSha: String
    ) async throws -> [String: Any] {
        let response = try await hostSupervisor.request(
            method: "feature.workflow.publishConfirm",
            params: [
                "agentId": agentId,
                "workflowId": workflowId,
                "pluginId": pluginId,
                "commitSha": commitSha,
            ]
        )
        guard let value = response.value as? [String: Any] else {
            throw SandMcpConfigError("Skill publish confirmation response is invalid.")
        }
        return value
    }

    func confirmPublishedWorkflowResync(
        agentId: String,
        workflowId: String,
        pluginId: String,
        commitSha: String
    ) async throws -> [String: Any] {
        let response = try await hostSupervisor.request(
            method: "feature.workflow.resyncConfirm",
            params: [
                "agentId": agentId,
                "workflowId": workflowId,
                "pluginId": pluginId,
                "commitSha": commitSha,
            ]
        )
        guard let value = response.value as? [String: Any] else {
            throw SandMcpConfigError("Skill resync confirmation response is invalid.")
        }
        return value
    }

    func exportPublishedWorkflowPublishPackage(
        agentId: String,
        workflowId: String
    ) async throws -> [String: Any] {
        let response = try await hostSupervisor.request(
            method: "feature.workflow.resyncPackage",
            params: [
                "agentId": agentId,
                "workflowId": workflowId,
            ]
        )
        guard let value = response.value as? [String: Any] else {
            throw SandMcpConfigError("Published skill package response is invalid.")
        }
        return value
    }

    func prepareWorkflowUnpublish(
        agentId: String,
        workflowId: String
    ) async throws -> [String: Any] {
        let response = try await hostSupervisor.request(
            method: "feature.workflow.unpublishPrepare",
            params: [
                "agentId": agentId,
                "workflowId": workflowId,
            ]
        )
        guard let value = response.value as? [String: Any] else {
            throw SandMcpConfigError("Skill unpublish preparation response is invalid.")
        }
        return value
    }

    func completeWorkflowUnpublish(
        agentId: String,
        pluginId: String
    ) async throws {
        _ = try await hostSupervisor.request(
            method: "feature.workflow.unpublishComplete",
            params: [
                "agentId": agentId,
                "pluginId": pluginId,
            ]
        )
    }

    func executeTool(
        serverIdentifier: String,
        toolName: String,
        args: McpJSONValue,
        toolCallId: String,
        agentId: String?
    ) async throws -> McpJSONValue {
        guard let scope = accountScope else {
            throw SandMcpConfigError("MCP tools require an authenticated account.")
        }
        let generation = scopeGeneration
        var params: [String: Any] = [
            "server": serverIdentifier,
            "tool": toolName,
            "arguments": foundationValue(args),
            "toolCallId": toolCallId,
        ]
        if let agentId,
           !agentId.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            params["agentId"] = agentId
        }
        let response = try await hostSupervisor.request(
            method: "feature.mcp.toolCall",
            params: params
        )
        guard generation == scopeGeneration, scope == accountScope else {
            throw SandMcpConfigError(
                "MCP tool result became stale because the account changed."
            )
        }
        return McpJSONValue.from(response.value)
    }

    private func loadSnapshot(forceFresh: Bool) async throws -> CoordinatorMcpSnapshot {
        guard let scope = accountScope else {
            let empty = CoordinatorMcpSnapshot(
                display: .init(servers: [], cacheScope: nil),
                backendServers: [],
                disabledToolsByServerId: [:],
                scopeGeneration: scopeGeneration
            )
            lastSnapshot = empty
            return empty
        }
        if !forceFresh,
           let lastSnapshot,
           lastSnapshot.scopeGeneration == scopeGeneration {
            return lastSnapshot
        }

        let generation = scopeGeneration
        let response = try await hostSupervisor.request(
            method: "feature.mcp.servers",
            params: [:]
        )
        guard generation == scopeGeneration, scope == accountScope else {
            throw SandMcpConfigError(
                "MCP server snapshot became stale because the account changed."
            )
        }
        guard let object = response.value as? [String: Any],
              let rows = object["servers"] as? [[String: Any]]
        else {
            throw SandMcpConfigError("MCP Host returned an invalid server snapshot.")
        }

        let names = rows.compactMap { row -> String? in
            guard let raw = row["name"] as? String else { return nil }
            let name = raw.trimmingCharacters(in: .whitespacesAndNewlines)
            return name.isEmpty ? nil : name
        }
        let ids = stableRowIDs(names)
        var displayServers: [DisplayServer] = []
        var backendServers: [BackendMcpToolServerWire] = []
        var disabledByServerId: [String: [String]] = [:]

        for row in rows {
            guard let rawName = row["name"] as? String else { continue }
            let name = rawName.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !name.isEmpty,
                  let rowID = ids[name],
                  let wireConfig = row["fabushiConfig"] as? [String: Any]
            else { continue }

            let config: McpServerConfig
            if let command = wireConfig["command"] as? String, !command.isEmpty {
                config = .stdio(
                    command: command,
                    args: stringArray(wireConfig["args"]),
                    env: stringDictionary(wireConfig["env"])
                )
            } else if let url = wireConfig["url"] as? String, !url.isEmpty {
                config = .http(
                    url: url,
                    headers: stringDictionary(wireConfig["http_headers"])
                )
            } else {
                continue
            }

            let disabled = stringArray(wireConfig["disabled_tools"]).sorted()
            disabledByServerId[rowID] = disabled
            displayServers.append(.init(
                id: rowID,
                name: name,
                serverIdentifier: name,
                config: config,
                isTeamServer: false
            ))

            let authStatus = row["authStatus"] as? String
            let status = authStatus == "notLoggedIn" ? "needsAuth" : "connected"
            let toolObject = row["tools"] as? [String: Any] ?? [:]
            let toolNames = Set(toolObject.keys).union(disabled)
            let tools = toolNames.sorted().compactMap { toolName -> BackendToolWire? in
                let detail = toolObject[toolName] as? [String: Any]
                let canonical = (detail?["name"] as? String)?
                    .trimmingCharacters(in: .whitespacesAndNewlines)
                let resolved = canonical?.isEmpty == false ? canonical! : toolName
                guard !resolved.isEmpty else { return nil }
                return .init(
                    name: "\(name).\(resolved)",
                    providerIdentifier: name,
                    toolName: resolved,
                    description: detail?["description"] as? String ?? "",
                    inputSchema: detail?["inputSchema"].map(McpJSONValue.from)
                )
            }
            backendServers.append(.init(
                serverIdentifier: name,
                status: status,
                tools: tools,
                accountLabel: DEFAULT_MCP_ACCOUNT_KEY,
                rowServerIdentifier: name
            ))
        }

        let snapshot = CoordinatorMcpSnapshot(
            display: .init(
                servers: displayServers.sorted { $0.name < $1.name },
                cacheScope: scope
            ),
            backendServers: backendServers,
            disabledToolsByServerId: disabledByServerId,
            scopeGeneration: generation
        )
        guard generation == scopeGeneration, scope == accountScope else {
            throw SandMcpConfigError(
                "MCP server snapshot became stale because the account changed."
            )
        }
        lastSnapshot = snapshot
        return snapshot
    }

    private func stableRowIDs(_ names: [String]) -> [String: String] {
        var result: [String: String] = [:]
        var used = Set<UInt32>()
        for name in Set(names).sorted() {
            var hash: UInt32 = 2_166_136_261
            for byte in name.utf8 {
                hash ^= UInt32(byte)
                hash &*= 16_777_619
            }
            var candidate = hash & 0x7fff_ffff
            if candidate == 0 { candidate = 1 }
            while used.contains(candidate) {
                candidate = candidate == 0x7fff_ffff ? 1 : candidate + 1
            }
            used.insert(candidate)
            result[name] = String(candidate)
        }
        return result
    }

    private func stringArray(_ value: Any?) -> [String] {
        (value as? [Any] ?? []).compactMap { $0 as? String }
    }

    private func stringDictionary(_ value: Any?) -> [String: String] {
        guard let object = value as? [String: Any] else { return [:] }
        return object.reduce(into: [:]) { result, entry in
            if let value = entry.value as? String {
                result[entry.key] = value
            }
        }
    }

    private func foundationValue(_ value: McpJSONValue) -> Any {
        switch value {
        case .null: NSNull()
        case .bool(let value): value
        case .number(let value): value
        case .string(let value): value
        case .array(let values): values.map(foundationValue)
        case .object(let object): object.mapValues(foundationValue)
        }
    }
}

private final class CoordinatorMcpBackendClient: DashboardMcpExecClient, @unchecked Sendable {
    private let port: CoordinatorMcpHostPort
    private let dashboard: IOSCursorDashboardClient

    init(
        port: CoordinatorMcpHostPort,
        dashboard: IOSCursorDashboardClient
    ) {
        self.port = port
        self.dashboard = dashboard
    }

    func listSandMcpTools(
        serverIdentifiers: [String],
        timeoutMs: Int
    ) async throws -> [BackendMcpToolServerWire] {
        var dashboardIdentifiers: [String] = []
        var hostIdentifiers: [String] = []
        for identifier in serverIdentifiers {
            if await port.usesDashboardServer(identifier) {
                dashboardIdentifiers.append(identifier)
            } else {
                hostIdentifiers.append(identifier)
            }
        }
        var rows: [BackendMcpToolServerWire] = []
        if !dashboardIdentifiers.isEmpty {
            rows += try await dashboard.listSandMcpTools(
                serverIdentifiers: dashboardIdentifiers,
                timeoutMs: timeoutMs
            )
        }
        if !hostIdentifiers.isEmpty {
            rows += try await port.listBackendServers(serverIdentifiers: hostIdentifiers)
        }
        return rows
    }

    func executeSandMcpTool(
        serverIdentifier: String,
        toolName: String,
        args: McpJSONValue,
        toolCallId: String,
        agentId: String,
        timeoutMs: Int
    ) async throws -> McpExecResult? {
        if await port.usesDashboardServer(serverIdentifier) {
            return try await dashboard.executeSandMcpTool(
                serverIdentifier: serverIdentifier,
                toolName: toolName,
                args: args,
                toolCallId: toolCallId,
                agentId: agentId,
                timeoutMs: timeoutMs
            )
        }
        return .init(
            caseName: "success",
            value: try await port.executeTool(
                serverIdentifier: serverIdentifier,
                toolName: toolName,
                args: args,
                toolCallId: toolCallId,
                agentId: agentId.isEmpty ? nil : agentId
            )
        )
    }

    func checkHttpMcpStatus(
        serverIds: [String],
        oauthRedirectUri: String,
        forceReauth: Bool,
        accountKey: String,
        timeoutMs: Int
    ) async throws -> [BackendMcpAuthStatusWire] {
        try await dashboard.checkHttpMcpStatus(
            serverIds: serverIds,
            oauthRedirectUri: oauthRedirectUri,
            forceReauth: forceReauth,
            accountKey: accountKey,
            timeoutMs: timeoutMs
        )
    }

    func completeMcpOAuth(
        stateId: String,
        authorizationCode: String,
        timeoutMs: Int
    ) async throws {
        try await dashboard.completeMcpOAuth(
            stateId: stateId,
            authorizationCode: authorizationCode,
            timeoutMs: timeoutMs
        )
    }

    func validateMcpOAuthTokens(
        targets: [BackendMcpTokenTarget],
        timeoutMs: Int
    ) async throws -> [BackendMcpTokenValidation] {
        try await dashboard.validateMcpOAuthTokens(targets: targets, timeoutMs: timeoutMs)
    }

    func deleteMcpOAuthToken(
        serverUrl: String,
        accountKey: String,
        source: String,
        timeoutMs: Int
    ) async throws {
        try await dashboard.deleteMcpOAuthToken(
            serverUrl: serverUrl,
            accountKey: accountKey,
            source: source,
            timeoutMs: timeoutMs
        )
    }

    func renameMcpOAuthAccount(
        serverId: String,
        accountKey: String,
        newAccountKey: String,
        timeoutMs: Int
    ) async throws {
        try await dashboard.renameMcpOAuthAccount(
            serverId: serverId,
            accountKey: accountKey,
            newAccountKey: newAccountKey,
            timeoutMs: timeoutMs
        )
    }

    func deleteMcpOAuthAccount(
        serverId: String,
        accountKey: String,
        timeoutMs: Int
    ) async throws {
        try await dashboard.deleteMcpOAuthAccount(
            serverId: serverId,
            accountKey: accountKey,
            timeoutMs: timeoutMs
        )
    }
}

struct IOSFeedbackSubmitArgs: Equatable, Sendable {
    let accountSlot: String
    let message: String
    let submissionId: String
    let conversationId: String?
}

private let IOS_FEEDBACK_UUID_PATTERN =
    #"^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$"#

func iosFeedbackSubmitArgs(_ value: [String: Any]) -> IOSFeedbackSubmitArgs? {
    guard let rawMessage = value["message"] as? String else { return nil }
    let message = rawMessage.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !message.isEmpty,
          message.count <= SAND_FEEDBACK_MESSAGE_MAX_CHARS,
          let rawSlot = value["accountSlot"] as? String
    else { return nil }
    let accountSlot = rawSlot.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !accountSlot.isEmpty,
          accountSlot.count <= SAND_FEEDBACK_ACCOUNT_SLOT_MAX_CHARS,
          let submissionId = value["submissionId"] as? String,
          submissionId.range(of: IOS_FEEDBACK_UUID_PATTERN, options: .regularExpression) != nil
    else { return nil }
    var conversationId: String?
    if let rawConversation = value["conversationId"] {
        guard let rawConversation = rawConversation as? String else { return nil }
        let normalized = rawConversation.trimmingCharacters(in: .whitespacesAndNewlines)
        if !normalized.isEmpty {
            guard normalized.count <= SAND_FEEDBACK_CONVERSATION_ID_MAX_CHARS else { return nil }
            conversationId = normalized
        }
    }
    return .init(
        accountSlot: accountSlot,
        message: message,
        submissionId: submissionId,
        conversationId: conversationId
    )
}

@MainActor
final class IOSFeedbackReportClient {
    typealias RequestExecutor = @Sendable (URLRequest) async throws -> (Data, URLResponse)

    private let getAccessToken: @MainActor @Sendable (String) async throws -> String
    private let getMachineID: @MainActor @Sendable () async throws -> String
    private let getBackendURL: @MainActor @Sendable () -> String
    private let recentSentryEventIDs: @MainActor @Sendable () -> [String]
    private let appVersion: @MainActor @Sendable () -> String
    private let osVersion: @MainActor @Sendable () -> String
    private let requestExecutor: RequestExecutor

    init(
        getAccessToken: @escaping @MainActor @Sendable (String) async throws -> String,
        getMachineID: @escaping @MainActor @Sendable () async throws -> String,
        getBackendURL: @escaping @MainActor @Sendable () -> String,
        recentSentryEventIDs: @escaping @MainActor @Sendable () -> [String] = { [] },
        appVersion: @escaping @MainActor @Sendable () -> String = {
            Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String
                ?? "unknown"
        },
        osVersion: @escaping @MainActor @Sendable () -> String = {
            ProcessInfo.processInfo.operatingSystemVersionString
        },
        requestExecutor: @escaping RequestExecutor = { request in
            try await URLSession.shared.data(for: request)
        }
    ) {
        self.getAccessToken = getAccessToken
        self.getMachineID = getMachineID
        self.getBackendURL = getBackendURL
        self.recentSentryEventIDs = recentSentryEventIDs
        self.appVersion = appVersion
        self.osVersion = osVersion
        self.requestExecutor = requestExecutor
    }

    func submit(_ value: [String: Any]) async -> [String: Any] {
        guard let feedback = iosFeedbackSubmitArgs(value) else {
            return ["ok": false, "code": "invalid-feedback"]
        }
        let backendURL = getBackendURL()
        let accessToken: String
        do {
            accessToken = try await getAccessToken(backendURL)
        } catch {
            return ["ok": false, "code": "not-signed-in"]
        }
        let token = parseJwtPayload(accessToken)
        guard (token?.sub ?? token?.email) == feedback.accountSlot else {
            return ["ok": false, "code": "not-signed-in"]
        }

        do {
            guard let baseURL = URL(string: backendURL) else {
                return ["ok": false, "code": "unavailable"]
            }
            var request = URLRequest(url: baseURL.appendingPathComponent("sand/feedback"))
            request.httpMethod = "POST"
            request.timeoutInterval = 15
            var headers = getSandBackendClientHeaders(ProcessInfo.processInfo.environment)
            headers["content-type"] = "application/json"
            headers["authorization"] = "Bearer \(accessToken)"
            headers["x-cursor-checksum"] = createCursorChecksum(
                machineId: try await getMachineID()
            )
            for (name, value) in headers {
                request.setValue(value, forHTTPHeaderField: name)
            }
            var body: [String: Any] = [
                "message": feedback.message,
                "submissionId": feedback.submissionId,
                "appVersion": appVersion(),
                "platform": "ios",
                "osVersion": osVersion(),
                "sentryEventIds": recentSentryEventIDs()
                    .filter(isValidFeedbackSentryEventID)
                    .prefix(SAND_FEEDBACK_SENTRY_EVENT_ID_MAX_COUNT)
                    .map { $0 },
            ]
            if let conversationId = feedback.conversationId {
                body["conversationId"] = conversationId
            }
            request.httpBody = try JSONSerialization.data(withJSONObject: body)

            let (_, response) = try await requestExecutor(request)
            guard let http = response as? HTTPURLResponse else {
                return ["ok": false, "code": "unavailable"]
            }
            switch http.statusCode {
            case 200..<300: return ["ok": true]
            case 400: return ["ok": false, "code": "invalid-feedback"]
            case 402: return ["ok": false, "code": "subscription-required"]
            case 403: return ["ok": false, "code": "access-denied"]
            case 429: return ["ok": false, "code": "rate-limited"]
            default: return ["ok": false, "code": "unavailable"]
            }
        } catch {
            return ["ok": false, "code": "unavailable"]
        }
    }
}


@MainActor
final class IOSCursorAuthMainRpcCompatibility {
    struct Dependencies {
        let readStatus: @MainActor () async -> IOSCursorAuthStatus
        let isLoginPending: @MainActor () -> Bool
        let beginLogin: @MainActor () throws -> IOSCursorLoginStart
        let pollLogin: @MainActor (_ attemptId: String) async throws -> IOSCursorAuthStatus?
        let cancelPendingLogins: @MainActor () -> Void
        let logout: @MainActor () async throws -> Void
        let openExternal: @MainActor (_ url: URL) async throws -> Void
        let reloadAuthoritativeMcp: @MainActor () async throws -> Void
        let invalidateMcp: @MainActor () async -> Void
        let sleepBeforePoll: @MainActor () async throws -> Void
    }

    private let dependencies: Dependencies
    private var activeAttemptId: String?

    init(dependencies: Dependencies) {
        self.dependencies = dependencies
    }

    func getStatus() async -> [String: Any] {
        Self.project(
            await dependencies.readStatus(),
            loggingIn: dependencies.isLoginPending()
        )
    }

    func login() async throws -> [String: Any] {
        if activeAttemptId != nil {
            dependencies.cancelPendingLogins()
            activeAttemptId = nil
        }
        let start = try dependencies.beginLogin()
        activeAttemptId = start.attemptId
        do {
            try await dependencies.openExternal(start.loginURL)
            for _ in 0..<150 {
                guard activeAttemptId == start.attemptId else {
                    return await getStatus()
                }
                if let status = try await dependencies.pollLogin(start.attemptId) {
                    guard activeAttemptId == start.attemptId else {
                        return await getStatus()
                    }
                    activeAttemptId = nil
                    try await dependencies.reloadAuthoritativeMcp()
                    return Self.project(status, loggingIn: false)
                }
                try await dependencies.sleepBeforePoll()
            }
            if activeAttemptId == start.attemptId {
                dependencies.cancelPendingLogins()
                activeAttemptId = nil
            }
            throw SandMcpConfigError("Cursor sign-in did not finish. Try again.")
        } catch is CancellationError {
            if activeAttemptId == start.attemptId {
                dependencies.cancelPendingLogins()
                activeAttemptId = nil
            }
            return await getStatus()
        } catch {
            if activeAttemptId == start.attemptId {
                dependencies.cancelPendingLogins()
                activeAttemptId = nil
            }
            throw error
        }
    }

    func cancelLogin() async throws -> [String: Any] {
        dependencies.cancelPendingLogins()
        activeAttemptId = nil
        try await dependencies.logout()
        await dependencies.invalidateMcp()
        return await getStatus()
    }

    func logout() async throws -> [String: Any] {
        dependencies.cancelPendingLogins()
        activeAttemptId = nil
        try await dependencies.logout()
        await dependencies.invalidateMcp()
        return await getStatus()
    }

    static func project(_ status: IOSCursorAuthStatus, loggingIn: Bool) -> [String: Any] {
        if loggingIn {
            return ["kind": "logging-in", "freshness": NSNumber(value: status.freshness)]
        }
        guard status.loggedIn else {
            return [
                "kind": "logged-out",
                "errorMessage": status.errorMessage ?? NSNull(),
                "freshness": NSNumber(value: status.freshness),
            ]
        }
        return [
            "kind": "logged-in",
            "authId": status.authId ?? NSNull(),
            "email": status.email ?? NSNull(),
            "displayName": status.displayName ?? NSNull(),
            "profilePictureUrl": status.profilePictureUrl ?? NSNull(),
            "isAnysphereUser": status.isAnysphereUser.map { NSNumber(value: $0) } ?? NSNull(),
            "expiresAt": status.expiresAtMs.map(NSNumber.init(value:)) ?? NSNull(),
            "freshness": NSNumber(value: status.freshness),
        ]
    }
}

let IOS_COMPUTER_MIGRATION_STALL_SECONDS = 30
let IOS_COMPUTER_MIGRATION_MAX_STALL_REATTACHES = 20

struct IOSComputerMigrationStallBudget: Equatable, Sendable {
    let limit: Int
    private(set) var remaining: Int

    init(limit: Int = IOS_COMPUTER_MIGRATION_MAX_STALL_REATTACHES) {
        self.limit = max(0, limit)
        self.remaining = max(0, limit)
    }

    mutating func reset() { remaining = limit }

    mutating func consumeReattach() -> Bool {
        guard remaining > 0 else { return false }
        remaining -= 1
        return true
    }
}

let IOS_AUTOMATION_RELAY_POLL_INTERVAL_NS: UInt64 = 4_000_000_000
let IOS_AUTOMATION_LISTENER_RECONNECT_INTERVAL_NS: UInt64 = 5_000_000_000
let IOS_AUTOMATION_RELAY_REGISTER_INTERVAL_MS: Int64 = 5 * 60_000
let IOS_AUTOMATION_RELAY_DEGRADED_REGISTER_INTERVAL_MS: Int64 = 60_000
let IOS_AUTOMATION_RELAY_ERROR_BACKOFF_MS: Int64 = 30_000
let IOS_AUTOMATION_CLOUD_SYNC_INTERVAL_NS: UInt64 = 60_000_000_000

struct IOSAutomationRelaySubscription: Equatable, Sendable {
    let slackChannels: [String]
    let githubRepos: [String]
    let githubKinds: [String]

    init(slackChannels: [String], githubRepos: [String], githubKinds: [String]) {
        self.slackChannels = Array(Set(slackChannels.map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }.filter { !$0.isEmpty })).sorted()
        self.githubRepos = Array(Set(githubRepos.map { $0.trimmingCharacters(in: .whitespacesAndNewlines).lowercased() }.filter { !$0.isEmpty })).sorted()
        self.githubKinds = Array(Set(githubKinds.map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }.filter { !$0.isEmpty })).sorted()
    }

    var registrationKey: String {
        [
            slackChannels.joined(separator: "\u{1f}"),
            githubRepos.joined(separator: "\u{1f}"),
            githubKinds.joined(separator: "\u{1f}"),
        ].joined(separator: "\u{1e}")
    }
}

@MainActor
final class IOSAutomationRelayLifecycle {
    typealias LoadSubscription = @MainActor () async throws -> IOSAutomationRelaySubscription
    typealias Register = @MainActor (IOSAutomationRelaySubscription) async throws -> [String: Any]
    typealias Drain = @MainActor ([String]) async throws -> [String: Any]
    typealias Sink = @MainActor ([String: Any]) async throws -> Bool
    typealias PollReconnects = @MainActor () async throws -> Void

    private let loadSubscription: LoadSubscription
    private let register: Register
    private let drain: Drain
    private let sink: Sink
    private let pollReconnects: PollReconnects
    private let nowMilliseconds: () -> Int64
    private let launchBackgroundTasks: Bool

    private(set) var accountScope: String?
    private(set) var generation: UInt64 = 0
    private(set) var suspended = true
    private(set) var pendingAckIds = Set<String>()
    private(set) var registeredKey = ""
    private(set) var registeredAtMs: Int64 = 0
    private(set) var backoffUntilMs: Int64 = 0
    private(set) var degradedStreak = 0
    private(set) var sinkDeliveryCount = 0
    private var recentlyAckedIds = Set<String>()
    private var recentlyAckedOrder: [String] = []
    private var relayTask: Task<Void, Never>?
    private var relaySleepTask: Task<Void, Never>?
    private var relayWakeRequested = false
    private var reconnectTask: Task<Void, Never>?

    init(
        loadSubscription: @escaping LoadSubscription,
        register: @escaping Register,
        drain: @escaping Drain,
        sink: @escaping Sink,
        pollReconnects: @escaping PollReconnects,
        nowMilliseconds: @escaping () -> Int64 = {
            Int64(Date().timeIntervalSince1970 * 1_000)
        },
        launchBackgroundTasks: Bool = true
    ) {
        self.loadSubscription = loadSubscription
        self.register = register
        self.drain = drain
        self.sink = sink
        self.pollReconnects = pollReconnects
        self.nowMilliseconds = nowMilliseconds
        self.launchBackgroundTasks = launchBackgroundTasks
    }

    func updateAccountScope(_ rawScope: String?) {
        let normalized = rawScope?.trimmingCharacters(in: .whitespacesAndNewlines)
        let next = normalized?.isEmpty == false ? normalized : nil
        guard next != accountScope else { return }
        generation = generation == UInt64.max ? 1 : generation + 1
        relayTask?.cancel()
        relaySleepTask?.cancel()
        reconnectTask?.cancel()
        relayTask = nil
        relaySleepTask = nil
        relayWakeRequested = false
        reconnectTask = nil
        accountScope = next
        clearTransportState(clearPendingAcks: true)
        if !suspended {
            launchTasks()
        }
    }

    func start() {
        guard accountScope != nil else { return }
        suspended = false
        reconcile()
        launchTasks()
    }

    func suspend() {
        guard !suspended else { return }
        suspended = true
        generation = generation == UInt64.max ? 1 : generation + 1
        relayTask?.cancel()
        relaySleepTask?.cancel()
        reconnectTask?.cancel()
        relayTask = nil
        relaySleepTask = nil
        relayWakeRequested = false
        reconnectTask = nil
    }

    func resume() {
        guard accountScope != nil else { return }
        if suspended {
            suspended = false
            generation = generation == UInt64.max ? 1 : generation + 1
        }
        reconcile()
        launchTasks()
    }

    func stop(clearPendingAcks: Bool = true) {
        suspended = true
        generation = generation == UInt64.max ? 1 : generation + 1
        relayTask?.cancel()
        relaySleepTask?.cancel()
        reconnectTask?.cancel()
        relayTask = nil
        relaySleepTask = nil
        relayWakeRequested = false
        reconnectTask = nil
        accountScope = nil
        clearTransportState(clearPendingAcks: clearPendingAcks)
    }

    func requestDrain() {
        backoffUntilMs = 0
        relayWakeRequested = true
        relaySleepTask?.cancel()
    }

    func reconcile() {
        registeredKey = ""
        registeredAtMs = 0
        backoffUntilMs = 0
        relayWakeRequested = true
        relaySleepTask?.cancel()
    }

    func runOneCycleForTesting() async {
        guard let scope = accountScope, !suspended else { return }
        await runCycle(generation: generation, scope: scope)
    }

    func runReconnectPollForTesting() async {
        guard let scope = accountScope, !suspended else { return }
        await runReconnectPoll(generation: generation, scope: scope)
    }

    private func launchTasks() {
        guard launchBackgroundTasks, !suspended, let scope = accountScope else { return }
        let token = generation
        if relayTask == nil {
            relayTask = Task { @MainActor [weak self] in
                guard let self else { return }
                while !Task.isCancelled && self.isCurrent(generation: token, scope: scope) {
                    self.relayWakeRequested = false
                    await self.runCycle(generation: token, scope: scope)
                    guard !Task.isCancelled && self.isCurrent(generation: token, scope: scope) else {
                        return
                    }
                    if self.relayWakeRequested {
                        continue
                    }
                    let sleeper = Task<Void, Never> {
                        try? await Task.sleep(nanoseconds: IOS_AUTOMATION_RELAY_POLL_INTERVAL_NS)
                    }
                    self.relaySleepTask = sleeper
                    await sleeper.value
                    self.relaySleepTask = nil
                }
            }
        }
        if reconnectTask == nil {
            reconnectTask = Task { @MainActor [weak self] in
                guard let self else { return }
                while !Task.isCancelled && self.isCurrent(generation: token, scope: scope) {
                    await self.runReconnectPoll(generation: token, scope: scope)
                    do {
                        try await Task.sleep(nanoseconds: IOS_AUTOMATION_LISTENER_RECONNECT_INTERVAL_NS)
                    } catch {
                        return
                    }
                }
            }
        }
    }

    private func runReconnectPoll(generation token: UInt64, scope: String) async {
        guard isCurrent(generation: token, scope: scope) else { return }
        do {
            try await pollReconnects()
        } catch {
            return
        }
        guard isCurrent(generation: token, scope: scope) else { return }
    }

    private func runCycle(generation token: UInt64, scope: String) async {
        let now = nowMilliseconds()
        guard isCurrent(generation: token, scope: scope), now >= backoffUntilMs else { return }
        do {
            let subscription = try await loadSubscription()
            guard isCurrent(generation: token, scope: scope) else { return }

            let shift = min(max(degradedStreak - 1, 0), 10)
            let multiplier = Int64(1 << shift)
            let staleAfter = degradedStreak == 0
                ? IOS_AUTOMATION_RELAY_REGISTER_INTERVAL_MS
                : min(
                    IOS_AUTOMATION_RELAY_REGISTER_INTERVAL_MS,
                    IOS_AUTOMATION_RELAY_DEGRADED_REGISTER_INTERVAL_MS * multiplier
                )
            let key = subscription.registrationKey
            if key != registeredKey || now - registeredAtMs >= staleAfter {
                let registration = try await register(subscription)
                guard isCurrent(generation: token, scope: scope) else { return }
                registeredKey = key
                registeredAtMs = now
                degradedStreak = Self.registrationIsDegraded(registration, subscription: subscription)
                    ? min(degradedStreak + 1, 11)
                    : 0
            }

            let previousAcks = pendingAckIds
            let response = try await drain(Array(previousAcks).sorted())
            guard isCurrent(generation: token, scope: scope) else { return }

            for id in previousAcks {
                rememberAcked(id)
            }
            var nextAcks = Set<String>()
            let events = response["events"] as? [Any] ?? []
            for raw in events {
                guard let wire = raw as? [String: Any],
                      let id = (wire["id"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines),
                      !id.isEmpty
                else { continue }

                if previousAcks.contains(id) || recentlyAckedIds.contains(id) {
                    nextAcks.insert(id)
                    continue
                }
                guard let event = Self.mapRelayWireEvent(wire, nowMilliseconds: now) else {
                    nextAcks.insert(id)
                    continue
                }
                do {
                    if try await sink(event) {
                        guard isCurrent(generation: token, scope: scope) else { return }
                        nextAcks.insert(id)
                        sinkDeliveryCount += 1
                    }
                } catch {
                    guard isCurrent(generation: token, scope: scope) else { return }
                }
            }
            pendingAckIds = nextAcks
        } catch {
            guard isCurrent(generation: token, scope: scope) else { return }
            registeredKey = ""
            registeredAtMs = 0
            degradedStreak = min(degradedStreak + 1, 11)
            backoffUntilMs = now + IOS_AUTOMATION_RELAY_ERROR_BACKOFF_MS
        }
    }

    private func isCurrent(generation token: UInt64, scope: String) -> Bool {
        !suspended && generation == token && accountScope == scope
    }

    private func clearTransportState(clearPendingAcks: Bool) {
        registeredKey = ""
        registeredAtMs = 0
        backoffUntilMs = 0
        degradedStreak = 0
        if clearPendingAcks {
            pendingAckIds.removeAll()
            recentlyAckedIds.removeAll()
            recentlyAckedOrder.removeAll()
        }
    }

    private func rememberAcked(_ id: String) {
        guard recentlyAckedIds.insert(id).inserted else { return }
        recentlyAckedOrder.append(id)
        while recentlyAckedOrder.count > 512 {
            let stale = recentlyAckedOrder.removeFirst()
            recentlyAckedIds.remove(stale)
        }
    }

    private static func registrationIsDegraded(
        _ response: [String: Any],
        subscription: IOSAutomationRelaySubscription
    ) -> Bool {
        func status(_ key: String) -> String? {
            (response[key] as? [String: Any])?["status"] as? String
        }
        if !subscription.slackChannels.isEmpty,
           let value = status("slack"),
           ["error", "not-linked", "disconnected"].contains(value) {
            return true
        }
        if !subscription.githubRepos.isEmpty,
           let value = status("github"),
           ["error", "not-connected", "disconnected"].contains(value) {
            return true
        }
        return false
    }

    static func mapRelayWireEvent(
        _ wire: [String: Any],
        nowMilliseconds: Int64
    ) -> [String: Any]? {
        guard let source = wire["source"] as? String else { return nil }
        let occurredAt = (wire["timestampMs"] as? NSNumber)?.int64Value ?? nowMilliseconds
        if source == "slack" {
            guard let channelId = wire["channelId"] as? String, !channelId.isEmpty else { return nil }
            let channelName = (wire["channelName"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines)
            let displayChannel = channelName?.isEmpty == false ? channelName! : channelId
            let kind = (wire["kind"] as? String) == "reaction"
                ? "reaction"
                : ((wire["isMention"] as? Bool) == true ? "mention" : "message")
            let sender = (wire["senderSlackUserId"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines)
            let text = wire["text"] as? String ?? ""
            var fields: [[String: String]] = [
                ["label": "channel", "value": channelId],
                ["label": "channelName", "value": displayChannel],
            ]
            for key in ["ts", "threadTs", "reactionEmoji"] {
                if let value = wire[key] as? String, !value.isEmpty {
                    fields.append(["label": key, "value": value])
                }
            }
            return [
                "source": "slack",
                "event": kind,
                "title": "#\(displayChannel)",
                "summary": text,
                "actor": sender?.isEmpty == false ? sender! : "someone",
                "fields": fields,
                "occurredAtMs": occurredAt,
            ]
        }
        guard source == "github",
              let repo = wire["repo"] as? String, !repo.isEmpty,
              let kind = wire["kind"] as? String, !kind.isEmpty
        else { return nil }
        var fields: [[String: String]] = [
            ["label": "repo", "value": repo],
            ["label": "kind", "value": kind],
        ]
        for key in ["prOwner", "branch"] {
            if let value = wire[key] as? String, !value.isEmpty {
                fields.append(["label": key, "value": value])
            }
        }
        var event: [String: Any] = [
            "source": "github",
            "event": kind,
            "title": wire["title"] as? String ?? repo,
            "summary": wire["detail"] as? String ?? (wire["title"] as? String ?? ""),
            "actor": wire["actor"] as? String ?? "someone",
            "fields": fields,
            "occurredAtMs": occurredAt,
        ]
        if let url = wire["url"] as? String, !url.isEmpty {
            event["url"] = url
        }
        return event
    }
}

@MainActor
final class IOSAutomationFireLifecycle {
    private struct DeliveryState {
        var phase: String
        var status: String?
        var errorMessage: String?
    }

    typealias PollState = @MainActor () async throws -> [String: Any]
    typealias Poll = @MainActor ([String]) async throws -> [String: Any]
    typealias Complete = @MainActor (String, String, String?) async throws -> Void
    typealias Admit = @MainActor ([String: Any]) async throws -> [String: Any]
    typealias Status = @MainActor (String) async throws -> [String: Any]

    private let pollState: PollState
    private let poll: Poll
    private let complete: Complete
    private let admit: Admit
    private let status: Status
    private let nowMilliseconds: () -> Int64
    private let launchBackgroundTask: Bool

    private(set) var accountScope: String?
    private(set) var generation: UInt64 = 0
    private(set) var suspended = true
    private(set) var backoffUntilMs: Int64 = 0
    private(set) var pollNotBeforeMs: Int64 = 0
    private(set) var drainedWhileUnschedulable = false
    private var states: [String: DeliveryState] = [:]
    private var task: Task<Void, Never>?
    private var sleepTask: Task<Void, Never>?
    private var wakeRequested = false

    init(
        pollState: @escaping PollState,
        poll: @escaping Poll,
        complete: @escaping Complete,
        admit: @escaping Admit,
        status: @escaping Status,
        nowMilliseconds: @escaping () -> Int64 = {
            Int64(Date().timeIntervalSince1970 * 1_000)
        },
        launchBackgroundTask: Bool = true
    ) {
        self.pollState = pollState
        self.poll = poll
        self.complete = complete
        self.admit = admit
        self.status = status
        self.nowMilliseconds = nowMilliseconds
        self.launchBackgroundTask = launchBackgroundTask
    }

    func updateAccountScope(_ rawScope: String?) {
        let normalized = rawScope?.trimmingCharacters(in: .whitespacesAndNewlines)
        let next = normalized?.isEmpty == false ? normalized : nil
        guard next != accountScope else { return }
        generation = generation == UInt64.max ? 1 : generation + 1
        task?.cancel()
        sleepTask?.cancel()
        task = nil
        sleepTask = nil
        wakeRequested = false
        accountScope = next
        states.removeAll()
        backoffUntilMs = 0
        pollNotBeforeMs = 0
        drainedWhileUnschedulable = false
        if !suspended { launchTask() }
    }

    func start() {
        guard accountScope != nil else { return }
        suspended = false
        reconcile()
        launchTask()
    }

    func suspend() {
        guard !suspended else { return }
        suspended = true
        generation = generation == UInt64.max ? 1 : generation + 1
        task?.cancel()
        sleepTask?.cancel()
        task = nil
        sleepTask = nil
        wakeRequested = false
    }

    func resume() {
        guard accountScope != nil else { return }
        if suspended {
            suspended = false
            generation = generation == UInt64.max ? 1 : generation + 1
        }
        reconcile()
        launchTask()
    }

    func stop() {
        suspended = true
        generation = generation == UInt64.max ? 1 : generation + 1
        task?.cancel()
        sleepTask?.cancel()
        task = nil
        sleepTask = nil
        wakeRequested = false
        accountScope = nil
        states.removeAll()
        backoffUntilMs = 0
        pollNotBeforeMs = 0
        drainedWhileUnschedulable = false
    }

    func requestDrain() {
        pollNotBeforeMs = 0
        wakeRequested = true
        sleepTask?.cancel()
    }

    func reconcile() {
        pollNotBeforeMs = 0
        drainedWhileUnschedulable = false
        wakeRequested = true
        sleepTask?.cancel()
    }

    func runOneCycleForTesting() async {
        guard let scope = accountScope, !suspended else { return }
        await runCycle(generation: generation, scope: scope)
    }

    func phaseForTesting(_ runUuid: String) -> String? {
        states[runUuid]?.phase
    }

    private func launchTask() {
        guard launchBackgroundTask, !suspended, let scope = accountScope, task == nil else { return }
        let token = generation
        task = Task { @MainActor [weak self] in
            guard let self else { return }
            while !Task.isCancelled && self.isCurrent(generation: token, scope: scope) {
                self.wakeRequested = false
                await self.runCycle(generation: token, scope: scope)
                guard !Task.isCancelled && self.isCurrent(generation: token, scope: scope) else {
                    return
                }
                if self.wakeRequested {
                    continue
                }
                let sleeper = Task<Void, Never> {
                    try? await Task.sleep(nanoseconds: IOS_AUTOMATION_RELAY_POLL_INTERVAL_NS)
                }
                self.sleepTask = sleeper
                await sleeper.value
                self.sleepTask = nil
            }
        }
    }

    private func runCycle(generation token: UInt64, scope: String) async {
        let now = nowMilliseconds()
        guard isCurrent(generation: token, scope: scope),
              now >= backoffUntilMs,
              now >= pollNotBeforeMs
        else { return }

        await refreshRunningAndRetryCompletions(generation: token, scope: scope)
        guard isCurrent(generation: token, scope: scope) else { return }

        let hasOwedWork = !states.isEmpty
        let pollProjection: [String: Any]
        do {
            pollProjection = try await pollState()
        } catch {
            guard isCurrent(generation: token, scope: scope) else { return }
            backoffUntilMs = now + IOS_AUTOMATION_RELAY_ERROR_BACKOFF_MS
            return
        }
        guard isCurrent(generation: token, scope: scope) else { return }
        let isReady = pollProjection["isReady"] as? Bool ?? false
        let hasServerSchedulable = pollProjection["hasServerSchedulable"] as? Bool ?? false
        guard isReady else { return }
        if !hasOwedWork && !hasServerSchedulable && drainedWhileUnschedulable {
            return
        }

        let ackRunUuids = states.compactMap { key, value in
            value.phase == "reported" ? key : nil
        }.sorted()
        do {
            let response = try await poll(ackRunUuids)
            guard isCurrent(generation: token, scope: scope) else { return }
            let events = response["events"] as? [Any] ?? []
            let returned = Set(events.compactMap { ($0 as? [String: Any])?["id"] as? String })
            states = states.filter { key, value in
                value.phase != "reported" || returned.contains(key)
            }

            for raw in events {
                guard let fire = raw as? [String: Any],
                      let runUuid = (fire["id"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines),
                      !runUuid.isEmpty,
                      states[runUuid] == nil
                else { continue }
                let admission = try await admit(fire)
                guard isCurrent(generation: token, scope: scope) else { return }
                switch admission["disposition"] as? String {
                case "admitted", "existingRunning":
                    states[runUuid] = .init(phase: "running", status: nil, errorMessage: nil)
                case "complete":
                    let completionStatus = admission["status"] as? String ?? "failed"
                    let errorMessage = admission["errorMessage"] as? String
                    states[runUuid] = .init(
                        phase: "completed",
                        status: completionStatus,
                        errorMessage: errorMessage
                    )
                    await retryCompletion(runUuid, generation: token, scope: scope)
                case "abandon":
                    break
                default:
                    break
                }
            }

            await refreshRunningAndRetryCompletions(generation: token, scope: scope)
            let nextPollAfter = min(
                max((response["nextPollAfterMs"] as? NSNumber)?.int64Value ?? 0, 0),
                60_000
            )
            pollNotBeforeMs = states.isEmpty && nextPollAfter > 0
                ? now + nextPollAfter
                : 0
            if !hasOwedWork {
                drainedWhileUnschedulable = !hasServerSchedulable
            }
        } catch {
            guard isCurrent(generation: token, scope: scope) else { return }
            backoffUntilMs = now + IOS_AUTOMATION_RELAY_ERROR_BACKOFF_MS
        }
    }

    private func refreshRunningAndRetryCompletions(generation token: UInt64, scope: String) async {
        let running = states.compactMap { key, value in
            value.phase == "running" ? key : nil
        }
        for runUuid in running {
            guard isCurrent(generation: token, scope: scope) else { return }
            do {
                let projection = try await status(runUuid)
                guard isCurrent(generation: token, scope: scope) else { return }
                if projection["state"] as? String == "completed" {
                    states[runUuid] = .init(
                        phase: "completed",
                        status: projection["status"] as? String ?? "failed",
                        errorMessage: projection["errorMessage"] as? String
                    )
                } else if projection["state"] as? String == "missing" {
                    states.removeValue(forKey: runUuid)
                }
            } catch {
                continue
            }
        }
        let completed = states.compactMap { key, value in
            value.phase == "completed" ? key : nil
        }
        for runUuid in completed {
            await retryCompletion(runUuid, generation: token, scope: scope)
        }
    }

    private func retryCompletion(_ runUuid: String, generation token: UInt64, scope: String) async {
        guard isCurrent(generation: token, scope: scope),
              let state = states[runUuid],
              state.phase == "completed",
              let completionStatus = state.status
        else { return }
        do {
            try await complete(runUuid, completionStatus, state.errorMessage)
            guard isCurrent(generation: token, scope: scope) else { return }
            states[runUuid] = .init(phase: "reported", status: completionStatus, errorMessage: state.errorMessage)
        } catch {
            return
        }
    }

    private func isCurrent(generation token: UInt64, scope: String) -> Bool {
        !suspended && generation == token && accountScope == scope
    }
}

@MainActor
final class IOSAutomationCloudSyncLifecycle {
    typealias Snapshot = @MainActor () async throws -> [String: Any]
    typealias Plan = @MainActor (String, String) async throws -> [String: Any]
    typealias Commit = @MainActor (String, String) async throws -> [String: Any]
    typealias MarkFailed = @MainActor (String) async throws -> Void
    typealias Unary = @MainActor (String, String) async throws -> String

    private let snapshot: Snapshot
    private let plan: Plan
    private let commit: Commit
    private let markFailed: MarkFailed
    private let unary: Unary
    private let nowMilliseconds: () -> Int64
    private let launchBackgroundTask: Bool

    private(set) var accountScope: String?
    private(set) var generation: UInt64 = 0
    private(set) var suspended = true
    private(set) var backoffUntilMs: Int64 = 0
    private var task: Task<Void, Never>?
    private var sleepTask: Task<Void, Never>?
    private var wakeRequested = false

    init(
        snapshot: @escaping Snapshot,
        plan: @escaping Plan,
        commit: @escaping Commit,
        markFailed: @escaping MarkFailed,
        unary: @escaping Unary,
        nowMilliseconds: @escaping () -> Int64 = {
            Int64(Date().timeIntervalSince1970 * 1_000)
        },
        launchBackgroundTask: Bool = true
    ) {
        self.snapshot = snapshot
        self.plan = plan
        self.commit = commit
        self.markFailed = markFailed
        self.unary = unary
        self.nowMilliseconds = nowMilliseconds
        self.launchBackgroundTask = launchBackgroundTask
    }

    func updateAccountScope(_ rawScope: String?) {
        let normalized = rawScope?.trimmingCharacters(in: .whitespacesAndNewlines)
        let next = normalized?.isEmpty == false ? normalized : nil
        guard next != accountScope else { return }
        generation = generation == UInt64.max ? 1 : generation + 1
        task?.cancel()
        sleepTask?.cancel()
        task = nil
        sleepTask = nil
        wakeRequested = false
        accountScope = next
        backoffUntilMs = 0
        if !suspended { launchTask() }
    }

    func start() {
        guard accountScope != nil else { return }
        suspended = false
        reconcile()
        launchTask()
    }

    func suspend() {
        guard !suspended else { return }
        suspended = true
        generation = generation == UInt64.max ? 1 : generation + 1
        task?.cancel()
        sleepTask?.cancel()
        task = nil
        sleepTask = nil
        wakeRequested = false
    }

    func resume() {
        guard accountScope != nil else { return }
        if suspended {
            suspended = false
            generation = generation == UInt64.max ? 1 : generation + 1
        }
        reconcile()
        launchTask()
    }

    func stop() {
        suspended = true
        generation = generation == UInt64.max ? 1 : generation + 1
        task?.cancel()
        sleepTask?.cancel()
        task = nil
        sleepTask = nil
        wakeRequested = false
        accountScope = nil
        backoffUntilMs = 0
    }

    func requestSync() {
        backoffUntilMs = 0
        wakeRequested = true
        sleepTask?.cancel()
    }

    func reconcile() {
        requestSync()
    }

    func runOneCycleForTesting() async {
        guard let scope = accountScope, !suspended else { return }
        await runCycle(generation: generation, scope: scope)
    }

    private func launchTask() {
        guard launchBackgroundTask, !suspended, let scope = accountScope, task == nil else {
            return
        }
        let token = generation
        task = Task { @MainActor [weak self] in
            guard let self else { return }
            while !Task.isCancelled && self.isCurrent(generation: token, scope: scope) {
                self.wakeRequested = false
                await self.runCycle(generation: token, scope: scope)
                guard !Task.isCancelled && self.isCurrent(generation: token, scope: scope) else {
                    return
                }
                if self.wakeRequested { continue }
                let sleeper = Task<Void, Never> {
                    try? await Task.sleep(nanoseconds: IOS_AUTOMATION_CLOUD_SYNC_INTERVAL_NS)
                }
                self.sleepTask = sleeper
                await sleeper.value
                self.sleepTask = nil
            }
        }
    }

    private func runCycle(generation token: UInt64, scope: String) async {
        guard isCurrent(generation: token, scope: scope) else { return }
        let now = nowMilliseconds()
        guard now >= backoffUntilMs else { return }
        do {
            let snapshotValue = try await snapshot()
            guard isCurrent(generation: token, scope: scope) else { return }
            let agents = snapshotValue["agents"] as? [[String: Any]] ?? []
            for row in agents {
                guard isCurrent(generation: token, scope: scope) else { return }
                guard let agentId = row["agentId"] as? String,
                      !agentId.isEmpty,
                      let listRequestBase64 = row["listRequestBase64"] as? String,
                      !listRequestBase64.isEmpty
                else {
                    throw SandMcpConfigError("Automation cloud sync snapshot row is invalid.")
                }
                do {
                    let before = try await unary("ListSandAutomations", listRequestBase64)
                    guard isCurrent(generation: token, scope: scope) else { return }
                    let planValue = try await plan(agentId, before)
                    guard isCurrent(generation: token, scope: scope) else { return }
                    if planValue["converged"] as? Bool != true {
                        let mutations = planValue["mutations"] as? [[String: Any]] ?? []
                        for mutation in mutations {
                            guard let method = mutation["method"] as? String,
                                  let bodyBase64 = mutation["bodyBase64"] as? String,
                                  !bodyBase64.isEmpty
                            else {
                                throw SandMcpConfigError("Automation cloud sync mutation is invalid.")
                            }
                            _ = try await unary(method, bodyBase64)
                            guard isCurrent(generation: token, scope: scope) else { return }
                        }
                        let after = try await unary("ListSandAutomations", listRequestBase64)
                        guard isCurrent(generation: token, scope: scope) else { return }
                        let committed = try await commit(agentId, after)
                        guard isCurrent(generation: token, scope: scope) else { return }
                        guard committed["converged"] as? Bool == true else {
                            throw SandMcpConfigError("Automation cloud sync readback did not converge.")
                        }
                    }
                } catch {
                    if isCurrent(generation: token, scope: scope) {
                        try? await markFailed(agentId)
                    }
                    throw error
                }
            }
            guard isCurrent(generation: token, scope: scope) else { return }
            backoffUntilMs = 0
        } catch {
            guard isCurrent(generation: token, scope: scope) else { return }
            backoffUntilMs = nowMilliseconds() + IOS_AUTOMATION_RELAY_ERROR_BACKOFF_MS
        }
    }

    private func isCurrent(generation token: UInt64, scope: String) -> Bool {
        !suspended && generation == token && accountScope == scope
    }
}

@MainActor
final class CoordinatorMcpSurface {
    private let manager: SandMcpManager
    private let port: CoordinatorMcpHostPort
    private let cursorAuth: IOSCursorAuthService
    private let mainRpcCursorAuth: IOSCursorAuthMainRpcCompatibility
    private let dashboard: IOSCursorDashboardClient
    private let automationRelay: IOSAutomationRelayLifecycle
    private let automationFire: IOSAutomationFireLifecycle
    private let automationCloudSync: IOSAutomationCloudSyncLifecycle
    private let sandAccessReader: IOSCursorSandAccessReader
    private let feedbackClient: IOSFeedbackReportClient
    private let avatarImageGenerator: CursorGenerateImageService
    private let localToolPermissionCeilingSynchronizer: IOSCursorLocalToolPermissionCeilingSynchronizer
    private let gatewayDescriptorCache: IOSGatewayDescriptorKeychainCache
    private let gatewayDescriptorFailureReporter: IOSGatewayDescriptorFastPath.FailureReporter
    private let reportMigrationWatchTelemetry: @MainActor (_ phase: String, _ consecutiveFailures: Int, _ errorClass: String?) -> Void
    private let isClientPaused: @MainActor () -> Bool
    private var experimentAuthObserver: ((IOSCursorAuthStatus) -> Void)?
    private var sentryAuthObserver: ((IOSCursorAuthStatus) -> Void)?
    private var computerMigrationWatchTask: Task<Void, Never>?
    private var computerMigrationStallTask: Task<Void, Never>?
    private var computerMigrationStallBudget = IOSComputerMigrationStallBudget()
    private var computerMigrationStallArmed = false
    private var computerMigrationConsecutiveFailures = 0
    private var computerMigrationResumeOffsetKey = ""
    private var computerMigrationStatus: [String: Any]?
    private var computerMigrationLastTerminal: [String: Any]?
    private var computerMigrationOwedOperationId: String?
    private var computerMigrationGeneration: UInt64 = 0
    private var accountScope: String?
    private var routedScopeGeneration: UInt64 = 0

    private var retainedGatewayDescriptorFastPath: IOSGatewayDescriptorFastPath?

    private var gatewayDescriptorFastPath: IOSGatewayDescriptorFastPath {
        if let retainedGatewayDescriptorFastPath {
            return retainedGatewayDescriptorFastPath
        }
        let fastPath = IOSGatewayDescriptorFastPath(
            cache: gatewayDescriptorCache,
            getAccountScope: { [weak self] in self?.accountScope },
            reportFailure: gatewayDescriptorFailureReporter,
            isBrokerDenial: { error in
                if let broker = error as? IOSCursorSandBoxBrokerError {
                    return broker.deniesCachedDescriptor
                }
                let message = (error as NSError).localizedDescription
                return message.contains(GATEWAY_NO_STORAGE_MESSAGE_MARKER)
                    || message.contains(GATEWAY_ACCESS_DENIED_MESSAGE_MARKER)
            },
            nowMilliseconds: {
                Int64((Date().timeIntervalSince1970 * 1_000).rounded())
            },
            resolveLive: { [weak self] in
                guard let self else { throw IOSRemoteRunnerFenceError.unavailable }
                return try await self.dashboard.ensureSandBox()
            }
        )
        retainedGatewayDescriptorFastPath = fastPath
        return fastPath
    }

    private lazy var routedMcpBridge: RoutedMCPBridge = {
        let manager = self.manager
        return RoutedMCPBridge(
            listTools: {
                let tools = try await manager.getTools()
                return tools.map { tool in
                    RoutedMCPTool(
                        name: tool.name,
                        providerIdentifier: tool.providerIdentifier,
                        toolName: tool.toolName,
                        description: tool.description,
                        inputSchema: tool.inputSchema.map(routedCoordinatorPayload)
                    )
                }
            },
            currentScope: { [weak self] in
                await self?.routedScopeToken()
            },
            callTool: { tool, arguments, toolCallId, agentId in
                routedCoordinatorPayload(
                    await manager.executeTool(
                        .init(
                            providerIdentifier: tool.providerIdentifier,
                            name: tool.toolName,
                            args: McpJSONValue.from(arguments.foundationValue),
                            toolCallId: toolCallId
                        ),
                        auditIdentity: .init(agentId: agentId)
                    )
                )
            }
        )
    }()

    private init(
        manager: SandMcpManager,
        port: CoordinatorMcpHostPort,
        cursorAuth: IOSCursorAuthService,
        mainRpcCursorAuth: IOSCursorAuthMainRpcCompatibility,
        dashboard: IOSCursorDashboardClient,
        automationRelay: IOSAutomationRelayLifecycle,
        automationFire: IOSAutomationFireLifecycle,
        automationCloudSync: IOSAutomationCloudSyncLifecycle,
        sandAccessReader: IOSCursorSandAccessReader,
        feedbackClient: IOSFeedbackReportClient,
        avatarImageGenerator: CursorGenerateImageService,
        localToolPermissionCeilingSynchronizer: IOSCursorLocalToolPermissionCeilingSynchronizer,
        gatewayDescriptorCache: IOSGatewayDescriptorKeychainCache,
        gatewayDescriptorFailureReporter: @escaping IOSGatewayDescriptorFastPath.FailureReporter,
        reportMigrationWatchTelemetry: @escaping @MainActor (_ phase: String, _ consecutiveFailures: Int, _ errorClass: String?) -> Void,
        isClientPaused: @escaping @MainActor () -> Bool
    ) {
        self.manager = manager
        self.port = port
        self.cursorAuth = cursorAuth
        self.mainRpcCursorAuth = mainRpcCursorAuth
        self.dashboard = dashboard
        self.automationRelay = automationRelay
        self.automationFire = automationFire
        self.automationCloudSync = automationCloudSync
        self.sandAccessReader = sandAccessReader
        self.feedbackClient = feedbackClient
        self.avatarImageGenerator = avatarImageGenerator
        self.localToolPermissionCeilingSynchronizer = localToolPermissionCeilingSynchronizer
        self.gatewayDescriptorCache = gatewayDescriptorCache
        self.gatewayDescriptorFailureReporter = gatewayDescriptorFailureReporter
        self.reportMigrationWatchTelemetry = reportMigrationWatchTelemetry
        self.isClientPaused = isClientPaused
    }

    func dispatchRemoteExecution(
        method: String,
        params: CoordinatorPayload
    ) async throws -> CoordinatorPayload {
        try refuseComputerConnectionWhileClientPaused()
        guard let token = routedScopeToken() else {
            throw IOSRemoteRunnerFenceError.unavailable
        }
        try Task.checkCancellation()
        let descriptor = try await gatewayDescriptorFastPath.resolve()
        try Task.checkCancellation()
        guard routedScopeToken() == token else {
            throw IOSRemoteRunnerFenceError.stale
        }
        let result = try await IOSRemoteExecDaemonClient(
            descriptor: descriptor
        ).dispatch(
            method: method,
            params: params
        )
        try Task.checkCancellation()
        guard routedScopeToken() == token else {
            throw IOSRemoteRunnerFenceError.stale
        }
        return result
    }

    func authorizeGatewayDescriptorAccount(_ accountScope: String) async -> Bool {
        await gatewayDescriptorCache.authorizeExistingDescriptor(accountScope: accountScope)
    }

    func structuredLogCredentials(
        backendURL: String,
        accountSlot: String
    ) async throws -> (accessToken: String, machineID: String) {
        let accessToken = try await cursorAuth.getValidAccessToken(backendURL: backendURL)
        let token = parseJwtPayload(accessToken)
        guard (token?.sub ?? token?.email) == accountSlot else {
            throw SandMcpConfigError("Structured-log account scope is stale.")
        }
        return (
            accessToken: accessToken,
            machineID: try await cursorAuth.getMachineID()
        )
    }

    func revokeCursorAuthForAccountRefusal() async -> IOSCursorAccountRefusalRevocation {
        stopComputerMigrationWatch(clearState: true)
        // Keep the durable gateway descriptor owner across refusal. Clearing
        // it here would let a foreign account retry and claim the same native
        // box after the first refusal.
        let result = await cursorAuth.revokeForAccountRefusal()
        await manager.reload()
        return result
    }

    static func make(
        hostSupervisor: MahayanaLocalHostSupervisor,
        settingsStore: SandSettingsStore,
        reportFailure: @escaping IOSCursorLocalToolPermissionCeilingSynchronizer.ReportFailure = { _, _, _ in },
        reportAuthTelemetry: @escaping IOSAuthTelemetryRelay.Sink = { _ in },
        reportMigrationWatchTelemetry: @escaping @MainActor (_ phase: String, _ consecutiveFailures: Int, _ errorClass: String?) -> Void = { _, _, _ in },
        backendUpdateRequirementSink: @escaping IOSCursorDashboardClient.BackendUpdateRequirementSink = { _ in },
        isClientPaused: @escaping @MainActor () -> Bool = { false }
    ) -> CoordinatorMcpSurface {
        let port = CoordinatorMcpHostPort(
            hostSupervisor: hostSupervisor,
            settingsStore: settingsStore
        )
        let authTelemetry = IOSAuthTelemetryRelay()
        authTelemetry.attach(reportAuthTelemetry)
        let cursorAuth = IOSCursorAuthService(authTelemetry: authTelemetry)
        let credentials = AccountMcpCredentials(
            getAccessToken: { backendURL in
                try await cursorAuth.getValidAccessToken(backendURL: backendURL)
            },
            getMachineId: {
                try await cursorAuth.getMachineID()
            }
        )
        let dashboard = IOSCursorDashboardClient(
            credentials: credentials,
            backendUpdateRequirementSink: backendUpdateRequirementSink
        )
        let gatewayDescriptorFailureReporter: IOSGatewayDescriptorFastPath.FailureReporter = {
            operation, error in reportFailure("gateway-descriptor", operation, error)
        }
        let gatewayDescriptorCache = IOSGatewayDescriptorKeychainCache(
            reportFailure: { operation, error in
                reportFailure("gateway-descriptor", "cache-\(operation)", error)
            }
        )
        cursorAuth.setProfileFetcher { [weak dashboard] in
            await dashboard?.getCursorAuthProfile()
        }
        let sandAccessReader = IOSCursorSandAccessReader(
            getAuthState: {
                await cursorAuth.sandAccessAuthState()
            },
            readAccess: {
                try await dashboard.getSandAccessStatus()
            }
        )
        let feedbackClient = IOSFeedbackReportClient(
            getAccessToken: { backendURL in
                try await cursorAuth.getValidAccessToken(backendURL: backendURL)
            },
            getMachineID: { try await cursorAuth.getMachineID() },
            getBackendURL: { getConfiguredBackendUrl() }
        )
        let localToolPermissionCeilingSynchronizer = IOSCursorLocalToolPermissionCeilingSynchronizer(
            settingsStore: settingsStore,
            fetchCeiling: { try await dashboard.getLocalToolPermissionCeiling() },
            syncHost: { permission in
                _ = try await hostSupervisor.request(
                    method: "feature.settings.localToolPermission",
                    params: ["permission": permission]
                )
            },
            reportFailure: reportFailure
        )
        let avatarImageGenerator = createCursorGenerateImageService(
            client: IOSCursorGenerateImageClient(credentials: credentials),
            modelId: SAND_DEFAULT_MODEL_ID
        )
        let accountDependencies = AccountMcpDependencies(
            getAccessToken: { backendURL in
                try await cursorAuth.getValidAccessToken(backendURL: backendURL)
            },
            getMachineId: {
                try await cursorAuth.getMachineID()
            },
            getBackendUrl: { getConfiguredBackendUrl() },
            createClient: { credentials in
                IOSCursorDashboardClient(credentials: credentials)
            }
        )
        let client = CoordinatorMcpBackendClient(port: port, dashboard: dashboard)
        let backend = DashboardSandBackendMcpExec(deps: .init(client: client))
        let definitionSource = SandMcpDefinitionSource(includeBuiltins: false)
        let discovery = SandMcpToolsDiscovery(deps: .init(
            definitionSource: definitionSource,
            settingsStore: settingsStore,
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
        let accountWriter = createAccountMcpWriter(accountDependencies)
        let catalog = SandMcpCatalogFlow(core: .init(
            isAuthenticated: {
                await cursorAuth.status().loggedIn
            },
            fetchMarketplace: {
                try await fetchMarketplaceMcpPlugins(deps: .init(
                    bestEffortToken: {
                        await bestEffortToken {
                            try await cursorAuth.getValidAccessToken(
                                backendURL: getConfiguredBackendUrl()
                            )
                        }
                    },
                    createClient: { dashboard },
                    rememberPluginLogoUrl: rememberPluginLogoUrl
                ))
            },
            listEffectivePlugins: {
                try await fetchEffectiveUserPlugins(accountDependencies)
            },
            requireAccountWriter: {
                accountWriter
            },
            // SandMcpManager performs the authoritative reload immediately after
            // catalog install/update. Keep this callback side-effect free so the
            // catalog never creates a second manager lifecycle.
            reloadServers: {}
        ))
        var dependencies = SandMcpManagerDependencies(
            settingsStore: settingsStore,
            backendMcpExec: backend,
            definitionSource: definitionSource,
            toolsDiscovery: discovery,
            accountDisplayConfigProvider: { requireFreshRead in
                if await cursorAuth.status().loggedIn,
                   let fetched = await fetchAccountMcpServers(accountDependencies),
                   !fetched.unavailable {
                    let display = accountDisplayConfig(from: fetched)
                    await port.updateDashboardServerIdentifiers(
                        Set(display.servers.compactMap(\.serverIdentifier))
                    )
                    return display
                }
                await port.updateDashboardServerIdentifiers([])
                return try await port.loadAccountDisplay(forceFresh: requireFreshRead)
            },
            accountMcpWriter: accountWriter,
            catalog: catalog,
            autoPollEnabled: true
        )
        dependencies.setToolDisabled = { identifier, tool, disabled in
            try await port.setToolDisabled(
                serverIdentifier: identifier,
                toolName: tool,
                disabled: disabled
            )
        }
        dependencies.effectivePluginsProvider = {
            try await fetchEffectiveUserPlugins(accountDependencies)
        }
        let manager = SandMcpManager(deps: dependencies)
        let mainRpcCursorAuth = IOSCursorAuthMainRpcCompatibility(
            dependencies: .init(
                readStatus: { await cursorAuth.status() },
                isLoginPending: { cursorAuth.hasPendingLogin() },
                beginLogin: { try cursorAuth.beginLogin() },
                pollLogin: { attemptId in try await cursorAuth.pollLogin(attemptId: attemptId) },
                cancelPendingLogins: { cursorAuth.cancelPendingLogins() },
                logout: { try await cursorAuth.logout() },
                openExternal: { url in
#if canImport(UIKit)
                    let opened = await UIApplication.shared.open(url)
                    guard opened else {
                        throw SandMcpConfigError("Unable to open the Cursor sign-in page.")
                    }
#else
                    throw SandMcpConfigError("Cursor sign-in requires iOS URL presentation.")
#endif
                },
                reloadAuthoritativeMcp: { _ = try await manager.reloadServers() },
                invalidateMcp: { await manager.reload() },
                sleepBeforePoll: { try await Task.sleep(for: .seconds(1)) }
            )
        )
        let automationRelay = IOSAutomationRelayLifecycle(
            loadSubscription: {
                try await port.automationRelaySubscription()
            },
            register: { subscription in
                try await dashboard.registerAutomationRelay(
                    slackChannels: subscription.slackChannels,
                    githubRepos: subscription.githubRepos,
                    githubKinds: subscription.githubKinds
                )
            },
            drain: { ackIds in
                try await dashboard.drainAutomationRelayEvents(ackIds: ackIds)
            },
            sink: { event in
                try await port.ingestAutomationRelayEvent(event)
            },
            pollReconnects: {
                try await port.pollAutomationListenerReconnects()
            }
        )
        let automationFire = IOSAutomationFireLifecycle(
            pollState: {
                try await port.backendAutomationFirePollState()
            },
            poll: { ackRunUuids in
                try await dashboard.pollAutomationFires(ackRunUuids: ackRunUuids)
            },
            complete: { runUuid, status, errorMessage in
                try await dashboard.completeAutomationRun(
                    runUuid: runUuid,
                    status: status,
                    errorMessage: errorMessage
                )
            },
            admit: { fire in
                try await port.admitBackendAutomationFire(fire)
            },
            status: { runUuid in
                try await port.backendAutomationFireStatus(runUuid)
            }
        )
        let automationCloudSync = IOSAutomationCloudSyncLifecycle(
            snapshot: {
                try await port.automationCloudSyncSnapshot()
            },
            plan: { agentId, responseBase64 in
                try await port.automationCloudSyncPlan(
                    agentId: agentId,
                    responseBase64: responseBase64
                )
            },
            commit: { agentId, responseBase64 in
                try await port.automationCloudSyncCommit(
                    agentId: agentId,
                    responseBase64: responseBase64
                )
            },
            markFailed: { agentId in
                try await port.markAutomationCloudSyncFailed(agentId: agentId)
            },
            unary: { method, bodyBase64 in
                try await dashboard.automationServiceUnary(
                    method: method,
                    bodyBase64: bodyBase64
                )
            }
        )
        let surface = CoordinatorMcpSurface(
            manager: manager,
            port: port,
            cursorAuth: cursorAuth,
            mainRpcCursorAuth: mainRpcCursorAuth,
            dashboard: dashboard,
            automationRelay: automationRelay,
            automationFire: automationFire,
            automationCloudSync: automationCloudSync,
            sandAccessReader: sandAccessReader,
            feedbackClient: feedbackClient,
            avatarImageGenerator: avatarImageGenerator,
            localToolPermissionCeilingSynchronizer: localToolPermissionCeilingSynchronizer,
            gatewayDescriptorCache: gatewayDescriptorCache,
            gatewayDescriptorFailureReporter: gatewayDescriptorFailureReporter,
            reportMigrationWatchTelemetry: reportMigrationWatchTelemetry,
            isClientPaused: isClientPaused
        )
        cursorAuth.setStatusObserver { [weak surface] status in
            surface?.consumeCursorAuthStatus(status)
        }
        return surface
    }

    private func consumeCursorAuthStatus(_ status: IOSCursorAuthStatus) {
        localToolPermissionCeilingSynchronizer.consume(status)
        experimentAuthObserver?(status)
        sentryAuthObserver?(status)
    }

    func setExperimentAuthObserver(_ observer: ((IOSCursorAuthStatus) -> Void)?) {
        experimentAuthObserver = observer
        guard observer != nil else { return }
        Task { @MainActor [weak self] in
            guard let self, let observer = self.experimentAuthObserver else { return }
            observer(await self.cursorAuth.status())
        }
    }

    func setSentryAuthObserver(_ observer: ((IOSCursorAuthStatus) -> Void)?) {
        sentryAuthObserver = observer
        guard observer != nil else { return }
        Task { @MainActor [weak self] in
            guard let self, let observer = self.sentryAuthObserver else { return }
            observer(await self.cursorAuth.status())
        }
    }

    func currentSentryPrivacyMode() async -> SandPrivacyMode? {
        let status = await cursorAuth.status()
        guard status.loggedIn else { return nil }
        return await dashboard.getCursorPrivacyMode()
    }


    func classifyAutoReview(_ params: [String: Any]) async throws -> [String: Any] {
        guard let scopeToken = routedScopeToken(),
              (await cursorAuth.status()).loggedIn
        else {
            throw SandMcpConfigError("Auto-review classifier requires an authenticated Cursor account.")
        }
        guard let rawAction = params["action"] as? String else {
            throw SandMcpConfigError("Auto-review classifier requires an action.")
        }
        let action = rawAction.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !action.isEmpty, action.count <= 2_000 else {
            throw SandMcpConfigError("Auto-review classifier action is empty or too large.")
        }

        let parentConversationId = (params["parentConversationId"] as? String)?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        if let parentConversationId, parentConversationId.count > 512 {
            throw SandMcpConfigError("Auto-review classifier conversation identity is too large.")
        }
        let mode = ((params["mode"] as? String) ?? "enforce")
            .trimmingCharacters(in: .whitespacesAndNewlines)
        guard !mode.isEmpty, mode.count <= 32 else {
            throw SandMcpConfigError("Auto-review classifier mode is invalid.")
        }
        let attemptIndex: UInt32 = {
            guard let value = params["attemptIndex"] as? NSNumber,
                  !(params["attemptIndex"] is Bool),
                  value.int64Value >= 0,
                  value.int64Value <= Int64(UInt32.max)
            else { return 0 }
            return UInt32(value.int64Value)
        }()

        let rawArguments = params["arguments"] as? [String: Any] ?? [:]
        guard rawArguments.count <= 64 else {
            throw SandMcpConfigError("Auto-review classifier arguments exceed the bounded field count.")
        }
        var arguments: [String: IOSCursorAutoReviewJSONValue] = [:]
        for (key, value) in rawArguments {
            guard !key.isEmpty, key.count <= 256 else {
                throw SandMcpConfigError("Auto-review classifier argument key is invalid.")
            }
            arguments[key] = try autoReviewJSONValue(value, depth: 0)
        }

        let rawContext = params["conversationContext"] as? [[String: Any]] ?? []
        guard rawContext.count <= 32 else {
            throw SandMcpConfigError("Auto-review classifier context exceeds the bounded message count.")
        }
        let context = try rawContext.map { row -> IOSCursorAutoReviewConversationMessage in
            guard let rawRole = row["role"] as? String,
                  let rawContent = row["content"] as? String
            else {
                throw SandMcpConfigError("Auto-review classifier context row is invalid.")
            }
            let role = rawRole.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !role.isEmpty, role.count <= 32, rawContent.count <= 8_000 else {
                throw SandMcpConfigError("Auto-review classifier context row exceeds its bounds.")
            }
            return .init(role: role, content: rawContent)
        }

        let request = IOSCursorAutoReviewClassifierRequest(
            parentConversationId: parentConversationId?.isEmpty == false ? parentConversationId : nil,
            action: action,
            arguments: arguments,
            conversationContext: context,
            attemptIndex: attemptIndex,
            mode: mode
        )
        let started = Date()
        let result: IOSCursorAutoReviewClassifierResult
        do {
            try Task.checkCancellation()
            result = try await dashboard.classifySandAutoReview(request, timeoutMs: 10_000)
            try Task.checkCancellation()
        } catch is CancellationError {
            throw CancellationError()
        } catch let error as URLError where error.code == .timedOut {
            return [
                "kind": "error",
                "message": "Auto-review classifier timed out.",
                "failureReason": "timeout",
                "retryable": true,
                "durationMs": max(0, Int(Date().timeIntervalSince(started) * 1_000)),
            ]
        } catch {
            return [
                "kind": "error",
                "message": String(error.localizedDescription.prefix(1_000)),
                "failureReason": "transport",
                "retryable": true,
                "durationMs": max(0, Int(Date().timeIntervalSince(started) * 1_000)),
            ]
        }
        guard routedScopeToken() == scopeToken else {
            throw SandMcpConfigError("Auto-review classifier result became stale because the account changed.")
        }

        let durationMs = max(0, Int(Date().timeIntervalSince(started) * 1_000))
        switch result {
        case .missing:
            return ["kind": "missing", "durationMs": durationMs]
        case .backendError(let message, let failureReason, let retryable):
            var value: [String: Any] = [
                "kind": "error",
                "message": String(message.prefix(1_000)),
                "durationMs": durationMs,
            ]
            if let failureReason { value["failureReason"] = failureReason }
            if let retryable { value["retryable"] = retryable }
            return value
        case .success(let success):
            let kind: String
            switch success.decision {
            case .allow: kind = "allow"
            case .block: kind = "block"
            case .unspecified: kind = "unspecified"
            case .unknown(let raw): kind = "unknown:\(raw)"
            }
            var value: [String: Any] = ["kind": kind, "durationMs": durationMs]
            if let blockReason = success.blockReason {
                value["blockReason"] = String(blockReason.prefix(1_000))
            }
            if let proposedAllowRule = success.proposedAllowRule {
                value["proposedAllowRule"] = String(proposedAllowRule.prefix(2_000))
            }
            return value
        }
    }

    private func autoReviewJSONValue(
        _ raw: Any,
        depth: Int
    ) throws -> IOSCursorAutoReviewJSONValue {
        guard depth <= 6 else {
            throw SandMcpConfigError("Auto-review classifier arguments exceed the maximum nesting depth.")
        }
        if raw is NSNull { return .null }
        if let value = raw as? Bool { return .bool(value) }
        if let value = raw as? String {
            guard value.count <= 8_000 else {
                throw SandMcpConfigError("Auto-review classifier string argument is too large.")
            }
            return .string(value)
        }
        if let value = raw as? NSNumber {
            let number = value.doubleValue
            guard number.isFinite else {
                throw SandMcpConfigError("Auto-review classifier numeric argument is not finite.")
            }
            return .number(number)
        }
        if let value = raw as? [String: Any] {
            guard value.count <= 64 else {
                throw SandMcpConfigError("Auto-review classifier object argument is too large.")
            }
            var result: [String: IOSCursorAutoReviewJSONValue] = [:]
            for (key, child) in value {
                guard !key.isEmpty, key.count <= 256 else {
                    throw SandMcpConfigError("Auto-review classifier object key is invalid.")
                }
                result[key] = try autoReviewJSONValue(child, depth: depth + 1)
            }
            return .object(result)
        }
        if let value = raw as? [Any] {
            guard value.count <= 64 else {
                throw SandMcpConfigError("Auto-review classifier array argument is too large.")
            }
            return .array(try value.map { try autoReviewJSONValue($0, depth: depth + 1) })
        }
        throw SandMcpConfigError("Auto-review classifier argument uses an unsupported value type.")
    }

    func productAnalyticsClient() -> any ProductAnalyticsClient {
        dashboard
    }

    func startAutomationRelay() {
        automationRelay.start()
        automationFire.start()
        automationCloudSync.start()
    }

    func suspendAutomationRelay() {
        automationRelay.suspend()
        automationFire.suspend()
        automationCloudSync.suspend()
    }

    func resumeAutomationRelay() {
        automationRelay.resume()
        automationFire.resume()
        automationCloudSync.resume()
    }

    func reconcileAutomationRelay() {
        automationRelay.reconcile()
        automationRelay.requestDrain()
        automationFire.reconcile()
        automationFire.requestDrain()
        automationCloudSync.reconcile()
        automationCloudSync.requestSync()
    }

    func requestAutomationRelayDrain() {
        automationRelay.requestDrain()
        automationFire.requestDrain()
        automationCloudSync.requestSync()
    }

    func hydrateAgentStoreForCurrentScope() async throws -> [String: Any] {
        guard let scope = accountScope else {
            throw SandMcpConfigError("Agent Store hydration requires an active account.")
        }
        let generation = routedScopeGeneration
        let result = try await port.hydrateAgentStore()
        guard scope == accountScope, generation == routedScopeGeneration else {
            throw SandMcpConfigError(
                "Agent Store hydration became stale because the account changed."
            )
        }
        return result
    }

    func snapshotAgentStoreForCurrentScope() async throws -> [String: Any] {
        guard let scope = accountScope else {
            throw SandMcpConfigError("Agent Store snapshot requires an active account.")
        }
        let generation = routedScopeGeneration
        let result = try await port.snapshotAgentStore()
        guard scope == accountScope, generation == routedScopeGeneration else {
            throw SandMcpConfigError(
                "Agent Store snapshot became stale because the account changed."
            )
        }
        return result
    }

    func stopAutomationRelay() {
        automationRelay.stop(clearPendingAcks: true)
        automationFire.stop()
        automationCloudSync.stop()
    }

    func experimentAccessToken(backendURL: String) async throws -> String {
        try await cursorAuth.getValidAccessToken(backendURL: backendURL)
    }

    func experimentMachineID() async throws -> String {
        try await cursorAuth.getMachineID()
    }

    func experimentIsAnysphereUser() async throws -> Bool {
        let status = await cursorAuth.status()
        guard status.loggedIn else { return false }
        return try await dashboard.isAnysphereUser()
    }

    func prepareAccountTransition() async {
        automationRelay.stop(clearPendingAcks: true)
        automationFire.stop()
        automationCloudSync.stop()
        stopComputerMigrationWatch(clearState: true)
        // Account-scoped in-memory/MCP state is reloaded, but the secure
        // descriptor envelope is intentionally retained as the native durable
        // account-binding authority.
        await manager.reload()
    }

    func updateAccountScope(_ scope: String?) {
        let normalized = scope?.trimmingCharacters(in: .whitespacesAndNewlines)
        let next = normalized?.isEmpty == false ? normalized : nil
        if next != accountScope {
            accountScope = next
            routedScopeGeneration = routedScopeGeneration == UInt64.max
                ? 1
                : routedScopeGeneration + 1
            stopComputerMigrationWatch(clearState: true)
        }
        port.updateAccountScope(next)
        automationRelay.updateAccountScope(next)
        automationFire.updateAccountScope(next)
        automationCloudSync.updateAccountScope(next)
        Task { @MainActor [weak self] in
            guard let self else { return }
            self.consumeCursorAuthStatus(await self.cursorAuth.status())
        }
    }

    private func routedScopeToken() -> RoutedMCPScopeToken? {
        guard let accountScope else { return nil }
        return .init(
            accountScope: accountScope,
            generation: routedScopeGeneration
        )
    }

    func dropObservedComputerConnectionForClientPause() {
        stopComputerMigrationWatch(clearState: true)
    }

    private func refuseComputerConnectionWhileClientPaused() throws {
        if isClientPaused() {
            dropObservedComputerConnectionForClientPause()
            throw IOSClientPausedError()
        }
    }

    private func stopComputerMigrationWatch(clearState: Bool) {
        computerMigrationGeneration &+= 1
        computerMigrationWatchTask?.cancel()
        computerMigrationStallTask?.cancel()
        computerMigrationWatchTask = nil
        computerMigrationStallTask = nil
        computerMigrationStallArmed = false
        computerMigrationStallBudget.reset()
        computerMigrationConsecutiveFailures = 0
        if clearState {
            computerMigrationResumeOffsetKey = ""
            computerMigrationStatus = nil
            computerMigrationLastTerminal = nil
            computerMigrationOwedOperationId = nil
        }
    }

    private func disarmComputerMigrationStallWatch(clearOwedOperation: Bool) {
        computerMigrationStallTask?.cancel()
        computerMigrationStallTask = nil
        computerMigrationStallArmed = false
        computerMigrationStallBudget.reset()
        if clearOwedOperation { computerMigrationOwedOperationId = nil }
    }

    private func scheduleComputerMigrationStallDeadline(generation: UInt64) {
        computerMigrationStallTask?.cancel()
        computerMigrationStallTask = Task { @MainActor [weak self] in
            do {
                try await Task.sleep(for: .seconds(IOS_COMPUTER_MIGRATION_STALL_SECONDS))
            } catch {
                return
            }
            guard let self,
                  self.computerMigrationGeneration == generation,
                  self.computerMigrationStallArmed,
                  self.computerMigrationWatchTask != nil
            else { return }
            guard self.computerMigrationStallBudget.consumeReattach() else {
                self.reportMigrationWatchTelemetry(
                    "watch_exhausted",
                    self.computerMigrationConsecutiveFailures,
                    nil
                )
                self.disarmComputerMigrationStallWatch(clearOwedOperation: true)
                return
            }
            self.reattachComputerMigrationWatchAfterStall(expectedGeneration: generation)
        }
    }

    private func armComputerMigrationStallWatch(
        operationId: String?,
        generation: UInt64
    ) {
        if !computerMigrationStallArmed {
            computerMigrationStallArmed = true
            computerMigrationStallBudget.reset()
        }
        if computerMigrationOwedOperationId == nil {
            computerMigrationOwedOperationId = operationId
        }
        scheduleComputerMigrationStallDeadline(generation: generation)
    }

    private func kickComputerMigrationStallWatch(generation: UInt64) {
        guard computerMigrationStallArmed else { return }
        scheduleComputerMigrationStallDeadline(generation: generation)
    }

    private func reattachComputerMigrationWatchAfterStall(expectedGeneration: UInt64) {
        guard computerMigrationGeneration == expectedGeneration,
              computerMigrationStallArmed
        else { return }
        computerMigrationWatchTask?.cancel()
        computerMigrationWatchTask = nil
        computerMigrationStallTask?.cancel()
        computerMigrationStallTask = nil
        startComputerMigrationWatch()
    }

    private func ensureComputerMigrationWatch() {
        guard computerMigrationWatchTask == nil else { return }
        startComputerMigrationWatch()
    }

    private func startComputerMigrationWatch() {
        guard computerMigrationWatchTask == nil else { return }
        computerMigrationGeneration &+= 1
        let generation = computerMigrationGeneration
        if computerMigrationStallArmed {
            scheduleComputerMigrationStallDeadline(generation: generation)
        }
        computerMigrationWatchTask = Task { @MainActor [weak self] in
            guard let self else { return }
            defer {
                if self.computerMigrationGeneration == generation {
                    self.computerMigrationWatchTask = nil
                }
            }
            while !Task.isCancelled, self.computerMigrationGeneration == generation {
                let fromOffset = self.computerMigrationResumeOffsetKey
                var received = false
                do {
                    for try await event in self.dashboard.watchSandBoxMigration(
                        fromOffsetKey: fromOffset,
                        includeFinished: true
                    ) {
                        guard !Task.isCancelled,
                              self.computerMigrationGeneration == generation
                        else { return }
                        received = true
                        if self.computerMigrationConsecutiveFailures > 0 {
                            self.computerMigrationConsecutiveFailures = 0
                            self.reportMigrationWatchTelemetry("attached", 0, nil)
                        }
                        self.ingestComputerMigration(event, generation: generation)
                    }
                } catch {
                    if !Task.isCancelled,
                       self.computerMigrationGeneration == generation {
                        self.computerMigrationConsecutiveFailures += 1
                        self.reportMigrationWatchTelemetry(
                            "dropped",
                            self.computerMigrationConsecutiveFailures,
                            String(describing: type(of: error))
                        )
                    }
                    if !received && !fromOffset.isEmpty {
                        self.computerMigrationResumeOffsetKey = ""
                    }
                }
                guard !Task.isCancelled,
                      self.computerMigrationGeneration == generation
                else { return }
                do {
                    try await Task.sleep(for: .seconds(3))
                } catch {
                    return
                }
                self.computerMigrationStatus = nil
            }
        }
    }

    private func ingestComputerMigration(
        _ event: IOSCursorSandBoxMigrationEvent,
        generation: UInt64
    ) {
        guard let phase = event.phaseName else { return }
        if !event.offsetKey.isEmpty {
            computerMigrationResumeOffsetKey = event.offsetKey
        }
        var projected: [String: Any] = [
            "phase": phase,
            "detail": event.detail,
        ]
        if !event.operationId.isEmpty {
            projected["operationId"] = event.operationId
        }
        computerMigrationStatus = projected
        if event.isTerminal {
            if let owed = computerMigrationOwedOperationId,
               !event.operationId.isEmpty,
               event.operationId != owed {
                kickComputerMigrationStallWatch(generation: generation)
                return
            }
            computerMigrationLastTerminal = projected
            disarmComputerMigrationStallWatch(clearOwedOperation: true)
        } else {
            armComputerMigrationStallWatch(
                operationId: event.operationId.isEmpty ? nil : event.operationId,
                generation: generation
            )
            if let terminalOperation = computerMigrationLastTerminal?["operationId"] as? String,
               !event.operationId.isEmpty,
               terminalOperation != event.operationId {
                computerMigrationLastTerminal = nil
            }
        }
    }

    private func projectRecreate(
        _ result: IOSCursorSandBoxRecreateResult
    ) -> [String: Any] {
        guard result.started else {
            return [
                "status": "rejected",
                "reason": result.reason.isEmpty
                    ? "Computer recreation was rejected by the backend."
                    : result.reason,
            ]
        }
        armComputerMigrationStallWatch(
            operationId: result.operationId.isEmpty ? nil : result.operationId,
            generation: computerMigrationGeneration
        )
        if !result.operationId.isEmpty {
            return [
                "status": "started",
                "operationId": result.operationId,
            ]
        }
        return ["status": "started-untrackable"]
    }

    private func requireCursorComputerLifecycle() async throws {
        guard (await cursorAuth.status()).loggedIn else {
            throw IOSCursorAuthError.signInRequired
        }
        ensureComputerMigrationWatch()
    }

    func cloudAgentInfo(bcId: String) async throws -> IOSCloudAgentComposerInfo {
        guard (await cursorAuth.status()).loggedIn else {
            throw IOSCursorAuthError.signInRequired
        }
        return try await dashboard.getBackgroundComposerInfo(bcId: bcId)
    }

    private func refreshAuthoritativePluginFacts(
        agentId: String
    ) async throws -> [[String: Any]] {
        guard (await cursorAuth.status()).loggedIn else {
            let empty: [[String: Any]] = []
            _ = try await port.syncWorkflowPluginFacts(agentId: agentId, plugins: empty)
            _ = try await port.syncInstalledPluginSkills(agentId: agentId, skills: empty)
            return empty
        }
        let scope = accountScope
        let generation = routedScopeGeneration
        let currentUserId = try await dashboard.getCurrentUserId()
        let effective = try await manager.listEffectivePlugins()
        let projected = projectAuthoritativePublishedPlugins(
            effective,
            currentUserId: currentUserId
        )
        _ = try await port.syncWorkflowPluginFacts(agentId: agentId, plugins: projected)

        let enabledByPluginId = Dictionary(
            uniqueKeysWithValues: effective
                .filter(\.isEnabled)
                .map { ($0.pluginId, $0) }
        )
        let catalog = try await manager.getCatalog(forceRefresh: false)
        var installedSkills: [[String: Any]] = []
        for plugin in catalog {
            guard installedSkills.count < 64,
                  let effectivePlugin = enabledByPluginId[plugin.id] else { continue }
            var usedSkillIds = Set<String>()
            for (index, skill) in plugin.skills.enumerated() {
                guard installedSkills.count < 64 else {
                    throw SandMcpConfigError(
                        "Installed plugin skills exceed the supported limit of 64; the previous Host snapshot was preserved."
                    )
                }
                let source = try requireInstalledMarketplaceSkillSource(
                    pluginId: plugin.id,
                    skillName: skill.name,
                    versionRef: effectivePlugin.versionRef,
                    sourceUrl: skill.sourceUrl
                )
                guard let markdown = await fetchMarketplaceSkillMarkdown(
                    sourceUrl: source.sourceUrl
                ) else {
                    throw SandMcpConfigError(
                        "Installed plugin \(plugin.id) skill \(skill.name) could not be materialized from its immutable source; the previous Host snapshot was preserved."
                    )
                }
                var skillId = marketplaceInstalledSkillId(skill.name)
                if skillId.isEmpty || usedSkillIds.contains(skillId) {
                    skillId = "skill-\(index + 1)"
                }
                usedSkillIds.insert(skillId)
                installedSkills.append([
                    "pluginId": plugin.id,
                    "pluginVersion": source.version,
                    "workflowId": "plugin-\(plugin.id)-\(skillId)",
                    "skillId": skillId,
                    "name": skill.name,
                    "description": skill.description,
                    "markdown": markdown,
                    "sourceRef": source.sourceUrl,
                    "publishedByCurrentUser": effectivePlugin.publisherUserId == currentUserId,
                    "isEnabledForAgent": true,
                ])
            }
        }
        guard scope == accountScope, generation == routedScopeGeneration else {
            throw SandMcpConfigError(
                "Installed plugin skill refresh became stale because the account changed."
            )
        }
        _ = try await port.syncInstalledPluginSkills(agentId: agentId, skills: installedSkills)
        return projected
    }

    private func marketplaceInstalledSkillId(_ raw: String) -> String {
        var output = ""
        var pendingDash = false
        for scalar in raw.lowercased().unicodeScalars {
            if CharacterSet.alphanumerics.contains(scalar), scalar.isASCII {
                if pendingDash, !output.isEmpty { output.append("-") }
                pendingDash = false
                if output.count < 40 { output.unicodeScalars.append(scalar) }
            } else {
                pendingDash = true
            }
            if output.count >= 40 { break }
        }
        return output.trimmingCharacters(in: CharacterSet(charactersIn: "-"))
    }

    private func publishTeamId(_ value: Any?) throws -> Int32 {
        guard let number = value as? NSNumber else {
            throw SandMcpConfigError("Skill publishing teamId is required.")
        }
        let raw = number.int64Value
        guard raw > 0, raw <= Int64(Int32.max) else {
            throw SandMcpConfigError("Skill publishing teamId is invalid.")
        }
        return Int32(raw)
    }

    private func uploadPrivateWorkflow(
        agentId: String,
        workflowId: String,
        teamId: Int32
    ) async throws -> IOSCursorPublishedSkill {
        let package = try await port.exportWorkflowPublishPackage(
            agentId: agentId,
            workflowId: workflowId
        )
        guard let name = nonEmptyString(package["name"]),
              let displayName = nonEmptyString(package["displayName"]),
              let description = nonEmptyString(package["description"]),
              let encoded = nonEmptyString(package["pluginTarGzBase64"]),
              let archive = Data(base64Encoded: encoded)
        else {
            throw SandMcpConfigError("Skill publish package is incomplete.")
        }
        return try await dashboard.publishSkillPlugin(
            teamId: teamId,
            name: name,
            displayName: displayName,
            description: description,
            pluginTarGz: archive
        )
    }

    func route(
        method: String,
        params: [String: Any]
    ) async throws -> CoordinatorDevControlRouting {
        switch method {
        case "submitFeedback":
            return .handled(await feedbackClient.submit(params))

        case "getComputerRunState":
            try refuseComputerConnectionWhileClientPaused()
            try await requireCursorComputerLifecycle()
            let state = try await dashboard.getSandBoxRunState()
            return .handled([
                "imageUpdateAvailable": state.imageUpdateAvailable,
            ])

        case "getBoxMigrationStatus":
            try refuseComputerConnectionWhileClientPaused()
            try await requireCursorComputerLifecycle()
            if let status = computerMigrationStatus ?? computerMigrationLastTerminal {
                return .handled(status)
            }
            return .handled(NSNull())

        case "updateComputer":
            try refuseComputerConnectionWhileClientPaused()
            try await requireCursorComputerLifecycle()
            guard nonEmptyString(params["id"]) != nil else {
                throw SandMcpConfigError("A computer update requires an agent id.")
            }
            let result = try await dashboard.recreateSandBox(
                preserveData: true,
                force: params["force"] as? Bool == true
            )
            return .handled(projectRecreate(result))

        case "forceRecreateComputer":
            try refuseComputerConnectionWhileClientPaused()
            try await requireCursorComputerLifecycle()
            return .handled(projectRecreate(try await dashboard.forceRecreateSandBox()))

        case "generateAgentAvatarImage":
            guard (await cursorAuth.status()).loggedIn else {
                throw SandMcpConfigError("Avatar generation requires Cursor sign-in.")
            }
            guard let description = nonEmptyString(params["description"]) else {
                throw SandMcpConfigError("Describe the avatar to generate first.")
            }
            let generated = try await avatarImageGenerator.generate(description: description)
            guard !generated.imageData.isEmpty, generated.mimeType.hasPrefix("image/") else {
                throw SandMcpConfigError("Avatar generation returned invalid image data.")
            }
            return .handled("data:\(generated.mimeType);base64,\(generated.imageData)")

        case "coordinator.skill.publishTargets":
            guard (await cursorAuth.status()).loggedIn else {
                throw SandMcpConfigError("Skill publishing requires Cursor sign-in.")
            }
            let teams = try await dashboard.getSkillPublishTeams()
            return .handled(["teams": projectSkillPublishTargets(teams)])

        case "coordinator.skill.pluginFactsSync":
            guard let agentId = nonEmptyString(params["agentId"]) else {
                throw SandMcpConfigError("Published skill refresh requires agentId.")
            }
            let plugins = try await refreshAuthoritativePluginFacts(agentId: agentId)
            return .handled(["plugins": plugins])

        case "coordinator.skill.publish":
            guard (await cursorAuth.status()).loggedIn else {
                throw SandMcpConfigError("Skill publishing requires Cursor sign-in.")
            }
            guard let agentId = nonEmptyString(params["agentId"]),
                  let workflowId = nonEmptyString(params["workflowId"])
            else {
                throw SandMcpConfigError(
                    "Skill publishing requires agentId and workflowId."
                )
            }
            let teamId = try publishTeamId(params["teamId"])
            let published = try await uploadPrivateWorkflow(
                agentId: agentId,
                workflowId: workflowId,
                teamId: teamId
            )
            var promotedWorkflowId: String?
            for _ in 0..<SKILL_PUBLISH_CONFIRM_MAX_ATTEMPTS {
                _ = try? await refreshAuthoritativePluginFacts(agentId: agentId)
                let confirmation = try await port.confirmWorkflowPublish(
                    agentId: agentId,
                    workflowId: workflowId,
                    pluginId: published.pluginId,
                    commitSha: published.commitSha
                )
                if confirmation["confirmed"] as? Bool == true {
                    promotedWorkflowId = nonEmptyString(confirmation["promotedWorkflowId"])
                    break
                }
            }
            var result: [String: Any] = [
                "workflowId": workflowId,
                "pluginId": published.pluginId,
                "commitSha": published.commitSha,
                "confirmed": promotedWorkflowId != nil,
            ]
            if let promotedWorkflowId {
                result["promotedWorkflowId"] = promotedWorkflowId
            }
            return .handled(result)

        case "coordinator.skill.publishUpload":
            guard (await cursorAuth.status()).loggedIn else {
                throw SandMcpConfigError("Skill publishing requires Cursor sign-in.")
            }
            guard let agentId = nonEmptyString(params["agentId"]),
                  let workflowId = nonEmptyString(params["workflowId"])
            else {
                throw SandMcpConfigError(
                    "Skill publishing requires agentId and workflowId."
                )
            }
            let published = try await uploadPrivateWorkflow(
                agentId: agentId,
                workflowId: workflowId,
                teamId: try publishTeamId(params["teamId"])
            )
            return .handled([
                "workflowId": workflowId,
                "pluginId": published.pluginId,
                "commitSha": published.commitSha,
                "confirmed": false,
            ])

        case "coordinator.skill.resync":
            guard (await cursorAuth.status()).loggedIn else {
                throw SandMcpConfigError("Skill sync requires Cursor sign-in.")
            }
            guard let agentId = nonEmptyString(params["agentId"]),
                  let workflowId = nonEmptyString(params["workflowId"])
            else {
                throw SandMcpConfigError("Skill sync requires agentId and workflowId.")
            }
            _ = try await refreshAuthoritativePluginFacts(agentId: agentId)
            let package = try await port.exportPublishedWorkflowPublishPackage(
                agentId: agentId,
                workflowId: workflowId
            )
            guard let pluginId = nonEmptyString(package["pluginId"]),
                  let teamIdNumber = package["teamId"] as? NSNumber,
                  let name = nonEmptyString(package["name"]),
                  let displayName = nonEmptyString(package["displayName"]),
                  let description = nonEmptyString(package["description"]),
                  let encoded = nonEmptyString(package["pluginTarGzBase64"]),
                  let archive = Data(base64Encoded: encoded)
            else {
                throw SandMcpConfigError("Published skill package is incomplete.")
            }
            let teamRaw = teamIdNumber.int64Value
            guard teamRaw > 0, teamRaw <= Int64(Int32.max) else {
                throw SandMcpConfigError("Published skill team id is invalid.")
            }
            let published = try await dashboard.publishSkillPlugin(
                teamId: Int32(teamRaw),
                name: name,
                displayName: displayName,
                description: description,
                pluginTarGz: archive
            )
            guard published.pluginId == pluginId else {
                throw SandMcpConfigError(
                    "Skill sync returned a different plugin id; local published state was preserved."
                )
            }
            var confirmed = false
            for _ in 0..<SKILL_PUBLISH_CONFIRM_MAX_ATTEMPTS {
                _ = try? await refreshAuthoritativePluginFacts(agentId: agentId)
                let confirmation = try await port.confirmPublishedWorkflowResync(
                    agentId: agentId,
                    workflowId: workflowId,
                    pluginId: published.pluginId,
                    commitSha: published.commitSha
                )
                if confirmation["confirmed"] as? Bool == true {
                    confirmed = true
                    break
                }
            }
            return .handled([
                "workflowId": workflowId,
                "pluginId": published.pluginId,
                "commitSha": published.commitSha,
                "confirmed": confirmed,
            ])

        case "coordinator.skill.unpublish":
            guard (await cursorAuth.status()).loggedIn else {
                throw SandMcpConfigError("Skill unpublish requires Cursor sign-in.")
            }
            guard let agentId = nonEmptyString(params["agentId"]),
                  let workflowId = nonEmptyString(params["workflowId"])
            else {
                throw SandMcpConfigError("Skill unpublish requires agentId and workflowId.")
            }
            _ = try await refreshAuthoritativePluginFacts(agentId: agentId)
            let prepared = try await port.prepareWorkflowUnpublish(
                agentId: agentId,
                workflowId: workflowId
            )
            guard let pluginId = nonEmptyString(prepared["pluginId"]),
                  let teamNumber = prepared["teamId"] as? NSNumber,
                  let restoredWorkflowId = nonEmptyString(prepared["restoredWorkflowId"])
            else {
                throw SandMcpConfigError("Skill unpublish preparation is incomplete.")
            }
            let teamRaw = teamNumber.int64Value
            guard teamRaw > 0, teamRaw <= Int64(Int32.max) else {
                throw SandMcpConfigError("Published skill team id is invalid.")
            }
            // Host restores the private copy before this remote mutation. If the
            // backend fails, that restored local copy intentionally remains.
            try await dashboard.unpublishSkillPlugin(
                pluginId: pluginId,
                teamId: Int32(teamRaw)
            )
            try await port.completeWorkflowUnpublish(
                agentId: agentId,
                pluginId: pluginId
            )
            _ = try? await refreshAuthoritativePluginFacts(agentId: agentId)
            return .handled([
                "workflowId": workflowId,
                "pluginId": pluginId,
                "restoredWorkflowId": restoredWorkflowId,
                "unpublished": true,
            ])

        case "coordinator.mcp.catalog":
            let forceRefresh = params["forceRefresh"] as? Bool ?? false
            let entries = try await manager.getCatalog(forceRefresh: forceRefresh)
            return .handled([
                "entries": entries.map { entry in
                    var row: [String: Any] = [
                        "id": entry.id,
                        "name": entry.name,
                        "displayName": entry.displayName,
                        "connectors": entry.connectors.map(\.name),
                        "fields": entry.fields.map { field -> [String: Any] in
                            [
                                "key": field.key,
                                "label": field.label,
                                "placeholder": field.placeholder,
                                "isRequired": field.isRequired,
                                "isSecret": field.isSecret,
                                "defaultValue": field.defaultValue ?? NSNull(),
                                "hint": field.hint ?? NSNull(),
                            ]
                        },
                    ]
                    if let iconUrl = entry.iconUrl, !iconUrl.isEmpty {
                        row["iconUrl"] = iconUrl
                    }
                    return row
                },
            ])

        case "coordinator.mcp.install":
            guard let entryId = nonEmptyString(params["entryId"]) else {
                throw SandMcpConfigError("MCP marketplace entry id is required.")
            }
            let values = (params["values"] as? [String: Any] ?? [:]).compactMapValues { $0 as? String }
            try await manager.installEntry(entryId: entryId, values: values)
            let state = try await manager.listServers()
            return .handled(["servers": state.servers.map(projectServer)])

        case "coordinator.mcp.updatePluginInstall":
            guard let pluginId = nonEmptyString(params["pluginId"]) else {
                throw SandMcpConfigError("MCP plugin id is required.")
            }
            let values = (params["values"] as? [String: Any] ?? [:]).compactMapValues { $0 as? String }
            try await manager.updatePluginInstall(pluginId: pluginId, values: values)
            let state = try await manager.listServers()
            return .handled(["servers": state.servers.map(projectServer)])

        case "coordinator.mcp.reloadServers":
            let state = try await manager.reloadServers()
            return .handled(["servers": state.servers.map(projectServer)])

        case "coordinator.mcp.removeServer":
            guard let serverId = nonEmptyString(params["serverId"]) else {
                throw SandMcpConfigError("MCP server id is required.")
            }
            let result = try await manager.removeServer(serverId)
            return .handled([
                "removed": result.removed,
                "reason": result.reason.map { $0 as Any } ?? NSNull(),
                "servers": result.state.servers.map(projectServer),
            ])

        case "coordinator.mcp.uninstallPlugin":
            guard let pluginId = nonEmptyString(params["pluginId"]) else {
                throw SandMcpConfigError("MCP plugin id is required.")
            }
            let state = try await manager.uninstallPlugin(pluginId)
            return .handled(["servers": state.servers.map(projectServer)])

        case "coordinator.mcp.setInstructions":
            guard let serverId = nonEmptyString(params["serverId"]),
                  let instructions = params["instructions"] as? String
            else {
                throw SandMcpConfigError("MCP server id and instructions are required.")
            }
            let state = try await manager.setServerCustomInstructions(
                serverId: serverId,
                instructions: instructions
            )
            return .handled(["servers": state.servers.map(projectServer)])

        case "coordinator.mcp.servers":
            let state = try await manager.listServers()
            return .handled(["servers": state.servers.map(projectServer)])

        case "coordinator.mcp.tools":
            guard let serverId = nonEmptyString(params["serverId"]) else {
                throw SandMcpConfigError("MCP server id is required.")
            }
            let tools = try await manager.listServerTools(serverId)
            return .handled([
                "serverId": serverId,
                "tools": tools.map(projectTool),
            ])

        case "listRoutedMcpTools":
            guard let scope = routedScopeToken() else {
                throw SandMcpConfigError("Routed MCP tools require an authenticated account.")
            }
            let tools = try await routedMcpBridge.listTools(scope: scope)
            return .handled([
                "tools": tools.map(projectRoutedTool),
            ])

        case "executeRoutedMcpTool":
            guard let scope = routedScopeToken() else {
                throw SandMcpConfigError("Routed MCP tools require an authenticated account.")
            }
            guard let requestId = nonEmptyString(params["_requestId"]),
                  let name = nonEmptyString(params["name"]),
                  let toolCallId = nonEmptyString(params["toolCallId"])
            else {
                throw SandMcpConfigError(
                    "Routed MCP execution requires requestId, name and toolCallId."
                )
            }
            let arguments = try CoordinatorPayload.fromFoundation(
                params["arguments"] ?? params["args"] ?? [String: Any]()
            )
            let result = try await routedMcpBridge.callTool(
                name: name,
                arguments: arguments,
                requestId: requestId,
                toolCallId: toolCallId,
                agentId: nonEmptyString(params["agentId"]),
                scope: scope
            )
            return .handled(result.foundationValue)

        case "getAvailableModels":
            let auth = await cursorAuth.status()
            guard auth.loggedIn, let scope = routedScopeToken() else {
                throw IOSCursorAuthError.signInRequired
            }
            try Task.checkCancellation()
            let models = try await dashboard.getAvailableModelsJSON().value
            try Task.checkCancellation()
            let latestAuth = await cursorAuth.status()
            guard routedScopeToken() == scope,
                  latestAuth.loggedIn,
                  latestAuth.authId == auth.authId,
                  latestAuth.freshness == auth.freshness
            else {
                throw SandMcpConfigError(
                    "Available models account scope changed before response."
                )
            }
            return .handled(models)

        case "getCursorAuthStatus":
            return .handled(await mainRpcCursorAuth.getStatus())

        case "loginCursor":
            return .handled(try await mainRpcCursorAuth.login())

        case "cancelCursorLogin":
            return .handled(try await mainRpcCursorAuth.cancelLogin())

        case "logoutCursor":
            return .handled(try await mainRpcCursorAuth.logout())

        case "coordinator.mcp.cursorAuth.status":
            return .handled(projectCursorAuthStatus(await cursorAuth.status()))

        case "getSandAccess":
            return .handled(iosSandAccessFoundationValue(await sandAccessReader.read()))

        case "getSandAccessFresh":
            return .handled(iosSandAccessFoundationValue(await sandAccessReader.readFresh()))

        case "updateCursorAccountName":
            guard (await cursorAuth.status()).loggedIn else {
                throw IOSCursorAuthError.signInRequired
            }
            guard let name = params["name"] as? String, name.count <= 200 else {
                throw SandMcpConfigError(
                    "updateCursorAccountName requires a bounded name string."
                )
            }
            let status = try await cursorAuth.updateDisplayName(name) { normalizedName in
                try await dashboard.updateCursorAccountName(normalizedName)
            }
            return .handled(projectCursorAuthStatus(status))

        case "getCursorAvatar":
            let auth = await cursorAuth.status()
            guard auth.loggedIn else { return .handled(NSNull()) }
            if let avatar = await dashboard.getCursorAvatarDataURL(authId: auth.authId) {
                return .handled(avatar)
            }
            return .handled(NSNull())

        case "getCursorWeeklyUsage":
            guard (await cursorAuth.status()).loggedIn else { return .handled(NSNull()) }
            if let usage = await dashboard.getCursorWeeklyUsageTransfer() {
                return .handled(usage.value)
            }
            return .handled(NSNull())

        case "getCursorUsageSummary":
            guard (await cursorAuth.status()).loggedIn else { return .handled(NSNull()) }
            return .handled(try await dashboard.getCursorUsageSummaryTransfer().value)

        case "getCursorPrivacyModeEnabled":
            guard (await cursorAuth.status()).loggedIn else { return .handled(true) }
            return .handled(await dashboard.getCursorPrivacyModeEnabled())

        case "cancelCursorSandTrial":
            guard (await cursorAuth.status()).loggedIn else {
                throw IOSCursorAuthError.signInRequired
            }
            return .handled(await dashboard.cancelCursorSandTrialTransfer().value)

        case "invokeCursorDashboardAction":
            guard (await cursorAuth.status()).loggedIn else {
                throw IOSCursorAuthError.signInRequired
            }
            guard let action = nonEmptyString(params["action"]) else {
                throw SandMcpConfigError("invokeCursorDashboardAction requires action.")
            }
            let rawArgs = params["args"] as? [String: Any] ?? [:]
            let args = rawArgs.compactMapValues { $0 as? String }
            return .handled(
                try await dashboard.invokeCursorDashboardActionTransfer(action: action, args: args).value
            )

        case "transcribeAudio":
            guard (await cursorAuth.status()).loggedIn else {
                throw IOSCursorAuthError.signInRequired
            }
            guard let rawAudio = params["audio"] as? [Any], !rawAudio.isEmpty else {
                throw SandMcpConfigError("transcribeAudio requires non-empty audio bytes.")
            }
            var bytes = [UInt8]()
            bytes.reserveCapacity(rawAudio.count)
            for value in rawAudio {
                guard let number = value as? NSNumber,
                      CFGetTypeID(number) != CFBooleanGetTypeID(),
                      number.intValue >= 0,
                      number.intValue <= 255
                else {
                    throw SandMcpConfigError("transcribeAudio requires byte-valued audio.")
                }
                bytes.append(UInt8(number.intValue))
            }
            let mimeType = nonEmptyString(params["mimeType"]) ?? "audio/webm"
            let language = nonEmptyString(params["language"])
            let result = try await dashboard.transcribeAudio(
                audio: Data(bytes),
                mimeType: mimeType,
                language: language
            )
            return .handled([
                "text": result.text,
                "transcriptionTimeMs": result.transcriptionTimeMs,
            ])

        case "getCursorPrReviewPreferences":
            guard (await cursorAuth.status()).loggedIn else {
                return .handled([
                    "user": NSNull(),
                    "team": NSNull(),
                ])
            }
            let preferences = try await dashboard.getPrReviewPreferences()
            let user: Any = preferences.user.map { $0.rawValue } ?? NSNull()
            let team: Any = preferences.team.map { $0.rawValue } ?? NSNull()
            return .handled([
                "user": user,
                "team": team,
            ])

        case "coordinator.mcp.cursorAuth.start":
            let start = try cursorAuth.beginLogin()
            return .handled([
                "attemptId": start.attemptId,
                "loginUrl": start.loginURL.absoluteString,
            ])

        case "coordinator.mcp.cursorAuth.poll":
            guard let attemptId = nonEmptyString(params["attemptId"]) else {
                throw SandMcpConfigError("MCP backend sign-in attempt id is required.")
            }
            if let status = try await cursorAuth.pollLogin(attemptId: attemptId) {
                _ = try await manager.reloadServers()
                return .handled([
                    "completed": true,
                    "auth": projectCursorAuthStatus(status),
                ])
            }
            return .handled(["completed": false])

        case "coordinator.mcp.cursorAuth.cancel":
            guard let attemptId = nonEmptyString(params["attemptId"]) else {
                throw SandMcpConfigError("MCP backend sign-in attempt id is required.")
            }
            cursorAuth.cancelLogin(attemptId: attemptId)
            return .handled(["cancelled": true])

        case "coordinator.mcp.cursorAuth.logout":
            try await cursorAuth.logout()
            await manager.reload()
            return .handled(["loggedIn": false])

        case "coordinator.mcp.authenticate":
            guard let serverId = nonEmptyString(params["serverId"]) else {
                throw SandMcpConfigError("MCP server id is required.")
            }
            let accountKey = nonEmptyString(params["accountKey"]) ?? DEFAULT_MCP_ACCOUNT_KEY
            let result = try await manager.authenticateServer(
                serverId,
                accountKey: accountKey,
                forceReauth: params["forceReauth"] as? Bool ?? false,
                trigger: "connector_card"
            )
            let payload: [String: Any] = [
                "status": result.status.rawValue,
                "serverName": result.serverName,
                "authorizationUrl": result.authorizationUrl ?? NSNull(),
                "message": result.message ?? NSNull(),
            ]
            return .handled(payload)

        case "coordinator.mcp.oauthCallback":
            guard let rawURL = nonEmptyString(params["url"]),
                  let url = URL(string: rawURL)
            else {
                throw SandMcpConfigError("MCP OAuth callback URL is invalid.")
            }
            let outcome = await manager.handleOAuthCallback(url)
            await manager.reload()
            return .handled(["outcome": projectOAuthOutcome(outcome)])

        case "coordinator.mcp.logoutAccount":
            guard let serverId = nonEmptyString(params["serverId"]),
                  let accountKey = nonEmptyString(params["accountKey"])
            else {
                throw SandMcpConfigError("MCP server id and account label are required.")
            }
            let state = try await manager.logoutAccount(serverId: serverId, accountKey: accountKey)
            return .handled(["servers": state.servers.map(projectServer)])

        case "coordinator.mcp.renameAccount":
            guard let serverId = nonEmptyString(params["serverId"]),
                  let accountKey = nonEmptyString(params["accountKey"]),
                  let newAccountKey = nonEmptyString(params["newAccountKey"])
            else {
                throw SandMcpConfigError("MCP server id and account labels are required.")
            }
            let state = try await manager.renameAccount(
                serverId: serverId,
                accountKey: accountKey,
                newAccountKey: newAccountKey
            )
            return .handled(["servers": state.servers.map(projectServer)])

        case "coordinator.mcp.removeAccount":
            guard let serverId = nonEmptyString(params["serverId"]),
                  let accountKey = nonEmptyString(params["accountKey"])
            else {
                throw SandMcpConfigError("MCP server id and account label are required.")
            }
            let state = try await manager.removeAccount(serverId: serverId, accountKey: accountKey)
            return .handled(["servers": state.servers.map(projectServer)])

        case "coordinator.mcp.setToolDisabled":
            guard let serverId = nonEmptyString(params["serverId"]),
                  let toolName = nonEmptyString(params["tool"]),
                  let disabled = params["disabled"] as? Bool
            else {
                throw SandMcpConfigError(
                    "MCP server id, tool name, and disabled state are required."
                )
            }
            let tools = try await manager.setMcpToolDisabled(
                serverId: serverId,
                toolName: toolName,
                disabled: disabled
            )
            return .handled([
                "serverId": serverId,
                "tools": tools.map(projectTool),
            ])

        default:
            return .notHandled
        }
    }

    private func projectServer(_ server: McpServerSummary) -> [String: Any] {
        [
            "id": server.id,
            "name": server.name,
            "serverIdentifier": server.serverIdentifier,
            "accountKey": server.accountKey,
            "rowServerIdentifier": server.rowServerIdentifier,
            "transport": server.transport.rawValue,
            "toolCount": server.toolCount,
            "disabledToolCount": server.disabledToolCount,
            "isTeamServer": server.isTeamServer,
            "pluginId": server.attribution.pluginId ?? NSNull(),
            "isRequired": server.attribution.isRequired,
            "managedByTeamPluginPolicy": server.attribution.managedByTeamPluginPolicy,
            "status": server.status,
            "statusDetail": server.statusDetail ?? NSNull(),
        ]
    }

    private func projectCursorAuthStatus(_ status: IOSCursorAuthStatus) -> [String: Any] {
        [
            "loggedIn": status.loggedIn,
            "authId": status.authId ?? NSNull(),
            "email": status.email ?? NSNull(),
            "displayName": status.displayName ?? NSNull(),
            "profilePictureUrl": status.profilePictureUrl ?? NSNull(),
            "isAnysphereUser": status.isAnysphereUser.map { NSNumber(value: $0) } ?? NSNull(),
            "errorMessage": status.errorMessage ?? NSNull(),
            "expiresAtMs": status.expiresAtMs.map(NSNumber.init(value:)) ?? NSNull(),
            "freshness": NSNumber(value: status.freshness),
        ]
    }

    private func projectOAuthOutcome(_ outcome: McpOAuthCallbackOutcome) -> String {
        switch outcome {
        case .success: return "success"
        case .refused(let reason): return "refused:\(reason.rawValue)"
        case .failed(let reason, let retryable):
            return "failed:\(reason.rawValue):\(retryable ? "retryable" : "terminal")"
        case .notFound: return "notFound"
        case .unsupportedURL: return "unsupportedURL"
        }
    }

    private func projectTool(_ tool: McpToolListing) -> [String: Any] {
        [
            "name": tool.name,
            "title": tool.title ?? NSNull(),
            "description": tool.description ?? NSNull(),
            "isDisabled": tool.isDisabled,
        ]
    }

    private func projectRoutedTool(_ tool: RoutedMCPTool) -> [String: Any] {
        [
            "name": tool.name,
            "providerIdentifier": tool.providerIdentifier,
            "toolName": tool.toolName,
            "description": tool.description ?? NSNull(),
            "inputSchema": tool.inputSchema?.foundationValue ?? NSNull(),
        ]
    }

    private func nonEmptyString(_ value: Any?) -> String? {
        guard let value = value as? String else { return nil }
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }
}


private func routedCoordinatorPayload(_ value: McpJSONValue) -> CoordinatorPayload {
    switch value {
    case .null: return .null
    case .bool(let value): return .bool(value)
    case .number(let value): return .number(value)
    case .string(let value): return .string(value)
    case .array(let values): return .array(values.map(routedCoordinatorPayload))
    case .object(let object): return .object(object.mapValues(routedCoordinatorPayload))
    }
}

private func routedCoordinatorPayload(_ result: SandMcpResult) -> CoordinatorPayload {
    switch result.result {
    case .error(let message):
        return .object([
            "isError": .bool(true),
            "content": .array([
                .object([
                    "type": .string("text"),
                    "text": .string(message),
                ]),
            ]),
        ])
    case .success(let success):
        let content = success.content.map { item -> CoordinatorPayload in
            switch item.content {
            case .text(let text):
                return .object([
                    "type": .string("text"),
                    "text": .string(text.text),
                ])
            case .image(let image):
                return .object([
                    "type": .string("image"),
                    "data": .string(image.data),
                    "mimeType": .string(image.mimeType),
                ])
            case .other(let caseName, let value):
                var object: [String: CoordinatorPayload] = [
                    "type": .string(caseName),
                ]
                if let value {
                    object["value"] = routedCoordinatorPayload(value)
                }
                return .object(object)
            }
        }
        var object: [String: CoordinatorPayload] = [
            "isError": .bool(success.isError),
            "content": .array(content),
        ]
        if let structured = success.structuredContent {
            object["structuredContent"] = routedCoordinatorPayload(structured)
        }
        return .object(object)
    case .other(let caseName, let value):
        var object: [String: CoordinatorPayload] = [
            "isError": .bool(true),
            "content": .array([
                .object([
                    "type": .string("text"),
                    "text": .string("Unsupported routed MCP result: \(caseName)"),
                ]),
            ]),
        ]
        if let value {
            object["structuredContent"] = routedCoordinatorPayload(value)
        }
        return .object(object)
    }
}
