import Foundation

/// iOS-owned counterpart of Grok's node-agent-coordinator boundary.
///
/// The coordinator is the only production owner allowed to invoke the native
/// Mahayana Host. Renderer and platform code communicate through typed
/// Coordinator frames carried by IOSMainRuntime/IOSPreloadBridge.
enum CoordinatorDevControlRouting {
    case notHandled
    case handled(Any)
}

@MainActor
protocol CoordinatorDevControlAdapting: AnyObject {
    func route(method: String, params: [String: Any]) async throws -> CoordinatorDevControlRouting
    func beforeProductionRequest() async throws
    func coordinatorDidLaunch()
}

let IOS_SAND_CLIENT_PAUSE_GATE = "sand_client_pause"
let IOS_SAND_CLIENT_PAUSE_BLOCKED_MESSAGE = "sand box blocked by kill switch: SAND_CLIENT_PAUSE\u{001F}\u{001F}"

struct IOSClientPausedError: LocalizedError, Equatable, Sendable {
    var errorDescription: String? { IOS_SAND_CLIENT_PAUSE_BLOCKED_MESSAGE }
}

@MainActor
final class IOSCoordinatorClientPauseControl {
    typealias IsPaused = @MainActor () -> Bool
    typealias ApplyCoordinatorPause = @MainActor (Bool) async throws -> Bool
    typealias DropObservedConnection = @MainActor () -> Void

    private let isPausedProvider: IsPaused
    private let applyCoordinatorPause: ApplyCoordinatorPause
    private let dropObservedConnection: DropObservedConnection
    private var coordinatorPaused = false
    private var egressDroppedForPause = false
    private var serialTail: Task<Void, Never>?
    private var lastSyncError: Error?

    init(
        isPaused: @escaping IsPaused,
        applyCoordinatorPause: @escaping ApplyCoordinatorPause,
        dropObservedConnection: @escaping DropObservedConnection
    ) {
        isPausedProvider = isPaused
        self.applyCoordinatorPause = applyCoordinatorPause
        self.dropObservedConnection = dropObservedConnection
    }

    var isPaused: Bool { isPausedProvider() }

    func synchronize() async throws {
        let desired = isPausedProvider()
        if desired && !egressDroppedForPause {
            dropObservedConnection()
        }
        egressDroppedForPause = desired

        let previous = serialTail
        let operation = Task { @MainActor [weak self] in
            if let previous { await previous.value }
            guard let self, self.coordinatorPaused != desired else { return }
            do {
                self.coordinatorPaused = try await self.applyCoordinatorPause(desired)
            } catch {
                self.lastSyncError = error
            }
        }
        serialTail = operation
        await operation.value
        if let error = lastSyncError {
            lastSyncError = nil
            throw error
        }
    }

    func reapplyAfterCoordinatorLaunch() {
        coordinatorPaused = false
    }
}

@MainActor
final class IOSHostSettingsReconciler {
    typealias ReadLocal = @MainActor () -> Bool?
    typealias WriteLocal = @MainActor (Bool) -> Void
    typealias ClearLocal = @MainActor () -> Void
    typealias ReadRemote = @MainActor () async throws -> MahayanaHostSettingsSnapshot
    typealias PushRemote = @MainActor (MahayanaHostSettingsSnapshot) async throws -> MahayanaHostSettingsSnapshot
    typealias HostGeneration = @MainActor () -> UInt64

    private enum SyncError: Error {
        case unavailableOrStale
    }

    private struct Token: Equatable {
        let accountScope: String
        let epoch: UInt64
        let hostGeneration: UInt64
    }

    private let readLocal: ReadLocal
    private let writeLocal: WriteLocal
    private let clearLocal: ClearLocal
    private let readRemote: ReadRemote
    private let pushRemote: PushRemote
    private let hostGeneration: HostGeneration

    private var accountScope: String?
    private var transportLive = false
    private var epoch: UInt64 = 1
    private var inFlight: Task<Void, Never>?
    private var lastSuccessfulEpoch: UInt64?

    private(set) var lastSuccessfulAccountScope: String?

