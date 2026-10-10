import Foundation
import SwiftUI
import Combine
import Observation
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

internal enum FabushiErrorBoundaryAccessibility {
    static let surface = "fabushi-root-error-surface"
    static let title = "fabushi-root-error-title"
    static let detail = "fabushi-root-error-detail"
    static let retry = "fabushi-root-error-retry"
    static let copyDiagnostics = "fabushi-root-error-copy-diagnostics"

    static let titleLabel = "Fabushi could not start"
    static let detailLabel = "A root-level error prevented the app from opening safely. You can retry or copy diagnostics."
    static let retryLabel = "Retry"
    static let copyLabel = "Copy diagnostics"
    static let copiedLabel = "Copied"
}

internal struct FabushiRootFailure: Equatable {
    let title: String
    let detail: String
    let diagnostics: String

    init(error: Error, context: String) {
        let nsError = error as NSError
        title = FabushiErrorBoundaryAccessibility.titleLabel
        detail = FabushiErrorBoundaryAccessibility.detailLabel
        diagnostics = [
            "Fabushi iOS root failure",
            "Context: \(context)",
            "Error type: \(String(reflecting: type(of: error)))",
            "Description: \(error.localizedDescription)",
            "Domain: \(nsError.domain)",
            "Code: \(nsError.code)",
        ].joined(separator: "\n")
    }
}

@MainActor
@Observable
internal final class FabushiRuntimeLifecycle {
    enum Phase {
        case idle
        case ready(FabushiRuntime)
        case failed(FabushiRootFailure)
    }

    typealias RuntimeFactory = @MainActor () throws -> FabushiRuntime

    private let makeRuntime: RuntimeFactory
    private(set) var phase: Phase = .idle
    private(set) var constructionAttempts = 0

    init(makeRuntime: @escaping RuntimeFactory = { try FabushiRuntime() }) {
        self.makeRuntime = makeRuntime
    }

    var rendersProductionRenderer: Bool {
        if case .ready = phase { return true }
        return false
    }

    func loadIfNeeded() {
        guard case .idle = phase else { return }
        rebuildRuntime()
    }

    func retry() {
        phase = .idle
        rebuildRuntime()
    }

    func presentRootFailure(_ error: Error, context: String = "root") {
        phase = .failed(FabushiRootFailure(error: error, context: context))
    }

    private func rebuildRuntime() {
        constructionAttempts += 1
        do {
            phase = .ready(try makeRuntime())
        } catch {
            presentRootFailure(error, context: "runtime-initialization")
        }
    }
}

internal func copyFabushiDiagnostics(
    _ diagnostics: String,
    using writer: (String) throws -> Void
) -> Bool {
    do {
        try writer(diagnostics)
        return true
    } catch {
        return false
    }
}

private enum FabushiClipboardError: Error {
    case writeFailed
}

@MainActor
private struct FabushiRootErrorSurface: View {
    let failure: FabushiRootFailure
    let retry: () -> Void
    @State private var copied = false

    var body: some View {
        VStack(spacing: 20) {
            VStack(spacing: 8) {
                Text(failure.title)
                    .font(.title2.weight(.semibold))
                    .accessibilityIdentifier(FabushiErrorBoundaryAccessibility.title)
                Text(failure.detail)
                    .font(.body)
                    .multilineTextAlignment(.center)
                    .accessibilityIdentifier(FabushiErrorBoundaryAccessibility.detail)
            }
            .accessibilityElement(children: .contain)

            HStack(spacing: 12) {
                Button(FabushiErrorBoundaryAccessibility.retryLabel) {
                    copied = false
                    retry()
                }
                .buttonStyle(.borderedProminent)
                .accessibilityIdentifier(FabushiErrorBoundaryAccessibility.retry)

                Button(copied
                    ? FabushiErrorBoundaryAccessibility.copiedLabel
                    : FabushiErrorBoundaryAccessibility.copyLabel
                ) {
                    copied = copyFabushiDiagnostics(failure.diagnostics) { text in
                        UIPasteboard.general.string = text
                        guard UIPasteboard.general.string == text else {
                            throw FabushiClipboardError.writeFailed
                        }
                    }
                }
                .buttonStyle(.bordered)
                .accessibilityIdentifier(FabushiErrorBoundaryAccessibility.copyDiagnostics)
            }
        }
        .padding(32)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Color(uiColor: .systemBackground))
        .accessibilityIdentifier(FabushiErrorBoundaryAccessibility.surface)
    }
}

/// Production scene adapter. Product orchestration remains in FabushiRuntime
/// and lower layers; FabushiApp stays a thin App/Scene entry point.
@MainActor
private struct FabushiProductionSceneRoot: View {
    @State private var lifecycle = FabushiRuntimeLifecycle()
    @Environment(\.scenePhase) private var scenePhase
    @Environment(\.openURL) private var openURL
    @Environment(\.layoutDirection) private var systemLayoutDirection
    @Environment(\.dynamicTypeSize) private var systemDynamicTypeSize
    @Environment(\.accessibilityReduceMotion) private var systemReduceMotion
    @Environment(\.colorSchemeContrast) private var systemColorSchemeContrast

    @ViewBuilder
    var body: some View {
        switch lifecycle.phase {
        case .idle:
            ProgressView()
                .task {
                    lifecycle.loadIfNeeded()
                }
        case .failed(let failure):
            FabushiRootErrorSurface(
                failure: failure,
                retry: lifecycle.retry
            )
        case .ready(let runtime):
            ProductionRenderer(
                model: runtime.marketplace,
                messaging: runtime.messaging,
                bridge: runtime.bridge,
                appAgentSurface: runtime.appAgentSurface,
                reconnectGeneration: runtime.reconnectGeneration,
                onRetryConnection: {
                    await runtime.retryConnection()
                },
                appVersionPolicyState: runtime.appVersionPolicyState,
                onRetryAppVersionPolicy: {
                    runtime.retryAppVersionPolicy()
                },
                onOpenUpdateURL: { url in
                    openURL(url)
                }
            )
            .environment(
                \.locale,
                runtime.uiPreferencesStore.preferences.resolvedLocale()
            )
            .environment(
                \.layoutDirection,
                runtime.uiPreferencesStore.preferences.resolvedLayoutDirection(
                    system: systemLayoutDirection
                )
            )
            .environment(
                \.dynamicTypeSize,
                runtime.uiPreferencesStore.preferences.adjustedDynamicTypeSize(
                    system: systemDynamicTypeSize
                )
            )
            .transaction { transaction in
                if systemReduceMotion || runtime.uiPreferencesStore.preferences.reducedMotion {
                    transaction.animation = nil
                    transaction.disablesAnimations = true
                }
            }
            .contrast(runtime.uiPreferencesStore.preferences.highContrast ? 1.15 : 1)
            .environment(\.mobileUiPreferencesStore, runtime.uiPreferencesStore)
            .environment(\.humanCallMediaPort, runtime.humanCallMediaPort)
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
}
