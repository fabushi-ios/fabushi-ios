import Foundation

let PRIVACY_MODE_CACHE_MAX_AGE_MS: Int64 = 5 * 60_000
let PRIVACY_MODE_FALLBACK_CACHE_MAX_AGE_MS: Int64 = 10_000
let PRIVACY_MODE_FETCH_TIMEOUT_MS = 3_000
let SAND_RUN_PRIVACY_MODE_FALLBACK: SandPrivacyMode = .noTraining

struct SandRequestedModel: Equatable, Sendable {
    let modelId: String
    let maxMode: Bool
    let parameters: [SandAgentModelParameter]
}

func createSandRequestedModelFromSelection(
    _ selection: SandAgentModelSelection
) -> SandRequestedModel {
    .init(
        modelId: selection.modelId,
        maxMode: selection.maxMode,
        parameters: selection.parameters
    )
}

func createSandDefaultRequestedModel() -> SandRequestedModel {
    createSandRequestedModelFromSelection(SAND_DEFAULT_MODEL_SELECTION)
}

func createSandSubagentRequestedModel(_ modelId: String) -> SandRequestedModel {
    .init(modelId: modelId, maxMode: true, parameters: [])
}

func createSandComputerUseRequestedModel(_ modelId: String) -> SandRequestedModel {
    createSandRequestedModelFromSelection(.init(
        modelId: modelId,
        maxMode: SAND_COMPUTER_USE_MODEL_SELECTION.maxMode,
        parameters: SAND_COMPUTER_USE_MODEL_SELECTION.parameters
    ))
}

func enhancedObfuscate(_ input: [UInt8]) -> [UInt8] {
    var bytes = input
    var lastByte: UInt8 = 165
    for index in bytes.indices {
        let current = bytes[index]
        bytes[index] = (current ^ lastByte) &+ UInt8(index & 255)
        lastByte = bytes[index]
    }
    return bytes
}

func getSandGhostModeHeaderFromPrivacyMode(
    _ privacyMode: SandPrivacyMode?
) -> String {
    switch privacyMode {
    case .usageDataTrainingAllowed, .usageCodebaseTrainingAllowed:
        return "false"
    default:
        return "true"
    }
}

struct PrivacyLookupOptions: Equatable, Sendable {
    let backendUrl: String
    let accessToken: String
    let machineId: String
}

typealias PrivacyModeFetcher = @Sendable (PrivacyLookupOptions) async throws -> SandPrivacyMode?

private final class SandPrivacyModeCacheState: @unchecked Sendable {
    struct Entry {
        let id: UUID
        let backendUrl: String
        let accountScope: String
        let value: Task<SandPrivacyMode?, Never>
        var expiresAtMs: Int64?
    }

    let lock = NSLock()
    var entry: Entry?
}

private let SAND_PRIVACY_MODE_CACHE = SandPrivacyModeCacheState()

func settlePrivacyMode(
    fetchPrivacyMode: @escaping PrivacyModeFetcher,
    options: PrivacyLookupOptions,
    log: @escaping @Sendable (String) -> Void = { _ in }
) async -> SandPrivacyMode? {
    do {
        return try await fetchPrivacyMode(options)
    } catch {
        log(
            "[sand:privacy] privacy-mode lookup failed, using privacy-safe fallback " +
            "backend=\(options.backendUrl) error=\(errorLogTag(error))"
        )
        return nil
    }
}

