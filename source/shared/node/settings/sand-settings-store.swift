import Foundation

let SETTINGS_VERSION = 1
let SAND_DOWNGRADE_MAX_FAST_MIGRATION_ID = "downgrade-persisted-max-fast"
let SAND_SETTINGS_MIGRATION_IDS = [SAND_DOWNGRADE_MAX_FAST_MIGRATION_ID]

indirect enum SandStoredJSONValue: Codable, Equatable, Sendable {
    case null
    case bool(Bool)
    case number(Double)
    case string(String)
    case array([SandStoredJSONValue])
    case object([String: SandStoredJSONValue])

    init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() { self = .null }
        else if let value = try? container.decode(Bool.self) { self = .bool(value) }
        else if let value = try? container.decode(Double.self) { self = .number(value) }
        else if let value = try? container.decode(String.self) { self = .string(value) }
        else if let value = try? container.decode([SandStoredJSONValue].self) { self = .array(value) }
        else { self = .object(try container.decode([String: SandStoredJSONValue].self)) }
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        switch self {
        case .null: try container.encodeNil()
        case .bool(let value): try container.encode(value)
        case .number(let value): try container.encode(value)
        case .string(let value): try container.encode(value)
        case .array(let value): try container.encode(value)
        case .object(let value): try container.encode(value)
        }
    }
}

struct SandStoredAutoReviewInstructions: Codable, Equatable, Sendable {
    var isEnabled: Bool
    var allowInstructions: [String]
    var blockInstructions: [String]
}

struct SandStoredSidebarSection: Codable, Equatable, Sendable {
    var id: String
    var name: String
    var agentIDs: [String]
    var isCollapsed: Bool?

    enum CodingKeys: String, CodingKey {
        case id, name, isCollapsed
        case agentIDs = "agentIds"
    }
}

struct SandStoredSettings: Codable, Equatable, Sendable {
    var version: Int
    var mcpBoxServers: [String]
    var autoUpdateWhenIdleOptIn: Bool
    var egressTunnelEnabled: Bool
    var webauthnProxyEnabled: Bool
    var mcpCustomInstructions: [String: String]
    var mcpCustomInstructionsByServerId: [String: String]
    var mcpDisabledToolsByServerId: [String: [String]]
    var conciergeConsent: String
    var settingsMigrations: [String]

    var hasSeenOnboarding: Bool?
    var hasSeenOnboardingAccountScope: String?
    var updateTrackOverride: String?
    var themePreference: String?
    var agentDefaultModel: SandAgentModelSelection?
    var computerUseModel: SandAgentModelSelection?
    var notifications: [String: SandStoredJSONValue]?
    var userTimeZone: String?
    var userTimeZoneOverride: String?
    var autoReviewInstructions: SandStoredAutoReviewInstructions?
    var localToolPermission: String?
    var localToolPermissionCeiling: String?
    var inferenceProvider: SandInferenceProvider?
    var inferenceRouterUsage: SandInferenceRouterUsage?
    var boxRuntime: SandBoxRuntime?
    var mcpCustomInstructionsAccountScope: String?
    var pinnedAgentIds: [String]?
    var sidebarSections: [SandStoredSidebarSection]?
}

func emptySandSettings() -> SandStoredSettings {
    .init(
        version: SETTINGS_VERSION,
        mcpBoxServers: [],
        autoUpdateWhenIdleOptIn: false,
        egressTunnelEnabled: false,
        webauthnProxyEnabled: true,
        mcpCustomInstructions: [:],
        mcpCustomInstructionsByServerId: [:],
        mcpDisabledToolsByServerId: [:],
        conciergeConsent: "unset",
        settingsMigrations: SAND_SETTINGS_MIGRATION_IDS
    )
}

private func uniqueNonEmpty(_ values: [String]) -> [String] {
    var seen = Set<String>()
    return values.filter { !$0.isEmpty && seen.insert($0).inserted }
}

private func validServerID(_ value: String) -> Bool {
    guard let first = value.first, first != "0", first.isNumber else { return false }
    return value.allSatisfy(\.isNumber)
}

