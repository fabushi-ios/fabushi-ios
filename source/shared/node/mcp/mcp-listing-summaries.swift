import Foundation

struct McpListedTool: Equatable, Sendable {
    let toolName: String
}

struct McpBackendEntry: Equatable, Sendable {
    let accountLabel: String
    var serverIdentifier: String? = nil
    var status: String? = nil
    var tools: [McpListedTool] = []
}

struct McpBoxEntry: Equatable, Sendable {
    var status: String? = nil
    var statusDetail: String? = nil
    var toolCount = 0
    var tools: [McpListedTool] = []
}

struct McpServerSummary: Equatable, Sendable {
    let id: String
    let name: String
    var serverIdentifier: String
    var accountKey: String
    let rowServerIdentifier: String
    let transport: McpTransport
    var command: String? = nil
    var url: String? = nil
    let toolCount: Int
    var disabledToolCount: Int = 0
    let customInstructions: String
    let isTeamServer: Bool
    let attribution: McpPluginAttribution
    let status: String
    var statusDetail: String? = nil
}

final class SandMcpListingSummaries {
    private let settingsStore: () -> any McpSettingsPort
    private let isRemoteRunnerExecWired: () -> Bool

    init(
        settingsStore: @escaping () -> any McpSettingsPort,
        isRemoteRunnerExecWired: @escaping () -> Bool
    ) {
        self.settingsStore = settingsStore
        self.isRemoteRunnerExecWired = isRemoteRunnerExecWired
    }

    private func enabledCount(serverId: String, tools: [McpListedTool]) -> Int {
        let disabled = settingsStore().getMcpDisabledToolsByServerId()[serverId] ?? []
        return disabled.isEmpty ? tools.count : tools.filter { !disabled.contains($0.toolName) }.count
    }

    func createBackendServerSummaries(
        server: DisplayServer,
        entries: [McpBackendEntry]
    ) -> [McpServerSummary] {
        let slots = server.accounts
        if slots.isEmpty {
            let backend = entries.first { $0.accountLabel == DEFAULT_MCP_ACCOUNT_KEY } ?? entries.first
            return [backendSummary(
                server: server,
                backend: backend,
                accountKey: DEFAULT_MCP_ACCOUNT_KEY,
                slot: nil
            )]
        }
        return slots.map { slot in
            backendSummary(
                server: server,
                backend: entries.first { $0.accountLabel == slot.accountKey },
                accountKey: slot.accountKey,
                slot: slot
            )
        }
    }

    private func backendSummary(
        server: DisplayServer,
        backend: McpBackendEntry?,
        accountKey: String,
        slot: McpDisplayAccountSlot?
    ) -> McpServerSummary {
        let row = server.serverIdentifier ?? "mcp-row-\(server.id)"
        let enabled = enabledCount(serverId: server.id, tools: backend?.tools ?? [])
        let disabled = max(0, (backend?.tools.count ?? 0) - enabled)
        let status = (slot != nil && slot?.hasToken == false)
            ? McpServerStatus(status: "needsAuth", statusDetail: "Authentication required")
            : statusFromBackendListStatus(backend?.status)
        return .init(
            id: server.id,
            name: server.name,
            serverIdentifier: slot?.serverIdentifier
                ?? backend?.serverIdentifier
                ?? provisionalMcpAccountServerIdentifier(row, accountKey: accountKey),
            accountKey: accountKey,
            rowServerIdentifier: row,
            transport: getTransport(server.config),
            command: getCommand(server.config),
            url: server.config.url,
            toolCount: enabled,
            disabledToolCount: disabled,
            customInstructions: instruction(server),
            isTeamServer: server.isTeamServer,
            attribution: pluginAttributionFromDisplayServer(server),
            status: status.status,
            statusDetail: status.statusDetail
        )
    }

    func createBoxServerSummary(
        server: DisplayServer,
        box: McpBoxEntry?,
        unavailable: Bool
    ) -> McpServerSummary {
        let identifier = server.serverIdentifier ?? "mcp-row-\(server.id)"
        let enabled = box == nil ? 0 : box!.tools.isEmpty
            ? box!.toolCount
            : enabledCount(serverId: server.id, tools: box!.tools)
        let disabled = box == nil || box!.tools.isEmpty ? 0 : max(0, box!.tools.count - enabled)
        let status = !isRemoteRunnerExecWired()
            ? McpServerStatus(status: "disconnected", statusDetail: "Runs on a Remote Runner")
            : statusFromBoxListStatus(box?.status, unavailable: unavailable, detail: box?.statusDetail)
        return .init(
            id: server.id,
            name: server.name,
            serverIdentifier: identifier,
            accountKey: DEFAULT_MCP_ACCOUNT_KEY,
            rowServerIdentifier: identifier,
            transport: getTransport(server.config),
            command: getCommand(server.config),
            toolCount: enabled,
            disabledToolCount: disabled,
            customInstructions: instruction(server),
            isTeamServer: server.isTeamServer,
            attribution: pluginAttributionFromDisplayServer(server),
            status: status.status,
            statusDetail: status.statusDetail
        )
    }

    func createAdminDisabledServerSummary(_ server: DisplayServer) -> McpServerSummary {
        let identifier = server.serverIdentifier ?? "mcp-row-\(server.id)"
        return .init(
            id: server.id,
            name: server.name,
            serverIdentifier: identifier,
            accountKey: DEFAULT_MCP_ACCOUNT_KEY,
            rowServerIdentifier: identifier,
            transport: getTransport(server.config),
            command: getCommand(server.config),
            url: server.config.url,
            toolCount: 0,
            customInstructions: instruction(server),
            isTeamServer: server.isTeamServer,
            attribution: pluginAttributionFromDisplayServer(server),
            status: "disabledByTeamAdminPolicy",
            statusDetail: "Disabled by team admin"
        )
    }

    private func instruction(_ server: DisplayServer) -> String {
        let settings = settingsStore()
        return resolveMcpCustomInstruction(
            server.name,
            storedInstruction: settings.getMcpCustomInstructionsByServerId()[server.id]
                ?? settings.getMcpCustomInstructions()[server.name]
        )
    }
}
