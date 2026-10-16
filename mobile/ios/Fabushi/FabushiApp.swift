import SwiftUI

@main
struct FabushiApp: App {
    @State private var model: MarketplaceModel
    @State private var appAgentSurface: FabushiAppAgentSurface
    @State private var messagingModel: MessagingModel
    private let host: MahayanaHost
    private let remoteDeviceGateway: FabushiRemoteDeviceGateway

    init() {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first!
            .appendingPathComponent("com.ombhrum.fabushi", isDirectory: true)
        do {
            #if DEBUG
            let featureHostTest = ProcessInfo.processInfo.environment["FABUSHI_FEATURE_HOST_SMOKE"] == "1"
                || ProcessInfo.processInfo.environment["FABUSHI_FEATURE_HOST_TEST"] == "1"
            #else
            let featureHostTest = false
            #endif
            let host = try MahayanaHost(appDataDirectory: base, featureHostTest: featureHostTest)
            let appAgentSurface = FabushiAppAgentSurface()
            self.host = host
            _model = State(initialValue: MarketplaceModel(host: host))
            _messagingModel = State(initialValue: MessagingModel(host: host))
            _appAgentSurface = State(initialValue: appAgentSurface)
            remoteDeviceGateway = FabushiRemoteDeviceGateway(
                host: host,
                surface: appAgentSurface,
                traceURL: base.appendingPathComponent("device-gateway-trace.jsonl")
            )
        } catch {
            fatalError("Failed to initialize Mahayana Host: \(error)")
        }
    }

    var body: some Scene {
        WindowGroup {
            GrokMobileShell(model: model, messaging: messagingModel, host: host, appAgentSurface: appAgentSurface)
                .task {
                    await model.runFeatureHostSmokeIfRequested()
                    await model.initializeApp()
                    await remoteDeviceGateway.setLoggedIn(model.loggedIn)
                    if model.loggedIn {
                        await messagingModel.refresh()
                    }
                }
                .onChange(of: model.loggedIn) { _, loggedIn in
                    Task { await remoteDeviceGateway.setLoggedIn(loggedIn) }
                }
                .onOpenURL { url in
                    consumeDeepLink(url)
                }
        }
    }

    @MainActor
    private func consumeDeepLink(_ url: URL) {
        guard url.scheme?.lowercased() == "fabushi",
              url.user == nil,
              url.password == nil,
              url.port == nil
        else { return }

        let host = url.host?.lowercased() ?? ""
        let parts = url.pathComponents.filter { $0 != "/" && !$0.isEmpty }
        switch host {
        case "auth":
            model.handleDeepLink(url)
        case "agent":
            guard let agentId = parts.first, !agentId.isEmpty, agentId.count <= 200 else { return }
            model.message = "已接收智能体链接：\(agentId)"
        case "settings", "feedback", "about", "widgets", "onboarding":
            model.message = "已接收应用链接：\(host)"
        default:
            return
        }
    }
}