    private lazy var onboardingSeen = BoxSettingsField<MahayanaHostSettingsSnapshot, Bool>(
        port: HostSettingsPort(
            isReadable: { [weak self] in
                self?.isReadable == true
            },
            read: { [weak self] in
                guard let self else { throw SyncError.unavailableOrStale }
                return try await self.readRemoteCurrent()
            },
            write: { [weak self] value in
                guard let self else { throw SyncError.unavailableOrStale }
                return try await self.writeRemoteCurrent(value)
            },
            value: { $0.hasSeenOnboarding }
        ),
        mirror: HostSettingsMirror(
            read: { [weak self] in self?.readLocal() },
            write: { [weak self] value in self?.writeLocal(value) },
            clear: { [weak self] in self?.clearLocal() }
        )
    )

    init(
        readLocal: @escaping ReadLocal,
        writeLocal: @escaping WriteLocal,
        clearLocal: @escaping ClearLocal = {},
        readRemote: @escaping ReadRemote,
        pushRemote: @escaping PushRemote,
        hostGeneration: @escaping HostGeneration
    ) {
        self.readLocal = readLocal
        self.writeLocal = writeLocal
        self.clearLocal = clearLocal
        self.readRemote = readRemote
        self.pushRemote = pushRemote
        self.hostGeneration = hostGeneration
    }

    var isReadable: Bool {
        transportLive && accountScope != nil
    }

    func scopeToAccount(_ scope: String) {
        if accountScope == scope {
            if transportLive {
                scheduleReconcile()
            }
            return
        }
        invalidateInFlight(resetFieldSession: true)
        accountScope = scope
        lastSuccessfulAccountScope = nil
        lastSuccessfulEpoch = nil
        if transportLive {
            scheduleReconcile()
        }
    }

    func accountDeparted() {
        invalidateInFlight(resetFieldSession: true)
        accountScope = nil
        transportLive = false
        lastSuccessfulAccountScope = nil
        lastSuccessfulEpoch = nil
    }

    func setTransportLive(_ live: Bool) {
        // A transport transition fences async work, but it is not an account
        // departure. Keep BoxSettingsField's answered-this-session state so a
        // local answer made while the transport was down can be written back
        // when the Host reconnects with an unwritten field.
        invalidateInFlight(resetFieldSession: false)
        transportLive = live
        guard live, accountScope != nil else { return }
        scheduleReconcile()
    }

    func scheduleReconcile() {
        invalidateInFlight(resetFieldSession: false)
        guard tokenIfReadable() != nil else { return }
        inFlight = Task { @MainActor [weak self] in
            guard let self else { return }
            _ = await self.reconcileIfReadable()
        }
    }

    func scheduleLocalWrite(_ value: Bool) {
        invalidateInFlight(resetFieldSession: false)
        inFlight = Task { @MainActor [weak self] in
            guard let self else { return }
            _ = await self.pushLocalIfWritable(value)
        }
    }

    @discardableResult
    func reconcileIfReadable() async -> Bool {
        guard let token = tokenIfReadable() else { return false }
        _ = await onboardingSeen.absorbFromBox()
        return isCurrent(token) && lastSuccessfulEpoch == token.epoch
    }

    @discardableResult
    func pushLocalIfWritable(_ value: Bool) async -> Bool {
        // BoxSettingsField.apply writes the canonical local mirror first and
        // records that this account session answered the field. The typed port
        // below fails closed while transport is down, so no remote write is
        // attempted; a later absorbFromBox can safely backfill the unwritten
        // remote field without losing the local answer.
        let result = await onboardingSeen.apply(value)
        guard case .persisted(let echoed) = result,
              echoed == value,
              let token = tokenIfReadable(),
              lastSuccessfulEpoch == token.epoch,
              isCurrent(token)
        else {
            return false
        }
        return true
    }

    private func readRemoteCurrent() async throws -> MahayanaHostSettingsSnapshot {
        guard let token = tokenIfReadable() else {
            throw SyncError.unavailableOrStale
        }
        let value = try await readRemote()
        guard isCurrent(token) else {
            throw SyncError.unavailableOrStale
        }
        lastSuccessfulAccountScope = token.accountScope
        lastSuccessfulEpoch = token.epoch
        return value
    }

    private func writeRemoteCurrent(_ value: Bool) async throws -> MahayanaHostSettingsSnapshot? {
        guard let token = tokenIfReadable() else {
            throw SyncError.unavailableOrStale
        }
        let response = try await pushRemote(.init(hasSeenOnboarding: value))
        guard isCurrent(token) else {
            throw SyncError.unavailableOrStale
        }
        if response.hasSeenOnboarding == value {
            lastSuccessfulAccountScope = token.accountScope
            lastSuccessfulEpoch = token.epoch
        }
        return response
    }

