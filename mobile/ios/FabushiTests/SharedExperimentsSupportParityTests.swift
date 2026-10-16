import XCTest
@testable import Fabushi

final class SharedExperimentsSupportParityTests: XCTestCase {
    func testPinnedFeatureFlagRegistryAndOverrideTtl() {
        XCTAssertEqual(BUNDLED_FEATURE_FLAGS.count, 605)
        XCTAssertEqual(BUNDLED_FEATURE_FLAGS["sand_multitask"]?.defaultValue, true)
        XCTAssertEqual(BUNDLED_FEATURE_FLAGS["sand_client_pause"]?.defaultValue, false)
        XCTAssertTrue(isFlagName("sand_computer_use_playwright"))
        XCTAssertFalse(isFlagName("not_a_real_flag"))

        var now: Int64 = 1_000
        let store = SandFeatureFlagOverrideStore(getCacheDir: { "/tmp" }, now: { now })
        XCTAssertTrue(store.set("sand_client_pause", value: true))
        XCTAssertEqual(store.read("sand_client_pause"), true)
        now += FEATURE_FLAG_OVERRIDE_TTL_MS + 1
        XCTAssertNil(store.read("sand_client_pause"))
    }

    func testOverridePersistenceUsesAtomicSandboxFile() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        var now: Int64 = 10_000
        let store = SandFeatureFlagOverrideStore(getCacheDir: { root.path }, now: { now })
        XCTAssertTrue(store.set("sand_multitask", value: false))
        store.persist()

        let restored = SandFeatureFlagOverrideStore(getCacheDir: { root.path }, now: { now })
        restored.hydrateFromDisk()
        XCTAssertEqual(restored.read("sand_multitask"), false)
        now += 1
        try? FileManager.default.removeItem(at: root)
    }

    func testModelConfigValidationAndExperimentResolution() {
        XCTAssertEqual(resolveSandDefaultModelConfig(raw: [:], hasHydratedStatsigUserId: false).rejection, .identityUnhydrated)
        let valid: [String: Any] = [
            "modelId":"claude-opus-4-8",
            "maxMode":true,
            "parameters":[["id":"thinking","value":"true"]],
        ]
        XCTAssertEqual(resolveSandDefaultModelConfig(raw: valid, hasHydratedStatsigUserId: true).selection?.modelId, "claude-opus-4-8")

        let routed: [String: Any] = ["modelId":"default","maxMode":true,"parameters":[]]
        XCTAssertEqual(resolveSandDefaultModelConfig(raw: routed, hasHydratedStatsigUserId: true).rejection, .routedModelParameters)

        XCTAssertEqual(readSandModelExperimentEnvOverride(["SAND_MODEL_EXPERIMENT_OVERRIDE":"test"])?.arm, .treatment)
        XCTAssertEqual(resolveSandModelExperimentState(groupName: "A", enabled: false)?.arm, .control)
        XCTAssertNil(resolveSandModelExperimentState(groupName: nil, enabled: true))
    }

    func testStatsigChecksumUserExtractionAndCache() throws {
        let checksum = createCursorChecksum(machineId: "machine", nowMs: 1_700_000_000_000)
        XCTAssertTrue(checksum.hasSuffix("machine"))
        let config = #"{"user":{"userID":"u1","email":"a@example.com"}}"#
        XCTAssertEqual(readStatsigBootstrapUserId(config), "u1")
        XCTAssertEqual(extractStatsigUser(config)["email"] as? String, "a@example.com")
        XCTAssertTrue(sandStatsigNetworkUrlAllowed("https://api3.cursor.sh/rgstr"))
        XCTAssertFalse(sandStatsigNetworkUrlAllowed("https://api3.cursor.sh/other"))

        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let cache = CachedStatsigBootstrap(config: config, userId: "u1", fetchedAtMs: 123)
        saveCachedBootstrap(root.path, cache: cache)
        XCTAssertEqual(loadCachedBootstrap(root.path), cache)
        try? FileManager.default.removeItem(at: root)
    }

    func testDiagnosticsBuffersBeforePinAndGatePropertyOnlyEmitsChanges() {
        pinExperimentsDiagnosticsReporter(nil)
        reportExperimentsDiagnostic(.init(kind: "before"))
        var kinds: [String] = []
        pinExperimentsDiagnosticsReporter { value in
            kinds.append(value.kind)
        }
        XCTAssertTrue(kinds.contains("before"))

        let gate = MutableGateProperty(false)
        var values: [Bool] = []
        let unsubscribe = gate.subscribe { value in values.append(value) }
        gate.set(false)
        gate.set(true)
        gate.set(true)
        XCTAssertEqual(values, [true])
        unsubscribe()
        gate.set(false)
        XCTAssertEqual(values, [true])
        pinExperimentsDiagnosticsReporter(nil)
    }
}
