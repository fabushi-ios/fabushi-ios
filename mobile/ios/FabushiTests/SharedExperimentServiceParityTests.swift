import XCTest
@testable import Fabushi

private actor ExperimentBootstrapScript {
    private var callCount = 0
    private let first: StatsigBootstrapResult
    private let second: StatsigBootstrapResult

    init(first: StatsigBootstrapResult, second: StatsigBootstrapResult) {
        self.first = first
        self.second = second
    }

    func next() async throws -> StatsigBootstrapResult {
        callCount += 1
        if callCount == 1 {
            try await Task.sleep(for: .milliseconds(60))
            return first
        }
        return second
    }

    func calls() -> Int { callCount }
}

final class SharedExperimentServiceParityTests: XCTestCase {
    private func makeRoot() throws -> URL {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        return root
    }

    func testGeneratedRegistryMatchesPinnedGrokInventory() {
        XCTAssertEqual(GROK_FEATURE_FLAG_NAMES.count, 605)
        XCTAssertEqual(GROK_EXPERIMENT_NAMES.count, 109)
        XCTAssertEqual(GROK_DYNAMIC_CONFIG_NAMES.count, 123)
        XCTAssertTrue(GROK_EXPERIMENT_NAMES.contains("sand_model_selection"))
        XCTAssertTrue(GROK_DYNAMIC_CONFIG_NAMES.contains("sand_rpc_tracing"))
        XCTAssertTrue(GROK_DYNAMIC_CONFIG_NAMES.contains("sand_default_model"))
        XCTAssertEqual(
            IOS_DYNAMIC_CONFIG_FALLBACKS["grok_bot_conversation_size_limits"]?["hard_limit_mb"],
            .number(1024)
        )
    }

    func testPollAndEnvironmentOverrideContracts() {
        XCTAssertEqual(
            computePollDelayMs(baseIntervalMs: 100_000, jitterRoll: 1),
            130_000
        )
        XCTAssertEqual(
            computePollDelayMs(baseIntervalMs: 100_000, jitterRoll: 0, retryAfterMs: 150_000),
            150_000
        )
        XCTAssertEqual(
            envGateOverride("sand_client_pause", env: [
                "SAND_FEATURE_GATE_OVERRIDES": "sand_multitask=0, sand_client_pause=on",
            ]),
            true
        )
        XCTAssertEqual(
            jitteredSandExperimentPollIntervalMs(isDevBuild: true, randomRoll: 0),
            MIN_POLL_INTERVAL_MS
        )
    }

    func testFabushiProductGateDefaultIsFallbackBelowRemoteState() throws {
        let root = try makeRoot()
        defer { try? FileManager.default.removeItem(at: root) }

        let service = SandExperimentService(
            getCacheDir: { root.path },
            productFeatureGateDefaults: ["sand_agent_network": true]
        )
        XCTAssertTrue(service.checkFeatureGate("sand_agent_network"))

        service.hydrate(
            config: #"{"featureGates":{"sand_agent_network":{"value":false}}}"#,
            fetchedAtMs: 1_000,
            live: true
        )
        XCTAssertFalse(service.checkFeatureGate("sand_agent_network"))
    }

