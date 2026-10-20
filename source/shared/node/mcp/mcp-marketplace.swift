import Foundation

struct SandMarketplaceConnector: Equatable, Sendable {
    let name: String
    let description: String
}

struct SandMarketplaceSkill: Equatable, Sendable {
    let name: String
    let description: String
    var sourceUrl: String? = nil
}

struct SandMarketplaceOwnership: Equatable, Sendable {
    enum Ownership: String, Equatable, Sendable { case team, user }
    let name: String
    let displayName: String
    let ownership: Ownership
}

struct SandMarketplacePublisher: Equatable, Sendable {
    let name: String
    let displayName: String
    let isUserOwned: Bool
}

struct SandMarketplacePlugin: Equatable, Sendable {
    let pluginId: String
    let name: String
    let displayName: String
    let description: String
    let category: String
    var logoUrl: String? = nil
    var homepage: String? = nil
    var sourceUrls: [String] = []
    var connectors: [SandMarketplaceConnector] = []
    var skills: [SandMarketplaceSkill] = []
    var variableFields: [PluginVariableField] = []
    var marketplace: SandMarketplaceOwnership? = nil
    var publisher: SandMarketplacePublisher? = nil
}


struct OfficialMcpEntry: Equatable, Sendable {
    let id: String
    let name: String
    let provider: String
    let url: String
    let description: String
    let documentation: String
    let preview: Bool
}

private let OFFICIAL_GOOGLE_MCP_DOCUMENTATION =
    "https://developers.google.com/workspace/guides/configure-mcp-servers"

private func officialGoogleMcpEntry(
    name: String,
    slug: String,
    host: String,
    description: String
) -> OfficialMcpEntry {
    .init(
        id: "fabushi-official-google-\(slug)",
        name: name,
        provider: "Google",
        url: "https://\(host).googleapis.com/mcp/v1",
        description: description,
        documentation: OFFICIAL_GOOGLE_MCP_DOCUMENTATION,
        preview: true
    )
}

/// Fabushi curates these listings; Google and GitHub operate the upstream servers.
let OFFICIAL_MCP_CATALOG: [OfficialMcpEntry] = [
    .init(
        id: "fabushi-official-github",
        name: "GitHub",
        provider: "GitHub",
        url: "https://api.githubcopilot.com/mcp/",
        description: "连接 GitHub 官方 MCP，按授权访问仓库、Issues 和 Pull Requests。",
        documentation: "https://github.com/github/github-mcp-server",
        preview: false
    ),
    officialGoogleMcpEntry(
        name: "Gmail",
        slug: "gmail",
        host: "gmailmcp",
        description: "搜索邮件、读取邮件线程、创建草稿和管理标签。官方当前工具不包含直接发送邮件。"
    ),
    officialGoogleMcpEntry(
        name: "Google Drive",
        slug: "drive",
        host: "drivemcp",
        description: "通过 Google 官方 MCP 搜索和读取云端文件。"
    ),
    officialGoogleMcpEntry(
        name: "Google Docs",
        slug: "docs",
        host: "docsmcp",
        description: "通过 Google 官方 MCP 读取和处理文档。"
    ),
    officialGoogleMcpEntry(
        name: "Google Sheets",
        slug: "sheets",
        host: "sheetsmcp",
        description: "通过 Google 官方 MCP 读取和处理电子表格。"
    ),
    officialGoogleMcpEntry(
        name: "Google Slides",
        slug: "slides",
        host: "slidesmcp",
        description: "通过 Google 官方 MCP 读取和处理演示文稿。"
    ),
    officialGoogleMcpEntry(
        name: "Google Calendar",
        slug: "calendar",
        host: "calendarmcp",
        description: "通过 Google 官方 MCP 查询日历和事件；实际能力以服务返回的工具为准。"
    ),
    officialGoogleMcpEntry(
        name: "Google Chat",
        slug: "chat",
        host: "chatmcp",
        description: "通过 Google 官方 MCP 访问 Google Chat；只读无需配置 Chat app，写能力要求关闭互动功能。"
    ),
    officialGoogleMcpEntry(
        name: "Google Contacts",
        slug: "people",
        host: "people",
        description: "通过 Google People 官方 MCP 读取授权的联系人和个人资料。"
    ),
]

