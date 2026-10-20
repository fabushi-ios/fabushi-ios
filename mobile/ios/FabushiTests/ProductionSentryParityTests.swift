import XCTest
@testable import Fabushi

@MainActor
private final class SentryPrivacyModeGate {
    private var continuations: [CheckedContinuation<SandPrivacyMode?, Never>] = []

    func read() async -> SandPrivacyMode? {
        await withCheckedContinuation { continuation in
            continuations.append(continuation)
        }
    }

    var pendingCount: Int { continuations.count }

    func resumeFirst(_ mode: SandPrivacyMode?) {
        continuations.removeFirst().resume(returning: mode)
    }
}

final class ProductionSentryParityTests: XCTestCase {
    @MainActor
    func testDefaultFatalMetadataFailsClosedAndDoesNotEmitConversationTag() {
        var emitted: [[String: String]] = []
        let controller = IOSProductionSentryController { _, metadata in
            emitted.append(metadata)
        }
        controller.rotate(accountSlot: "account-a")
        XCTAssertTrue(controller.setConversationReport(["agentId": "conversation-safe"]))
        controller.captureFatal(
            eventID: "0123456789abcdef0123456789abcdef",
            origin: "client_failure",
            failureCode: "SAND-E0703"
        )

        XCTAssertEqual(emitted.count, 1)
        XCTAssertEqual(emitted[0]["privacy_tier"], "fatal-metadata")
        XCTAssertEqual(emitted[0]["sentry_tag.sand.failure_code"], "SAND-E0703")
        XCTAssertNil(emitted[0]["sentry_tag.\(SAND_SENTRY_CONVERSATION_TAG)"])
        XCTAssertFalse(emitted[0].values.contains("account-a"))
        XCTAssertEqual(controller.recentEventIDs, ["0123456789abcdef0123456789abcdef"])
    }

    @MainActor
    func testScrubbedTierCanCarryBoundedConversationTag() {
        var emitted: [[String: String]] = []
        let controller = IOSProductionSentryController { _, metadata in
            emitted.append(metadata)
        }
        controller.rotate(accountSlot: "account-a")
        controller.setPrivacyMode(.noTraining)
        XCTAssertTrue(controller.setConversationReport(["agentId": "agent_123"]))
        controller.captureFatal(
            eventID: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            origin: "native_exception"
        )

        XCTAssertEqual(emitted.last?["privacy_tier"], "scrubbed")
        XCTAssertEqual(
            emitted.last?["sentry_tag.\(SAND_SENTRY_CONVERSATION_TAG)"],
            "agent_123"
        )
    }

