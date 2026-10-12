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

    func testPendingResultIsRejectedWhenPackageVersionOrDigestChanges() async throws {
        let registry = ActiveMiniAppWebMcpSessionRegistry.shared
        let tool = MiniAppToolContract(
            name: "offline.state.read",
            description: "read local state",
            approval: "none"
        )

        registry.register(
            accountKey: "account-a",
            pluginId: "global-dharma",
            version: "1.1.0",
            artifactSha256: String(repeating: "a", count: 64),
            pluginInstanceId: "stable-instance",
            nonce: "stable-nonce",
            generation: 9,
            tools: [tool]
        ) { _, _ in
            registry.register(
                accountKey: "account-a",
                pluginId: "global-dharma",
                version: "1.1.1",
                artifactSha256: String(repeating: "b", count: 64),
                pluginInstanceId: "stable-instance",
                nonce: "stable-nonce",
                generation: 9,
                tools: [tool]
            ) { _, _ in ["revision": 8] }
            return ["revision": 7]
        }

        do {
            _ = try await registry.callIfActive(
                accountKey: "account-a",
                pluginId: "global-dharma",
                name: "offline.state.read",
                arguments: [:]
            )
            XCTFail("A pending result from an older package version/digest must be stale")
        } catch is CancellationError {
            // Expected: package identity changed before the result committed.
        }

        let trace = try XCTUnwrap(
            registry.traces(accountKey: "account-a", pluginId: "global-dharma").last
        )
        XCTAssertEqual(trace.version, "1.1.0")
        XCTAssertEqual(trace.artifactSha256, String(repeating: "a", count: 64))
        XCTAssertEqual(trace.stateRevision, 7)
        XCTAssertEqual(trace.outcome, "stale-session")

        let current = try XCTUnwrap(
            registry.snapshot(accountKey: "account-a", pluginId: "global-dharma")
        )
        XCTAssertEqual(current.version, "1.1.1")
        XCTAssertEqual(current.artifactSha256, String(repeating: "b", count: 64))
    }

    func testAccountRevocationRejectsPendingResultAndPreservesOtherAccounts() async throws {
        let registry = ActiveMiniAppWebMcpSessionRegistry.shared
        let tool = MiniAppToolContract(
            name: "offline.state.read",
            description: "read local state",
            approval: "none"
        )
        var pendingContinuation: CheckedContinuation<String, Never>?

        registry.register(
            accountKey: "account-a",
            pluginId: "global-dharma",
            version: "1.2.0",
            artifactSha256: String(repeating: "e", count: 64),
            pluginInstanceId: "account-a-instance",
            nonce: "account-a-nonce",
            generation: 4,
            tools: [tool]
        ) { _, _ in
            await withCheckedContinuation { continuation in
                pendingContinuation = continuation
            }
        }
        registry.register(
            accountKey: "account-b",
            pluginId: "other-app",
            version: "2.0.0",
            artifactSha256: String(repeating: "f", count: 64),
            pluginInstanceId: "account-b-instance",
            nonce: "account-b-nonce",
            generation: 1,
            tools: [tool]
        ) { _, _ in "account-b" }

        let pending = Task {
            try await registry.callIfActive(
                accountKey: "account-a",
                pluginId: "global-dharma",
                name: "offline.state.read",
                arguments: [:]
            )
        }
        for _ in 0..<100 where pendingContinuation == nil {
            await Task.yield()
        }
        let continuation = try XCTUnwrap(pendingContinuation)

        registry.revokeSessions(accountKey: "account-a")
        XCTAssertNil(registry.snapshot(accountKey: "account-a", pluginId: "global-dharma"))
        XCTAssertNotNil(registry.snapshot(accountKey: "account-b", pluginId: "other-app"))

        continuation.resume(returning: "stale")
        do {
            _ = try await pending.value
            XCTFail("Revoked-account result must not commit")
        } catch is CancellationError {
            // Expected: account revocation invalidates the in-flight page result.
        }

        XCTAssertEqual(
            registry.traces(accountKey: "account-a", pluginId: "global-dharma").last?.outcome,
            "stale-session"
        )
    }

    func testMiniAppSourceGenerationKeyIsAccountScoped() {
        let preview = String(repeating: "a", count: 64)
        let installedA = miniAppSourceGenerationKey(
            accountKey: "account-a",
            pluginId: "global-dharma",
            previewDigest: nil
        )
        let installedB = miniAppSourceGenerationKey(
            accountKey: "account-b",
            pluginId: "global-dharma",
            previewDigest: nil
        )
        let previewA = miniAppSourceGenerationKey(
            accountKey: "account-a",
            pluginId: "global-dharma",
            previewDigest: preview
        )

        XCTAssertNotEqual(installedA, installedB)
        XCTAssertNotEqual(installedA, previewA)
        XCTAssertFalse(installedA.contains("account-a"))
        XCTAssertFalse(installedB.contains("account-b"))
    }

    func testReRegistrationImmediatelyDisablesRemovedTool() async throws {
        let registry = ActiveMiniAppWebMcpSessionRegistry.shared
        let status = MiniAppToolContract(
            name: "offline.state.read",
            description: "read local state",
            approval: "none"
        )
        let mutation = MiniAppToolContract(
            name: "offline.practice.record",
            description: "mutate local state",
            approval: "required"
        )

        registry.register(
            accountKey: "account-a",
            pluginId: "global-dharma",
            version: "1.1.0",
            artifactSha256: String(repeating: "c", count: 64),
            pluginInstanceId: "instance-1",
            nonce: "nonce-1",
            generation: 4,
            tools: [status, mutation]
        ) { name, _ in ["tool": name, "revision": 1] }

        registry.register(
            accountKey: "account-a",
            pluginId: "global-dharma",
            version: "1.1.0",
            artifactSha256: String(repeating: "c", count: 64),
            pluginInstanceId: "instance-1",
            nonce: "nonce-1",
            generation: 4,
            tools: [status]
        ) { name, _ in ["tool": name, "revision": 2] }

        XCTAssertNil(try await registry.callIfActive(
            accountKey: "account-a",
            pluginId: "global-dharma",
            name: "offline.practice.record",
            arguments: [:]
        ))
        let read = try await registry.callIfActive(
            accountKey: "account-a",
            pluginId: "global-dharma",
            name: "offline.state.read",
            arguments: [:]
        )
        XCTAssertEqual((read?.value as? [String: Any])?["revision"] as? Int, 2)
    }

}