func officialMcpEntry(_ id: String) -> OfficialMcpEntry? {
    OFFICIAL_MCP_CATALOG.first { $0.id == id }
}

func officialMcpCatalogPlugins() -> [SandMarketplacePlugin] {
    OFFICIAL_MCP_CATALOG.map { entry in
        let providerName = entry.provider.lowercased()
        return .init(
            pluginId: entry.id,
            name: entry.id,
            displayName: entry.name,
            description: entry.description + (
                entry.preview
                    ? " Google Developer Preview：需要预览资格、Cloud 项目、启用对应 API 和 OAuth 授权。"
                    : ""
            ),
            category: entry.provider == "GitHub" ? "Development" : "Productivity",
            homepage: entry.documentation,
            connectors: [
                .init(
                    name: entry.name,
                    description: "\(entry.provider) 官方远程 MCP · Streamable HTTP"
                ),
            ],
            variableFields: [
                .init(
                    key: "ACCESS_TOKEN",
                    label: entry.provider == "GitHub"
                        ? "GitHub 访问令牌（可选）"
                        : "Google OAuth 访问令牌（可选）",
                    placeholder: "",
                    isRequired: false,
                    isSecret: true,
                    hint: "通常安装后点击连接并授权即可；手动令牌仅供高级设置。"
                ),
            ],
            marketplace: .init(
                name: "fabushi-official",
                displayName: "Fabushi 官方插件市场",
                ownership: .team
            ),
            publisher: .init(
                name: providerName,
                displayName: "\(entry.provider) · 官方服务",
                isUserOwned: false
            )
        )
    }
}

struct SandMarketplacePluginView: Equatable, Sendable {
    let id: String
    let name: String
    let displayName: String
    let description: String
    let category: String
    var homepage: String? = nil
    var iconUrl: String? = nil
    var connectors: [SandMarketplaceConnector] = []
    var skills: [SandMarketplaceSkill] = []
    var fields: [PluginVariableField] = []
    var marketplace: SandMarketplaceOwnership? = nil
    var publisher: SandMarketplacePublisher? = nil
}

func marketplacePluginToView(
    _ plugin: SandMarketplacePlugin
) -> SandMarketplacePluginView {
    .init(
        id: plugin.pluginId,
        name: plugin.name,
        displayName: plugin.displayName,
        description: plugin.description,
        category: plugin.category,
        homepage: plugin.homepage,
        iconUrl: plugin.logoUrl,
        connectors: plugin.connectors,
        skills: plugin.skills,
        fields: plugin.variableFields,
        marketplace: plugin.marketplace,
        publisher: plugin.publisher
    )
}

func toRawGithubUrl(_ blobUrl: String) -> String? {
    guard let parsed = URL(string: blobUrl),
          parsed.scheme == "https",
          parsed.host?.lowercased() == "github.com" else { return nil }
    let parts = parsed.path.split(separator: "/").map(String.init)
    guard parts.count >= 5, parts[2] == "blob" else { return nil }
    let owner = parts[0]
    let repo = parts[1]
    let ref = parts[3]
    let rest = parts.dropFirst(4).joined(separator: "/")
    guard !owner.isEmpty, !repo.isEmpty, !ref.isEmpty, !rest.isEmpty else { return nil }
    return "https://raw.githubusercontent.com/\(owner)/\(repo)/\(ref)/\(rest)"
}

