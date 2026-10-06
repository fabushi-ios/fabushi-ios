import Foundation
import SwiftUI
import Combine
import UIKit

/// Scene-facing adapter for the iOS product runtime.
///
/// This wrapper keeps XCTest's unsigned unit-test host from constructing the
/// production runtime before the test bundle is injected. Normal app launches
/// and UI-test launches still construct the full product runtime.
@MainActor
internal struct FabushiSceneRoot: View {
    private let bypassProductRuntime: Bool

    init(environment: [String: String] = ProcessInfo.processInfo.environment) {
        #if DEBUG
        bypassProductRuntime = IOSUnitTestHostPolicy.shouldBypassProductRuntime(
            environment: environment
        )
        #else
        bypassProductRuntime = false
        #endif
    }

    @ViewBuilder
    var body: some View {
        if bypassProductRuntime {
            FabushiUnitTestHostRoot()
        } else {
            FabushiProductionSceneRoot()
        }
    }
}

@MainActor
private struct FabushiUnitTestHostRoot: View {
    var body: some View {
        Color.clear
            .accessibilityIdentifier("fabushi-unit-test-host")
    }
}

/// Production scene adapter. Product orchestration remains in FabushiRuntime
/// and lower layers; FabushiApp stays a thin App/Scene entry point.
@MainActor
private struct FabushiProductionSceneRoot: View {
    @State private var runtime = FabushiRuntime()
    @Environment(\.scenePhase) private var scenePhase

    var body: some View {
        ProductionRenderer(
            model: runtime.marketplace,
            messaging: runtime.messaging,
            bridge: runtime.bridge,
            appAgentSurface: runtime.appAgentSurface,
            reconnectGeneration: runtime.reconnectGeneration
        )
        .task {
            await runtime.start()
        }
        .onChange(of: runtime.marketplace.loggedIn) { _, loggedIn in
            Task { await runtime.loginStateChanged(loggedIn) }
        }
        .onChange(of: scenePhase) { _, phase in
            runtime.scenePhaseChanged(phase)
        }
        .onOpenURL { url in
            runtime.handleOpenURL(url)
        }
        .onContinueUserActivity(NSUserActivityTypeBrowsingWeb) { activity in
            guard let url = activity.webpageURL else { return }
            runtime.handleOpenURL(url)
        }
        .onReceive(NotificationCenter.default.publisher(
            for: UIApplication.protectedDataWillBecomeUnavailableNotification
        )) { _ in
            runtime.protectedDataWillBecomeUnavailable()
        }
        .onReceive(NotificationCenter.default.publisher(
            for: UIApplication.protectedDataDidBecomeAvailableNotification
        )) { _ in
            Task { await runtime.protectedDataDidBecomeAvailable() }
        }
        .onReceive(NotificationCenter.default.publisher(
            for: UIApplication.didReceiveMemoryWarningNotification
        )) { _ in
            runtime.memoryPressureReceived()
        }
    }
}
