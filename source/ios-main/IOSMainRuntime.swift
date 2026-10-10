import Foundation
import SwiftUI

@MainActor
private final class CoordinatorLocalHumanIdentityState {
    private(set) var value: String?

    func replace(with slot: String?) {
        let normalized = slot?.trimmingCharacters(in: .whitespacesAndNewlines)
        value = normalized?.isEmpty == false ? normalized : nil
    }
}

/// iOS platform-main counterpart of Grok's Electron main process.
///
/// Owns lifecycle forwarding and the coordinator. It does not expose Host.
@MainActor
final class IOSMainRuntime {
    let coordinator: MahayanaCoordinator
    let lifecycleReporter: IOSLifecycleReporter
    private let coordinatorAppVersion: String
    private let coordinatorDataDirectory: String
    private let coordinatorLocalHumanIdentity: CoordinatorLocalHumanIdentityState
    private let lifecycleRecovery: IOSLifecycleRecoveryStore
    private let boxVisibilityTracker: IOSBoxVisibilityTracker
    private let passkeyProvider: IOSAuthenticationServicesPasskeyProvider
    private let accountRuntime: CoordinatorAccountRuntime
    private let updateWiring: IOSUpdateServiceWiring
    let devCapability: IOSDevCapability
    private let devControlAdapter: IOSNativeDevControlAdapter

    init(
        appDataDirectory: URL,
        featureHostTest: Bool = false,
        devCapability: IOSDevCapability = .live(),
        devControlsGate: IOSDevControlsGate = .live()
    ) throws {
        let appVersion = (Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String) ?? "development"
        coordinatorAppVersion = appVersion
        coordinatorDataDirectory = appDataDirectory.path
        let coordinatorLocalHumanIdentity = CoordinatorLocalHumanIdentityState()
        self.coordinatorLocalHumanIdentity = coordinatorLocalHumanIdentity

        self.devCapability = devCapability
        devControlAdapter = IOSNativeDevControlAdapter(gate: devControlsGate)
        lifecycleRecovery = try IOSLifecycleRecoveryStore(appDataDirectory: appDataDirectory)

        let reporter = IOSLifecycleReporter()
        lifecycleReporter = reporter
        let boxVisibilityTracker = IOSBoxVisibilityTracker(reporter: reporter)
        self.boxVisibilityTracker = boxVisibilityTracker

        let passkeyProvider = IOSAuthenticationServicesPasskeyProvider()
        self.passkeyProvider = passkeyProvider

        let coordinator = try MahayanaCoordinator.make(
            appDataDirectory: appDataDirectory,
            featureHostTest: featureHostTest,
            passkeyProvider: passkeyProvider,
            devControlAdapter: devControlAdapter,
            mcpFailureReporter: { area, leg, error in
                reporter.report(
                    .localExecLifecycle,
                    level: .error,
                    metadata: [
                        "area": area,
                        "leg": leg,
                        "error": String(error.localizedDescription.prefix(512)),
                    ]
                )
            },
            mcpAuthTelemetryReporter: { projection in
                reporter.report(
                    projection.stream == .session ? .desktopSession : .desktopSignin,
                    level: projection.level,
                    metadata: projection.metadata
                )
            }
        )
        self.coordinator = coordinator

        let cleanup = ProductionAccountTransitionCleanup(
            dependencies: .init(
                clearAccountScope: {
                    coordinatorLocalHumanIdentity.replace(with: nil)
                    boxVisibilityTracker.noteAccountSlot(nil)
                    coordinator.updateAccountSettingsScope(nil)
                },
                didClearAccountScope: { _, nextSlot in
                    reporter.report(
                        .coordinatorHandoff,
                        metadata: [
                            "account_scope": "cleared",
                            "next_scope": nextSlot == nil ? "logged-out" : "replacement",
                        ]
                    )
                }
            )
        )
        let accountAuthorizer = IOSAccountAuthorizer(
            applyAccountScope: { slot in
                boxVisibilityTracker.noteAccountSlot(slot)
                coordinator.updateAccountSettingsScope(slot)
            },
            applyLocalHumanIdentity: { slot in
                coordinatorLocalHumanIdentity.replace(with: slot)
            }
        )
        accountRuntime = CoordinatorAccountRuntime(
            cleanup: cleanup,
            authorize: { slot, previousSlot in
                let authorization = accountAuthorizer.authorizeSettledHostSlot(
                    slot,
                    previousSlot: previousSlot
                )
                if case .ready(let adoptedSlot) = authorization {
                    reporter.report(
                        .coordinatorHandoff,
                        metadata: [
                            "account_scope": adoptedSlot == nil ? "logged-out" : "adopted",
                        ]
                    )
                }
                return authorization
            }
        )
        updateWiring = IOSUpdateServiceWiring()

        lifecycleReporter.report(
            .startup,
            metadata: [
                "cold_start_resync": lifecycleRecovery.requiresColdStartResync ? "true" : "false",
                "session_id": lifecycleRecovery.currentCheckpoint.sessionID,
            ]
        )
    }

    var coordinatorBootstrap: ValidatedCoordinatorBootstrap {
        // The preload/auth carrier must exist before login so it can obtain the
        // canonical Host auth reply. Human-scoped Coordinator process launch is
        // separately fenced by humanScopedCoordinatorBootstrap().
        try! CoordinatorBootstrap(
            processConfig: .init(
                appVersion: coordinatorAppVersion,
                isPackaged: true,
                dataDir: coordinatorDataDirectory,
                localHumanId: coordinatorLocalHumanIdentity.value
            )
        ).validatedForCarrier()
    }

    func humanScopedCoordinatorBootstrap() throws -> ValidatedCoordinatorBootstrap {
        try coordinatorBootstrap.requiringLocalHumanIdentity()
    }