    @MainActor
    func testAccountMismatchIsRejectedBeforePrivacyProjection() {
        var emitted = 0
        let controller = IOSProductionSentryController { _, _ in emitted += 1 }
        controller.rotate(accountSlot: "account-a")
        controller.setPrivacyMode(.noTraining)

        let envelope = SandSentryEnvelope(
            header: ["event_id": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"],
            items: [
                .init(
                    header: ["type": "event"],
                    payload: [
                        "event_id": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                        "level": "fatal",
                        "platform": "cocoa",
                        "user": ["id": iosLifecycleTelemetryAccountScope("account-b")],
                        "tags": ["event.process": "ios-main"],
                    ]
                )
            ]
        )
        controller.accept(envelope)

        XCTAssertEqual(emitted, 0)
        XCTAssertTrue(controller.recentEventIDs.isEmpty)
    }

    @MainActor
    func testAccountSwitchClearsConversationAndEventIdentity() {
        let controller = IOSProductionSentryController { _, _ in }
        controller.rotate(accountSlot: "account-a")
        controller.setPrivacyMode(.noTraining)
        XCTAssertTrue(controller.setConversationReport(["agentId": "agent_123"]))
        controller.captureFatal(
            eventID: "cccccccccccccccccccccccccccccccc",
            origin: "native_exception"
        )
        XCTAssertFalse(controller.recentEventIDs.isEmpty)

        controller.rotate(accountSlot: "account-b")
        XCTAssertNil(controller.conversationID)
        XCTAssertTrue(controller.recentEventIDs.isEmpty)
        XCTAssertEqual(
            controller.accountScopeHash,
            iosLifecycleTelemetryAccountScope("account-b")
        )
        XCTAssertNotEqual(controller.accountScopeHash, "account-b")
    }

    func testDashboardPrivacyModeProjectionPreservesPolicyTier() {
        XCTAssertEqual(IOSCursorDashboardClient.cursorPrivacyMode(from: NSNumber(value: 2)), .noTraining)
        XCTAssertEqual(
            IOSCursorDashboardClient.cursorPrivacyMode(from: "PRIVACY_MODE_USAGE_DATA_TRAINING_ALLOWED"),
            .usageDataTrainingAllowed
        )
        XCTAssertEqual(IOSCursorDashboardClient.cursorPrivacyMode(from: "NO_STORAGE"), .noStorage)
        XCTAssertNil(IOSCursorDashboardClient.cursorPrivacyMode(from: "unexpected"))
    }

    @MainActor
    func testPrivacySynchronizerRejectsStaleAsyncPolicyAndFailsClosedOnLogout() async {
        let controller = IOSProductionSentryController { _, _ in }
        controller.rotate(accountSlot: "account-a")
        let gate = SentryPrivacyModeGate()
        let synchronizer = IOSProductionSentryPrivacySynchronizer(
            controller: controller,
            readMode: { await gate.read() }
        )

        synchronizer.refresh(loggedIn: true)
        for _ in 0..<10 where gate.pendingCount < 1 { await Task.yield() }
        XCTAssertEqual(gate.pendingCount, 1)

        synchronizer.refresh(loggedIn: true)
        for _ in 0..<10 where gate.pendingCount < 2 { await Task.yield() }
        XCTAssertEqual(gate.pendingCount, 2)
        XCTAssertEqual(controller.privacyTier, .fatalMetadata)

        gate.resumeFirst(.usageDataTrainingAllowed)
        for _ in 0..<4 { await Task.yield() }
        XCTAssertEqual(controller.privacyTier, .fatalMetadata)

        gate.resumeFirst(.noTraining)
        for _ in 0..<10 where controller.privacyTier != .scrubbed { await Task.yield() }
        XCTAssertEqual(controller.privacyTier, .scrubbed)

        synchronizer.refresh(loggedIn: false)
        XCTAssertEqual(controller.privacyTier, .fatalMetadata)
    }

    func testShippingRuntimeRoutesConversationAndReusesLifecycleTransport() throws {
        let root = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let runtime = try String(
            contentsOf: root.appendingPathComponent("source/ios-main/IOSMainRuntime.swift"),
            encoding: .utf8
        )
        let registry = try String(
            contentsOf: root.appendingPathComponent("source/shared/rpc/coordinator-main.swift"),
            encoding: .utf8
        )
        let sentry = try String(
            contentsOf: root.appendingPathComponent("source/ios-main/telemetry/production-sentry.swift"),
            encoding: .utf8
        )
        let dashboard = try String(
            contentsOf: root.appendingPathComponent("source/ios-main/account/cursor-dashboard-client.swift"),
            encoding: .utf8
        )
        let mcpSurface = try String(
            contentsOf: root.appendingPathComponent("source/mahayana-agent-coordinator/coordinator-mcp-manager.swift"),
            encoding: .utf8
        )

        XCTAssertTrue(registry.contains("\"reportSentryConversation\""))
        XCTAssertTrue(runtime.contains("if method == \"reportSentryConversation\""))
        XCTAssertTrue(runtime.contains("sentryController.setConversationReport(args.foundationValue)"))
        XCTAssertTrue(runtime.contains("let accountSentryController = sentryController"))
        XCTAssertTrue(runtime.contains("accountSentryController.rotate(accountSlot: slot)"))
        XCTAssertTrue(runtime.contains("sentryController.rotate(accountSlot: nil)"))
        XCTAssertTrue(runtime.contains("reporter.report(.processRecovery"))
        XCTAssertTrue(runtime.contains("lifecycleTelemetrySink.submit(record)"))
        XCTAssertTrue(sentry.contains("SandSentryPrivacyGate()"))
        XCTAssertTrue(sentry.contains("SandSentryEventIDRing(capacity: 64)"))
        XCTAssertTrue(sentry.contains("SandSentrySpoolStore()"))
        XCTAssertTrue(sentry.contains("IOSProductionSentryPrivacySynchronizer"))
        XCTAssertTrue(runtime.contains("coordinator.setSentryAuthObserver"))
        XCTAssertTrue(runtime.contains("sentryPrivacySynchronizer?.refresh(loggedIn: status.loggedIn)"))
        XCTAssertTrue(dashboard.contains("func getCursorPrivacyMode(timeoutMs: Int = 10_000) async -> SandPrivacyMode?"))
        XCTAssertTrue(mcpSurface.contains("func currentSentryPrivacyMode() async -> SandPrivacyMode?"))
        XCTAssertFalse(sentry.contains("SentrySDK"))
        XCTAssertFalse(sentry.contains("dsn"))
    }
}
