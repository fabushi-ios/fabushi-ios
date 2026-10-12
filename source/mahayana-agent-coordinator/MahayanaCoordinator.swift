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
    typealias ReadLocalUserTimeZone = @MainActor () -> String?
    typealias PushTimeZoneRemote = @MainActor (String?) async throws -> MahayanaHostSettingsSnapshot
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
    private let readLocalUserTimeZone: ReadLocalUserTimeZone
    private let pushTimeZoneRemote: PushTimeZoneRemote
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
        readLocalUserTimeZone: @escaping ReadLocalUserTimeZone = { nil },
        pushTimeZoneRemote: @escaping PushTimeZoneRemote = {
            .init(hasSeenOnboarding: nil, userTimeZone: $0)
        },
        hostGeneration: @escaping HostGeneration
    ) {
        self.readLocal = readLocal
        self.writeLocal = writeLocal
        self.clearLocal = clearLocal
        self.readRemote = readRemote
        self.pushRemote = pushRemote
        self.readLocalUserTimeZone = readLocalUserTimeZone
        self.pushTimeZoneRemote = pushTimeZoneRemote
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

    func scheduleUserTimeZoneWrite(_ value: String) {
        invalidateInFlight(resetFieldSession: false)
        inFlight = Task { @MainActor [weak self] in
            guard let self else { return }
            _ = await self.pushUserTimeZoneIfCurrent(value)
        }
    }

    @discardableResult
    func reconcileIfReadable() async -> Bool {
        guard let token = tokenIfReadable() else { return false }
        _ = await onboardingSeen.absorbFromBox()
        guard isCurrent(token) else { return false }
        if let userTimeZone = readLocalUserTimeZone(),
           !(await pushUserTimeZoneIfCurrent(userTimeZone)) {
            return false
        }
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

    @discardableResult
    func pushUserTimeZoneIfCurrent(_ value: String) async -> Bool {
        guard let token = tokenIfReadable() else { return false }
        do {
            let response = try await pushTimeZoneRemote(value)
            guard isCurrent(token),
                  response.userTimeZone == value
            else {
                return false
            }
            lastSuccessfulAccountScope = token.accountScope
            lastSuccessfulEpoch = token.epoch
            return true
        } catch {
            return false
        }
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

struct IOSLocalToolApprovalScopeFence {
    struct Token: Equatable, Sendable {
        let accountScope: String
        let revision: UInt64
    }

    private(set) var accountScope: String?
    private(set) var revision: UInt64 = 0
    private(set) var isDisposed = false
    private var highestRevisionByScope: [String: UInt64] = [:]

    mutating func enter(_ rawScope: String?) {
        guard !isDisposed else { return }
        let trimmed = rawScope?.trimmingCharacters(in: .whitespacesAndNewlines)
        let nextScope: String? = trimmed?.isEmpty == false ? trimmed : nil
        guard nextScope != accountScope else { return }

        rememberActiveRevision()
        accountScope = nextScope
        guard let nextScope else {
            _ = advanceRevision()
            return
        }

        let reentryFloor = highestRevisionByScope[nextScope] ?? 0
        guard advanceRevision(minimumExclusive: reentryFloor) else { return }
        highestRevisionByScope[nextScope] = max(
            highestRevisionByScope[nextScope] ?? 0,
            revision
        )
    }

    mutating func reset() {
        guard !isDisposed, accountScope != nil else { return }
        rememberActiveRevision()
        accountScope = nil
        _ = advanceRevision()
    }

    mutating func dispose() {
        guard !isDisposed else { return }
        isDisposed = true
        accountScope = nil
        if revision < UInt64.max {
            revision += 1
        }
        highestRevisionByScope.removeAll()
    }

    func token() -> Token? {
        guard !isDisposed, let accountScope else { return nil }
        return .init(accountScope: accountScope, revision: revision)
    }

    func accepts(_ token: Token) -> Bool {
        !isDisposed
            && accountScope == token.accountScope
            && revision == token.revision
    }

    private mutating func rememberActiveRevision() {
        guard let accountScope else { return }
        highestRevisionByScope[accountScope] = max(
            highestRevisionByScope[accountScope] ?? 0,
            revision
        )
    }

    @discardableResult
    private mutating func advanceRevision(minimumExclusive floor: UInt64 = 0) -> Bool {
        let base = max(revision, floor)
        guard base < UInt64.max else {
            isDisposed = true
            accountScope = nil
            highestRevisionByScope.removeAll()
            return false
        }
        revision = base + 1
        return true
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
    private let remoteExecutionProvider: any IOSRemoteExecutionProviding
    private lazy var productionLocalExecutor: IOSProductionLocalExecutor = {
        IOSProductionLocalExecutor(
            localBackend: nativeLocalCapabilities,
            remoteTransport: IOSGenerationFencedRemoteRunnerTransport(
                currentIdentity: { [weak self] in
                    self?.currentRemoteRunnerIdentity()
                },
                dispatch: { [weak self] method, params in
                    guard let self else { throw CoordinatorError.unavailable }
                    return try await self.remoteExecutionProvider.dispatch(
                        method: method,
                        params: params
                    )
                }
            )
        )
    }()
    private let transportStageRecorder: TransportStageRecorder
    private let gatewayTelemetryReporter: CoordinatorGatewayClient.TelemetrySink
    private var rendererEventSink: ((String, CoordinatorPayload) -> Void)?
    private var autoReviewHostSyncNeeded = true
    private var experimentSnapshotUnsubscribe: (() -> Void)?
    private var experimentRefreshTask: Task<Void, Never>?
    private var experimentMembershipTask: Task<Void, Never>?
    private var experimentOverridePushTask: Task<Void, Never>?
    private var experimentMembershipGeneration: UInt64 = 0
    private var experimentOverrideGeneration: UInt64 = 0
    private var lastProjectedExperimentOverrides: [String: Bool]?
    private var localToolApprovalScopeFence = IOSLocalToolApprovalScopeFence()
    private var localToolApprovalAccountScope: String? {
        localToolApprovalScopeFence.accountScope
    }
    private var localToolApprovalScopeRevision: UInt64 {
        localToolApprovalScopeFence.revision
    }
    private var localToolApprovalCleanupStatus = "idle"
    private var localToolApprovalCleanupFailure: String?
    private(set) var lifecycleState: LifecycleState = .starting
    private var inFlight = Set<String>()

    private lazy var shippingGatewayDispatcher: GatewayRequestDispatcher = {
        let gatewaySupervisor = CoordinatorHostSupervisor(
            resolveConnection: { [weak self] in
                guard let self, self.lifecycleState != .shuttingDown else {
                    throw CoordinatorError.unavailable
                }
                return .init(
                    baseURL: URL(string: "https://localhost/fabushi-ios-host")!,
                    headers: ["x-fabushi-transport": "canonical-local-host"]
                )
            },
            healthProbe: { [weak self] _ in
                guard let self else { return false }
                return self.lifecycleState != .shuttingDown
            }
        )
        let gatewayClient = CoordinatorGatewayClient(
            supervisor: gatewaySupervisor,
            transportStageRecorder: transportStageRecorder,
            telemetrySink: gatewayTelemetryReporter,
            tracedTransport: { [weak self] _, method, args, _ in
                guard let self else { throw CoordinatorError.unavailable }
                guard case .object = args,
                      let params = args.foundationValue as? [String: Any]
                else { throw CoordinatorError.invalidParams }
                let result = try await self.request(method: method, params: params)
                return try CoordinatorPayload.fromFoundation(result.value)
            }
        )
        return GatewayRequestDispatcher(
            serves: { _ in true },
            dispatchCommand: { requestId, method, args in
                try await gatewayClient.dispatchCommand(
                    requestId: requestId,
                    method: method,
                    args: args
                )
            }
        )
    }()

    init(
        hostSupervisor: MahayanaLocalHostSupervisor,
        passkeyProvider: (any PasskeyProviding)? = nil,
        settingsStore: SandSettingsStore? = nil,
        experimentService: SandExperimentService? = nil,
        devControlAdapter: (any CoordinatorDevControlAdapting)? = nil,
        mcpSurface: CoordinatorMcpSurface? = nil,
        transportStageRecorder: TransportStageRecorder = .init(),
        remoteExecutionProvider: (any IOSRemoteExecutionProviding)? = nil,
        mcpFailureReporter: @escaping IOSCursorLocalToolPermissionCeilingSynchronizer.ReportFailure = { _, _, _ in },
        mcpAuthTelemetryReporter: @escaping IOSAuthTelemetryRelay.Sink = { _ in },
        gatewayTelemetryReporter: @escaping CoordinatorGatewayClient.TelemetrySink = { _ in },
        backendUpdateRequirementSink: @escaping IOSCursorDashboardClient.BackendUpdateRequirementSink = { _ in }
    ) {
        self.hostSupervisor = hostSupervisor
        self.settingsStore = settingsStore
        self.transportStageRecorder = transportStageRecorder
        self.gatewayTelemetryReporter = gatewayTelemetryReporter
        self.hostSettingsReconciler = settingsStore.map { store in
            IOSHostSettingsReconciler(
                readLocal: { store.getHasSeenOnboarding() },
                writeLocal: { store.setHasSeenOnboarding($0) },
                clearLocal: { store.clearHasSeenOnboarding() },
                readRemote: { try await hostSupervisor.readHostSettings() },
                pushRemote: { try await hostSupervisor.pushHostSettings($0) },
                readLocalUserTimeZone: {
                    let detected = store.getDetectedUserTimeZone()
                        ?? TimeZone.autoupdatingCurrent.identifier
                    return store.getUserTimeZoneOverride() ?? detected
                },
                pushTimeZoneRemote: {
                    try await hostSupervisor.pushHostUserTimeZone($0)
                },
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
                reportMigrationWatchTelemetry: { phase, failures, errorClass in
                    var metadata = [
                        "event": "box_migration_watch",
                        "phase": phase,
                        "consecutive_failures": String(failures),
                    ]
                    if let errorClass {
                        metadata["error_class"] = String(errorClass.prefix(128))
                    }
                    gatewayTelemetryReporter(.init(
                        event: "box_migration_watch",
                        level: phase == "attached" ? "info" : "warn",
                        metadata: metadata
                    ))
                },
                backendUpdateRequirementSink: backendUpdateRequirementSink,
                isClientPaused: isClientPaused
            )
        } else {
            resolvedMcpSurface = nil
        }
        if let remoteExecutionProvider {
            self.remoteExecutionProvider = remoteExecutionProvider
        } else if let surface = resolvedMcpSurface {
            self.remoteExecutionProvider = IOSClosureRemoteExecutionProvider(
                dispatch: { [weak surface] method, params in
                    guard let surface else { throw CoordinatorError.unavailable }
                    return try await surface.dispatchRemoteExecution(
                        method: method,
                        params: params
                    )
                }
            )
        } else {
            self.remoteExecutionProvider = IOSUnavailableRemoteExecutionProvider()
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

    private func currentRemoteRunnerIdentity() -> IOSRemoteRunnerExecutionIdentity? {
        guard lifecycleState != .shuttingDown,
              let accountScope = localToolApprovalAccountScope?.trimmingCharacters(in: .whitespacesAndNewlines),
              !accountScope.isEmpty
        else { return nil }
        return IOSRemoteRunnerExecutionIdentity(
            accountScope: accountScope,
            accountRevision: localToolApprovalScopeRevision,
            hostGeneration: hostSupervisor.generation
        )
    }

    private func rejectPlatformExecIfCurrent(
        requestId: String,
        code: String,
        message: String,
        expectedHostGeneration: UInt64
    ) async {
        guard hostSupervisor.generation == expectedHostGeneration else { return }
        _ = try? await hostSupervisor.request(
            method: "platformExec.reject",
            params: [
                "requestId": requestId,
                "code": code,
                "message": message,
            ]
        )
    }

    @discardableResult
    func servicePendingPlatformExecution(
        expectedIdentity: IOSRemoteRunnerExecutionIdentity?
    ) async throws -> Bool {
        let expectedHostGeneration = expectedIdentity?.hostGeneration ?? hostSupervisor.generation
        guard hostSupervisor.generation == expectedHostGeneration else { return false }

        let next = try await hostSupervisor.request(
            method: "platformExec.next",
            params: [:]
        )
        if next.value is NSNull {
            return false
        }
        guard let request = next.value as? [String: Any],
              let requestId = (request["requestId"] as? String)?
                .trimmingCharacters(in: .whitespacesAndNewlines),
              !requestId.isEmpty,
              let method = (request["method"] as? String)?
                .trimmingCharacters(in: .whitespacesAndNewlines),
              !method.isEmpty,
              let rawParams = request["params"]
        else {
            throw CoordinatorError.invalidResponse
        }

        guard let expectedIdentity,
              currentRemoteRunnerIdentity() == expectedIdentity,
              hostSupervisor.generation == expectedHostGeneration
        else {
            await rejectPlatformExecIfCurrent(
                requestId: requestId,
                code: "stale_execution_scope",
                message: "Platform execution scope changed before dispatch.",
                expectedHostGeneration: expectedHostGeneration
            )
            return true
        }

        do {
            try Task.checkCancellation()
            let params = try CoordinatorPayload.fromFoundation(rawParams)
            let result = try await productionLocalExecutor.execute(
                method: method,
                params: params
            )
            try Task.checkCancellation()
            guard currentRemoteRunnerIdentity() == expectedIdentity,
                  hostSupervisor.generation == expectedHostGeneration
            else {
                await rejectPlatformExecIfCurrent(
                    requestId: requestId,
                    code: "stale_execution_scope",
                    message: "Platform execution scope changed before result commit.",
                    expectedHostGeneration: expectedHostGeneration
                )
                return true
            }
            _ = try await hostSupervisor.request(
                method: "platformExec.resolve",
                params: [
                    "requestId": requestId,
                    "result": result.foundationValue,
                ]
            )
        } catch is CancellationError {
            await rejectPlatformExecIfCurrent(
                requestId: requestId,
                code: "cancelled",
                message: "Platform execution was cancelled.",
                expectedHostGeneration: expectedHostGeneration
            )
            throw CancellationError()
        } catch {
            await rejectPlatformExecIfCurrent(
                requestId: requestId,
                code: "remote_execution_failed",
                message: error.localizedDescription,
                expectedHostGeneration: expectedHostGeneration
            )
        }
        return true
    }

    func structuredLogCredentials(
        backendURL: String,
        accountSlot: String
    ) async throws -> (accessToken: String, machineID: String) {
        guard let mcpSurface else { throw CoordinatorError.unavailable }
        return try await mcpSurface.structuredLogCredentials(
            backendURL: backendURL,
            accountSlot: accountSlot
        )
    }

    func revokeCursorAuthForAccountRefusal() async -> IOSCursorAccountRefusalRevocation? {
        guard let mcpSurface else { return nil }
        return await mcpSurface.revokeCursorAuthForAccountRefusal()
    }

    static func make(
        appDataDirectory: URL,
        featureHostTest: Bool = false,
        passkeyProvider: (any PasskeyProviding)? = nil,
        devControlAdapter: (any CoordinatorDevControlAdapting)? = nil,
        mcpFailureReporter: @escaping IOSCursorLocalToolPermissionCeilingSynchronizer.ReportFailure = { _, _, _ in },
        mcpAuthTelemetryReporter: @escaping IOSAuthTelemetryRelay.Sink = { _ in },
        gatewayTelemetryReporter: @escaping CoordinatorGatewayClient.TelemetrySink = { _ in },
        backendUpdateRequirementSink: @escaping IOSCursorDashboardClient.BackendUpdateRequirementSink = { _ in },
        experimentDiagnosticReporter: ((ExperimentDiagnostic) -> Void)? = nil
    ) throws -> MahayanaCoordinator {
        if let experimentDiagnosticReporter {
            pinExperimentsDiagnosticsReporter(experimentDiagnosticReporter)
        }
        let experimentCacheDirectory = appDataDirectory
            .appendingPathComponent("experiments", isDirectory: true)
        let experimentService = SandExperimentService(
            getCacheDir: { experimentCacheDirectory.path },
            isDevBuild: featureHostTest,
            productFeatureGateDefaults: ["sand_agent_network": true]
        )
        experimentService.startFromCache()
        let coordinator = MahayanaCoordinator(
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
            mcpAuthTelemetryReporter: mcpAuthTelemetryReporter,
            gatewayTelemetryReporter: gatewayTelemetryReporter,
            backendUpdateRequirementSink: backendUpdateRequirementSink
        )
        coordinator.startExperimentRuntime()
        return coordinator
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

    private func emitExperimentSnapshot(_ snapshot: SandExperimentSnapshot) {
        guard let payload = try? CoordinatorPayload.fromFoundation(
            sandExperimentSnapshotFoundation(snapshot)
        ) else { return }
        rendererEventSink?("experiments-changed", payload)
    }

    private func refreshExperiments(_ trigger: SandExperimentRefreshTrigger) async {
        guard let experimentService, let mcpSurface else { return }
        await experimentService.refresh(
            backendUrl: getConfiguredBackendUrl(),
            getAccessToken: { backendURL in
                try await mcpSurface.experimentAccessToken(backendURL: backendURL)
            },
            getMachineId: {
                try await mcpSurface.experimentMachineID()
            },
            trigger: trigger
        )
    }

    private func startExperimentRuntime() {
        guard let experimentService, let mcpSurface else { return }
        experimentSnapshotUnsubscribe?()
        experimentSnapshotUnsubscribe = experimentService.subscribe { [weak self] snapshot in
            Task { @MainActor [weak self] in
                self?.handleExperimentSnapshot(snapshot)
            }
        }
        mcpSurface.setExperimentAuthObserver { [weak self] status in
            self?.handleExperimentAuthStatus(status)
        }
        experimentService.startPolling { [weak self] in
            guard let self else { return }
            await self.refreshExperiments(.poll)
        }
        emitExperimentSnapshot(experimentService.getSnapshot())
        experimentRefreshTask?.cancel()
        experimentRefreshTask = Task { @MainActor [weak self] in
            await self?.refreshExperiments(.startup)
        }
    }

    private func handleExperimentAuthStatus(_ status: IOSCursorAuthStatus) {
        guard let experimentService else { return }
        experimentService.handleAuthChange()
        experimentRefreshTask?.cancel()
        experimentRefreshTask = Task { @MainActor [weak self] in
            await self?.refreshExperiments(.authChange)
        }

        experimentMembershipGeneration = experimentMembershipGeneration == UInt64.max
            ? 1
            : experimentMembershipGeneration + 1
        let generation = experimentMembershipGeneration
        experimentMembershipTask?.cancel()
        guard status.loggedIn,
              localToolApprovalAccountScope != nil,
              let mcpSurface else {
            experimentService.setIsAnysphereUser(false)
            return
        }
        let expectedScope = localToolApprovalAccountScope
        experimentMembershipTask = Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                let isMember = try await mcpSurface.experimentIsAnysphereUser()
                guard !Task.isCancelled,
                      self.experimentMembershipGeneration == generation,
                      self.localToolApprovalAccountScope == expectedScope
                else { return }
                experimentService.setIsAnysphereUser(isMember)
            } catch {
                guard !Task.isCancelled,
                      self.experimentMembershipGeneration == generation,
                      self.localToolApprovalAccountScope == expectedScope
                else { return }
                experimentService.setIsAnysphereUser(false)
                reportExperimentsDiagnostic(.init(
                    kind: "membership_refresh_failed",
                    metadata: ["errorClass": errorLogTag(error)]
                ))
            }
        }
    }

    private func handleExperimentSnapshot(_ snapshot: SandExperimentSnapshot) {
        emitExperimentSnapshot(snapshot)
        guard localToolApprovalAccountScope != nil else { return }
        let overrides = snapshot.featureFlagOverrides
        guard lastProjectedExperimentOverrides != overrides else { return }

        experimentOverrideGeneration = experimentOverrideGeneration == UInt64.max
            ? 1
            : experimentOverrideGeneration + 1
        let generation = experimentOverrideGeneration
        let expectedHostGeneration = hostSupervisor.generation
        let expectedScope = localToolApprovalAccountScope
        experimentOverridePushTask?.cancel()
        experimentOverridePushTask = Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                _ = try await self.hostSupervisor.pushHostFeatureFlagOverrides(overrides)
                guard !Task.isCancelled,
                      self.experimentOverrideGeneration == generation,
                      self.hostSupervisor.generation == expectedHostGeneration,
                      self.localToolApprovalAccountScope == expectedScope
                else { return }
                self.lastProjectedExperimentOverrides = overrides
            } catch {
                guard !Task.isCancelled,
                      self.experimentOverrideGeneration == generation,
                      self.localToolApprovalAccountScope == expectedScope
                else { return }
                reportExperimentsDiagnostic(.init(
                    kind: "host_override_push_failed",
                    metadata: ["errorClass": errorLogTag(error)]
                ))
            }
        }
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
              let channel = envelope["channel"] as? String,
              let payload = envelope["payload"]
        else { return }

        if channel == ClientSideToolV2Transport.family {
            guard let transportEvent = ClientSideToolV2TransportEvent.fromFoundation(payload),
                  let projected = clientSideToolV2Relay.accept(transportEvent)
            else { return }
            rendererEventSink?(
                ClientSideToolV2Transport.family,
                projected.coordinatorPayload
            )
            return
        }

        guard let family = CoordinatorEventFamilies.family(forChannel: channel),
              let projected = try? CoordinatorPayload.fromFoundation(payload)
        else { return }
        rendererEventSink?(family, projected)
    }

    private func recordTransportEchoIfPresent(_ value: Any) async {
        guard let envelope = value as? [String: Any],
              let payload = envelope["payload"],
              let clientNonce = coordinatorTransportEchoClientNonce(from: payload)
        else { return }

        _ = await transportStageRecorder.recordSendEcho(
            accountSlot: "host",
            clientNonce: clientNonce,
            nowEpochMilliseconds: coordinatorTransportEpochMilliseconds(),
            nowMonotonicMilliseconds: coordinatorTransportMonotonicMilliseconds()
        )
    }

    func prepareAccountTransition() async {
        await mcpSurface?.prepareAccountTransition()
    }

    func updateAccountSettingsScope(_ accountScope: String?) {
        let previousExperimentScope = localToolApprovalAccountScope
        localToolApprovalScopeFence.enter(accountScope)
        let normalizedAccountScope = localToolApprovalAccountScope
        localToolApprovalCleanupStatus = "idle"
        localToolApprovalCleanupFailure = nil
        if previousExperimentScope != normalizedAccountScope {
            experimentMembershipGeneration = experimentMembershipGeneration == UInt64.max
                ? 1
                : experimentMembershipGeneration + 1
            experimentOverrideGeneration = experimentOverrideGeneration == UInt64.max
                ? 1
                : experimentOverrideGeneration + 1
            experimentMembershipTask?.cancel()
            experimentOverridePushTask?.cancel()
            lastProjectedExperimentOverrides = nil
            experimentService?.setIsAnysphereUser(false)
            experimentService?.resetFeatureFlagOverridesForAccountTransition()
        }
        if let normalizedAccountScope {
            settingsStore?.scopeToAccount(normalizedAccountScope)
            hostSettingsReconciler?.scopeToAccount(normalizedAccountScope)
        } else {
            hostSettingsReconciler?.accountDeparted()
            settingsStore?.clearAccountScope()
        }
        autoReviewHostSyncNeeded = true
        mcpSurface?.updateAccountScope(normalizedAccountScope)
        if let experimentService, normalizedAccountScope != nil {
            handleExperimentSnapshot(experimentService.getSnapshot())
        }
    }

    private func localToolApprovalCleanupStateObject() -> [String: Any] {
        [
            "status": localToolApprovalCleanupStatus,
            "failure": localToolApprovalCleanupFailure.map { $0 as Any } ?? NSNull(),
            "scopeRevision": NSNumber(value: localToolApprovalScopeRevision),
        ]
    }

    private func resolveLocalToolPermissionWithApprovalLifecycle(
        _ params: [String: Any]
    ) async throws -> JSONResult {
        guard let settingsStore,
              let entryId = (params["entryId"] as? String)?
                .trimmingCharacters(in: .whitespacesAndNewlines),
              !entryId.isEmpty,
              let requestId = (params["requestId"] as? String)?
                .trimmingCharacters(in: .whitespacesAndNewlines),
              !requestId.isEmpty,
              let agentId = (params["agentId"] as? String)?
                .trimmingCharacters(in: .whitespacesAndNewlines),
              !agentId.isEmpty,
              let requestedResolution = params["resolution"] as? String,
              ["always", "allow-once", "never", "deny"].contains(requestedResolution),
              let action = params["action"] as? String,
              isSandLocalToolAction(action),
              let target = params["target"] as? String,
              let accountScope = localToolApprovalAccountScope,
              !accountScope.isEmpty
        else {
            throw CoordinatorError.invalidParams
        }

        let scopeRevision = localToolApprovalScopeRevision
        var resolution = requestedResolution
        if requestedResolution == "always" || requestedResolution == "never" {
            do {
                guard scopeRevision == localToolApprovalScopeRevision,
                      localToolApprovalAccountScope == accountScope
                else {
                    throw CoordinatorError.requestFailed("Local tool permission scope changed.")
                }
                let ceiling = settingsStore.getLocalToolPermissionCeiling()
                guard resolveSandLocalToolPermission(
                    requestedResolution,
                    adminCeiling: ceiling
                ) == requestedResolution else {
                    throw CoordinatorError.requestFailed(
                        "Local tool permission exceeds the administrator ceiling."
                    )
                }
                try settingsStore.setLocalToolPermissionPersisting(requestedResolution)
            } catch {
                resolution = requestedResolution == "always" ? "allow-once" : "deny"
            }
        }

        if resolution == "allow-once" {
            guard scopeRevision == localToolApprovalScopeRevision,
                  localToolApprovalAccountScope == accountScope,
                  try settingsStore.recordLocalToolApproval(
                      id: requestId,
                      action: action,
                      target: target,
                      expectedAccountScope: accountScope
                  )
            else {
                throw CoordinatorError.requestFailed(
                    "Local tool approval scope changed before it could be persisted."
                )
            }
        }

        guard scopeRevision == localToolApprovalScopeRevision,
              localToolApprovalAccountScope == accountScope
        else {
            throw CoordinatorError.requestFailed("Local tool permission scope changed.")
        }

        _ = try await request(
            method: "resolveLocalToolPermission",
            params: [
                "entryId": entryId,
                "requestId": requestId,
                "resolution": resolution,
                "agentId": agentId,
            ]
        )

        guard scopeRevision == localToolApprovalScopeRevision,
              localToolApprovalAccountScope == accountScope
        else {
            throw CoordinatorError.requestFailed("Local tool permission scope changed.")
        }
        return JSONResult(value: resolution)
    }

    private func clearLocalToolApprovalsAfterAcceptedSend(
        scopeRevision: UInt64
    ) {
        guard scopeRevision == localToolApprovalScopeRevision else { return }
        guard let settingsStore,
              let accountScope = localToolApprovalAccountScope,
              !accountScope.isEmpty
        else {
            localToolApprovalCleanupStatus = "failed"
            localToolApprovalCleanupFailure =
                "Temporary local-tool approval cleanup has no active account scope."
            return
        }

        localToolApprovalCleanupStatus = "clearing"
        localToolApprovalCleanupFailure = nil
        do {
            let cleared = try settingsStore.clearLocalToolApprovals(
                expectedAccountScope: accountScope
            )
            guard scopeRevision == localToolApprovalScopeRevision,
                  localToolApprovalAccountScope == accountScope
            else {
                localToolApprovalCleanupStatus = "idle"
                localToolApprovalCleanupFailure = nil
                return
            }
            if cleared {
                localToolApprovalCleanupStatus = "idle"
            } else {
                localToolApprovalCleanupStatus = "failed"
                localToolApprovalCleanupFailure =
                    "Temporary local-tool approval cleanup was fenced by account scope."
            }
        } catch {
            guard scopeRevision == localToolApprovalScopeRevision,
                  localToolApprovalAccountScope == accountScope
            else { return }
            localToolApprovalCleanupStatus = "failed"
            localToolApprovalCleanupFailure = error.localizedDescription
        }
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
        if method == "sand:experiments-snapshot-sync" || method == "experiments.getSnapshot" {
            guard let snapshot = experimentService?.getSnapshot() else {
                return JSONResult(value: NSNull())
            }
            return JSONResult(value: sandExperimentSnapshotFoundation(snapshot))
        }
        if method == "experiments.applyFeatureFlagOverrideCommand" {
            guard let experimentService else { throw CoordinatorError.unavailable }
            let command = (params["command"] as? [String: Any]) ?? params
            guard command["kind"] is String else { throw CoordinatorError.invalidParams }
            experimentService.applyFeatureFlagOverrideCommand(command)
            return JSONResult(value: sandExperimentSnapshotFoundation(experimentService.getSnapshot()))
        }
        if method == "experiments.refreshNow" {
            guard experimentService != nil else { throw CoordinatorError.unavailable }
            await refreshExperiments(.manual)
            guard let snapshot = experimentService?.getSnapshot() else {
                return JSONResult(value: NSNull())
            }
            return JSONResult(value: sandExperimentSnapshotFoundation(snapshot))
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
            let override = settingsStore.getUserTimeZoneOverride()
            hostSettingsReconciler?.scheduleUserTimeZoneWrite(override ?? detected)
            let overrideTimeZone: Any = override.map { $0 as Any } ?? NSNull()
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
            let effective = override ?? detected
            if let hostSettingsReconciler,
               hostSettingsReconciler.isReadable,
               !(await hostSettingsReconciler.pushUserTimeZoneIfCurrent(effective)) {
                throw CoordinatorError.requestFailed("Time zone scope changed before Host projection completed.")
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
        if method == "resolveLocalToolPermissionWithApprovalLifecycle" {
            return try await resolveLocalToolPermissionWithApprovalLifecycle(params)
        }
        if method == "getLocalToolApprovalCleanupState" {
            return JSONResult(value: localToolApprovalCleanupStateObject())
        }

        if method == "native.composerQueue.enqueue" {
            guard let settingsStore,
                  let accountScope = localToolApprovalAccountScope, !accountScope.isEmpty,
                  let command = params["command"] as? [String: Any],
                  command["type"] as? String == "chat.send",
                  let nonce = command["requestId"] as? String, !nonce.isEmpty,
                  let agentId = command["agentId"] as? String, !agentId.isEmpty,
                  JSONSerialization.isValidJSONObject(command)
            else { throw CoordinatorError.invalidParams }
            let data = try JSONSerialization.data(withJSONObject: command, options: [.sortedKeys])
            guard let commandJSON = String(data: data, encoding: .utf8),
                  try settingsStore.enqueueComposerSubmission(
                    nonce: nonce, agentId: agentId,
                    createdAtMs: Int64(Date().timeIntervalSince1970 * 1_000),
                    commandJSON: commandJSON, expectedAccountScope: accountScope
                  )
            else { throw CoordinatorError.requestFailed("Composer submission scope changed before persistence.") }
            return JSONResult(value: ["nonce": nonce, "queued": true])
        }
        if method == "native.composerQueue.list" {
            guard let settingsStore,
                  let accountScope = localToolApprovalAccountScope, !accountScope.isEmpty
            else { throw CoordinatorError.unavailable }
            let rawAgentId = (params["agentId"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines)
            let rows = settingsStore.composerSubmissions(
                expectedAccountScope: accountScope,
                agentId: rawAgentId?.isEmpty == false ? rawAgentId : nil
            )
            let value: [[String: Any]] = rows.compactMap { row in
                guard let data = row.commandJSON.data(using: .utf8),
                      let command = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
                else { return nil }
                return ["nonce": row.nonce, "agentId": row.agentId,
                        "createdAtMs": NSNumber(value: row.createdAtMs), "command": command]
            }
            return JSONResult(value: value)
        }
        if method == "native.composerQueue.remove" || method == "native.composerQueue.cancel" {
            guard let settingsStore,
                  let accountScope = localToolApprovalAccountScope, !accountScope.isEmpty,
                  let nonce = (params["nonce"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines),
                  !nonce.isEmpty
            else { throw CoordinatorError.invalidParams }
            guard try settingsStore.removeComposerSubmission(nonce: nonce, expectedAccountScope: accountScope)
            else { throw CoordinatorError.requestFailed("Composer submission scope changed before removal.") }
            return JSONResult(value: ["nonce": nonce, "removed": true])
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
                let result = try await productionLocalExecutor.execute(
                    method: "openExternalURL",
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

        let localToolApprovalCleanupRevision: UInt64? = {
            guard method == "feature.execute",
                  let command = params["command"] as? [String: Any],
                  command["type"] as? String == "chat.send"
            else { return nil }
            return localToolApprovalScopeRevision
        }()
        let requestId = UUID().uuidString.lowercased()
        let observedHostGeneration = hostSupervisor.generation
        let observedRemoteRunnerIdentity = currentRemoteRunnerIdentity()
        let awaitTurn = method == "feature.execute" && (params["awaitTurn"] as? Bool) == true
        var awaitedOperationId: String?
        inFlight.insert(requestId)
        defer { inFlight.remove(requestId) }

        do {
            let result = try await hostSupervisor.request(method: method, params: params)
            if let localToolApprovalCleanupRevision {
                clearLocalToolApprovalsAfterAcceptedSend(
                    scopeRevision: localToolApprovalCleanupRevision
                )
            }
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
                    _ = try await servicePendingPlatformExecution(
                        expectedIdentity: observedRemoteRunnerIdentity
                    )
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
        await dispatchTransport(
            requestId: UUID().uuidString.lowercased(),
            method: method,
            args: args
        )
    }

    func dispatchTransport(
        requestId: String,
        method: String,
        args: CoordinatorPayload
    ) async -> CoordinatorReplyOutcome {
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
        if method == "listRoutedMcpTools" || method == "executeRoutedMcpTool" {
            params["_requestId"] = requestId
        }
        guard let routedArgs = try? CoordinatorPayload.fromFoundation(params) else {
            return .failed(.init(code: "invalid-params", message: CoordinatorError.invalidParams.localizedDescription))
        }
        let outcome = await shippingGatewayDispatcher.dispatch(
            requestId: requestId,
            method: method,
            args: routedArgs
        )
        if method == "feature.receive",
           case .ok(let value) = outcome {
            await recordTransportEchoIfPresent(value.foundationValue)
            acceptHostEventEnvelope(value.foundationValue)
        }
        return outcome
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
        localToolApprovalScopeFence.dispose()
        localToolApprovalCleanupStatus = "idle"
        localToolApprovalCleanupFailure = nil
        inFlight.removeAll()
        clientSideToolV2Relay.clear()
        rendererEventSink = nil
        experimentSnapshotUnsubscribe?()
        experimentSnapshotUnsubscribe = nil
        experimentRefreshTask?.cancel()
        experimentRefreshTask = nil
        experimentMembershipTask?.cancel()
        experimentMembershipTask = nil
        experimentOverridePushTask?.cancel()
        experimentOverridePushTask = nil
        mcpSurface?.setExperimentAuthObserver(nil)
        experimentService?.dispose()
    }
}