    private func tokenIfReadable() -> Token? {
        guard transportLive, let accountScope else { return nil }
        return .init(
            accountScope: accountScope,
            epoch: epoch,
            hostGeneration: hostGeneration()
        )
    }

    private func isCurrent(_ token: Token) -> Bool {
        transportLive
            && accountScope == token.accountScope
            && epoch == token.epoch
            && hostGeneration() == token.hostGeneration
    }

    private func invalidateInFlight(resetFieldSession: Bool) {
        inFlight?.cancel()
        inFlight = nil
        epoch = epoch == UInt64.max ? 1 : epoch + 1
        if resetFieldSession {
            onboardingSeen.abandonInFlight()
        }
    }
}

@MainActor
final class MahayanaCoordinator {
    struct JSONResult: @unchecked Sendable {
        let value: Any
    }

    enum CoordinatorError: LocalizedError {
        case invalidResponse
        case invalidParams
        case unavailable
        case requestFailed(String)

        var errorDescription: String? {
            switch self {
            case .invalidResponse: "Mahayana Coordinator returned an invalid response"
            case .invalidParams: "Mahayana Coordinator request params must be a JSON object"
            case .unavailable: "Mahayana Coordinator is unavailable"
            case .requestFailed(let message): message
            }
        }
    }

    enum LifecycleState: Equatable, Sendable {
        case starting
        case ready
        case background
        case suspended
        case shuttingDown
        case failed(String)
    }

    private let hostSupervisor: MahayanaLocalHostSupervisor
    private let settingsStore: SandSettingsStore?
    private let hostSettingsReconciler: IOSHostSettingsReconciler?
    private let mcpSurface: CoordinatorMcpSurface?
    private let experimentService: SandExperimentService?
    private var clientPauseControl: IOSCoordinatorClientPauseControl?
    private let webAuthnSigner: CoordinatorWebAuthnSigner?
    private let devControlAdapter: (any CoordinatorDevControlAdapting)?
    private let clientSideToolV2Relay = ClientSideToolV2Relay()
    private let nativeLocalCapabilities = IOSNativeLocalCapabilityBackend()
    private var rendererEventSink: ((String, CoordinatorPayload) -> Void)?
    private var autoReviewHostSyncNeeded = true
    private(set) var lifecycleState: LifecycleState = .starting
    private var inFlight = Set<String>()

    init(
        hostSupervisor: MahayanaLocalHostSupervisor,
        passkeyProvider: (any PasskeyProviding)? = nil,
        settingsStore: SandSettingsStore? = nil,
        experimentService: SandExperimentService? = nil,
        devControlAdapter: (any CoordinatorDevControlAdapting)? = nil,
        mcpSurface: CoordinatorMcpSurface? = nil,
        mcpFailureReporter: @escaping IOSCursorLocalToolPermissionCeilingSynchronizer.ReportFailure = { _, _, _ in },
        mcpAuthTelemetryReporter: @escaping IOSAuthTelemetryRelay.Sink = { _ in }
    ) {
        self.hostSupervisor = hostSupervisor
        self.settingsStore = settingsStore
        self.hostSettingsReconciler = settingsStore.map { store in
            IOSHostSettingsReconciler(
                readLocal: { store.getHasSeenOnboarding() },
                writeLocal: { store.setHasSeenOnboarding($0) },
                clearLocal: { store.clearHasSeenOnboarding() },
                readRemote: { try await hostSupervisor.readHostSettings() },
                pushRemote: { try await hostSupervisor.pushHostSettings($0) },
                hostGeneration: { hostSupervisor.generation }
            )
        }
        self.experimentService = experimentService

        let isClientPaused: @MainActor () -> Bool = {
            experimentService?.checkFeatureGate(IOS_SAND_CLIENT_PAUSE_GATE)
                ?? BUNDLED_FEATURE_FLAGS[IOS_SAND_CLIENT_PAUSE_GATE]?.defaultValue
                ?? false
        }

        let resolvedMcpSurface: CoordinatorMcpSurface?
        if let mcpSurface {
            resolvedMcpSurface = mcpSurface
        } else if let settingsStore {
            resolvedMcpSurface = CoordinatorMcpSurface.make(
                hostSupervisor: hostSupervisor,
                settingsStore: settingsStore,
                reportFailure: mcpFailureReporter,
                reportAuthTelemetry: mcpAuthTelemetryReporter,
                isClientPaused: isClientPaused
            )
        } else {
            resolvedMcpSurface = nil
        }
        self.mcpSurface = resolvedMcpSurface
        self.devControlAdapter = devControlAdapter
        let pauseSurface = resolvedMcpSurface
        clientPauseControl = IOSCoordinatorClientPauseControl(
            isPaused: isClientPaused,
            applyCoordinatorPause: { paused in paused },
            dropObservedConnection: {
                pauseSurface?.dropObservedComputerConnectionForClientPause()
            }
        )
        webAuthnSigner = passkeyProvider.map {
            CoordinatorWebAuthnSigner(
                passkeys: CoordinatorPasskeyProvider(provider: $0)
            )
        }
        lifecycleState = .ready
    }

