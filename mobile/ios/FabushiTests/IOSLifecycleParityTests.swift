import XCTest
@testable import Fabushi

final class IOSLifecycleParityTests: XCTestCase {
    func testUnitTestHostPolicyIsNarrowAndFailClosed() {
        XCTAssertTrue(IOSUnitTestHostPolicy.shouldBypassProductRuntime(
            environment: ["FABUSHI_UNIT_TEST_HOST": "1"]
        ))
        XCTAssertTrue(IOSUnitTestHostPolicy.shouldBypassProductRuntime(
            environment: [
                "XCTestBundlePath": "/tmp/FabushiTests.xctest",
                "XCInjectBundleInto": "/tmp/Fabushi.app/Fabushi",
            ]
        ))
        XCTAssertFalse(IOSUnitTestHostPolicy.shouldBypassProductRuntime(
            environment: ["FABUSHI_UNIT_TEST_HOST": "true"]
        ))
        XCTAssertFalse(IOSUnitTestHostPolicy.shouldBypassProductRuntime(
            environment: [
                "XCTestBundlePath": "/tmp/FabushiUITests.xctest",
                "XCInjectBundleInto": "/tmp/FabushiUITests-Runner.app/FabushiUITests-Runner",
            ]
        ))
        XCTAssertFalse(IOSUnitTestHostPolicy.shouldBypassProductRuntime(
            environment: ["XCTestBundlePath": "/tmp/FabushiTests.xctest"]
        ))
        XCTAssertFalse(IOSUnitTestHostPolicy.shouldBypassProductRuntime(environment: [:]))
    }

    func testAuthCallbackRegistrationRequiresTheShippingScheme() throws {
        let valid: [String: Any] = [
            "CFBundleURLTypes": [
                [
                    "CFBundleURLName": "com.ombhrum.fabushi",
                    "CFBundleURLSchemes": ["FABUSHI"],
                ],
            ],
        ]

        let registration = try IOSAuthCallbackRegistrar.requireShippingRegistration(
            infoDictionary: valid
        )
        XCTAssertEqual(registration.redirectTarget, FabushiDeepLinkParser.customScheme)
        XCTAssertEqual(registration.protocolScheme, FabushiDeepLinkParser.customScheme)
        XCTAssertTrue(registration.registered)

        XCTAssertThrowsError(
            try IOSAuthCallbackRegistrar.requireShippingRegistration(
                infoDictionary: [
                    "CFBundleURLTypes": [
                        ["CFBundleURLSchemes": ["other-app"]],
                    ],
                ]
            )
        ) { error in
            XCTAssertEqual(
                error as? IOSAuthCallbackRegistrationError,
                .missingURLScheme(FabushiDeepLinkParser.customScheme)
            )
        }
    }

    func testDeepLinkParserCanonicalizesAuthAndRejectsUnsafeInputs() throws {
        let parsed = try XCTUnwrap(FabushiDeepLinkParser.parse(
            "fabushi://auth/complete?attemptId=abcdefgh&status=completed"
        ))
        XCTAssertEqual(parsed.route, .authComplete(attemptId: "abcdefgh", status: "completed"))
        XCTAssertEqual(
            parsed.canonicalURL.absoluteString,
            "fabushi://auth/complete?attemptId=abcdefgh&status=completed"
        )
        XCTAssertNil(FabushiDeepLinkParser.parse(
            "fabushi://user:password@auth/complete?attemptId=abcdefgh"
        ))
        XCTAssertNil(FabushiDeepLinkParser.parse(
            "fabushi://auth/../complete?attemptId=abcdefgh"
        ))
        XCTAssertNil(FabushiDeepLinkParser.parse(
            "fabushi://auth/complete?attemptId=%ZZ"
        ))
    }

    func testReferenceDeepLinksShareTheSameCanonicalRouter() throws {
        let custom = try XCTUnwrap(FabushiDeepLinkParser.parse(
            "fabushi://app/v1/info?topic=deep-links"
        ))
        XCTAssertEqual(custom.route, .info(topic: "deep-links"))
        XCTAssertEqual(custom.source, .customScheme)

        let universal = try XCTUnwrap(FabushiDeepLinkParser.parse(
            "https://fabushi.app/link/v1/open"
        ))
        XCTAssertEqual(universal.route, .open)
        XCTAssertEqual(universal.source, .universalLink)
        XCTAssertEqual(universal.canonicalURL.absoluteString, "fabushi://app/v1/open")
    }

    @MainActor
    func testDeepLinkControllerQueuesBeforeRendererReadyAndDedupes() {
        var now = Date(timeIntervalSince1970: 100)
        var dispatched: [ParsedFabushiDeepLink] = []
        let controller = IOSDeepLinkController(
            dispatch: { dispatched.append($0) },
            now: { now }
        )

        XCTAssertTrue(controller.handleCandidate("fabushi://app/v1/open", origin: "test"))
        XCTAssertTrue(controller.hasPendingActivation)
        XCTAssertFalse(controller.handleCandidate("fabushi://app/v1/open", origin: "duplicate"))
        XCTAssertTrue(dispatched.isEmpty)

        controller.markReady()
        XCTAssertEqual(dispatched.map(\.route), [.open])

        now = now.addingTimeInterval(IOSDeepLinkController.dedupeWindow + 0.1)
        XCTAssertTrue(controller.handleCandidate("fabushi://app/v1/open", origin: "after-window"))
        XCTAssertEqual(dispatched.count, 2)
    }

    @MainActor
    func testLifecycleCheckpointRequiresResyncAfterBackgroundOrUncleanTermination() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        defer { try? FileManager.default.removeItem(at: directory) }