    var localHumanId: String? {
        coordinatorLocalHumanIdentity.value
    }

    /// Applies only a stable account slot already settled by canonical Host auth.
    /// Callers must never pass device/session identity here.
    func applySettledHostLocalHumanIdentity(_ slot: String?) {
        coordinatorLocalHumanIdentity.replace(with: slot)
    }

    var devControlsEnabled: Bool {
        devControlAdapter.isEnabled
    }

    func coordinatorDidLaunchForDevControls() {
        devControlAdapter.coordinatorDidLaunch()
    }

    var requiresColdStartResync: Bool {
        lifecycleRecovery.requiresColdStartResync
    }

    func markResyncCompleted() {
        lifecycleRecovery.markResyncCompleted()
    }

    func protectedDataWillBecomeUnavailable() {
        lifecycleRecovery.markResyncRequired()
        lifecycleReporter.report(
            .rendererLifecycle,
            level: .warn,
            metadata: ["phase": "protected-data-unavailable"]
        )
        coordinator.sceneWillSuspend()
        reportSessionActivity(active: false)
    }

    func protectedDataDidBecomeAvailable() {
        lifecycleReporter.report(
            .rendererLifecycle,
            metadata: ["phase": "protected-data-available"]
        )
    }

    func memoryPressureReceived() {
        lifecycleReporter.report(
            .processRecovery,
            level: .warn,
            metadata: ["reason": "memory-pressure"]
        )
    }

    /// Compatibility entry for platform-only callers. Renderer-facing code uses
    /// IOSPreloadBridge and never receives a Coordinator or Host reference.
    func dispatch(method: String, params: [String: Any] = [:]) async throws -> MahayanaCoordinator.JSONResult {
        let args = try CoordinatorPayload.fromFoundation(params)
        let outcome = await dispatchTransport(method: method, args: args)
        switch outcome {
        case .ok(let payload):
            return .init(value: payload.foundationValue)
        case .failed(let failure):
            throw MahayanaCoordinator.CoordinatorError.requestFailed(failure.message)
        }
    }

    func makeRendererPortServer(port: CoordinatorPort) -> RendererPortServer {
        let server = RendererPortServer(
            port: port,
            dispatch: { [weak self] method, args in
                guard let self else {
                    return .failed(.init(code: "coordinator-unavailable", message: "iOS main runtime was released"))
                }
                return await self.dispatchTransport(method: method, args: args)
            },
            onServing: { [weak self] in
                self?.coordinator.replayClientSideToolEvents()
            }
        )
        coordinator.setRendererEventSink { [weak server] family, payload in
            server?.postEvent(family: family, payload: payload)
        }
        return server
    }

    private func dispatchTransport(
        method: String,
        args: CoordinatorPayload
    ) async -> CoordinatorReplyOutcome {
        if method == "reportBoxVisibility" {
            if let report = args.foundationValue as? [String: Any] {
                boxVisibilityTracker.handle(report, documentKey: "ios-main")
            }
            return .ok(.object([:]))
        }
        if let updateOutcome = updateWiring.route(method: method, args: args) {
            return updateOutcome
        }
        let outcome = await coordinator.dispatchTransport(method: method, args: args)
        if let accountAuthorization = await accountRuntime.observeAuthReply(method: method, outcome: outcome),
           case .refused(_, let reason) = accountAuthorization {
            lifecycleReporter.report(
                .coordinatorHandoff,
                level: .warn,
                metadata: [
                    "account_scope": "refused",
                    "reason": reason,
                ]
            )
        }
        return outcome
    }

    func scenePhaseChanged(_ phase: ScenePhase) {
        switch phase {
        case .active:
            lifecycleRecovery.transition(to: .active)
            lifecycleReporter.report(.rendererLifecycle, metadata: ["phase": "active"])
            coordinator.sceneBecameActive()
            reportSessionActivity(active: true)
        case .background:
            lifecycleRecovery.transition(to: .background)
            lifecycleReporter.report(.rendererLifecycle, metadata: ["phase": "background"])
            coordinator.sceneEnteredBackground()
            reportSessionActivity(active: false)
        case .inactive:
            lifecycleRecovery.transition(to: .inactive)
            lifecycleReporter.report(.rendererLifecycle, metadata: ["phase": "inactive"])
            coordinator.sceneWillSuspend()
            reportSessionActivity(active: false)
        @unknown default:
            lifecycleRecovery.transition(to: .inactive)
            lifecycleReporter.report(
                .rendererLifecycle,
                level: .warn,
                metadata: ["phase": "unknown"]
            )
            coordinator.sceneWillSuspend()
            reportSessionActivity(active: false)
        }
    }

    private func reportSessionActivity(active: Bool) {
        Task { [weak self] in
            guard let self else { return }
            do {
                _ = try await coordinator.request(
                    method: "feature.sessionActivity",
                    params: ["active": active]
                )
            } catch {
                lifecycleReporter.report(
                    .coordinatorHandoff,
                    level: .warn,
                    metadata: [
                        "session_activity": active ? "active" : "inactive",
                        "result": "host-unavailable",
                    ]
                )
            }
        }
    }

    func shutdown() {
        lifecycleRecovery.transition(to: .shuttingDown)
        lifecycleReporter.report(
            .rendererLifecycle,
            metadata: ["phase": "shutting-down"]
        )
        accountRuntime.reset()
        boxVisibilityTracker.abandonAll()
        coordinatorLocalHumanIdentity.replace(with: nil)
        boxVisibilityTracker.noteAccountSlot(nil)
        coordinator.updateAccountSettingsScope(nil)
        reportSessionActivity(active: false)
        coordinator.beginShutdown()
    }
}
