import Foundation
import SwiftUI

/// In-process real Rust Host FFI smoke harness for `FabushiUITests.testCompleteFeatureHostUserJourneyInAppProcess`
/// (`FABUSHI_FEATURE_HOST_SMOKE == "1"`).
private struct FeatureHostSmokeHarnessView: View {
    @State private var smokeStatus: String = "running"

    var body: some View {
        VStack(spacing: 12) {
            Text(smokeStatus)
                .font(.system(size: 16, weight: .semibold, design: .monospaced))
                .accessibilityIdentifier("feature-host-smoke")
                .accessibilityLabel(smokeStatus)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .task {
            await runRealHostSmokeJourney()
        }
    }

    private func runRealHostSmokeJourney() async {
        let sandbox = FileManager.default.temporaryDirectory
            .appendingPathComponent("FabushiSmoke_\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: sandbox) }

        let client = FabushiHostClient()
        do {
            let boot = try await client.initialize(
                appSupportDir: sandbox,
                keychainBridge: KeychainStore(
                    serviceNamespace: "com.ombhrum.fabushi.smoke.\(UUID().uuidString)",
                    appSupportDir: sandbox,
                    environment: ["FABUSHI_UI_TEST_MODE": "1"]
                ),
                biometricsBridge: BiometricsAuthenticator(environment: ["FABUSHI_UI_TEST_MODE": "1"])
            )
            guard !boot.serverURL.isEmpty else {
                smokeStatus = "failed:empty_server_url"
                return
            }

            // 1. Real C-ABI ping roundtrip
            struct PingParams: Codable, Sendable { let nonce: UInt64 }
            struct PingResp: Decodable, Sendable { let pong: Bool; let nonce: UInt64 }
            let ping: PingResp = try await client.call("config.ping", params: PingParams(nonce: 20261118))
            guard ping.pong && ping.nonce == 20261118 else {
                smokeStatus = "failed:ping_mismatch"
                return
            }

            // 2. Real SQLite Outbox + Message persistence + E2EE key generation roundtrip
            struct SendParams: Codable, Sendable {
                let chat_id: Int64
                let text: String
                let client_nonce: UInt64
            }
            struct MsgResp: Decodable, Sendable { let id: Int64; let text: String }
            let sent: MsgResp = try await client.call(
                "messages.send",
                params: SendParams(chat_id: 777, text: "Smoke FFI Journey", client_nonce: 888001)
            )
            guard sent.id > 0 && sent.text == "Smoke FFI Journey" else {
                smokeStatus = "failed:send_message"
                return
            }

            // 3. Real SemanticWorkspaceTree SHA-256 verification across Swift & Rust
            let snap: SemanticWorkspaceTree = try await client.call("semantic.snapshot")
            guard snap.stateHash.count == 64 && snap.computeFullStateHash() == snap.stateHash else {
                smokeStatus = "failed:semantic_hash_mismatch"
                return
            }

            // 4. Real ScenePhase background WAL checkpoint + foreground resume
            await client.handleScenePhaseChange(.background)
            await client.handleScenePhaseChange(.active)
            await client.shutdown()

            smokeStatus = "passed"
        } catch {
            await client.shutdown()
            smokeStatus = "failed:\(error.localizedDescription)"
        }
    }
}

/// Canonical `@main` entrypoint for `Fabushi` (`mobile/ios/project.yml` excludes legacy `FabushiApp.swift`).
@main
struct FabushiApp: App {
    @UIApplicationDelegateAdaptor(FabushiAppDelegate.self)
    private var appDelegate

    var body: some Scene {
        WindowGroup {
            rootSceneContent
        }
    }

    @ViewBuilder
    private var rootSceneContent: some View {
        let env = ProcessInfo.processInfo.environment
        if IOSUnitTestHostPolicy.shouldBypassProductRuntime(environment: env) {
            Color.clear
                .accessibilityIdentifier("fabushi-unit-test-host")
        } else if env["FABUSHI_FEATURE_HOST_SMOKE"] == "1" {
            FeatureHostSmokeHarnessView()
        } else if env["FABUSHI_FEATURE_HOST_TEST"] == "1"
            || env["FABUSHI_CI_ACCOUNT_SESSION_BASE64"] != nil
            || env["FABUSHI_LEGACY_GROK_SHELL"] == "1" {
            FabushiSceneRoot()
        } else {
            FabushiRootShellView()
        }
    }
}