private func normalizeStoredSettings(_ decoded: SandStoredSettings) -> SandStoredSettings? {
    guard decoded.version == SETTINGS_VERSION else { return nil }
    var value = decoded
    value.mcpBoxServers = uniqueNonEmpty(value.mcpBoxServers)
    value.conciergeConsent = ["unset", "allowed", "denied"].contains(value.conciergeConsent)
        ? value.conciergeConsent : "unset"
    value.settingsMigrations = uniqueNonEmpty(value.settingsMigrations)
    value.mcpCustomInstructions = value.mcpCustomInstructions.reduce(into: [:]) { result, pair in
        let clamped = clampMcpCustomInstruction(pair.value)
        if !clamped.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            || !getDefaultMcpCustomInstruction(pair.key).isEmpty {
            result[pair.key] = clamped
        }
    }
    value.mcpCustomInstructionsByServerId = value.mcpCustomInstructionsByServerId.reduce(into: [:]) { result, pair in
        if validServerID(pair.key) {
            result[pair.key] = clampMcpCustomInstruction(pair.value)
        }
    }
    value.mcpDisabledToolsByServerId = value.mcpDisabledToolsByServerId.reduce(into: [:]) { result, pair in
        guard validServerID(pair.key) else { return }
        let tools = uniqueNonEmpty(pair.value)
        if !tools.isEmpty { result[pair.key] = tools }
    }
    if let raw = value.updateTrackOverride,
       FabushiUpdateTrack(rawValue: raw) == nil {
        value.updateTrackOverride = nil
    }
    if let raw = value.themePreference,
       FabushiThemePreference(rawValue: raw) == nil {
        value.themePreference = nil
    }
    if let raw = value.localToolPermission,
       !isSandLocalToolPermission(raw) {
        value.localToolPermission = nil
    }
    if let raw = value.localToolPermissionCeiling,
       !isSandLocalToolPermission(raw) {
        value.localToolPermissionCeiling = nil
    }
    value.pinnedAgentIds = value.pinnedAgentIds.map(uniqueNonEmpty)
    value.sidebarSections = value.sidebarSections?.filter {
        !$0.id.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && !$0.name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }
    if value.userTimeZone?.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty == true {
        value.userTimeZone = nil
    }
    if value.userTimeZoneOverride?.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty == true {
        value.userTimeZoneOverride = nil
    }
    if value.mcpCustomInstructionsAccountScope?.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty == true {
        value.mcpCustomInstructionsAccountScope = nil
    }
    return value
}

private func storedStringArray(_ raw: Any?) -> [String] {
    (raw as? [Any])?.compactMap { $0 as? String } ?? []
}

private func storedStringMap(_ raw: Any?) -> [String: String] {
    guard let raw = raw as? [String: Any] else { return [:] }
    return raw.reduce(into: [:]) { result, pair in
        if let value = pair.value as? String { result[pair.key] = value }
    }
}

private func storedStringListMap(_ raw: Any?) -> [String: [String]] {
    guard let raw = raw as? [String: Any] else { return [:] }
    return raw.reduce(into: [:]) { result, pair in
        let values = storedStringArray(pair.value)
        if !values.isEmpty { result[pair.key] = values }
    }
}

private func decodeStoredValue<T: Decodable>(_ type: T.Type, from raw: Any?) -> T? {
    guard let raw, JSONSerialization.isValidJSONObject(raw),
          let data = try? JSONSerialization.data(withJSONObject: raw) else { return nil }
    return try? JSONDecoder().decode(type, from: data)
}

