import Foundation
import Observation
import SwiftUI

/// Production composition root. FabushiApp only owns Scene/App lifecycle and
/// forwards events here.
@MainActor
@Observable
final class FabushiRuntime {
    let main: IOSMainRuntime
    let bridge: IOSPreloadBridge
    let appAgentSurface: FabushiAppAgentSurface
    let remoteDeviceGateway: FabushiRemoteDeviceGateway
    let marketplace: MarketplaceModel
    let messaging: MessagingModel
    let authCallbackRegistration: IOSAuthCallbackRegistration
    private(set) var reconnectGeneration = 0

    @ObservationIgnored private var deepLinkController: IOSDeepLinkController?
    @ObservationIgnored private var wasBackgrounded = false
    @ObservationIgnored private var resumeTask: Task<Void, Never>?
    #if DEBUG
    @ObservationIgnored private var devControlsPreload: IOSDevControlsPreload?
    #endif

    init() {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first!
            .appendingPathComponent("com.ombhrum.fabushi", isDirectory: true)
        #if DEBUG
        let featureHostTest = ProcessInfo.processInfo.environment["FABUSHI_FEATURE_HOST_SMOKE"] == "1"
            || ProcessInfo.processInfo.environment["FABUSHI_FEATURE_HOST_TEST"] == "1"
        #else
        let featureHostTest = false
        #endif

        do {
            let authCallbackRegistration = try IOSAuthCallbackRegistrar.requireShippingRegistration()
            let main = try IOSMainRuntime(appDataDirectory: base, featureHostTest: featureHostTest)
            let bridge = IOSPrimaryPreloadEntrypoint.install(main: main)
            let surface = FabushiAppAgentSurface()
            self.main = main
            self.bridge = bridge
            self.authCallbackRegistration = authCallbackRegistration
            #if DEBUG
            devControlsPreload = IOSDevControlsPreloadEntrypoint.installIfEnabled(
                bridge: bridge,
                capability: main.devCapability
            )
            #endif
            appAgentSurface = surface
            marketplace = MarketplaceModel(bridge: bridge)
            messaging = MessagingModel(bridge: bridge)
            remoteDeviceGateway = FabushiRemoteDeviceGateway(
                bridge: bridge,
                surface: surface,
                traceURL: base.appendingPathComponent("device-gateway-trace.jsonl")
            )
            deepLinkController = IOSDeepLinkController(
                dispatch: { [weak self] parsed in
                    self?.dispatchGrokDeepLink(parsed)
                }
            )
        } catch {
            fatalError("Failed to initialize iOS runtime: \(error)")
        }
    }

    func start() async {
        await marketplace.runFeatureHostSmokeIfRequested()
        await marketplace.initializeApp()
        await remoteDeviceGateway.setLoggedIn(marketplace.loggedIn)
        if marketplace.loggedIn {
            await messaging.refresh()
        }
        deepLinkController?.markReady()
        if main.requiresColdStartResync {
            await resyncAfterLifecycleRecovery(reason: "cold-start")
        } else {
            main.markResyncCompleted()
        }
    }

    func loginStateChanged(_ loggedIn: Bool) async {
        await remoteDeviceGateway.setLoggedIn(loggedIn)
    }

    func scenePhaseChanged(_ phase: ScenePhase) {
        main.scenePhaseChanged(phase)
        switch phase {
        case .background:
            wasBackgrounded = true
        case .active where wasBackgrounded:
            wasBackgrounded = false
            resumeTask?.cancel()
            resumeTask = Task { [weak self] in
                await self?.resyncAfterLifecycleRecovery(reason: "background-resume")
            }
        default:
            break
        }
    }

    func protectedDataWillBecomeUnavailable() {
        main.protectedDataWillBecomeUnavailable()
    }

    func protectedDataDidBecomeAvailable() async {
        main.protectedDataDidBecomeAvailable()
        if !wasBackgrounded {
            await resyncAfterLifecycleRecovery(reason: "protected-data-available")
        }
    }

    func memoryPressureReceived() {
        main.memoryPressureReceived()
    }

    func handleOpenURL(_ url: URL) {
        _ = deepLinkController?.handleCandidate(
            url.absoluteString,
            origin: "scene-open-url"
        )
    }

    private func dispatchGrokDeepLink(_ parsed: ParsedFabushiDeepLink) {
        switch parsed.route {
        case .authComplete:
            marketplace.handleDeepLink(parsed.canonicalURL)
        case .agent(let agentID):
            marketplace.message = "已接收智能体链接：\(agentID)"
        case .section(let section):
            marketplace.message = "已接收应用链接：\(section)"
        case .info:
            marketplace.message = "Deep Link 支持已就绪"
        case .open:
            marketplace.message = "已通过 Deep Link 打开 Fabushi"
        case .pluginAdd(let pluginID):
            marketplace.query = pluginID
            Task { [weak self] in
                await self?.marketplace.refresh()
            }
        }
    }

    private func resyncAfterLifecycleRecovery(reason: String) async {
        await marketplace.refresh()
        await remoteDeviceGateway.resumeAfterBackground()
        if marketplace.loggedIn {
            await messaging.refresh()
        }
        main.markResyncCompleted()
        reconnectGeneration &+= 1
        main.lifecycleReporter.report(
            .coordinatorHandoff,
            metadata: [
                "resync_reason": reason,
                "result": "completed",
            ]
        )
    }
}