func pinnedMarketplaceSkillSourceUrl(
    _ sourceUrl: String,
    versionRef: String
) -> String? {
    let commit = versionRef.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
    guard commit.count == 40,
          commit.utf8.allSatisfy({
              ($0 >= 48 && $0 <= 57) || ($0 >= 97 && $0 <= 102)
          }),
          let parsed = URL(string: sourceUrl),
          parsed.scheme == "https",
          parsed.host?.lowercased() == "github.com"
    else { return nil }
    let parts = parsed.path.split(separator: "/").map(String.init)
    guard parts.count >= 5, parts[2] == "blob" else { return nil }
    let owner = parts[0]
    let repo = parts[1]
    let rest = parts.dropFirst(4).joined(separator: "/")
    guard !owner.isEmpty, !repo.isEmpty, !rest.isEmpty else { return nil }
    return "https://github.com/\(owner)/\(repo)/blob/\(commit)/\(rest)"
}

private let MARKETPLACE_SKILL_MARKDOWN_MAX_BYTES = 256 * 1024

func fetchMarketplaceSkillMarkdown(
    sourceUrl: String,
    session: URLSession = .shared
) async -> String? {
    guard let raw = toRawGithubUrl(sourceUrl),
          let url = URL(string: raw) else { return nil }
    var request = URLRequest(url: url)
    request.httpMethod = "GET"
    request.timeoutInterval = 15
    do {
        let (data, response) = try await session.data(for: request)
        guard let http = response as? HTTPURLResponse,
              (200..<300).contains(http.statusCode),
              !data.isEmpty,
              data.count <= MARKETPLACE_SKILL_MARKDOWN_MAX_BYTES,
              let markdown = String(data: data, encoding: .utf8),
              !markdown.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        else { return nil }
        return markdown
    } catch {
        return nil
    }
}

func normalizeMarketplaceServer(_ value: [String: Any]) -> [String: Any] {
    var rest = value
    let transport = rest.removeValue(forKey: "transport") as? String
    if rest["type"] == nil {
        if transport == "sse" {
            rest["type"] = "sse"
        } else if transport == "http" || transport == "streamableHttp" {
            rest["type"] = "http"
        }
    }
    return rest
}

func normalizePluginConfig(
    _ raw: Any,
    safeParse: (Any) -> McpServerConfig?
) -> [String: McpServerConfig] {
    guard let object = raw as? [String: Any] else { return [:] }
    let candidate = object["mcpServers"] as? [String: Any] ?? object
    var servers: [String: McpServerConfig] = [:]
    for (name, value) in candidate {
        guard let record = value as? [String: Any],
              let parsed = safeParse(normalizeMarketplaceServer(record)) else { continue }
        servers[name] = parsed
    }
    return servers
}

struct MarketplaceWireConnector: Equatable, Sendable {
    let name: String
    var description: String = ""
    var sourceUrl: String? = nil
}

struct MarketplaceWireSkill: Equatable, Sendable {
    let name: String
    var description: String = ""
    var sourceUrl: String? = nil
}

struct MarketplaceWireMarketplace: Equatable, Sendable {
    let id: UInt64
    let name: String
    var displayName: String = ""
    var teamId: UInt64? = nil
    var userId: UInt64? = nil
}

struct MarketplaceWirePublisher: Equatable, Sendable {
    let name: String
    var displayName: String = ""
    var logoUrl: String? = nil
    var websiteUrl: String? = nil
    var isUserOwned: Bool = false
}

struct MarketplaceWirePlugin: Equatable, Sendable {
    let id: UInt64
    let name: String
    var displayName: String = ""
    var description: String = ""
    var logoUrl: String? = nil
    var repositoryUrl: String? = nil
    var curatedCategoryKeys: [String] = []
    var mcpServers: [MarketplaceWireConnector] = []
    var skills: [MarketplaceWireSkill] = []
    var variableFields: [PluginVariableField] = []
    var marketplace: MarketplaceWireMarketplace? = nil
    var publisher: MarketplaceWirePublisher? = nil
}

struct MarketplacePluginListing: Equatable, Sendable {
    let plugins: [MarketplaceWirePlugin]
}