private func parseStoredSettingsObject(_ rawValue: Any) -> SandStoredSettings? {
    guard let raw = rawValue as? [String: Any],
          (raw["version"] as? NSNumber)?.intValue == SETTINGS_VERSION else {
        return nil
    }

    var value = emptySandSettings()
    value.settingsMigrations = storedStringArray(raw["settingsMigrations"])
    value.mcpBoxServers = storedStringArray(raw["mcpBoxServers"])
    value.autoUpdateWhenIdleOptIn = raw["autoUpdateWhenIdleOptIn"] as? Bool ?? false
    value.egressTunnelEnabled = raw["egressTunnelEnabled"] as? Bool ?? false
    value.webauthnProxyEnabled = (raw["webauthnProxyEnabled"] as? Bool) != false
    value.mcpCustomInstructions = storedStringMap(raw["mcpCustomInstructions"])
    value.mcpCustomInstructionsByServerId = storedStringMap(raw["mcpCustomInstructionsByServerId"])
    value.mcpDisabledToolsByServerId = storedStringListMap(raw["mcpDisabledToolsByServerId"])
    if let consent = raw["conciergeConsent"] as? String,
       ["unset", "allowed", "denied"].contains(consent) {
        value.conciergeConsent = consent
    }

    value.hasSeenOnboarding = raw["hasSeenOnboarding"] as? Bool
    value.hasSeenOnboardingAccountScope = raw["hasSeenOnboardingAccountScope"] as? String
    value.updateTrackOverride = raw["updateTrackOverride"] as? String
    value.themePreference = raw["themePreference"] as? String
    value.agentDefaultModel = decodeStoredValue(SandAgentModelSelection.self, from: raw["agentDefaultModel"])
    value.computerUseModel = decodeStoredValue(SandAgentModelSelection.self, from: raw["computerUseModel"])
    value.notifications = decodeStoredValue([String: SandStoredJSONValue].self, from: raw["notifications"])
    value.userTimeZone = raw["userTimeZone"] as? String
    value.userTimeZoneOverride = raw["userTimeZoneOverride"] as? String

    if let review = raw["autoReviewInstructions"] as? [String: Any] {
        value.autoReviewInstructions = .init(
            isEnabled: review["isEnabled"] as? Bool ?? true,
            allowInstructions: storedStringArray(review["allowInstructions"]),
            blockInstructions: storedStringArray(review["blockInstructions"])
        )
    }

    value.localToolPermission = raw["localToolPermission"] as? String
    value.localToolPermissionCeiling = raw["localToolPermissionCeiling"] as? String
    if let provider = raw["inferenceProvider"] as? String {
        value.inferenceProvider = SandInferenceProvider(rawValue: provider)
    }
    value.inferenceRouterUsage = decodeStoredValue(SandInferenceRouterUsage.self, from: raw["inferenceRouterUsage"])
    if let runtime = raw["boxRuntime"] as? String {
        value.boxRuntime = SandBoxRuntime(rawValue: runtime)
    }
    value.mcpCustomInstructionsAccountScope = raw["mcpCustomInstructionsAccountScope"] as? String
    value.pinnedAgentIds = storedStringArray(raw["pinnedAgentIds"])

    if let sections = raw["sidebarSections"] as? [[String: Any]] {
        value.sidebarSections = sections.compactMap { section in
            guard let id = section["id"] as? String,
                  let name = section["name"] as? String else { return nil }
            return .init(
                id: id,
                name: name,
                agentIDs: storedStringArray(section["agentIds"] ?? section["agentIDs"]),
                isCollapsed: section["isCollapsed"] as? Bool
            )
        }
    }

    return normalizeStoredSettings(value)
}

private func downgradePersistedFast(_ model: SandAgentModelSelection) -> SandAgentModelSelection {
    .init(
        modelId: model.modelId,
        maxMode: true,
        parameters: model.parameters.map {
            .init(id: $0.id, value: $0.id == "fast" ? "false" : $0.value)
        }
    )
}

final class SandSettingsStore: @unchecked Sendable {
    let settingsPath: String
    private let lock = NSLock()

    init(settingsPath: String) {
        self.settingsPath = settingsPath
    }

    func load() -> SandStoredSettings {
        lock.lock()
        defer { lock.unlock() }
        return loadLocked()
    }

    private func loadLocked() -> SandStoredSettings {
        guard FileManager.default.fileExists(atPath: settingsPath) else { return emptySandSettings() }
        do {
            let data = try Data(contentsOf: URL(fileURLWithPath: settingsPath))
            let raw = try JSONSerialization.jsonObject(with: data)
            guard let parsed = parseStoredSettingsObject(raw) else { return emptySandSettings() }
            return applyPendingMigrationsLocked(parsed)
        } catch {
            return emptySandSettings()
        }
    }

