import XCTest
@testable import Fabushi

@MainActor
private final class SettingsParityHost: MahayanaHostRequesting {
    func request(method: String, params: [String: Any]) async throws -> MahayanaHostJSONResult {
        .init(value: ["method": method])
    }
}

final class SharedSettingsParityTests: XCTestCase {
    private func makeRoot() throws -> URL {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        return root
    }

    func testLegacyPartialSettingsArePreservedAndNormalized() throws {
        let root = try makeRoot()
        defer { try? FileManager.default.removeItem(at: root) }
        let path = root.appendingPathComponent("settings.json")
        let legacy: [String: Any] = [
            "version": 1,
            "themePreference": "dark",
            "webauthnProxyEnabled": false,
            "updateTrackOverride": "nightly",
            "mcpBoxServers": ["alpha", "alpha", ""],
            "mcpCustomInstructionsByServerId": [
                "12": "keep",
                "bad": "drop",
            ],
            "mcpDisabledToolsByServerId": [
                "12": ["search", "search", ""],
                "0": ["drop"],
            ],
            "settingsMigrations": [],
            "agentDefaultModel": [
                "modelId": "claude-opus-4-8",
                "maxMode": false,
                "parameters": [
                    ["id": "fast", "value": "true"],
                    ["id": "thinking", "value": "true"],
                ],
            ],
        ]
        try JSONSerialization.data(withJSONObject: legacy)
            .write(to: path, options: .atomic)

        let store = SandSettingsStore(settingsPath: path.path)
        let loaded = store.load()

        XCTAssertEqual(loaded.themePreference, "dark")
        XCTAssertFalse(loaded.webauthnProxyEnabled)
        XCTAssertEqual(loaded.mcpBoxServers, ["alpha"])
        XCTAssertEqual(loaded.mcpCustomInstructionsByServerId, ["12": "keep"])
        XCTAssertEqual(loaded.mcpDisabledToolsByServerId, ["12": ["search"]])
        XCTAssertEqual(loaded.agentDefaultModel?.maxMode, true)
        XCTAssertEqual(
            loaded.agentDefaultModel?.parameters.first(where: { $0.id == "fast" })?.value,
            "false"
        )
        XCTAssertTrue(loaded.settingsMigrations.contains(SAND_DOWNGRADE_MAX_FAST_MIGRATION_ID))

        XCTAssertEqual(store.getUpdateTrackOverride(), .stable)
        XCTAssertEqual(store.load().updateTrackOverride, "stable")
    }

    func testFabushiIsFreshInferenceDefaultAndExplicitProviderPersists() throws {
        let root = try makeRoot()
        defer { try? FileManager.default.removeItem(at: root) }
        let path = root.appendingPathComponent("settings.json")
        let store = SandSettingsStore(settingsPath: path.path)

        XCTAssertEqual(store.getInferenceProvider(), .fabushi)
        store.setInferenceProvider(.openrouter)

        let restored = SandSettingsStore(settingsPath: path.path)
        XCTAssertEqual(restored.getInferenceProvider(), .openrouter)
    }

    func testAccountScopePreservesFirstOwnerThenClearsCrossAccountState() throws {
        let root = try makeRoot()
        defer { try? FileManager.default.removeItem(at: root) }
        let path = root.appendingPathComponent("settings.json")
        let store = SandSettingsStore(settingsPath: path.path)

        store.setMcpCustomInstructions(["GitHub": "owner-only"])
        store.setMcpCustomInstructionsByServerId(["12": "owner-only"])
        store.setMcpDisabledToolsByServerId(["12": ["delete"]])
        store.setAgentDefaultModel(.init(
            modelId: "claude-opus-4-8",
            maxMode: false,
            parameters: [.init(id: "thinking", value: "true")]
        ))

        store.scopeToAccount("owner-a")
        XCTAssertEqual(store.getMcpCustomInstructions()["GitHub"], "owner-only")
        XCTAssertNotNil(store.getAgentDefaultModel())

        store.setHasSeenOnboarding(true)
        store.scopeToAccount("owner-b")
        let scoped = store.load()

        XCTAssertTrue(scoped.mcpCustomInstructions.isEmpty)
        XCTAssertTrue(scoped.mcpCustomInstructionsByServerId.isEmpty)
        XCTAssertTrue(scoped.mcpDisabledToolsByServerId.isEmpty)
        XCTAssertNil(scoped.agentDefaultModel)
        XCTAssertNil(scoped.hasSeenOnboarding)
        XCTAssertEqual(scoped.mcpCustomInstructionsAccountScope, "owner-b")
    }

