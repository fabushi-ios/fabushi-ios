import Foundation

protocol McpSettingsPort: AnyObject {
    func migrateMcpCustomInstructionToServerId(serverId: String, displayName: String)
    func setMcpCustomInstructionByServerId(
        serverId: String,
        displayName: String,
        value: String,
        mirrorLegacyName: Bool
    )
    func getMcpDisabledToolsByServerId() -> [String: [String]]
    func setMcpDisabledToolsByServerId(_ value: [String: [String]])
    func getMcpCustomInstructions() -> [String: String]
    func getMcpCustomInstructionsByServerId() -> [String: String]
}

struct McpToolListing: Equatable, Sendable {
    let name: String
    var title: String? = nil
    var description: String? = nil
    let isDisabled: Bool
}

struct McpRawTool: Equatable, Sendable {
    let providerIdentifier: String
    let toolName: String
    var title: String? = nil
    var description: String? = nil
}

final class SandMcpInstructionsAndToggles {
    typealias ResolveServer = (String) async -> DisplayServer?
    typealias ListServers = () async -> Any
    typealias LastDisplay = () -> AccountDisplayConfig?
    typealias GetTools = () async -> [McpRawTool]

    private let settingsStore: () -> any McpSettingsPort
    private let resolveDisplayServer: ResolveServer
    private let listServersAction: ListServers
    private let lastAccountDisplayConfig: LastDisplay
    private let getToolsRaw: GetTools

    init(
        settingsStore: @escaping () -> any McpSettingsPort,
        resolveDisplayServer: @escaping ResolveServer,
        listServers: @escaping ListServers,
        lastAccountDisplayConfig: @escaping LastDisplay,
        getToolsRaw: @escaping GetTools
    ) {
        self.settingsStore = settingsStore
        self.resolveDisplayServer = resolveDisplayServer
        self.listServersAction = listServers
        self.lastAccountDisplayConfig = lastAccountDisplayConfig
        self.getToolsRaw = getToolsRaw
    }

    func migrateCustomInstructions(_ servers: [DisplayServer]) {
        for server in servers where server.id != "0" {
            settingsStore().migrateMcpCustomInstructionToServerId(
                serverId: server.id,
                displayName: server.name
            )
        }
    }

    func setServerCustomInstructions(
        serverId rawId: String,
        instructions: String
    ) async throws -> Any {
        let serverId = try validateMcpServerId(rawId)
        guard let server = await resolveDisplayServer(serverId) else {
            throw SandMcpConfigError("MCP server not found.")
        }
        let sameName = lastAccountDisplayConfig()?.servers.filter { $0.name == server.name }.count ?? 0
        settingsStore().setMcpCustomInstructionByServerId(
            serverId: serverId,
            displayName: server.name,
            value: instructions,
            mirrorLegacyName: sameName <= 1
        )
        return await listServersAction()
    }

    private func displayRow(_ identifier: String) -> DisplayServer? {
        lastAccountDisplayConfig()?.servers.first {
            guard let row = $0.serverIdentifier else { return false }
            return displayRowOwnsIdentifier(identifier, rowIdentifier: row, slots: $0.accounts)
        }
    }

    func listServerTools(_ rawId: String) async throws -> [McpToolListing] {
        let serverId = try validateMcpServerId(rawId)
        guard await resolveDisplayServer(serverId) != nil else {
            throw SandMcpConfigError("MCP server not found.")
        }
        let disabled = settingsStore().getMcpDisabledToolsByServerId()[serverId] ?? []
        var seen = Set<String>()
        var result: [McpToolListing] = []
        for tool in await getToolsRaw() {
            guard displayRow(tool.providerIdentifier)?.id == serverId,
                  seen.insert(tool.toolName).inserted else { continue }
            result.append(.init(
                name: tool.toolName,
                title: tool.title,
                description: tool.description,
                isDisabled: disabled.contains(tool.toolName)
            ))
        }
        return result
    }

    func toggleMcpToolDisabled(
        serverId rawId: String,
        toolName: String
    ) async throws -> [McpToolListing] {
        let serverId = try validateMcpServerId(rawId)
        guard !toolName.isEmpty else { throw SandMcpConfigError("MCP tool name is required.") }
        guard await resolveDisplayServer(serverId) != nil else {
            throw SandMcpConfigError("MCP server not found.")
        }
        var all = settingsStore().getMcpDisabledToolsByServerId()
        let current = all[serverId] ?? []
        all[serverId] = current.contains(toolName)
            ? current.filter { $0 != toolName }
            : current + [toolName]
        settingsStore().setMcpDisabledToolsByServerId(all)
        return try await listServerTools(serverId)
    }

    func deleteDisabledToolsForServer(_ serverId: String) {
        var all = settingsStore().getMcpDisabledToolsByServerId()
        guard all.removeValue(forKey: serverId) != nil else { return }
        settingsStore().setMcpDisabledToolsByServerId(all)
    }

    func getMcpCustomInstructions() -> [String: String] {
        let legacy = settingsStore().getMcpCustomInstructions()
        let byId = settingsStore().getMcpCustomInstructionsByServerId()
        var result = legacy
        for server in lastAccountDisplayConfig()?.servers ?? [] {
            guard let identifier = server.serverIdentifier else { continue }
            let instruction = resolveMcpCustomInstruction(
                server.name,
                storedInstruction: byId[server.id] ?? legacy[server.name]
            )
            result[identifier] = instruction
            for slot in server.accounts {
                if let slotIdentifier = slot.serverIdentifier {
                    result[slotIdentifier] = instruction
                }
            }
        }
        return result
    }
}