protocol SandMarketplaceListingClient: Sendable {
    func listMarketplacePlugins(
        marketplaceId: UInt64?,
        excludeCloudAgentPlugins: Bool,
        timeoutMs: Int
    ) async throws -> MarketplacePluginListing
    func listMarketplaces(timeoutMs: Int) async throws -> [MarketplaceWireMarketplace]
    func getPluginMcpConfig(pluginId: UInt64, timeoutMs: Int) async throws -> String?
}

private func defaultMarketplaceRememberPluginLogoUrl(_ url: String) {
    rememberPluginLogoUrl(url)
}

struct MarketplaceListingDependencies: @unchecked Sendable {
    let bestEffortToken: @Sendable () async -> String?
    let createClient: @Sendable () async throws -> any SandMarketplaceListingClient
    var timeoutMs: Int = CURSOR_MARKETPLACE_REQUEST_TIMEOUT_MS
    var rememberPluginLogoUrl: @Sendable (String) -> Void = defaultMarketplaceRememberPluginLogoUrl
}

private func marketplaceCategory(_ keys: [String]) -> String {
    guard let key = keys.first(where: { !$0.isEmpty }) else { return "MCP" }
    return key.lowercased().split(separator: "_").map {
        guard let first = $0.first else { return "" }
        return first.uppercased() + $0.dropFirst()
    }.joined(separator: " ")
}

private func toSandMarketplacePlugin(
    _ plugin: MarketplaceWirePlugin,
    rememberLogo: @Sendable (String) -> Void
) -> SandMarketplacePlugin? {
    guard !plugin.mcpServers.isEmpty || !plugin.skills.isEmpty else { return nil }
    let logo = plugin.publisher?.logoUrl?.isEmpty == false
        ? plugin.publisher?.logoUrl
        : (plugin.logoUrl?.isEmpty == false ? plugin.logoUrl : nil)
    if let logo { rememberLogo(logo) }

    let marketplace = plugin.marketplace.flatMap { value -> SandMarketplaceOwnership? in
        guard value.teamId != nil || value.userId != nil else { return nil }
        return .init(
            name: value.name,
            displayName: value.displayName.isEmpty ? value.name : value.displayName,
            ownership: value.teamId != nil ? .team : .user
        )
    }
    let publisher = plugin.publisher.map {
        SandMarketplacePublisher(
            name: $0.name,
            displayName: $0.displayName.isEmpty ? $0.name : $0.displayName,
            isUserOwned: $0.isUserOwned
        )
    }
    return .init(
        pluginId: String(plugin.id),
        name: plugin.mcpServers.first?.name ?? plugin.name,
        displayName: plugin.displayName.isEmpty ? plugin.name : plugin.displayName,
        description: plugin.description,
        category: marketplaceCategory(plugin.curatedCategoryKeys),
        logoUrl: logo,
        homepage: plugin.repositoryUrl?.isEmpty == false
            ? plugin.repositoryUrl
            : plugin.publisher?.websiteUrl,
        sourceUrls: plugin.mcpServers.compactMap {
            guard let url = $0.sourceUrl, !url.isEmpty else { return nil }
            return url
        },
        connectors: plugin.mcpServers.map {
            .init(name: $0.name, description: $0.description)
        },
        skills: plugin.skills.map {
            .init(name: $0.name, description: $0.description, sourceUrl: $0.sourceUrl)
        },
        variableFields: plugin.variableFields,
        marketplace: marketplace,
        publisher: publisher
    )
}

struct SandMarketplaceListingResult: Equatable, Sendable {
    let plugins: [SandMarketplacePlugin]
    let includesPrivateMarketplaces: Bool
}