    private func applyPendingMigrationsLocked(_ settings: SandStoredSettings) -> SandStoredSettings {
        guard !settings.settingsMigrations.contains(SAND_DOWNGRADE_MAX_FAST_MIGRATION_ID) else {
            return settings
        }
        var migrated = settings
        migrated.settingsMigrations.append(SAND_DOWNGRADE_MAX_FAST_MIGRATION_ID)
        if let model = migrated.agentDefaultModel {
            migrated.agentDefaultModel = downgradePersistedFast(model)
        }
        try? persistLocked(migrated)
        return migrated
    }

    func persist(_ settings: SandStoredSettings) throws {
        lock.lock()
        defer { lock.unlock() }
        try persistLocked(settings)
    }

    private func persistLocked(_ settings: SandStoredSettings) throws {
        guard let normalized = normalizeStoredSettings(settings) else {
            throw CocoaError(.coderInvalidValue)
        }
        let data = try JSONEncoder().encode(normalized)
        try writeFileAtomic(targetPath: settingsPath, data: data)
    }

    private func update(_ mutation: (inout SandStoredSettings) -> Void) {
        lock.lock()
        defer { lock.unlock() }
        var current = loadLocked()
        mutation(&current)
        try? persistLocked(current)
    }

    func getHasSeenOnboarding() -> Bool? { load().hasSeenOnboarding }

    func setHasSeenOnboarding(_ value: Bool) {
        update {
            $0.hasSeenOnboarding = value
            $0.hasSeenOnboardingAccountScope = $0.mcpCustomInstructionsAccountScope
        }
    }

    func clearHasSeenOnboarding() {
        update {
            $0.hasSeenOnboarding = nil
            $0.hasSeenOnboardingAccountScope = nil
        }
    }

    func getAutoUpdateWhenIdleOptIn() -> Bool { load().autoUpdateWhenIdleOptIn }
    func setAutoUpdateWhenIdleOptIn(_ value: Bool) { update { $0.autoUpdateWhenIdleOptIn = value } }

    func getThemePreference() -> FabushiThemePreference {
        load().themePreference.flatMap(FabushiThemePreference.init(rawValue:)) ?? FabushiDesktopPolicy.defaultTheme
    }

    func setThemePreference(_ value: FabushiThemePreference) {
        update { $0.themePreference = value.rawValue }
    }

    func getBoxRuntime() -> SandBoxRuntime { load().boxRuntime ?? DEFAULT_SAND_BOX_RUNTIME }
    func setBoxRuntime(_ value: SandBoxRuntime) { update { $0.boxRuntime = value } }

    func getEgressTunnelEnabled() -> Bool { load().egressTunnelEnabled }
    func setEgressTunnelEnabled(_ value: Bool) { update { $0.egressTunnelEnabled = value } }

    func getWebauthnProxyEnabled() -> Bool { load().webauthnProxyEnabled }
    func setWebauthnProxyEnabled(_ value: Bool) { update { $0.webauthnProxyEnabled = value } }

    func getAgentDefaultModel() -> SandAgentModelSelection? {
        guard let model = load().agentDefaultModel else { return nil }
        return .init(modelId: model.modelId, maxMode: true, parameters: model.parameters)
    }

    func setAgentDefaultModel(_ model: SandAgentModelSelection?) {
        update {
            $0.agentDefaultModel = model.map {
                .init(modelId: $0.modelId, maxMode: true, parameters: $0.parameters)
            }
        }
    }

    func getComputerUseModel() -> SandAgentModelSelection? { load().computerUseModel }
    func setComputerUseModel(_ model: SandAgentModelSelection?) { update { $0.computerUseModel = model } }

    func getUpdateTrackOverride() -> FabushiUpdateTrack? {
        guard let raw = load().updateTrackOverride,
              let track = FabushiUpdateTrack(rawValue: raw) else { return nil }
        let coerced = UpdateTrackPolicy.coerceToEnabled(track)
        if coerced != track { setUpdateTrackOverride(coerced) }
        return coerced
    }

    func setUpdateTrackOverride(_ track: FabushiUpdateTrack?) {
        update { $0.updateTrackOverride = track?.rawValue }
    }

    func getAutoReviewInstructions() -> SandAutoReviewInstructions {
        guard let stored = load().autoReviewInstructions else {
            return normalizeSandAutoReviewInstructions(isEnabled: nil, allowInstructions: nil, blockInstructions: nil)
        }
        return normalizeSandAutoReviewInstructions(
            isEnabled: stored.isEnabled,
            allowInstructions: stored.allowInstructions,
            blockInstructions: stored.blockInstructions
        )
    }

