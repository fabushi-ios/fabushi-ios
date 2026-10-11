import Foundation

let BASE_POLL_INTERVAL_MS = 5 * 60_000
let MIN_POLL_INTERVAL_MS = 30_000
let POLL_JITTER_FRACTION = 0.3
let BOOTSTRAP_TIMEOUT_MS = 30_000
let GATE_READY_TIMEOUT_MS = 10_000

enum SandExperimentRefreshTrigger: String, Sendable {
    case startup
    case poll
    case authChange = "auth_change"
    case manual
}

struct SandExperimentBootstrapFetchRequest: @unchecked Sendable {
    let backendUrl: String
    let getAccessToken: @Sendable (String) async throws -> String
    let getMachineId: @Sendable () async throws -> String
    let env: [String: String]
    let timeoutSeconds: TimeInterval
    let session: URLSession
}

typealias SandExperimentBootstrapFetcher = @Sendable (
    SandExperimentBootstrapFetchRequest
) async throws -> StatsigBootstrapResult

func computePollDelayMs(
    baseIntervalMs: Int,
    jitterRoll: Double,
    retryAfterMs: Int? = nil
) -> Int {
    let clampedRoll = min(max(jitterRoll, 0), 1)
    let jittered = Int(floor(Double(baseIntervalMs) * (1 + clampedRoll * POLL_JITTER_FRACTION)))
    return retryAfterMs.map { max($0, jittered) } ?? jittered
}

func envGateOverride(
    _ name: String,
    env: [String: String] = ProcessInfo.processInfo.environment
) -> Bool? {
    guard let raw = env["SAND_FEATURE_GATE_OVERRIDES"], !raw.isEmpty else { return nil }
    for entry in raw.split(separator: ",") {
        let parts = entry.split(separator: "=", maxSplits: 1).map {
            $0.trimmingCharacters(in: .whitespacesAndNewlines)
        }
        guard parts.count == 2, parts[0] == name else { continue }
        return ["1", "true", "on"].contains(parts[1].lowercased())
    }
    return nil
}

func jitteredSandExperimentPollIntervalMs(
    isDevBuild: Bool,
    randomRoll: Double = Double.random(in: 0...1)
) -> Int {
    let base = isDevBuild ? MIN_POLL_INTERVAL_MS : max(BASE_POLL_INTERVAL_MS, MIN_POLL_INTERVAL_MS)
    return computePollDelayMs(baseIntervalMs: base, jitterRoll: randomRoll)
}

struct SandExperimentSnapshot: Equatable, Sendable {
    var isInitialized: Bool
    var featureGates: [String: Bool]
    var experiments: [String: [String: IOSExperimentConfigValue]]
    var dynamicConfigs: [String: [String: IOSExperimentConfigValue]]
    var sandModelExperiment: SandModelExperimentState?
    var sandModelFilterAllowedIds: [String]
    var featureFlagOverrides: [String: Bool]
}

func sandExperimentSnapshotFoundation(_ snapshot: SandExperimentSnapshot) -> [String: Any] {
    var value: [String: Any] = [
        "isInitialized": snapshot.isInitialized,
        "featureGates": snapshot.featureGates,
        "experiments": snapshot.experiments.mapValues { $0.mapValues(foundationValue) },
        "dynamicConfigs": snapshot.dynamicConfigs.mapValues { $0.mapValues(foundationValue) },
        "sandModelFilterAllowedIds": snapshot.sandModelFilterAllowedIds,
        "featureFlagOverrides": snapshot.featureFlagOverrides,
    ]
    if let model = snapshot.sandModelExperiment {
        value["sandModelExperiment"] = [
            "active": model.active,
            "arm": model.arm.rawValue,
        ]
    } else {
        value["sandModelExperiment"] = NSNull()
    }
    return value
}

private struct SandHydratedExperiment {
    var values: [String: IOSExperimentConfigValue]
    var groupName: String?
}

private func iosExperimentValue(_ raw: Any) -> IOSExperimentConfigValue? {
    if raw is NSNull { return .null }
    if let value = raw as? Bool { return .bool(value) }
    if let value = raw as? NSNumber { return .number(value.doubleValue) }
    if let value = raw as? String { return .string(value) }
    if let value = raw as? [Any] {
        let mapped = value.compactMap(iosExperimentValue)
        return mapped.count == value.count ? .array(mapped) : nil
    }
    if let value = raw as? [String: Any] {
        var mapped: [String: IOSExperimentConfigValue] = [:]
        for (key, item) in value {
            guard let projected = iosExperimentValue(item) else { return nil }
            mapped[key] = projected
        }
        return .object(mapped)
    }
    return nil
}