func fetchMarketplaceMcpPlugins(
    deps: MarketplaceListingDependencies
) async throws -> SandMarketplaceListingResult {
    var byId = Dictionary(
        uniqueKeysWithValues: officialMcpCatalogPlugins().map { ($0.pluginId, $0) }
    )
    let officialFallback: () -> SandMarketplaceListingResult = {
        .init(
            plugins: byId.values.sorted {
                $0.displayName.localizedCaseInsensitiveCompare($1.displayName) == .orderedAscending
            },
            includesPrivateMarketplaces: false
        )
    }

    let client: any SandMarketplaceListingClient
    do {
        client = try await deps.createClient()
    } catch {
        return officialFallback()
    }

    let base: MarketplacePluginListing
    do {
        base = try await client.listMarketplacePlugins(
            marketplaceId: nil,
            excludeCloudAgentPlugins: true,
            timeoutMs: deps.timeoutMs
        )
    } catch {
        return officialFallback()
    }

    func add(_ plugin: MarketplaceWirePlugin) {
        guard let converted = toSandMarketplacePlugin(
            plugin,
            rememberLogo: deps.rememberPluginLogoUrl
        ) else { return }
        byId[converted.pluginId] = converted
    }
    base.plugins.forEach(add)

    let authenticated = await deps.bestEffortToken() != nil
    if authenticated {
        let privateMarketplaces: [MarketplaceWireMarketplace]
        do {
            privateMarketplaces = try await client.listMarketplaces(
                timeoutMs: deps.timeoutMs
            ).filter { $0.teamId != nil || $0.userId != nil }
        } catch {
            privateMarketplaces = []
        }

        for marketplace in privateMarketplaces {
            do {
                let listing = try await client.listMarketplacePlugins(
                    marketplaceId: marketplace.id,
                    excludeCloudAgentPlugins: true,
                    timeoutMs: deps.timeoutMs
                )
                listing.plugins.forEach(add)
            } catch {
                continue
            }
        }
    }

    return .init(
        plugins: byId.values.sorted {
            $0.displayName.localizedCaseInsensitiveCompare($1.displayName) == .orderedAscending
        },
        includesPrivateMarketplaces: authenticated
    )
}

struct MarketplacePluginServerDependencies: @unchecked Sendable {
    let bestEffortToken: @Sendable () async -> String?
    let createClient: @Sendable () async throws -> any SandMarketplaceListingClient
    let safeParseServer: @Sendable (Any) -> McpServerConfig?
    var timeoutMs: Int = CURSOR_MARKETPLACE_REQUEST_TIMEOUT_MS
    var fetchTimeoutMs: Int = 15_000
    var session: URLSession = .shared
}

private func parseMarketplaceConfigText(
    _ text: String,
    safeParse: (Any) -> McpServerConfig?
) -> [String: McpServerConfig] {
    guard let data = text.data(using: .utf8),
          let raw = try? JSONSerialization.jsonObject(with: data) else { return [:] }
    return normalizePluginConfig(raw, safeParse: safeParse)
}

func fetchPluginServers(
    plugin: SandMarketplacePlugin,
    deps: MarketplacePluginServerDependencies
) async -> [String: McpServerConfig] {
    if await deps.bestEffortToken() != nil,
       let pluginId = UInt64(plugin.pluginId) {
        do {
            let client = try await deps.createClient()
            if let configJson = try await client.getPluginMcpConfig(
                pluginId: pluginId,
                timeoutMs: deps.timeoutMs
            ) {
                let parsed = parseMarketplaceConfigText(
                    configJson,
                    safeParse: deps.safeParseServer
                )
                if !parsed.isEmpty { return parsed }
            }
        } catch {
            // Public source fallback below is intentionally best effort.
        }
    }

    for source in plugin.sourceUrls {
        guard let raw = toRawGithubUrl(source),
              let url = URL(string: raw) else { continue }
        do {
            var request = URLRequest(url: url)
            request.timeoutInterval = TimeInterval(deps.fetchTimeoutMs) / 1_000
            request.httpMethod = "GET"
            let (data, response) = try await deps.session.data(for: request)
            guard let http = response as? HTTPURLResponse,
                  (200..<300).contains(http.statusCode),
                  let text = String(data: data, encoding: .utf8) else { continue }
            let parsed = parseMarketplaceConfigText(
                text,
                safeParse: deps.safeParseServer
            )
            if !parsed.isEmpty { return parsed }
        } catch {
            continue
        }
    }
    return [:]
}
