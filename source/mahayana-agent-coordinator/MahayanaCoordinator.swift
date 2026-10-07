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
    private let mcpSurface: CoordinatorMcpSurface?
    private let experimentService: SandExperimentService?
    private let webAuthnSigner: CoordinatorWebAuthnSigner?
    private let devControlAdapter: (any CoordinatorDevControlAdapting)?
    private let clientSideToolV2Relay = ClientSideToolV2Relay()
    private let nativeLocalCapabilities = IOSNativeLocalCapabilityBackend()
    private var rendererEventSink: ((String, CoordinatorPayload) -> Void)?
    private(set) var lifecycleState: LifecycleState = .starting
    private var inFlight = Set<String>()

    init(
        hostSupervisor: MahayanaLocalHostSupervisor,
        passkeyProvider: (any PasskeyProviding)? = nil,
        settingsStore: SandSettingsStore? = nil,
        experimentService: SandExperimentService? = nil,
        devControlAdapter: (any CoordinatorDevControlAdapting)? = nil,
        mcpSurface: CoordinatorMcpSurface? = nil
    ) {
        self.hostSupervisor = hostSupervisor
        self.settingsStore = settingsStore

        if let mcpSurface {
            self.mcpSurface = mcpSurface
        } else if let settingsStore {
            self.mcpSurface = CoordinatorMcpSurface.make(
                hostSupervisor: hostSupervisor,
                settingsStore: settingsStore
            )
        } else {
            self.mcpSurface = nil
        }
        self.experimentService = experimentService
        self.devControlAdapter = devControlAdapter
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
        devControlAdapter: (any CoordinatorDevControlAdapting)? = nil
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
            devControlAdapter: devControlAdapter
        )
    }

    func sharedSettingsSnapshot() -> SandStoredSettings {
        settingsStore?.load() ?? emptySandSettings()
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
        } else {
            settingsStore?.clearAccountScope()
        }
        mcpSurface?.updateAccountScope(accountScope)
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

    /// Compatibility entry used while feature-specific typed facades are
    /// replacing dictionary-shaped calls. Host ownership remains here.
    func request(method: String, params: [String: Any] = [:]) async throws -> JSONResult {
        guard lifecycleState != .shuttingDown else { throw CoordinatorError.unavailable }
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

    func sceneEnteredBackground() {
        if case .failed = lifecycleState { return }
        lifecycleState = .background
    }

    func sceneWillSuspend() {
        if case .failed = lifecycleState { return }
        lifecycleState = .suspended
    }

    func beginShutdown() {
        lifecycleState = .shuttingDown
        inFlight.removeAll()
        clientSideToolV2Relay.clear()
        rendererEventSink = nil
        experimentService?.dispose()
    }
}