private func iosExperimentObject(_ raw: Any?) -> [String: IOSExperimentConfigValue]? {
    guard let raw = raw as? [String: Any] else { return nil }
    var result: [String: IOSExperimentConfigValue] = [:]
    for (key, value) in raw {
        guard let projected = iosExperimentValue(value) else { return nil }
        result[key] = projected
    }
    return result
}

private func foundationValue(_ value: IOSExperimentConfigValue) -> Any {
    switch value {
    case .null: return NSNull()
    case .bool(let value): return value
    case .number(let value): return value
    case .string(let value): return value
    case .array(let values): return values.map(foundationValue)
    case .object(let values): return values.mapValues(foundationValue)
    }
}

private func foundationObject(_ value: [String: IOSExperimentConfigValue]) -> [String: Any] {
    value.mapValues(foundationValue)
}

private func namedStatsigRecords(_ raw: Any?) -> [String: [String: Any]] {
    if let dictionary = raw as? [String: Any] {
        return dictionary.reduce(into: [:]) { result, pair in
            if let record = pair.value as? [String: Any] {
                result[pair.key] = record
            } else {
                result[pair.key] = ["value": pair.value]
            }
        }
    }
    if let array = raw as? [[String: Any]] {
        return array.reduce(into: [:]) { result, record in
            guard let name = record["name"] as? String else { return }
            result[name] = record
        }
    }
    return [:]
}

final class SandExperimentService: @unchecked Sendable {
    private let lock = NSLock()
    private let overrideStore: SandFeatureFlagOverrideStore
    private let getCacheDir: @Sendable () -> String
    private let env: [String: String]
    private let isDevBuild: Bool
    private let productFeatureGateDefaults: [String: Bool]
    private let bootstrapFetcher: SandExperimentBootstrapFetcher

    private var remoteGates: [String: Bool] = [:]
    private var experiments: [String: SandHydratedExperiment] = [:]
    private var dynamicConfigs: [String: [String: IOSExperimentConfigValue]] = [:]
    private var listeners: [UUID: @Sendable (SandExperimentSnapshot) -> Void] = [:]
    private var gateProperties: [String: MutableGateProperty] = [:]
    private var lastHydratedUserId: String?
    private var flagsFetchedAtMs: Int64?
    private var hasLiveNetworkBootstrap = false
    private var hasAuthenticatedNetworkBootstrap = false
    private var isAnysphereUser = false
    private var disposed = false
    private var pollTask: Task<Void, Never>?
    private var authRevision: UInt64 = 0
    private var refreshRevision: UInt64 = 0
    private var rateLimitedUntilMs: Int64?

    init(
        getCacheDir: @escaping @Sendable () -> String,
        isDevBuild: Bool = false,
        env: [String: String] = ProcessInfo.processInfo.environment,
        productFeatureGateDefaults: [String: Bool] = [:],
        bootstrapFetcher: @escaping SandExperimentBootstrapFetcher = { request in
            try await fetchStatsigBootstrap(
                backendUrl: request.backendUrl,
                getAccessToken: request.getAccessToken,
                getMachineId: request.getMachineId,
                env: request.env,
                timeoutSeconds: request.timeoutSeconds,
                session: request.session
            )
        }
    ) {
        self.getCacheDir = getCacheDir
        self.isDevBuild = isDevBuild
        self.env = env
        self.productFeatureGateDefaults = productFeatureGateDefaults
        self.bootstrapFetcher = bootstrapFetcher
        overrideStore = SandFeatureFlagOverrideStore(getCacheDir: getCacheDir)
        if isDevBuild { overrideStore.hydrateFromDisk() }
    }

    func startFromCache() {
        if let cached = loadCachedBootstrap(getCacheDir()) {
            hydrate(config: cached.config, fetchedAtMs: cached.fetchedAtMs, live: false)
        } else {
            publishSnapshot()
        }
    }

