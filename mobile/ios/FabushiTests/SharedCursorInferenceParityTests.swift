import XCTest
@testable import Fabushi

private actor PrivacyFetchCounter {
    private(set) var count = 0
    func fetch(_ options: PrivacyLookupOptions) -> SandPrivacyMode? {
        count += 1
        return .usageDataTrainingAllowed
    }
    func value() -> Int { count }
}

private actor TokenSequence {
    private var values: [String]
    init(_ values: [String]) { self.values = values }
    func next() throws -> String {
        guard !values.isEmpty else { throw URLError(.userAuthenticationRequired) }
        return values.removeFirst()
    }
}

final class SharedCursorInferenceParityTests: XCTestCase {
    func testRequestedModelMappingsPreserveGrokShapes() {
        XCTAssertEqual(createSandDefaultRequestedModel().modelId, SAND_DEFAULT_MODEL_ID)
        XCTAssertTrue(createSandDefaultRequestedModel().maxMode)
        XCTAssertEqual(createSandSubagentRequestedModel("sub").parameters, [])
        XCTAssertEqual(createSandComputerUseRequestedModel("computer").modelId, "computer")
        XCTAssertEqual(
            createSandComputerUseRequestedModel("computer").parameters,
            SAND_COMPUTER_USE_MODEL_SELECTION.parameters
        )
    }

    func testGhostModeIsPrivacySafeByDefault() {
        XCTAssertEqual(getSandGhostModeHeaderFromPrivacyMode(nil), "true")
        XCTAssertEqual(getSandGhostModeHeaderFromPrivacyMode(.noStorage), "true")
        XCTAssertEqual(getSandGhostModeHeaderFromPrivacyMode(.noTraining), "true")
        XCTAssertEqual(getSandGhostModeHeaderFromPrivacyMode(.usageDataTrainingAllowed), "false")
        XCTAssertEqual(getSandGhostModeHeaderFromPrivacyMode(.usageCodebaseTrainingAllowed), "false")
    }

    func testPrivacyCacheIsScopedToAccountAndCoalescesSameScope() async {
        clearSandPrivacyModeCacheForTesting()
        defer { clearSandPrivacyModeCacheForTesting() }
        let counter = PrivacyFetchCounter()
        let first = PrivacyLookupOptions(
            backendUrl: "https://api.example.test",
            accessToken: "account-a",
            machineId: "machine"
        )

        let one = await resolveCachedSandPrivacyMode(
            options: first,
            fetchPrivacyMode: { options in await counter.fetch(options) },
            nowMs: { 1_000 }
        )
        let two = await resolveCachedSandPrivacyMode(
            options: first,
            fetchPrivacyMode: { options in await counter.fetch(options) },
            nowMs: { 1_001 }
        )
        XCTAssertEqual(one, .usageDataTrainingAllowed)
        XCTAssertEqual(two, .usageDataTrainingAllowed)
        let sameAccountFetches = await counter.value()
        XCTAssertEqual(sameAccountFetches, 1)

        _ = await resolveCachedSandPrivacyMode(
            options: .init(
                backendUrl: first.backendUrl,
                accessToken: "account-b",
                machineId: first.machineId
            ),
            fetchPrivacyMode: { options in await counter.fetch(options) },
            nowMs: { 1_002 }
        )
        let crossAccountFetches = await counter.value()
        XCTAssertEqual(crossAccountFetches, 2)
    }

    func testRunPrivacyFallsBackWhenAccountChangesMidLookup() async {
        clearSandPrivacyModeCacheForTesting()
        defer { clearSandPrivacyModeCacheForTesting() }
        let tokens = TokenSequence(["account-a", "account-b"])

        let resolved = await resolveSandRunPrivacyMode(
            backendUrl: "https://api.example.test",
            getAccessToken: { _ in try await tokens.next() },
            getMachineId: { "machine" },
            fetchPrivacyMode: { _ in .usageDataTrainingAllowed }
        )

        XCTAssertEqual(resolved, SAND_RUN_PRIVACY_MODE_FALLBACK)
    }

    func testInferenceHeadersPinRequestIdAndSanitizeLineage() async throws {
        let result = try await createSandInferenceHeaders(
            backendUrl: "https://api.example.test",
            existingRequestId: "request-1",
            lineage: .init(
                parentRequestId: "parent\r\nspoof",
                rootParentRequestId: "root\nspoof",
                parentAgentToolCallId: "tool\r1"
            ),
            getAccessToken: { _ in "secret-token" },
            getMachineId: { "machine-1" },
            resolveGhostMode: { _ in "false" },
            randomUUID: { "unused" },
            env: [
                "SAND_CLIENT_APP_VERSION": "1.2.3",
                "SAND_PACKAGED": "1",
            ]
        )

        XCTAssertEqual(result.requestId, "request-1")
        XCTAssertEqual(result.headers["authorization"], "Bearer secret-token")
        XCTAssertEqual(result.headers["x-ghost-mode"], "false")
        XCTAssertEqual(result.headers["x-request-id"], "request-1")
        XCTAssertEqual(result.headers["x-parent-request-id"], "parentspoof")
        XCTAssertEqual(result.headers["x-root-parent-request-id"], "rootspoof")
        XCTAssertEqual(result.headers["x-parent-agent-tool-call-id"], "tool1")
        XCTAssertEqual(result.headers["x-cursor-client-version"], "1.2.3")
        XCTAssertEqual(result.headers[SAND_BOX_NAMESPACE_HEADER], "prod")
        XCTAssertTrue(result.headers["x-cursor-checksum"]?.hasSuffix("machine-1") == true)
    }

    func testAnonymousInferenceNeverProjectsAuthorization() async throws {
        let result = try await createSandInferenceHeaders(
            backendUrl: "https://api.example.test",
            authMode: .anonymous,
            getAccessToken: { _ in XCTFail("anonymous mode must not fetch auth"); return "unexpected" },
            getMachineId: { "machine" },
            resolveGhostMode: { _ in XCTFail("anonymous mode must not resolve private mode"); return "false" },
            randomUUID: { "generated-id" }
        )
        XCTAssertNil(result.headers["authorization"])
        XCTAssertEqual(result.headers["x-ghost-mode"], "true")
        XCTAssertEqual(result.requestId, "generated-id")
    }

    func testSettingsSelectCursorOrProviderRouteAndTrackUsage() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let store = SandSettingsStore(settingsPath: root.appendingPathComponent("settings.json").path)

        XCTAssertEqual(resolveSandPromptInferenceRoute(settings: store), .provider(.fabushi))
        XCTAssertNotNil(store.getInferenceRouterUsage().providers[.fabushi])
        store.setInferenceProvider(.openrouter)
        XCTAssertEqual(resolveSandPromptInferenceRoute(settings: store), .provider(.openrouter))

        store.recordInferenceUsage(
            provider: .openrouter,
            inputTokens: 10,
            outputTokens: 5,
            cacheReadTokens: -1
        )
        let usage = store.getInferenceRouterUsage().providers[.openrouter]
        XCTAssertEqual(usage?.requests, 1)
        XCTAssertEqual(usage?.inputTokens, 10)
        XCTAssertEqual(usage?.outputTokens, 5)
        XCTAssertEqual(usage?.cacheReadTokens, 0)
        XCTAssertNotNil(usage?.lastUsedAt)
    }
}
