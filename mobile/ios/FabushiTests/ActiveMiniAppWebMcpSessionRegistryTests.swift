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

        let traces = registry.traces(accountKey: "account-a", pluginId: "global-dharma")
        XCTAssertEqual(traces.count, 1)
        let trace = try XCTUnwrap(traces.first)
        XCTAssertFalse(trace.callId.isEmpty)
        XCTAssertEqual(trace.sequence, 1)
        XCTAssertEqual(trace.version, "1.0.0")
        XCTAssertEqual(trace.artifactSha256, String(repeating: "a", count: 64))
        XCTAssertEqual(trace.pluginInstanceId, "global-dharma:instance-1")
        XCTAssertTrue(trace.nonceIdentifier.hasPrefix("fnv64-"))
        XCTAssertEqual(trace.nonceIdentifier.count, "fnv64-".count + 16)
        XCTAssertFalse(trace.nonceIdentifier.contains("nonce-1"))
        XCTAssertEqual(trace.generation, 7)
        XCTAssertEqual(trace.toolName, "status")
        XCTAssertEqual(trace.permissionDecision, "not-required")
        XCTAssertEqual(trace.stateRevision, 3)
        XCTAssertEqual(trace.outcome, "success")

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

    func testActiveSessionRejectsArgumentsAgainstPageSchemaBeforeExecution() async throws {
        let registry = ActiveMiniAppWebMcpSessionRegistry.shared
        let tool = MiniAppToolContract(
            name: "offline.practice.record",
            description: "mutate local state",
            approval: "required",
            inputSchemaJSON: #"{"additionalProperties":false,"properties":{"label":{"maxLength":5,"type":"string"}},"type":"object"}"#
        )
        var invocationCount = 0

        registry.register(
            accountKey: "account-a",
            pluginId: "global-dharma",
            version: "1.1.0",
            artifactSha256: String(repeating: "d", count: 64),
            pluginInstanceId: "global-dharma:instance-schema",
            nonce: "nonce-schema",
            generation: 12,
            tools: [tool]
        ) { _, arguments in
            invocationCount += 1
            return ["revision": 1, "label": arguments["label"] as? String ?? ""]
        }

        do {
            _ = try await registry.callIfActive(
                accountKey: "account-a",
                pluginId: "global-dharma",
                name: "offline.practice.record",
                arguments: ["label": "too-long"]
            )
            XCTFail("Oversized label must be rejected before page execution")
        } catch let error as MiniAppWebMcpSchemaViolation {
            XCTAssertTrue(error.localizedDescription.contains("长度不能超过 5"))
        }

        do {
            _ = try await registry.callIfActive(
                accountKey: "account-a",
                pluginId: "global-dharma",
                name: "offline.practice.record",
                arguments: ["extra": true]
            )
            XCTFail("additionalProperties=false must reject unreviewed arguments")
        } catch let error as MiniAppWebMcpSchemaViolation {
            XCTAssertTrue(error.localizedDescription.contains("$.extra 不允许"))
        }

        XCTAssertEqual(invocationCount, 0)
        let rejected = registry.traces(accountKey: "account-a", pluginId: "global-dharma")
        XCTAssertEqual(rejected.map(\.outcome), ["schema-rejected", "schema-rejected"])
        XCTAssertEqual(rejected.map(\.permissionDecision), ["not-reached", "not-reached"])

        let accepted = try await registry.callIfActive(
            accountKey: "account-a",
            pluginId: "global-dharma",
            name: "offline.practice.record",
            arguments: ["label": "valid"]
        )
        XCTAssertEqual(invocationCount, 1)
        XCTAssertEqual((accepted?.value as? [String: Any])?["label"] as? String, "valid")
        XCTAssertEqual(
            registry.traces(accountKey: "account-a", pluginId: "global-dharma").last?.outcome,
            "success"
        )
    }

    func testLivePageCannotWeakenCatalogApproval() {
        XCTAssertEqual(
            miniAppEffectiveApproval(catalog: "required", page: "none"),
            "required"
        )
        XCTAssertEqual(
            miniAppEffectiveApproval(catalog: "destructive", page: "none"),
            "destructive"
        )
        XCTAssertEqual(
            miniAppEffectiveApproval(catalog: "none", page: "required"),
            "required"
        )
        XCTAssertEqual(
            miniAppEffectiveApproval(catalog: "none", page: "none"),
            "none"
        )
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

    func testDeniedMutationProducesOrderedFailClosedTrace() async throws {
        let registry = ActiveMiniAppWebMcpSessionRegistry.shared
        let tool = MiniAppToolContract(
            name: "offline.practice.record",
            description: "mutate local state",
            approval: "confirm"
        )

        registry.register(
            accountKey: "account-a",
            pluginId: "global-dharma",
            version: "1.1.0",
            artifactSha256: String(repeating: "c", count: 64),
            pluginInstanceId: "global-dharma:instance-denied",
            nonce: "nonce-denied",
            generation: 11,
            tools: [tool]
        ) { _, _ in
            throw MiniAppWebMcpApprovalDenied()
        }

        do {
            _ = try await registry.callIfActive(
                accountKey: "account-a",
                pluginId: "global-dharma",
                name: "offline.practice.record",
                arguments: ["label": "should-not-run"]
            )
            XCTFail("Denied mutation must not return a result")
        } catch is MiniAppWebMcpApprovalDenied {
            // Expected.
        }

        let traces = registry.traces(accountKey: "account-a", pluginId: "global-dharma")
        XCTAssertEqual(traces.count, 1)
        let trace = try XCTUnwrap(traces.first)
        XCTAssertEqual(trace.sequence, 1)
        XCTAssertEqual(trace.toolName, "offline.practice.record")
        XCTAssertEqual(trace.permissionDecision, "denied")
        XCTAssertNil(trace.stateRevision)
        XCTAssertEqual(trace.outcome, "denied")
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