    func setAutoReviewInstructions(_ value: SandAutoReviewInstructions) {
        let normalized = normalizeSandAutoReviewInstructions(
            isEnabled: value.isEnabled,
            allowInstructions: value.allowInstructions,
            blockInstructions: value.blockInstructions
        )
        update {
            $0.autoReviewInstructions = .init(
                isEnabled: normalized.isEnabled,
                allowInstructions: normalized.allowInstructions,
                blockInstructions: normalized.blockInstructions
            )
        }
    }

    func getMcpCustomInstructions() -> [String: String] { load().mcpCustomInstructions }
    func setMcpCustomInstructions(_ value: [String: String]) { update { $0.mcpCustomInstructions = value } }

    func getMcpCustomInstructionsByServerId() -> [String: String] { load().mcpCustomInstructionsByServerId }
    func setMcpCustomInstructionsByServerId(_ value: [String: String]) {
        update { $0.mcpCustomInstructionsByServerId = value }
    }

    func getMcpDisabledToolsByServerId() -> [String: [String]] { load().mcpDisabledToolsByServerId }
    func setMcpDisabledToolsByServerId(_ value: [String: [String]]) {
        update { $0.mcpDisabledToolsByServerId = value }
    }

    func scopeToAccount(_ accountScope: String) {
        update {
            let changed = $0.mcpCustomInstructionsAccountScope != nil
                && $0.mcpCustomInstructionsAccountScope != accountScope
            if changed {
                $0.mcpCustomInstructions = [:]
                $0.mcpCustomInstructionsByServerId = [:]
                $0.mcpDisabledToolsByServerId = [:]
                $0.autoReviewInstructions = nil
                $0.agentDefaultModel = nil
                $0.computerUseModel = nil
                $0.localToolPermission = nil
                $0.localToolPermissionCeiling = nil
            }
            if let seen = $0.hasSeenOnboarding,
               $0.hasSeenOnboardingAccountScope == nil || $0.hasSeenOnboardingAccountScope == accountScope {
                $0.hasSeenOnboarding = seen
                $0.hasSeenOnboardingAccountScope = accountScope
            } else if $0.hasSeenOnboardingAccountScope != accountScope {
                $0.hasSeenOnboarding = nil
                $0.hasSeenOnboardingAccountScope = nil
            }
            $0.mcpCustomInstructionsAccountScope = accountScope
        }
    }

    func clearAccountScope() {
        update {
            $0.mcpCustomInstructionsAccountScope = nil
            $0.mcpCustomInstructions = [:]
            $0.mcpCustomInstructionsByServerId = [:]
            $0.mcpDisabledToolsByServerId = [:]
            $0.autoReviewInstructions = nil
            $0.agentDefaultModel = nil
            $0.computerUseModel = nil
            $0.localToolPermission = nil
            $0.localToolPermissionCeiling = nil
        }
    }

    func getUserTimeZone() -> String? {
        let value = load()
        return value.userTimeZoneOverride ?? value.userTimeZone
    }

    func getDetectedUserTimeZone() -> String? { load().userTimeZone }
    func getUserTimeZoneOverride() -> String? { load().userTimeZoneOverride }

    func setUserTimeZone(_ value: String?) {
        update {
            let trimmed = value?.trimmingCharacters(in: .whitespacesAndNewlines)
            $0.userTimeZone = trimmed?.isEmpty == false ? trimmed : nil
        }
    }

    func setUserTimeZoneOverride(_ value: String?) {
        update {
            let trimmed = value?.trimmingCharacters(in: .whitespacesAndNewlines)
            $0.userTimeZoneOverride = trimmed?.isEmpty == false ? trimmed : nil
        }
    }

    func getMcpBoxServers() -> [String] { load().mcpBoxServers }
    func setMcpBoxServers(_ names: [String]) { update { $0.mcpBoxServers = names } }

    func getPinnedAgentIDs() -> [String] { load().pinnedAgentIds ?? [] }
    func setPinnedAgentIDs(_ ids: [String]) { update { $0.pinnedAgentIds = ids } }