func resolveCachedSandPrivacyMode(
    options: PrivacyLookupOptions,
    fetchPrivacyMode: @escaping PrivacyModeFetcher,
    nowMs: @escaping @Sendable () -> Int64 = {
        Int64(Date().timeIntervalSince1970 * 1_000)
    },
    log: @escaping @Sendable (String) -> Void = { _ in }
) async -> SandPrivacyMode? {
    let accountScope = accountCacheScope(options.accessToken)

    let resolution = SAND_PRIVACY_MODE_CACHE.lock.withLock { () -> (
        task: Task<SandPrivacyMode?, Never>,
        id: UUID?,
        startedAtMs: Int64?
    ) in
        if let cached = SAND_PRIVACY_MODE_CACHE.entry,
           cached.backendUrl == options.backendUrl,
           cached.accountScope == accountScope,
           cached.expiresAtMs == nil || nowMs() <= cached.expiresAtMs! {
            return (cached.value, nil, nil)
        }

        let startedAtMs = nowMs()
        let id = UUID()
        let task = Task {
            await settlePrivacyMode(
                fetchPrivacyMode: fetchPrivacyMode,
                options: options,
                log: log
            )
        }
        SAND_PRIVACY_MODE_CACHE.entry = .init(
            id: id,
            backendUrl: options.backendUrl,
            accountScope: accountScope,
            value: task,
            expiresAtMs: nil
        )
        return (task, id, startedAtMs)
    }

    let privacyMode = await resolution.task.value
    guard let id = resolution.id, let startedAtMs = resolution.startedAtMs else {
        return privacyMode
    }
    let ttl = privacyMode == nil
        ? PRIVACY_MODE_FALLBACK_CACHE_MAX_AGE_MS
        : PRIVACY_MODE_CACHE_MAX_AGE_MS

    SAND_PRIVACY_MODE_CACHE.lock.withLock {
        if var current = SAND_PRIVACY_MODE_CACHE.entry, current.id == id {
            current.expiresAtMs = startedAtMs + ttl
            SAND_PRIVACY_MODE_CACHE.entry = current
        }
    }
    return privacyMode
}

func clearSandPrivacyModeCacheForTesting() {
    let task = SAND_PRIVACY_MODE_CACHE.lock.withLock {
        let task = SAND_PRIVACY_MODE_CACHE.entry?.value
        SAND_PRIVACY_MODE_CACHE.entry = nil
        return task
    }
    task?.cancel()
}

func resolveSandPrivacyMode(
    options: PrivacyLookupOptions,
    fetchPrivacyMode: @escaping PrivacyModeFetcher
) async -> SandPrivacyMode? {
    await resolveCachedSandPrivacyMode(
        options: options,
        fetchPrivacyMode: fetchPrivacyMode
    )
}

func resolveSandRunPrivacyMode(
    backendUrl: String = getConfiguredBackendUrl(),
    getAccessToken: @escaping @Sendable (String) async throws -> String,
    getMachineId: @escaping @Sendable () async throws -> String,
    fetchPrivacyMode: @escaping PrivacyModeFetcher
) async -> SandPrivacyMode {
    do {
        async let tokenValue = getAccessToken(backendUrl)
        async let machineValue = getMachineId()
        let (accessToken, machineId) = try await (tokenValue, machineValue)
        let accountScopeAtStart = accountCacheScope(accessToken)
        let privacyMode = await resolveCachedSandPrivacyMode(
            options: .init(
                backendUrl: backendUrl,
                accessToken: accessToken,
                machineId: machineId
            ),
            fetchPrivacyMode: fetchPrivacyMode
        )
        let currentToken = try await getAccessToken(backendUrl)
        guard accountCacheScope(currentToken) == accountScopeAtStart else {
            return SAND_RUN_PRIVACY_MODE_FALLBACK
        }
        guard let privacyMode else { return SAND_RUN_PRIVACY_MODE_FALLBACK }
        switch privacyMode {
        case .noStorage, .noTraining, .usageDataTrainingAllowed, .usageCodebaseTrainingAllowed:
            return privacyMode
        case .unspecified:
            return SAND_RUN_PRIVACY_MODE_FALLBACK
        }
    } catch {
        return SAND_RUN_PRIVACY_MODE_FALLBACK
    }
}

func resolveSandGhostModeHeader(
    options: PrivacyLookupOptions,
    fetchPrivacyMode: @escaping PrivacyModeFetcher
) async -> String {
    getSandGhostModeHeaderFromPrivacyMode(
        await resolveSandPrivacyMode(
            options: options,
            fetchPrivacyMode: fetchPrivacyMode
        )
    )
}

func getSandInferenceBackendUrl(
    _ env: [String: String] = ProcessInfo.processInfo.environment
) -> String {
    getConfiguredBackendUrl(env)
}

