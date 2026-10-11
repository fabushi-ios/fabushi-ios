import XCTest
@testable import Fabushi

@MainActor
final class ActiveMiniAppWebMcpSessionRegistryTests: XCTestCase {
    override func tearDown() {
        ActiveMiniAppWebMcpSessionRegistry.shared.resetForTesting()
        super.tearDown()
    }

    func testActiveSessionIsBoundToAccountPackageInstanceAndGeneration() async throws {
        let registry = ActiveMiniAppWebMcpSessionRegistry.shared
        let tool = MiniAppToolContract(
            name: "status",
            description: "read local state",
            approval: "none"
        )

        registry.register(
            accountKey: "account-a",
            pluginId: "global-dharma",
            version: "1.0.0",
            artifactSha256: String(repeating: "a", count: 64),
            pluginInstanceId: "global-dharma:instance-1",
            nonce: "nonce-1",
            generation: 7,
            tools: [tool]
        ) { name, arguments in
            [
                "name": name,
                "revision": 3,
                "marker": arguments["marker"] as? String ?? "",
            ] as [String: Any]
        }

        XCTAssertNil(registry.snapshot(accountKey: "account-b", pluginId: "global-dharma"))
        let snapshot = try XCTUnwrap(
            registry.snapshot(accountKey: "account-a", pluginId: "global-dharma")
        )
        XCTAssertEqual(snapshot.version, "1.0.0")
        XCTAssertEqual(snapshot.artifactSha256, String(repeating: "a", count: 64))
        XCTAssertEqual(snapshot.pluginInstanceId, "global-dharma:instance-1")
        XCTAssertEqual(snapshot.generation, 7)
        XCTAssertEqual(snapshot.toolNames, ["status"])

        let invocation = try await registry.callIfActive(
            accountKey: "account-a",
            pluginId: "global-dharma",
            name: "status",
            arguments: ["marker": "live-page"]
        )
        let result = try XCTUnwrap(invocation?.value as? [String: Any])
        XCTAssertEqual(result["name"] as? String, "status")
        XCTAssertEqual(result["revision"] as? Int, 3)
        XCTAssertEqual(result["marker"] as? String, "live-page")

        XCTAssertNil(try await registry.callIfActive(
            accountKey: "account-b",
            pluginId: "global-dharma",
            name: "status",
            arguments: [:]
        ))
        XCTAssertNil(try await registry.callIfActive(
            accountKey: "account-a",
            pluginId: "global-dharma",
            name: "not-registered",
            arguments: [:]
        ))
    }

    func testStaleUnregisterCannotTearDownNewerSession() async throws {
        let registry = ActiveMiniAppWebMcpSessionRegistry.shared
        let tool = MiniAppToolContract(
            name: "status",
            description: "read local state",
            approval: "none"
        )

        registry.register(
            accountKey: "account-a",
            pluginId: "global-dharma",
            version: "1.0.0",
            artifactSha256: String(repeating: "a", count: 64),
            pluginInstanceId: "old-instance",
            nonce: "old-nonce",
            generation: 1,
            tools: [tool]
        ) { _, _ in "old" }

        registry.register(
            accountKey: "account-a",
            pluginId: "global-dharma",
            version: "1.0.1",
            artifactSha256: String(repeating: "b", count: 64),
            pluginInstanceId: "new-instance",
            nonce: "new-nonce",
            generation: 2,
            tools: [tool]
        ) { _, _ in "new" }

        registry.unregister(
            pluginId: "global-dharma",
            pluginInstanceId: "old-instance",
            generation: 1
        )

        let snapshot = try XCTUnwrap(
            registry.snapshot(accountKey: "account-a", pluginId: "global-dharma")
        )
        XCTAssertEqual(snapshot.pluginInstanceId, "new-instance")
        XCTAssertEqual(snapshot.generation, 2)
        let value = try await registry.callIfActive(
            accountKey: "account-a",
            pluginId: "global-dharma",
            name: "status",
            arguments: [:]
        )
        XCTAssertEqual(value?.value as? String, "new")
    }

    func testBridgeSessionGenerationParticipatesInAuthorization() {
        let session = MiniAppWebMcpBridgeSession(
            pluginInstanceId: "global-dharma:instance",
            nonce: "nonce",
            grants: ["status"],
            generation: 9
        )
        XCTAssertTrue(session.allows(
            pluginInstanceId: "global-dharma:instance",
            nonce: "nonce",
            generation: 9,
            toolName: "status"
        ))
        XCTAssertFalse(session.allows(
            pluginInstanceId: "global-dharma:instance",
            nonce: "nonce",
            generation: 8,
            toolName: "status"
        ))
    }
}