    var hostGeneration: UInt64 {
        hostSupervisor.generation
    }

    static func make(
        appDataDirectory: URL,
        featureHostTest: Bool = false,
        passkeyProvider: (any PasskeyProviding)? = nil,
        devControlAdapter: (any CoordinatorDevControlAdapting)? = nil,
        mcpFailureReporter: @escaping IOSCursorLocalToolPermissionCeilingSynchronizer.ReportFailure = { _, _, _ in },
        mcpAuthTelemetryReporter: @escaping IOSAuthTelemetryRelay.Sink = { _ in }
    ) throws -> MahayanaCoordinator {
        let experimentCacheDirectory = appDataDirectory
            .appendingPathComponent("experiments", isDirectory: true)
        let experimentService = SandExperimentService(
            getCacheDir: { experimentCacheDirectory.path },
            isDevBuild: featureHostTest,
            productFeatureGateDefaults: ["sand_agent_network": true]
        )
        experimentService.startFromCache()
        return MahayanaCoordinator(
            hostSupervisor: try MahayanaLocalHostSupervisor.make(
                appDataDirectory: appDataDirectory,
                featureHostTest: featureHostTest
            ),
            passkeyProvider: passkeyProvider,
            settingsStore: SandSettingsStore(
                settingsPath: appDataDirectory.appendingPathComponent("sand-settings.json").path
            ),
            experimentService: experimentService,
            devControlAdapter: devControlAdapter,
            mcpFailureReporter: mcpFailureReporter,
            mcpAuthTelemetryReporter: mcpAuthTelemetryReporter
        )
    }

    func sharedSettingsSnapshot() -> SandStoredSettings {
        settingsStore?.load() ?? emptySandSettings()
    }

    struct UiPreferencesProjection: Equatable, Sendable {
        let locale: String
        let direction: String
        let reducedMotion: Bool
        let highContrast: Bool
        let textScale: Double
    }

    struct CallMediaPreferencesProjection: Equatable, Sendable {
        let microphoneId: String?
        let cameraId: String?
    }

    func uiPreferencesProjection() -> UiPreferencesProjection {
        let value = settingsStore?.getUiPreferences()
            ?? normalizeSandUiPreferences(
                locale: nil,
                direction: nil,
                reducedMotion: nil,
                highContrast: nil,
                textScale: nil
            )
        return .init(
            locale: value.locale,
            direction: value.direction.rawValue,
            reducedMotion: value.reducedMotion,
            highContrast: value.highContrast,
            textScale: value.textScale
        )
    }

    func updateUiPreferences(
        locale: String,
        direction: String,
        reducedMotion: Bool,
        highContrast: Bool,
        textScale: Double
    ) -> UiPreferencesProjection {
        guard let settingsStore else { return uiPreferencesProjection() }
        settingsStore.setUiPreferences(
            normalizeSandUiPreferences(
                locale: locale,
                direction: direction,
                reducedMotion: reducedMotion,
                highContrast: highContrast,
                textScale: textScale
            )
        )
        return uiPreferencesProjection()
    }

    func callMediaPreferencesProjection() -> CallMediaPreferencesProjection {
        let value = settingsStore?.getCallMediaPreferences()
            ?? normalizeSandCallMediaPreferences(microphoneId: nil, cameraId: nil)
        return .init(
            microphoneId: value.microphoneId,
            cameraId: value.cameraId
        )
    }