        let first = try IOSLifecycleRecoveryStore(appDataDirectory: directory)
        XCTAssertFalse(first.requiresColdStartResync)
        first.transition(to: .background)

        let second = try IOSLifecycleRecoveryStore(appDataDirectory: directory)
        XCTAssertTrue(second.requiresColdStartResync)
        second.markResyncCompleted()
        XCTAssertFalse(second.requiresColdStartResync)
    }

    @MainActor
    func testCleanShutdownDoesNotForceColdStartResync() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        defer { try? FileManager.default.removeItem(at: directory) }

        let first = try IOSLifecycleRecoveryStore(appDataDirectory: directory)
        first.transition(to: .shuttingDown)
        let second = try IOSLifecycleRecoveryStore(appDataDirectory: directory)
        XCTAssertFalse(second.requiresColdStartResync)
    }


    @MainActor
    func testExplicitRecoveryRequirementPersistsAcrossRelaunch() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        defer { try? FileManager.default.removeItem(at: directory) }

        let first = try IOSLifecycleRecoveryStore(appDataDirectory: directory)
        first.markResyncRequired()
        let second = try IOSLifecycleRecoveryStore(appDataDirectory: directory)
        XCTAssertTrue(second.requiresColdStartResync)
    }

    @MainActor
    func testLifecycleReporterBuffersAndFlushesInOrder() {
        let reporter = IOSLifecycleReporter()
        reporter.report(.startup, metadata: ["phase": "one"])
        reporter.report(.rendererLifecycle, level: .warn, metadata: ["phase": "two"])
        XCTAssertEqual(reporter.bufferedCount, 2)

        var records: [IOSLifecycleTelemetryRecord] = []
        reporter.attach { records.append($0) }
        XCTAssertEqual(records.map(\.family), [.startup, .rendererLifecycle])
        XCTAssertEqual(records.map(\.level), [.info, .warn])
        XCTAssertEqual(reporter.bufferedCount, 0)
    }

    func testAuthSessionTelemetryMatchesDesktopErrorTaxonomyAndBuckets() {
        let http = iosCursorSessionTelemetry(.refreshFailed(.httpStatus(503)))
        XCTAssertEqual(http.stream, .session)
        XCTAssertEqual(http.level, .warn)
        XCTAssertEqual(http.metadata["error_code"], "SAND-E0214")
        XCTAssertEqual(http.metadata["http_status"], "503")
        XCTAssertEqual(http.metadata["error_retryable"], "true")

        let network = iosCursorSessionTelemetry(.refreshFailed(.network("-1009")))
        XCTAssertEqual(network.metadata["error_code"], "SAND-E0215")
        XCTAssertEqual(network.metadata["errno"], "-1009")

        let bad = iosCursorSessionTelemetry(.refreshFailed(.badPayload))
        XCTAssertEqual(bad.metadata["error_code"], "SAND-E0216")

        let recovered = iosCursorSessionTelemetry(.refreshRecovered(
            consecutiveFailures: 20_000,
            degradedMs: 30_000
        ))
        XCTAssertEqual(recovered.metadata["consecutive_failures"], "10000")
        XCTAssertEqual(recovered.metadata["degraded_ms"], "60000")

        let revoked = iosCursorSessionTelemetry(.signedOut(
            cause: .sessionRevoked,
            durable: true
        ))
        XCTAssertEqual(revoked.metadata["error_code"], "SAND-E0217")
        XCTAssertEqual(revoked.metadata["error_retryable"], "false")

        let policy = iosCursorSessionTelemetry(.signedOut(cause: .policy, durable: true))
        XCTAssertEqual(policy.metadata["error_code"], "SAND-E0218")

        let keychain = iosCursorSessionTelemetry(.keychainUnavailable)
        XCTAssertEqual(keychain.metadata["error_code"], "SAND-E0219")
    }

    @MainActor
    func testAuthTelemetryRelayBuffersAtMostSixteenAndFlushesInOrder() {
        let relay = IOSAuthTelemetryRelay()
        for index in 0..<20 {
            relay.report(iosCursorSigninTelemetry(.gate("gate-\(index)")))
        }
        XCTAssertEqual(relay.pendingCount, 16)

        var records: [IOSAuthTelemetryProjection] = []
        relay.attach { records.append($0) }
        XCTAssertEqual(records.count, 16)
        XCTAssertEqual(records.first?.metadata["gate"], "gate-0")
        XCTAssertEqual(records.last?.metadata["gate"], "gate-15")
        XCTAssertEqual(relay.pendingCount, 0)
    }

    func testSigninTelemetryUsesBoundedCanonicalPhasesAndCauses() {
        XCTAssertEqual(
            iosCursorSigninTelemetry(.loginStarted).metadata,
            ["phase": "login_started"]
        )
        XCTAssertEqual(
            iosCursorSigninTelemetry(.loginCompleted).metadata,
            ["phase": "login_completed"]
        )
        XCTAssertEqual(
            iosCursorSigninTelemetry(.loginFailed(cause: "policy_refused")).metadata,
            ["phase": "login_failed", "cause": "policy_refused"]
        )
        XCTAssertEqual(
            iosCursorSigninTelemetry(.signedOut(cause: "session_expired")).metadata,
            ["phase": "signed_out", "cause": "session_expired"]
        )
        XCTAssertEqual(
            iosCursorSigninTelemetry(.gate("onboarding")).metadata,
            ["phase": "boot_gate", "gate": "onboarding"]
        )
        XCTAssertEqual(
            iosCursorSigninTelemetry(.consult("fresh")).metadata,
            ["phase": "account_consult", "outcome": "fresh"]
        )
    }

}