private func sanitizedHeaderValue(_ value: String) -> String {
    value.replacingOccurrences(of: "\r", with: "")
        .replacingOccurrences(of: "\n", with: "")
}

func sandLineageHeaders(_ lineage: SandRequestLineage?) -> [String: String] {
    guard let lineage else { return [:] }
    var headers = [
        "x-parent-request-id": sanitizedHeaderValue(lineage.parentRequestId),
        "x-root-parent-request-id": sanitizedHeaderValue(lineage.rootParentRequestId),
    ]
    if let toolCall = lineage.parentAgentToolCallId {
        headers["x-parent-agent-tool-call-id"] = sanitizedHeaderValue(toolCall)
    }
    return headers
}

enum SandInferenceAuthMode: Equatable, Sendable {
    case required
    case anonymous
}

struct SandInferenceHeaderResult: Equatable, Sendable {
    let requestId: String
    let headers: [String: String]
}

func createSandInferenceHeaders(
    backendUrl: String,
    authMode: SandInferenceAuthMode = .required,
    existingRequestId: String? = nil,
    lineage: SandRequestLineage? = nil,
    getAccessToken: @escaping @Sendable (String) async throws -> String,
    getMachineId: @escaping @Sendable () async throws -> String,
    resolveGhostMode: @escaping @Sendable (PrivacyLookupOptions) async -> String,
    randomUUID: @escaping @Sendable () -> String = { UUID().uuidString.lowercased() },
    env: [String: String] = ProcessInfo.processInfo.environment
) async throws -> SandInferenceHeaderResult {
    async let machineValue = getMachineId()
    let accessToken: String?
    switch authMode {
    case .anonymous:
        accessToken = nil
    case .required:
        accessToken = try await getAccessToken(backendUrl)
    }
    let machineId = try await machineValue

    let ghostMode: String
    if let accessToken {
        ghostMode = await resolveGhostMode(.init(
            backendUrl: backendUrl,
            accessToken: accessToken,
            machineId: machineId
        ))
    } else {
        ghostMode = "true"
    }

    let pinned = existingRequestId?.trimmingCharacters(in: .whitespacesAndNewlines)
    let requestId = pinned?.isEmpty == false ? pinned! : randomUUID()
    var headers = getSandBackendClientHeaders(env)
    headers["x-cursor-checksum"] = createCursorChecksum(machineId: machineId)
    headers["x-ghost-mode"] = ghostMode
    headers["x-request-id"] = requestId
    if let accessToken {
        headers["authorization"] = "Bearer \(accessToken)"
    }
    for (name, value) in sandLineageHeaders(lineage) {
        headers[name] = value
    }
    if env["CURSOR_AGENT_CLI_LOCAL_MODE"] == "true" {
        headers["local-cli-mode"] = "true"
    }
    return .init(requestId: requestId, headers: headers)
}

struct SandAttachedMediaRequest: Equatable, Sendable {
    let conversationId: String
    let key: String
    let mimeType: String
    var contentLengthBytes: Int64? = nil
}

struct SandAttachedMediaSignedURL: Equatable, Sendable {
    let key: String
    let putUrl: String
    let getUrl: String
    let expiresAtUnixMs: Int64
    let refreshAfterUnixMs: Int64
}

protocol SandAttachedMediaSigning: Sendable {
    func getSignedURLForAttachedMedia(
        _ request: SandAttachedMediaRequest
    ) async throws -> SandAttachedMediaSignedURL
}

struct SandAttachedMediaURLProvider: Sendable {
    let client: any SandAttachedMediaSigning

    func getSignedUrlForAttachedMedia(
        _ request: SandAttachedMediaRequest
    ) async throws -> SandAttachedMediaSignedURL {
        try await client.getSignedURLForAttachedMedia(request)
    }
}

enum SandPromptInferenceRoute: Equatable, Sendable {
    case cursor
    case provider(SandInferenceProvider)
}

func resolveSandPromptInferenceRoute(
    settings: SandSettingsStore
) -> SandPromptInferenceRoute {
    let provider = settings.getInferenceProvider()
    return provider == .cursor ? .cursor : .provider(provider)
}