    func startPolling(
        refresh: @escaping @Sendable () async -> Void,
        randomRoll: @escaping @Sendable () -> Double = { Double.random(in: 0...1) }
    ) {
        stopPolling()
        let interval = jitteredSandExperimentPollIntervalMs(
            isDevBuild: isDevBuild,
            randomRoll: randomRoll()
        )
        pollTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .milliseconds(interval))
                guard let self, !self.isDisposed() else { return }
                await refresh()
            }
        }
    }

    func stopPolling() {
        lock.lock()
        let task = pollTask
        pollTask = nil
        lock.unlock()
        task?.cancel()
    }

    func handleAuthChange() {
        lock.lock()
        authRevision = authRevision == UInt64.max ? 1 : authRevision + 1
        refreshRevision = refreshRevision == UInt64.max ? 1 : refreshRevision + 1
        rateLimitedUntilMs = nil
        lock.unlock()
    }

    func resetFeatureFlagOverridesForAccountTransition() {
        overrideStore.clearAll()
        overrideStore.persist()
        refreshGateProperties()
        publishSnapshot()
    }

    func refresh(
        backendUrl: String,
        getAccessToken: @escaping @Sendable (String) async throws -> String,
        getMachineId: @escaping @Sendable () async throws -> String,
        trigger: SandExperimentRefreshTrigger = .manual,
        session: URLSession = .shared
    ) async {
        let nowMs = Int64(Date().timeIntervalSince1970 * 1_000)
        lock.lock()
        if disposed {
            lock.unlock()
            return
        }
        if trigger != .authChange,
           let until = rateLimitedUntilMs,
           until > nowMs {
            lock.unlock()
            reportExperimentsDiagnostic(.init(
                kind: "bootstrap_rate_limit_wait",
                metadata: [
                    "trigger": trigger.rawValue,
                    "retryAfterMs": String(until - nowMs),
                ]
            ))
            return
        }
        refreshRevision = refreshRevision == UInt64.max ? 1 : refreshRevision + 1
        let expectedRefreshRevision = refreshRevision
        let expectedAuthRevision = authRevision
        lock.unlock()

        do {
            try Task.checkCancellation()
            let result = try await bootstrapFetcher(.init(
                backendUrl: backendUrl,
                getAccessToken: getAccessToken,
                getMachineId: getMachineId,
                env: env,
                timeoutSeconds: Double(BOOTSTRAP_TIMEOUT_MS) / 1_000,
                session: session
            ))
            try Task.checkCancellation()

            if let retryAfterMs = result.retryAfterMs {
                lock.lock()
                if !disposed,
                   refreshRevision == expectedRefreshRevision,
                   authRevision == expectedAuthRevision {
                    rateLimitedUntilMs = Int64(Date().timeIntervalSince1970 * 1_000)
                        + Int64(max(0, retryAfterMs))
                }
                lock.unlock()
                reportExperimentsDiagnostic(.init(
                    kind: "bootstrap_rate_limited",
                    metadata: [
                        "trigger": trigger.rawValue,
                        "retryAfterMs": String(retryAfterMs),
                    ]
                ))
            }
            guard let config = result.config else { return }
            let fetchedAt = Int64(Date().timeIntervalSince1970 * 1_000)
            guard hydrate(
                config: config,
                fetchedAtMs: fetchedAt,
                live: true,
                expectedRefreshRevision: expectedRefreshRevision,
                expectedAuthRevision: expectedAuthRevision
            ) else {
                reportExperimentsDiagnostic(.init(
                    kind: "bootstrap_discarded_auth_changed",
                    metadata: ["trigger": trigger.rawValue]
                ))
                return
            }
            lock.lock()
            rateLimitedUntilMs = nil
            lock.unlock()
            saveCachedBootstrap(
                getCacheDir(),
                cache: .init(
                    config: config,
                    userId: readStatsigBootstrapUserId(config),
                    fetchedAtMs: fetchedAt
                )
            )
            reportExperimentsDiagnostic(.init(
                kind: "bootstrap_resolved",
                metadata: [
                    "trigger": trigger.rawValue,
                    "authenticated": hasAuthenticatedStatsigBootstrap() ? "true" : "false",
                    "gatesOnCount": String(getSnapshot().featureGates.values.filter { $0 }.count),
                ]
            ))
        } catch is CancellationError {
            reportExperimentsDiagnostic(.init(
                kind: "bootstrap_cancelled",
                metadata: ["trigger": trigger.rawValue]
            ))
        } catch {
            reportExperimentsDiagnostic(.init(
                kind: "bootstrap_failed",
                metadata: [
                    "trigger": trigger.rawValue,
                    "errorClass": errorLogTag(error),
                ]
            ))
        }
    }

    @discardableResult
    func hydrate(
        config: String,
        fetchedAtMs: Int64? = nil,
        live: Bool = true,
        expectedRefreshRevision: UInt64? = nil,
        expectedAuthRevision: UInt64? = nil
    ) -> Bool {    func hydrate(config: String, fetchedAtMs: Int64? = nil, live: Bool = true) {
        guard let data = config.data(using: .utf8),
              let root = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            reportExperimentsDiagnostic(.init(kind: "bootstrap_hydrate_failed"))
            return false
        }

        var gates: [String: Bool] = [:]
        for (name, record) in namedStatsigRecords(
            root["feature_gates"] ?? root["featureGates"] ?? root["gates"]
        ) {
            if let value = record["value"] as? Bool { gates[name] = value }
        }

        var hydratedExperiments: [String: SandHydratedExperiment] = [:]
        for (name, record) in namedStatsigRecords(root["experiments"]) {
            let values = iosExperimentObject(record["value"]) ?? [:]
            hydratedExperiments[name] = .init(
                values: values,
                groupName: record["groupName"] as? String ?? record["group_name"] as? String
            )
        }

        var configs: [String: [String: IOSExperimentConfigValue]] = [:]
        for (name, record) in namedStatsigRecords(
            root["dynamic_configs"] ?? root["dynamicConfigs"]
        ) {
            if let value = iosExperimentObject(record["value"]) {
                configs[name] = value
            }
        }

        let userId = readStatsigBootstrapUserId(config)
        lock.lock()
        if let expectedRefreshRevision,
           refreshRevision != expectedRefreshRevision {
            lock.unlock()
            return false
        }
        if let expectedAuthRevision,
           authRevision != expectedAuthRevision {
            lock.unlock()
            return false
        }
        remoteGates = gates
        experiments = hydratedExperiments
        dynamicConfigs = configs
        lastHydratedUserId = userId
        flagsFetchedAtMs = fetchedAtMs
        hasLiveNetworkBootstrap = live
        hasAuthenticatedNetworkBootstrap = live && !(userId?.isEmpty ?? true)
        lock.unlock()
        refreshGateProperties()
        publishSnapshot()
        return true
    }

    func subscribe(_ listener: @escaping @Sendable (SandExperimentSnapshot) -> Void) -> () -> Void {
        let id = UUID()
        lock.lock()
        listeners[id] = listener
        lock.unlock()
        return { [weak self] in
            self?.removeListener(id)
        }
    }

    private func removeListener(_ id: UUID) {
        lock.lock()
        listeners.removeValue(forKey: id)
        lock.unlock()
    }

    func getSnapshot() -> SandExperimentSnapshot {
        let gates = GROK_FEATURE_FLAG_NAMES.reduce(into: [String: Bool]()) {
            $0[$1] = checkFeatureGate($1)
        }
        let currentExperiments: [String: [String: IOSExperimentConfigValue]]
        let currentConfigs: [String: [String: IOSExperimentConfigValue]]
        let initialized: Bool
        lock.lock()
        currentExperiments = experiments.mapValues(\.values)
        currentConfigs = dynamicConfigs
        initialized = hasLiveNetworkBootstrap || flagsFetchedAtMs != nil
        lock.unlock()

        let modelFilter = getDynamicConfig(SAND_MODEL_FILTER_CONFIG_NAME)["allowedModelIds"]
            .flatMap(parseExperimentStringArray) ?? []
        return .init(
            isInitialized: initialized,
            featureGates: gates,
            experiments: currentExperiments,
            dynamicConfigs: currentConfigs,
            sandModelExperiment: getSandModelExperimentState(),
            sandModelFilterAllowedIds: modelFilter,
            featureFlagOverrides: overrideStore.activeOverrides()
        )
    }

    func checkFeatureGate(_ name: String) -> Bool {
        if let local = overrideStore.read(name) { return local }
        if canUseFeatureFlagOverrides(), let environment = envGateOverride(name, env: env) {
            return environment
        }
        lock.lock()
        let remote = remoteGates[name]
        lock.unlock()
        return remote
            ?? productFeatureGateDefaults[name]
            ?? BUNDLED_FEATURE_FLAGS[name]?.defaultValue
            ?? false
    }

    func getFeatureGateProperty(_ name: String) -> MutableGateProperty {
        lock.lock()
        if let existing = gateProperties[name] {
            lock.unlock()
            return existing
        }
        lock.unlock()

        let created = MutableGateProperty(checkFeatureGate(name))
        lock.lock()
        if let existing = gateProperties[name] {
            lock.unlock()
            return existing
        }
        gateProperties[name] = created
        lock.unlock()
        return created
    }

    func pinGateOnAuthenticatedBootstrap(
        _ name: String,
        pin: @escaping @Sendable (Bool) -> Void
    ) {
        if hasAuthenticatedStatsigBootstrap() {
            pin(checkFeatureGate(name))
            return
        }
        let listenerId = UUID()
        lock.lock()
        listeners[listenerId] = { [weak self] _ in
            guard let self, self.hasAuthenticatedStatsigBootstrap() else { return }
            self.removeListener(listenerId)
            pin(self.checkFeatureGate(name))
        }
        lock.unlock()
    }

    func hasAuthenticatedStatsigBootstrap() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return hasAuthenticatedNetworkBootstrap
    }

    func hasLiveStatsigBootstrap() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return hasLiveNetworkBootstrap
    }

    func hasHydratedStatsigUserId() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return !(lastHydratedUserId?.isEmpty ?? true)
    }

    func getFlagsAgeMs(nowMs: Int64 = Int64(Date().timeIntervalSince1970 * 1_000)) -> Int64? {
        lock.lock()
        let fetched = flagsFetchedAtMs
        lock.unlock()
        return fetched.map { max(0, nowMs - $0) }
    }

    func canUseFeatureFlagOverrides() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return isDevBuild || isAnysphereUser
    }

    func setIsAnysphereUser(_ value: Bool) {
        lock.lock()
        let before = isDevBuild || isAnysphereUser
        isAnysphereUser = value
        let after = isDevBuild || isAnysphereUser
        lock.unlock()
        guard before != after else { return }
        if after { overrideStore.hydrateFromDisk() }
        else { overrideStore.clearAll() }
        refreshGateProperties()
        publishSnapshot()
    }

    func setFeatureFlagOverride(_ name: String, value: Bool) {
        guard canUseFeatureFlagOverrides(), overrideStore.set(name, value: value) else { return }
        overrideStore.persist()
        refreshGateProperties()
        publishSnapshot()
    }

    func clearFeatureFlagOverride(_ name: String) {
        guard canUseFeatureFlagOverrides(), overrideStore.clear(name) else { return }
        overrideStore.persist()
        refreshGateProperties()
        publishSnapshot()
    }

    func clearAllFeatureFlagOverrides() {
        guard canUseFeatureFlagOverrides() else { return }
        overrideStore.clearAll()
        overrideStore.persist()
        refreshGateProperties()
        publishSnapshot()
    }

    func setAllFeatureFlagOverridesToBundledValues() {
        guard canUseFeatureFlagOverrides() else { return }
        overrideStore.setAllToBundledDefaults()
        overrideStore.persist()
        refreshGateProperties()
        publishSnapshot()
    }

    func replaceFeatureFlagOverrides(_ replacements: [String: Bool]) {
        overrideStore.replaceAll(replacements)
        overrideStore.persist()
        refreshGateProperties()
        publishSnapshot()
    }

    func getFeatureFlagOverridesRecord() -> [String: Bool] {
        overrideStore.activeOverrides()
    }

    func applyFeatureFlagOverrideCommand(_ command: [String: Any]) {
        guard let kind = command["kind"] as? String else { return }
        switch kind {
        case "set":
            guard let name = command["name"] as? String,
                  let value = command["value"] as? Bool else { return }
            setFeatureFlagOverride(name, value: value)
        case "clear":
            guard let name = command["name"] as? String else { return }
            clearFeatureFlagOverride(name)
        case "clear-all":
            clearAllFeatureFlagOverrides()
        case "set-all-bundled":
            setAllFeatureFlagOverridesToBundledValues()
        default:
            return
        }
    }

    func getExperiment(_ name: String) -> [String: IOSExperimentConfigValue] {
        let fallback = IOS_EXPERIMENT_FALLBACKS[name] ?? [:]
        lock.lock()
        let hydrated = experiments[name]?.values
        lock.unlock()
        return fallback.merging(hydrated ?? [:]) { _, remote in remote }
    }

    func getExperimentGroupName(_ name: String) -> String? {
        lock.lock()
        defer { lock.unlock() }
        return experiments[name]?.groupName
    }

    func getDynamicConfig(_ name: String) -> [String: IOSExperimentConfigValue] {
        let fallback = IOS_DYNAMIC_CONFIG_FALLBACKS[name] ?? [:]
        lock.lock()
        let hydrated = dynamicConfigs[name]
        lock.unlock()
        return fallback.merging(hydrated ?? [:]) { _, remote in remote }
    }

    func getSandModelExperimentState() -> SandModelExperimentState? {
        if let environment = readSandModelExperimentEnvOverride(env) { return environment }
        guard hasHydratedStatsigUserId(),
              GROK_EXPERIMENT_NAMES.contains(SAND_MODEL_EXPERIMENT_NAME) else { return nil }
        return resolveSandModelExperimentState(
            groupName: getExperimentGroupName(SAND_MODEL_EXPERIMENT_NAME),
            enabled: getExperiment(SAND_MODEL_EXPERIMENT_NAME)["enabled"]?.boolValue == true
        )
    }

    func getConfiguredDefaultModel() -> SandAgentModelSelection? {
        resolveConfiguredModel(SAND_DEFAULT_MODEL_CONFIG_NAME)
    }

    func getConfiguredAutomationsModel() -> SandAgentModelSelection? {
        resolveConfiguredModel(SAND_AUTOMATIONS_MODEL_CONFIG_NAME)
    }

    private func resolveConfiguredModel(_ name: String) -> SandAgentModelSelection? {
        let resolution = resolveSandDefaultModelConfig(
            raw: foundationObject(getDynamicConfig(name)),
            hasHydratedStatsigUserId: hasHydratedStatsigUserId()
        )
        if let rejection = resolution.rejection {
            reportExperimentsDiagnostic(.init(
                kind: "config_not_applied",
                metadata: ["stage": name, "reason": rejection.rawValue]
            ))
        }
        return resolution.selection
    }

    func getComputerUseModelOverride() -> SandAgentModelSelection? {
        guard checkFeatureGate("sand_computer_use_playwright") else { return nil }
        return modelSelection(from: getDynamicConfig("sand_computer_use_playwright_config"))
    }

    func getBrowserUseModelOverride() -> SandAgentModelSelection? {
        guard checkFeatureGate("sand_browser_use_subagent") else { return nil }
        return modelSelection(from: getDynamicConfig("sand_browser_use_model"))
    }

    private func modelSelection(
        from config: [String: IOSExperimentConfigValue]
    ) -> SandAgentModelSelection? {
        resolveSandDefaultModelConfig(
            raw: foundationObject(config),
            hasHydratedStatsigUserId: true
        ).selection
    }

    func dispose() {
        stopPolling()
        lock.lock()
        disposed = true
        authRevision = authRevision == UInt64.max ? 1 : authRevision + 1
        refreshRevision = refreshRevision == UInt64.max ? 1 : refreshRevision + 1
        rateLimitedUntilMs = nil
        listeners.removeAll()
        let properties = Array(gateProperties.values)
        gateProperties.removeAll()
        remoteGates.removeAll()
        experiments.removeAll()
        dynamicConfigs.removeAll()
        hasLiveNetworkBootstrap = false
        hasAuthenticatedNetworkBootstrap = false
        lock.unlock()
        for property in properties { property.clearListeners() }
    }

    private func isDisposed() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return disposed
    }

    private func refreshGateProperties() {
        lock.lock()
        let properties = gateProperties
        lock.unlock()
        for (name, property) in properties {
            property.set(checkFeatureGate(name))
        }
    }

    private func publishSnapshot() {
        let snapshot = getSnapshot()
        lock.lock()
        let callbacks = Array(listeners.values)
        lock.unlock()
        for callback in callbacks { callback(snapshot) }
    }
}