    func testDefaultsFailClosedForInvalidOrCorruptSettings() throws {
        let root = try makeRoot()
        defer { try? FileManager.default.removeItem(at: root) }
        let path = root.appendingPathComponent("settings.json")
        try Data(#"{ "version": 99, "themePreference": "dark" }"#.utf8).write(to: path)

        let unsupported = SandSettingsStore(settingsPath: path.path).load()
        XCTAssertEqual(unsupported.version, SETTINGS_VERSION)
        XCTAssertEqual(unsupported.themePreference, nil)
        XCTAssertTrue(unsupported.webauthnProxyEnabled)

        try Data("not-json".utf8).write(to: path)
        let corrupt = SandSettingsStore(settingsPath: path.path).load()
        XCTAssertEqual(corrupt.version, SETTINGS_VERSION)
        XCTAssertTrue(corrupt.mcpBoxServers.isEmpty)
        XCTAssertEqual(corrupt.conciergeConsent, "unset")
    }
    func testCurrentMainUiAndCallPreferencesNormalizeAndPersist() throws {
        let root = try makeRoot()
        defer { try? FileManager.default.removeItem(at: root) }
        let path = root.appendingPathComponent("settings.json")
        let store = SandSettingsStore(settingsPath: path.path)

        store.setUiPreferences(.init(
            locale: "AR-sa",
            direction: .auto,
            reducedMotion: true,
            highContrast: true,
            textScale: 9
        ))
        store.setCallMediaPreferences(.init(
            microphoneId: " mic-1 ",
            cameraId: String(repeating: "x", count: 513)
        ))

        let restored = SandSettingsStore(settingsPath: path.path)
        let ui = restored.getUiPreferences()
        XCTAssertEqual(ui.locale, "ar-sa")
        XCTAssertEqual(ui.direction, .auto)
        XCTAssertEqual(resolveSandUiDirection(ui), .rtl)
        XCTAssertTrue(ui.reducedMotion)
        XCTAssertTrue(ui.highContrast)
        XCTAssertEqual(ui.textScale, 2)

        let media = restored.getCallMediaPreferences()
        XCTAssertEqual(media.microphoneId, "mic-1")
        XCTAssertNil(media.cameraId)
    }

    func testServerAcceptedBrowserAuthUrlMustStayOnConfiguredOrigin() {
        XCTAssertEqual(
            ExternalURLPolicy.parseServerAcceptedAuthExternalURL(
                "https://api.ombhrum.com/auth/browser?attempt=1",
                expectedOrigin: "https://api.ombhrum.com"
            ),
            "https://api.ombhrum.com/auth/browser?attempt=1"
        )
        XCTAssertNil(ExternalURLPolicy.parseServerAcceptedAuthExternalURL(
            "https://attacker.invalid/auth/browser",
            expectedOrigin: "https://api.ombhrum.com"
        ))
        XCTAssertNil(ExternalURLPolicy.parseServerAcceptedAuthExternalURL(
            "http://api.ombhrum.com/auth/browser",
            expectedOrigin: "https://api.ombhrum.com"
        ))
        XCTAssertNil(ExternalURLPolicy.parseServerAcceptedAuthExternalURL(
            "https://user@api.ombhrum.com/auth/browser",
            expectedOrigin: "https://api.ombhrum.com"
        ))
    }

    @MainActor
    func testCoordinatorOwnsAndScopesSettingsWithoutRendererHostBypass() throws {
        let root = try makeRoot()
        defer { try? FileManager.default.removeItem(at: root) }
        let store = SandSettingsStore(settingsPath: root.appendingPathComponent("settings.json").path)
        store.setMcpCustomInstructions(["GitHub": "owner-a"])
        store.scopeToAccount("owner-a")

        let host = SettingsParityHost()
        let supervisor = MahayanaLocalHostSupervisor(host: host, factory: { host })
        let coordinator = MahayanaCoordinator(hostSupervisor: supervisor, settingsStore: store)

        XCTAssertEqual(coordinator.sharedSettingsSnapshot().mcpCustomInstructions["GitHub"], "owner-a")
        coordinator.updateAccountSettingsScope("owner-b")
        XCTAssertTrue(coordinator.sharedSettingsSnapshot().mcpCustomInstructions.isEmpty)
        coordinator.updateAccountSettingsScope(nil)
        XCTAssertNil(coordinator.sharedSettingsSnapshot().mcpCustomInstructionsAccountScope)
    }

}