    func getLocalToolPermission() -> SandLocalToolPermission {
        normalizeSandLocalToolPermission(load().localToolPermission)
    }

    func setLocalToolPermission(_ value: SandLocalToolPermission?) {
        update { $0.localToolPermission = value }
    }

    func getLocalToolPermissionCeiling() -> SandLocalToolPermission? {
        let raw = load().localToolPermissionCeiling
        return isSandLocalToolPermission(raw) ? raw : nil
    }

    func setLocalToolPermissionCeiling(_ value: SandLocalToolPermission?) {
        update { $0.localToolPermissionCeiling = value }
    }

    func getResolvedLocalToolPermission() -> SandLocalToolPermission {
        resolveSandLocalToolPermission(getLocalToolPermission(), adminCeiling: getLocalToolPermissionCeiling())
    }
    func getInferenceProvider() -> SandInferenceProvider {
        // Fabushi product composition owns the fresh-profile default. Persisted
        // explicit choices remain authoritative across launches and accounts.
        load().inferenceProvider ?? .fabushi
    }

    func setInferenceProvider(_ provider: SandInferenceProvider) {
        update { $0.inferenceProvider = provider }
    }

    func getInferenceRouterUsage() -> SandInferenceRouterUsage {
        load().inferenceRouterUsage ?? emptySandInferenceRouterUsage()
    }

    func recordInferenceUsage(
        provider: SandInferenceProvider,
        inputTokens: Int? = nil,
        outputTokens: Int? = nil,
        cacheReadTokens: Int? = nil,
        cacheWriteTokens: Int? = nil,
        now: Date = Date()
    ) {
        func safe(_ value: Int?) -> Int {
            guard let value, value >= 0 else { return 0 }
            return value
        }
        update {
            var usage = $0.inferenceRouterUsage ?? emptySandInferenceRouterUsage()
            var previous = usage.providers[provider] ?? .init(
                requests: 0,
                inputTokens: 0,
                outputTokens: 0,
                cacheReadTokens: 0,
                cacheWriteTokens: 0,
                lastUsedAt: nil
            )
            previous.requests += 1
            previous.inputTokens += safe(inputTokens)
            previous.outputTokens += safe(outputTokens)
            previous.cacheReadTokens += safe(cacheReadTokens)
            previous.cacheWriteTokens += safe(cacheWriteTokens)
            previous.lastUsedAt = ISO8601DateFormatter().string(from: now)
            usage.providers[provider] = previous
            $0.inferenceRouterUsage = usage
        }
    }

}


extension SandSettingsStore: McpSettingsPort {
    func migrateMcpCustomInstructionToServerId(
        serverId: String,
        displayName: String
    ) {
        update { settings in
            guard settings.mcpCustomInstructionsByServerId[serverId] == nil,
                  let legacy = settings.mcpCustomInstructions[displayName] else {
                return
            }
            settings.mcpCustomInstructionsByServerId[serverId] =
                clampMcpCustomInstruction(legacy)
        }
    }

    func setMcpCustomInstructionByServerId(
        serverId: String,
        displayName: String,
        value: String,
        mirrorLegacyName: Bool
    ) {
        update { settings in
            let clamped = clampMcpCustomInstruction(value)
            if clamped.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                settings.mcpCustomInstructionsByServerId.removeValue(forKey: serverId)
                if mirrorLegacyName {
                    settings.mcpCustomInstructions.removeValue(forKey: displayName)
                }
                return
            }
            settings.mcpCustomInstructionsByServerId[serverId] = clamped
            if mirrorLegacyName {
                settings.mcpCustomInstructions[displayName] = clamped
            }
        }
    }

    func getRawMcpCustomInstruction(_ displayName: String) -> String? {
        load().mcpCustomInstructions[displayName]
    }

    func getRawMcpCustomInstructionByServerId(_ serverId: String) -> String? {
        load().mcpCustomInstructionsByServerId[serverId]
    }

    func deleteMcpCustomInstructionByServerId(
        serverId: String,
        displayName: String,
        deleteLegacyName: Bool
    ) {
        update { settings in
            settings.mcpCustomInstructionsByServerId.removeValue(forKey: serverId)
            if deleteLegacyName {
                settings.mcpCustomInstructions.removeValue(forKey: displayName)
            }
        }
    }
}