    func updateCallMediaPreferences(
        microphoneId: String?,
        cameraId: String?
    ) -> CallMediaPreferencesProjection {
        guard let settingsStore else { return callMediaPreferencesProjection() }
        settingsStore.setCallMediaPreferences(
            normalizeSandCallMediaPreferences(
                microphoneId: microphoneId,
                cameraId: cameraId
            )
        )
        return callMediaPreferencesProjection()
    }


    func experimentSnapshot() -> SandExperimentSnapshot? {
        experimentService?.getSnapshot()
    }

    func featureGate(_ name: String) -> Bool {
        experimentService?.checkFeatureGate(name)
            ?? BUNDLED_FEATURE_FLAGS[name]?.defaultValue
            ?? false
    }

    func configuredDefaultModel() -> SandAgentModelSelection? {
        experimentService?.getConfiguredDefaultModel()
    }

    func setRendererEventSink(_ sink: ((String, CoordinatorPayload) -> Void)?) {
        rendererEventSink = sink
    }

    func replayClientSideToolEvents() {
        guard let rendererEventSink else { return }
        for event in clientSideToolV2Relay.replay() {
            rendererEventSink(ClientSideToolV2Transport.family, event.coordinatorPayload)
        }
    }

    func acceptHostEventEnvelope(_ value: Any) {
        guard let envelope = value as? [String: Any],
              envelope["channel"] as? String == ClientSideToolV2Transport.family,
              let payload = envelope["payload"],
              let transportEvent = ClientSideToolV2TransportEvent.fromFoundation(payload),
              let projected = clientSideToolV2Relay.accept(transportEvent)
        else { return }
        rendererEventSink?(ClientSideToolV2Transport.family, projected.coordinatorPayload)
    }

    func updateAccountSettingsScope(_ accountScope: String?) {
        if let accountScope {
            settingsStore?.scopeToAccount(accountScope)
            hostSettingsReconciler?.scopeToAccount(accountScope)
        } else {
            hostSettingsReconciler?.accountDeparted()
            settingsStore?.clearAccountScope()
        }
        autoReviewHostSyncNeeded = true
        mcpSurface?.updateAccountScope(accountScope)
    }

    private func autoReviewInstructionsObject() -> [String: Any] {
        let value = settingsStore?.getAutoReviewInstructions()
            ?? normalizeSandAutoReviewInstructions(isEnabled: nil, allowInstructions: nil, blockInstructions: nil)
        return [
            "isEnabled": value.isEnabled,
            "allowInstructions": value.allowInstructions,
            "blockInstructions": value.blockInstructions,
        ]
    }

    private func syncAutoReviewRulesToHost() async throws {
        let value = settingsStore?.getAutoReviewInstructions()
            ?? normalizeSandAutoReviewInstructions(isEnabled: nil, allowInstructions: nil, blockInstructions: nil)
        let rules: [[String: Any]]
        if value.isEnabled {
            let allow = value.allowInstructions.enumerated().map { index, text in
                ["id": "ios-auto-review-allow-\(index + 1)", "behavior": "allow", "text": text]
            }
            let ask = value.blockInstructions.enumerated().map { index, text in
                ["id": "ios-auto-review-ask-\(index + 1)", "behavior": "ask", "text": text]
            }
            rules = allow + ask
        } else {
            rules = []
        }
        _ = try await hostSupervisor.request(
            method: "feature.settings.autoReviewRules",
            params: ["rules": rules]
        )
        autoReviewHostSyncNeeded = false
    }

    func signPasskey(_ challenge: PasskeyChallenge) async throws -> CoordinatorPayload {
        guard let webAuthnSigner else {
            throw CoordinatorError.requestFailed("passkey_provider_unavailable")
        }
        return try await webAuthnSigner.sign(challenge)
    }

    func cloudAgentInfo(bcId: String) async throws -> IOSCloudAgentComposerInfo {
        guard let mcpSurface else {
            throw CoordinatorError.unavailable
        }
        do {
            return try await mcpSurface.cloudAgentInfo(bcId: bcId)
        } catch {
            throw CoordinatorError.requestFailed(error.localizedDescription)
        }
    }

    nonisolated static func isClientPauseBlockedMethod(_ method: String) -> Bool {
        switch method {
        case "forceReconnectGateway",
             "resolveGatewayConnection",
             "ensureForeverBox",
             "computer.agentBox.ensure",
             "getBoxMigrationStatus",
             "updateComputer",
             "forceRecreateComputer",
             "mintLocalExecDaemonCredential",
             "spawnLocalExecDaemon":
            return true
        default:
            return false
        }
    }