    func testHydrationAppliesRemoteGatesExperimentsAndModelConfigs() throws {
        let root = try makeRoot()
        defer { try? FileManager.default.removeItem(at: root) }

        let service = SandExperimentService(
            getCacheDir: { root.path },
            isDevBuild: true,
            env: ["SAND_FEATURE_GATE_OVERRIDES": "sand_client_pause=on"]
        )
        let config = #"""
        {
          "user": {"userID":"user-1","email":"u@example.test"},
          "featureGates": {
            "sand_multitask": {"value": false},
            "sand_browser_use_subagent": {"value": true}
          },
          "experiments": {
            "sand_model_selection": {
              "groupName": "Treatment",
              "value": {"enabled": true}
            }
          },
          "dynamicConfigs": {
            "sand_default_model": {
              "value": {
                "modelId": "claude-opus-4-8",
                "maxMode": true,
                "parameters": [{"id":"thinking","value":"true"}]
              }
            },
            "sand_browser_use_model": {
              "value": {
                "modelId": "claude-opus-4-8",
                "maxMode": false,
                "parameters": [{"id":"effort","value":"low"}]
              }
            }
          }
        }
        """#

        service.hydrate(config: config, fetchedAtMs: 1_000, live: true)

        XCTAssertFalse(service.checkFeatureGate("sand_multitask"))
        XCTAssertTrue(service.checkFeatureGate("sand_client_pause"))
        XCTAssertTrue(service.hasAuthenticatedStatsigBootstrap())
        XCTAssertTrue(service.hasHydratedStatsigUserId())
        XCTAssertEqual(service.getFlagsAgeMs(nowMs: 1_250), 250)
        XCTAssertEqual(service.getSandModelExperimentState()?.arm, .treatment)
        XCTAssertEqual(service.getConfiguredDefaultModel()?.modelId, "claude-opus-4-8")
        XCTAssertEqual(service.getBrowserUseModelOverride()?.parameters.first?.value, "low")
        XCTAssertTrue(service.getSnapshot().isInitialized)
    }

    func testDeveloperOverrideUpdatesLiveGatePropertyAndPersists() throws {
        let root = try makeRoot()
        defer { try? FileManager.default.removeItem(at: root) }

        let service = SandExperimentService(getCacheDir: { root.path }, isDevBuild: true)
        let property = service.getFeatureGateProperty("sand_client_pause")
        XCTAssertFalse(property.get())

        service.setFeatureFlagOverride("sand_client_pause", value: true)
        XCTAssertTrue(property.get())
        XCTAssertEqual(service.getSnapshot().featureFlagOverrides["sand_client_pause"], true)

        let restored = SandExperimentService(getCacheDir: { root.path }, isDevBuild: true)
        restored.startFromCache()
        XCTAssertTrue(restored.checkFeatureGate("sand_client_pause"))
    }

    func testCachedBootstrapHydratesIdentityWithoutClaimingLiveNetworkAuth() throws {
        let root = try makeRoot()
        defer { try? FileManager.default.removeItem(at: root) }
        let config = #"{"user":{"userID":"cached-user"},"featureGates":{"sand_multitask":{"value":false}}}"#
        saveCachedBootstrap(
            root.path,
            cache: .init(config: config, userId: "cached-user", fetchedAtMs: 500)
        )

        let service = SandExperimentService(getCacheDir: { root.path })
        service.startFromCache()

        XCTAssertTrue(service.hasHydratedStatsigUserId())
        XCTAssertFalse(service.hasLiveStatsigBootstrap())
        XCTAssertFalse(service.hasAuthenticatedStatsigBootstrap())
        XCTAssertFalse(service.checkFeatureGate("sand_multitask"))
        XCTAssertTrue(service.getSnapshot().isInitialized)
    }
    func testAuthChangeFencesStaleBootstrapAndLatestIdentityWins() async throws {
        let root = try makeRoot()
        defer { try? FileManager.default.removeItem(at: root) }

        let oldConfig = #"{"user":{"userID":"old-user"},"featureGates":{"sand_client_pause":{"value":false}}}"#
        let newConfig = #"{"user":{"userID":"new-user"},"featureGates":{"sand_client_pause":{"value":true}}}"#
        let script = ExperimentBootstrapScript(
            first: .init(config: oldConfig),
            second: .init(config: newConfig)
        )
        let service = SandExperimentService(
            getCacheDir: { root.path },
            bootstrapFetcher: { _ in try await script.next() }
        )

        let first = Task {
            await service.refresh(
                backendUrl: "https://backend.example.test",
                getAccessToken: { _ in "old-token" },
                getMachineId: { "machine" },
                trigger: .startup
            )
        }
        try await Task.sleep(for: .milliseconds(10))
        service.handleAuthChange()
        await service.refresh(
            backendUrl: "https://backend.example.test",
            getAccessToken: { _ in "new-token" },
            getMachineId: { "machine" },
            trigger: .authChange
        )
        await first.value

        let bootstrapCalls = await script.calls()
        XCTAssertEqual(bootstrapCalls, 2)
        XCTAssertTrue(service.checkFeatureGate("sand_client_pause"))
        XCTAssertTrue(service.hasAuthenticatedStatsigBootstrap())
    }

    func testOverrideCommandSurfaceMatchesDesktopReplaceAndBundledContracts() throws {
        let root = try makeRoot()
        defer { try? FileManager.default.removeItem(at: root) }
        let service = SandExperimentService(getCacheDir: { root.path }, isDevBuild: true)

        service.applyFeatureFlagOverrideCommand([
            "kind": "set",
            "name": "sand_client_pause",
            "value": true,
        ])
        XCTAssertEqual(service.getFeatureFlagOverridesRecord()["sand_client_pause"], true)

        service.applyFeatureFlagOverrideCommand(["kind": "clear-all"])
        XCTAssertTrue(service.getFeatureFlagOverridesRecord().isEmpty)

        service.applyFeatureFlagOverrideCommand(["kind": "set-all-bundled"])
        XCTAssertEqual(service.getFeatureFlagOverridesRecord().count, BUNDLED_FEATURE_FLAGS.count)
        service.replaceFeatureFlagOverrides([
            "sand_client_pause": false,
            "definitely_not_a_real_flag": true,
        ])
        XCTAssertEqual(service.getFeatureFlagOverridesRecord(), ["sand_client_pause": false])
    }

}
