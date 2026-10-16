import Foundation

struct SandMcpCatalogCore: @unchecked Sendable {
    let isAuthenticated: @Sendable () async -> Bool
    let fetchMarketplace: @Sendable () async throws -> SandMarketplaceListingResult
    var listEffectivePlugins: (@Sendable () async throws -> [EffectiveUserPlugin])? = nil
    let requireAccountWriter: @Sendable () throws -> AccountMcpWriter
    let reloadServers: @Sendable () async throws -> Void
    var resolveLogo: @Sendable (String) async -> String? = { await resolvePluginLogo($0) }
    var nowMs: @Sendable () -> Int64 = {
        Int64(Date().timeIntervalSince1970 * 1_000)
    }
}

actor SandMcpCatalogFlow {
    private struct CachedViews {
        let views: [SandMarketplacePluginView]
        let atMs: Int64
        let includesPrivateMarketplaces: Bool
    }

    private let core: SandMcpCatalogCore
    private var catalog: [String: SandMarketplacePlugin] = [:]
    private var viewsCache: CachedViews?

    init(core: SandMcpCatalogCore) {
        self.core = core
    }

    func getCatalog(forceRefresh: Bool = false) async throws -> [SandMarketplacePluginView] {
        let authenticated = await core.isAuthenticated()
        let cached = viewsCache
        let usable = cached?.includesPrivateMarketplaces == authenticated
        let now = core.nowMs()

        if !forceRefresh,
           usable,
           let cached,
           now - cached.atMs < Int64(CATALOG_CACHE_TTL_MS) {
            return cached.views
        }

        let listing: SandMarketplaceListingResult
        do {
            listing = try await core.fetchMarketplace()
        } catch {
            if usable, let cached { return cached.views }
            throw error
        }

        catalog.removeAll(keepingCapacity: true)
        let views = listing.plugins.map { plugin in
            catalog[plugin.pluginId] = plugin
            return marketplacePluginToView(plugin)
        }.sorted { (lhs: SandMarketplacePluginView, rhs: SandMarketplacePluginView) in
            lhs.displayName.localizedCaseInsensitiveCompare(rhs.displayName) == .orderedAscending
        }
        viewsCache = .init(
            views: views,
            atMs: core.nowMs(),
            includesPrivateMarketplaces: listing.includesPrivateMarketplaces
        )
        return views
    }

    func resolvePluginLogo(_ url: String) async -> String? {
        await core.resolveLogo(url)
    }

    private func requirePlugin(_ id: String) async throws -> SandMarketplacePlugin {
        if let plugin = catalog[id] { return plugin }
        _ = try await getCatalog(forceRefresh: true)
        guard let plugin = catalog[id] else {
            throw SandMcpConfigError(
                "Unknown marketplace plugin \"\(id)\". Reopen settings and try again."
            )
        }
        return plugin
    }

    private func assertRequired(
        plugin: SandMarketplacePlugin,
        values: [String: String]
    ) throws {
        let optionalValues = values.mapValues(Optional.some)
        let missing = findMissingRequiredCatalogFields(
            plugin.variableFields,
            values: optionalValues
        )
        guard missing.isEmpty else {
            let labels = missing.map(\.label).joined(separator: ", ")
            let keys = missing.map(\.key).joined(separator: ", ")
            throw SandMcpConfigError(
                "\"\(plugin.displayName)\" needs a value for \(labels) before it can be installed. " +
                "Ask the user for it and pass it in values (keys: \(keys)), then try again."
            )
        }
    }

    func installEntry(
        entryId: String,
        values: [String: String] = [:],
        hasTeamConfiguredVariables: Bool = false
    ) async throws {
        let plugin = try await requirePlugin(entryId)
        var teamKnown = hasTeamConfiguredVariables

        if !teamKnown, let listEffectivePlugins = core.listEffectivePlugins {
            do {
                teamKnown = try await listEffectivePlugins().contains {
                    $0.pluginId == plugin.pluginId && $0.hasTeamConfiguredVariables
                }
            } catch {
                teamKnown = false
            }
        }

        if !teamKnown {
            try assertRequired(plugin: plugin, values: values)
        }
        guard let pluginId = UInt64(plugin.pluginId) else {
            throw SandMcpConfigError("Invalid marketplace plugin id \"\(plugin.pluginId)\".")
        }

        let writer = try core.requireAccountWriter()
        try await writer.installPlugin(
            pluginId: pluginId,
            variables: values.isEmpty ? nil : values
        )
        try await core.reloadServers()
    }

    func updatePluginInstall(
        pluginId: String,
        values: [String: String]
    ) async throws {
        let plugin = try await requirePlugin(pluginId)
        try assertRequired(plugin: plugin, values: values)
        guard let numericId = UInt64(plugin.pluginId) else {
            throw SandMcpConfigError("Invalid marketplace plugin id \"\(plugin.pluginId)\".")
        }

        let writer = try core.requireAccountWriter()
        try await writer.updatePluginInstall(
            pluginId: numericId,
            variables: values
        )
        try await core.reloadServers()
    }

    func invalidateCatalog() {
        catalog.removeAll()
        viewsCache = nil
    }
}