    /// Compatibility entry used while feature-specific typed facades are
    /// replacing dictionary-shaped calls. Host ownership remains here.
    func request(method: String, params: [String: Any] = [:]) async throws -> JSONResult {
        guard lifecycleState != .shuttingDown else { throw CoordinatorError.unavailable }
        if let clientPauseControl {
            try await clientPauseControl.synchronize()
            if clientPauseControl.isPaused && Self.isClientPauseBlockedMethod(method) {
                throw IOSClientPausedError()
            }
        }
        if method == "getCloudAgentInfo" {
            guard let rawBcId = params["bcId"] as? String,
                  params["includeFiles"] as? Bool == false
            else { throw CoordinatorError.invalidParams }
            let bcId = rawBcId.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !bcId.isEmpty else { throw CoordinatorError.invalidParams }
            let info = try await cloudAgentInfo(bcId: bcId)
            let status: String
            switch info.status {
            case 1: status = "running"
            case 2: status = "finished"
            case 3: status = "error"
            case 4: status = "creating"
            case 5: status = "expired"
            default: status = "unknown"
            }
            var value: [String: Any] = [
                "bcId": bcId,
                "status": status,
            ]
            if let name = info.name { value["name"] = name }
            if let prompt = info.prompt { value["prompt"] = prompt }
            if let branchName = info.branchName { value["branchName"] = branchName }
            if let filesChanged = info.filesChanged { value["filesChanged"] = filesChanged }
            if let linesAdded = info.linesAdded { value["linesAdded"] = linesAdded }
            if let linesRemoved = info.linesRemoved { value["linesRemoved"] = linesRemoved }
            if let prURL = info.prURL { value["prUrl"] = prURL }
            if let prState = info.prState { value["prState"] = prState }
            if let prNumber = info.prNumber { value["prNumber"] = Int(prNumber) }
            return JSONResult(value: value)
        }
        if method == "getOnboardingSeen" {
            guard let settingsStore else { throw CoordinatorError.unavailable }
            _ = await hostSettingsReconciler?.reconcileIfReadable()
            return JSONResult(value: settingsStore.getHasSeenOnboarding() == true)
        }
        if method == "setOnboardingSeen" {
            guard let settingsStore,
                  let seen = params["seen"] as? Bool
            else { throw CoordinatorError.invalidParams }
            settingsStore.setHasSeenOnboarding(seen)
            hostSettingsReconciler?.scheduleLocalWrite(seen)
            return JSONResult(value: seen)
        }
        if method == "getTimeZone" {
            guard let settingsStore else { throw CoordinatorError.unavailable }
            let detected = settingsStore.getDetectedUserTimeZone()
                ?? TimeZone.autoupdatingCurrent.identifier
            if settingsStore.getDetectedUserTimeZone() == nil {
                settingsStore.setUserTimeZone(detected)
            }
            let overrideTimeZone: Any = settingsStore.getUserTimeZoneOverride()
                .map { $0 as Any } ?? NSNull()
            return JSONResult(value: [
                "detectedTimeZone": detected,
                "overrideTimeZone": overrideTimeZone,
            ])
        }
        if method == "setTimeZoneOverride" {
            guard let settingsStore else { throw CoordinatorError.unavailable }
            let raw = params["timeZone"]
            let override: String?
            if raw == nil || raw is NSNull {
                override = nil
            } else if let string = raw as? String {
                let normalized = string.trimmingCharacters(in: .whitespacesAndNewlines)
                guard !normalized.isEmpty, TimeZone(identifier: normalized) != nil else {
                    throw CoordinatorError.requestFailed("Unknown IANA time zone.")
                }
                override = normalized
            } else {
                throw CoordinatorError.invalidParams
            }
            settingsStore.setUserTimeZoneOverride(override)
            let detected = settingsStore.getDetectedUserTimeZone()
                ?? TimeZone.autoupdatingCurrent.identifier
            if settingsStore.getDetectedUserTimeZone() == nil {
                settingsStore.setUserTimeZone(detected)
            }
            let overrideTimeZone: Any = settingsStore.getUserTimeZoneOverride()
                .map { $0 as Any } ?? NSNull()
            return JSONResult(value: [
                "detectedTimeZone": detected,
                "overrideTimeZone": overrideTimeZone,
            ])
        }
        if method == "getLocalToolPermission" {
            guard let settingsStore else { throw CoordinatorError.unavailable }
            return JSONResult(value: settingsStore.getLocalToolPermission())
        }
        if method == "getLocalToolPermissionCeiling" {
            guard let settingsStore else { throw CoordinatorError.unavailable }
            return JSONResult(value: settingsStore.getLocalToolPermissionCeiling() ?? NSNull())
        }
        if method == "setLocalToolPermission" {
            guard let settingsStore,
                  let raw = params["permission"] as? String,
                  isSandLocalToolPermission(raw)
            else { throw CoordinatorError.invalidParams }
            let ceiling = settingsStore.getLocalToolPermissionCeiling()
            guard resolveSandLocalToolPermission(raw, adminCeiling: ceiling) == raw else {
                throw CoordinatorError.requestFailed("Local tool permission exceeds the administrator ceiling.")
            }
            settingsStore.setLocalToolPermission(raw)
            return JSONResult(value: settingsStore.getLocalToolPermission())
        }

        if method == "getAutoReviewInstructions" {
            return JSONResult(value: autoReviewInstructionsObject())
        }
        if method == "setAutoReviewInstructions" {
            guard let settingsStore else { throw CoordinatorError.unavailable }
            let normalized = normalizeSandAutoReviewInstructions(
                isEnabled: params["isEnabled"] as? Bool,
                allowInstructions: params["allowInstructions"] as? [Any],
                blockInstructions: params["blockInstructions"] as? [Any]
            )
            settingsStore.setAutoReviewInstructions(normalized)
            autoReviewHostSyncNeeded = true
            try await syncAutoReviewRulesToHost()
            return JSONResult(value: autoReviewInstructionsObject())
        }
        if method == "getInferenceProvider" {
            guard let settingsStore else { throw CoordinatorError.unavailable }
            return JSONResult(value: [
                "provider": settingsStore.getInferenceProvider().rawValue,
            ])
        }
        if method == "setInferenceProvider" {
            guard let settingsStore,
                  let rawProvider = params["provider"] as? String,
                  let provider = SandInferenceProvider(rawValue: rawProvider)
            else {
                throw CoordinatorError.requestFailed("Unknown inference provider.")
            }
            settingsStore.setInferenceProvider(provider)
            return JSONResult(value: ["provider": provider.rawValue])
        }
        if method == "openExternal" {
            do {
                let payload = try CoordinatorPayload.fromFoundation(params)
                let result = try await nativeLocalCapabilities.execute(
                    capability: .openExternalURL,
                    params: payload
                )
                return JSONResult(value: result.foundationValue)
            } catch {
                throw CoordinatorError.requestFailed(error.localizedDescription)
            }
        }
        if case .failed = lifecycleState {
            do {
                _ = try hostSupervisor.recoverAfterFailure(
                    observedGeneration: hostSupervisor.generation
                )
                lifecycleState = .ready
            } catch {
                throw CoordinatorError.unavailable
            }
        }

        if let mcpSurface {
            switch try await mcpSurface.route(method: method, params: params) {
            case .handled(let value):
                return JSONResult(value: value)
            case .notHandled:
                break
            }
        }

        if let devControlAdapter {
            switch try await devControlAdapter.route(method: method, params: params) {
            case .handled(let value):
                return JSONResult(value: value)
            case .notHandled:
                break
            }
            do {
                try await devControlAdapter.beforeProductionRequest()
            } catch {
                throw CoordinatorError.requestFailed(error.localizedDescription)
            }
        }

        if autoReviewHostSyncNeeded,
           settingsStore != nil,
           method == "feature.execute" {
            try await syncAutoReviewRulesToHost()
        }

        let requestId = UUID().uuidString.lowercased()
        let observedHostGeneration = hostSupervisor.generation
        let awaitTurn = method == "feature.execute" && (params["awaitTurn"] as? Bool) == true
        var awaitedOperationId: String?
        inFlight.insert(requestId)
        defer { inFlight.remove(requestId) }

        do {
            let result = try await hostSupervisor.request(method: method, params: params)
            guard awaitTurn,
                  let accepted = result.value as? [String: Any],
                  let operationId = (accepted["operationId"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines),
                  !operationId.isEmpty
            else {
                return JSONResult(value: result.value)
            }

            awaitedOperationId = operationId
            while true {
                try Task.checkCancellation()
                let terminal = try await hostSupervisor.request(
                    method: "feature.awaitOperation",
                    params: [
                        "operationId": operationId,
                        "timeoutMs": 250,
                    ]
                )
                guard let value = terminal.value as? [String: Any],
                      let status = value["status"] as? String
                else {
                    throw CoordinatorError.invalidResponse
                }
                switch status {
                case "pending":
                    continue
                case "completed":
                    awaitedOperationId = nil
                    return JSONResult(value: result.value)
                case "interrupted":
                    awaitedOperationId = nil
                    let reason = (value["reason"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines)
                    throw CoordinatorError.requestFailed(
                        reason?.isEmpty == false ? reason! : "turn interrupted"
                    )
                case "failed":
                    awaitedOperationId = nil
                    let code = (value["code"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines)
                    let message = (value["message"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines)
                    let detail = [code, message]
                        .compactMap { item -> String? in
                            guard let item, !item.isEmpty else { return nil }
                            return item
                        }
                        .joined(separator: ": ")
                    throw CoordinatorError.requestFailed(
                        detail.isEmpty ? "turn failed" : detail
                    )
                default:
                    throw CoordinatorError.invalidResponse
                }
            }
        } catch is CancellationError {
            if let awaitedOperationId {
                _ = try? await hostSupervisor.request(
                    method: "feature.awaitOperation.cancel",
                    params: ["operationId": awaitedOperationId]
                )
            }
            throw CancellationError()
        } catch let hostError as MahayanaHostRuntime.HostError {
            if hostError.requiresRecovery {
                do {
                    _ = try hostSupervisor.recoverAfterFailure(
                        observedGeneration: observedHostGeneration
                    )
                } catch {
                    lifecycleState = .failed(error.localizedDescription)
                }
            }
            throw CoordinatorError.requestFailed(hostError.localizedDescription)
        } catch {
            throw CoordinatorError.requestFailed(error.localizedDescription)
        }
    }

    /// Typed transport entry used by RendererPortServer.
    func dispatchTransport(method: String, args: CoordinatorPayload) async -> CoordinatorReplyOutcome {
        guard lifecycleState != .shuttingDown else {
            return .failed(.init(code: "coordinator-unavailable", message: CoordinatorError.unavailable.localizedDescription))
        }
        guard case .object = args, var params = args.foundationValue as? [String: Any] else {
            return .failed(.init(code: "invalid-params", message: CoordinatorError.invalidParams.localizedDescription))
        }
        if method == "feature.execute",
           let command = params["command"] as? [String: Any],
           command["type"] as? String == "workflow.run" {
            params["awaitTurn"] = true
            params["source"] = "workflow-reference"
        }

        do {
            let result = try await request(method: method, params: params)
            if method == "feature.receive" {
                acceptHostEventEnvelope(result.value)
            }
            return .ok(try CoordinatorPayload.fromFoundation(result.value))
        } catch {
            return .failed(.init(code: "request-failed", message: error.localizedDescription))
        }
    }

    func reapplyClientPauseAfterCoordinatorLaunch() {
        clientPauseControl?.reapplyAfterCoordinatorLaunch()
        Task { @MainActor [weak self] in
            try? await self?.clientPauseControl?.synchronize()
        }
    }

    func sceneBecameActive() {
        if case .failed = lifecycleState {
            do {
                _ = try hostSupervisor.recoverAfterFailure(
                    observedGeneration: hostSupervisor.generation
                )
                lifecycleState = .ready
            } catch {
                return
            }
        } else {
            lifecycleState = .ready
        }
    }

    func hostSettingsTransportConnected() {
        hostSettingsReconciler?.setTransportLive(true)
    }

    func hostSettingsTransportDown() {
        hostSettingsReconciler?.setTransportLive(false)
    }

    func sceneEnteredBackground() {
        if case .failed = lifecycleState { return }
        lifecycleState = .background
    }

    func sceneWillSuspend() {
        if case .failed = lifecycleState { return }
        lifecycleState = .suspended
    }

    func beginShutdown() {
        hostSettingsTransportDown()
        lifecycleState = .shuttingDown
        inFlight.removeAll()
        clientSideToolV2Relay.clear()
        rendererEventSink = nil
        experimentService?.dispose()
    }
}
