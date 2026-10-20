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

    func testShippingLifecycleTelemetryComposesAccountScopedSubmitLogsBackend() throws {
        let iosRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let source = try String(
            contentsOf: iosRoot
                .deletingLastPathComponent()
                .deletingLastPathComponent()
                .appendingPathComponent("source/ios-main/IOSMainRuntime.swift"),
            encoding: .utf8
        )
        XCTAssertTrue(source.contains("lifecycleTelemetrySink.configureRemoteShipping"))
        XCTAssertTrue(source.contains("coordinator.structuredLogCredentials("))
        XCTAssertTrue(source.contains("IOSCursorStructuredLogBackend("))
        XCTAssertTrue(source.contains(".submit("))
        XCTAssertTrue(source.contains("accessToken: credentials.accessToken"))
        XCTAssertTrue(source.contains("machineID: credentials.machineID"))
        XCTAssertTrue(source.contains("lifecycleTelemetrySink.rotate(accountSlot: slot)"))
        XCTAssertTrue(source.contains("lifecycleTelemetrySink.rotate(accountSlot: nil)"))
    }


    func testShippingShutdownFlushesRemoteTelemetryBeforeAccountReset() throws {
        let iosRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let source = try String(
            contentsOf: iosRoot
                .deletingLastPathComponent()
                .deletingLastPathComponent()
                .appendingPathComponent("source/ios-main/IOSMainRuntime.swift"),
            encoding: .utf8
        )
        let shutdownStart = try XCTUnwrap(source.range(of: "func shutdown() {"))
        let shutdownSource = String(source[shutdownStart.lowerBound...])
        let flush = try XCTUnwrap(shutdownSource.range(of: "lifecycleTelemetrySink.flushRemoteNow()"))
        let settle = try XCTUnwrap(shutdownSource.range(of: "lifecycleTelemetrySink.settleLoggedOutIfNeeded()"))
        let reset = try XCTUnwrap(shutdownSource.range(of: "accountRuntime.reset()"))
        XCTAssertLessThan(flush.lowerBound, settle.lowerBound)
        XCTAssertLessThan(settle.lowerBound, reset.lowerBound)
    }




    func testMainThreadPressureWindowMatchesDesktopThresholdsAndMetadataCaps() {
        var elevated = IOSMainThreadPressureAccumulator()
        for _ in 0..<19 {
            XCTAssertNil(elevated.record(
                delayMilliseconds: 100,
                elapsedMilliseconds: 3_000
            ))
        }
        let elevatedResult = elevated.record(
            delayMilliseconds: 100,
            elapsedMilliseconds: 3_000
        )
        XCTAssertEqual(elevatedResult?.severity, .elevated)
        XCTAssertEqual(elevatedResult?.summary.p95Ms, 100)

        var severe = IOSMainThreadPressureAccumulator()
        for _ in 0..<19 {
            _ = severe.record(
                delayMilliseconds: 1_000,
                elapsedMilliseconds: 3_000
            )
        }
        let severeResult = severe.record(
            delayMilliseconds: 90_000,
            elapsedMilliseconds: 3_000
        )
        XCTAssertEqual(severeResult?.severity, .severe)
        let metadata = iosMainThreadPressureMetadata(
            summary: severeResult!.summary,
            severity: severeResult!.severity
        )
        XCTAssertEqual(metadata["trigger"], "pressure")
        XCTAssertEqual(metadata["severity"], "severe")
        XCTAssertEqual(metadata["p95_ms"], "1000")
        XCTAssertEqual(metadata["max_ms"], "60000")
        XCTAssertEqual(metadata["window_ms"], "60000")
        XCTAssertEqual(metadata["platform_replacement"], "ios_main_run_loop")
    }

    func testShippingRuntimeScopesMainThreadPressureMonitorToActiveScene() throws {
        let iosRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let source = try String(
            contentsOf: iosRoot
                .deletingLastPathComponent()
                .deletingLastPathComponent()
                .appendingPathComponent("source/ios-main/IOSMainRuntime.swift"),
            encoding: .utf8
        )
        XCTAssertTrue(source.contains("mainThreadPressureMonitor.start()"))
        XCTAssertGreaterThanOrEqual(
            source.components(separatedBy: "mainThreadPressureMonitor.stop()").count - 1,
            4
        )
    }


    func testAgentStoreReconcileCadenceMatchesDesktopAndFailureBackoff() {
        XCTAssertEqual(IOS_AGENT_STORE_RECONCILE_INTERVAL_NS, 120_000_000_000)
        XCTAssertEqual(IOS_AGENT_STORE_ERROR_BACKOFF_NS, 30_000_000_000)
    }

    func testAgentStoreReconcileResultFenceRejectsLogoutCancellationAndStaleGeneration() {
        XCTAssertTrue(iosAgentStoreReconcileResultIsCurrent(
            expectedGeneration: 7,
            currentGeneration: 7,
            loggedIn: true,
            taskCancelled: false
        ))
        XCTAssertFalse(iosAgentStoreReconcileResultIsCurrent(
            expectedGeneration: 7,
            currentGeneration: 8,
            loggedIn: true,
            taskCancelled: false
        ))
        XCTAssertFalse(iosAgentStoreReconcileResultIsCurrent(
            expectedGeneration: 7,
            currentGeneration: 7,
            loggedIn: false,
            taskCancelled: false
        ))
        XCTAssertFalse(iosAgentStoreReconcileResultIsCurrent(
            expectedGeneration: 7,
            currentGeneration: 7,
            loggedIn: true,
            taskCancelled: true
        ))
    }

    func testShippingRuntimeScopesAgentStorePeriodicReconcileToActiveAccountLifecycle() throws {
        let iosRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let source = try String(
            contentsOf: iosRoot
                .deletingLastPathComponent()
                .deletingLastPathComponent()
                .appendingPathComponent("source/ios-main/FabushiRuntime.swift"),
            encoding: .utf8
        )

        XCTAssertTrue(source.contains("startAgentStorePeriodicReconcileIfNeeded()"))
        XCTAssertTrue(source.contains("stopAgentStorePeriodicReconcile()"))
        XCTAssertTrue(source.contains("reconcileAgentStore(reason: \"periodic\")"))
        XCTAssertTrue(source.contains("delay = IOS_AGENT_STORE_ERROR_BACKOFF_NS"))
        XCTAssertTrue(source.contains("delay = IOS_AGENT_STORE_RECONCILE_INTERVAL_NS"))
        XCTAssertTrue(source.contains("case .background:"))
        XCTAssertTrue(source.contains("scheduleAgentStoreReconcile(reason: \"background\")"))
        XCTAssertTrue(source.contains("agentStorePeriodicTask?.cancel()"))
        XCTAssertTrue(source.contains("marketplace.loggedIn"))
        XCTAssertTrue(source.contains("!wasBackgrounded"))
    }

    func testShippingRuntimeCoalescesAgentStoreReconcileAndFencesLogout() throws {
        let iosRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let source = try String(
            contentsOf: iosRoot
                .deletingLastPathComponent()
                .deletingLastPathComponent()
                .appendingPathComponent("source/ios-main/FabushiRuntime.swift"),
            encoding: .utf8
        )

        XCTAssertTrue(source.contains("agentStoreReconcileInFlight: Task<Bool, Never>?"))
        XCTAssertTrue(source.contains("agentStoreReconcileGeneration: UInt64 = 0"))
        XCTAssertTrue(source.contains("if let inFlight = agentStoreReconcileInFlight"))
        XCTAssertTrue(source.contains("let completed = await inFlight.value"))
        XCTAssertTrue(source.contains("iosAgentStoreReconcileResultIsCurrent("))
        XCTAssertTrue(source.contains("expectedGeneration: generation"))
        XCTAssertTrue(source.contains("currentGeneration: agentStoreReconcileGeneration"))
        XCTAssertTrue(source.contains("coalesced-completed"))
        XCTAssertTrue(source.contains("coalesced-failed"))
        XCTAssertTrue(source.contains("if agentStoreReconcileGeneration == generation"))
        XCTAssertTrue(source.contains("agentStoreReconcileInFlight?.cancel()"))
        XCTAssertTrue(source.contains("agentStoreReconcileInFlight = nil"))
        XCTAssertEqual(
            source.components(separatedBy: "reconcileAgentStoreForCurrentScope()").count - 1,
            1,
            "single-flight owner must keep exactly one production Host reconcile dispatch"
        )
    }

    func testProcessCrashTelemetryLimiterMatchesDesktopCapAndErrorTaxonomy() {
        var limiter = IOSProcessCrashTelemetryLimiter()
        var records: [[String: String]] = []
        for _ in 0..<25 {
            if let metadata = limiter.nextMetadata() {
                records.append(metadata)
            }
        }
        XCTAssertEqual(records.count, 20)
        XCTAssertEqual(records.first?["crash_seq"], "1")
        XCTAssertEqual(records.last?["crash_seq"], "20")
        XCTAssertEqual(records.last?["capped"], "true")
        XCTAssertEqual(records.first?["error_code"], "SAND-E0603")
        XCTAssertEqual(records.first?["error_domain"], "desktop")
        XCTAssertEqual(records.first?["error_retryable"], "false")
        XCTAssertEqual(records.first?["platform_replacement"], "metric_kit")
        XCTAssertNil(limiter.nextMetadata())
    }

    func testShippingRuntimeRegistersAndStopsMetricKitCrashMonitor() throws {
        let iosRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let source = try String(
            contentsOf: iosRoot
                .deletingLastPathComponent()
                .deletingLastPathComponent()
                .appendingPathComponent("source/ios-main/IOSMainRuntime.swift"),
            encoding: .utf8
        )
        XCTAssertTrue(source.contains("processCrashDiagnosticMonitor.start()"))
        XCTAssertTrue(source.contains("processCrashDiagnosticMonitor.stop()"))
    }

    func testShippingCloudAgentWakeWatcherHasExplicitAccountScopedStopOwner() throws {
        let iosRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let source = try String(
            contentsOf: iosRoot
                .deletingLastPathComponent()
                .deletingLastPathComponent()
                .appendingPathComponent("source/ios-main/FabushiRuntime.swift"),
            encoding: .utf8
        )

        XCTAssertTrue(source.contains("func stop() {"))
        XCTAssertTrue(source.contains("scanTask?.cancel()"))
        XCTAssertTrue(source.contains("scanTask = nil"))
        XCTAssertTrue(source.contains("for task in watches.values"))
        XCTAssertTrue(source.contains("watches.removeAll()"))
        XCTAssertTrue(source.contains("cloudAgentWakeWatcher.stop()"))
        XCTAssertTrue(source.contains("cloudAgentWakeWatcher.start()"))
        let logout = try XCTUnwrap(source.range(of: "} else {\n            cloudAgentWakeWatcher.stop()\n            stopAgentStorePeriodicReconcile()"))
        let relayStop = try XCTUnwrap(source.range(of: "main.coordinator.stopAutomationRelay()", range: logout.lowerBound..<source.endIndex))
        XCTAssertLessThan(logout.lowerBound, relayStop.lowerBound)
    }


    func testCloudAgentRateLimitPauseMatchesDesktopPolicy() {
        XCTAssertEqual(
            IOSCloudAgentPollRetryPolicy.pauseMilliseconds(
                retryAfterMilliseconds: nil,
                randomFraction: 0
            ),
            60_000
        )
        XCTAssertEqual(
            IOSCloudAgentPollRetryPolicy.pauseMilliseconds(
                retryAfterMilliseconds: nil,
                randomFraction: 1
            ),
            75_000
        )
        XCTAssertEqual(
            IOSCloudAgentPollRetryPolicy.pauseMilliseconds(
                retryAfterMilliseconds: 2_000,
                randomFraction: 0.5
            ),
            2_250
        )
        XCTAssertEqual(
            IOSCloudAgentPollRetryPolicy.pauseMilliseconds(
                retryAfterMilliseconds: 2_000,
                randomFraction: .nan
            ),
            2_000
        )
        XCTAssertEqual(iosCursorRetryAfterMilliseconds("1.5"), 1_500)
        XCTAssertEqual(iosCursorRetryAfterMilliseconds("0"), 0)
        XCTAssertNil(iosCursorRetryAfterMilliseconds("-1"))
        XCTAssertNil(iosCursorRetryAfterMilliseconds("n/a"))
    }

    func testCloudAgentWakeWatcherUsesRateLimitPauseFence() throws {
        let iosRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let source = try String(
            contentsOf: iosRoot
                .deletingLastPathComponent()
                .deletingLastPathComponent()
                .appendingPathComponent("source/ios-main/FabushiRuntime.swift"),
            encoding: .utf8
        )
        XCTAssertTrue(source.contains("dashboardError.httpStatus == 429"))
        XCTAssertTrue(source.contains("dashboardError.retryAfterMilliseconds"))
        XCTAssertTrue(source.contains("pausedUntilMs = now.saturatingAdding(pauseMs)"))
        XCTAssertTrue(source.contains("if now < pausedUntilMs"))
    }


    func testCloudAgentWakeCompletionBestEffortDumpsTranscriptBeforeSettlement() throws {
        let iosRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let source = try String(
            contentsOf: iosRoot
                .deletingLastPathComponent()
                .deletingLastPathComponent()
                .appendingPathComponent("source/ios-main/FabushiRuntime.swift"),
            encoding: .utf8
        )
        XCTAssertTrue(source.contains("augmentWithTranscriptDump(wake, result: result)"))
        XCTAssertTrue(source.contains("coordinator.cloudAgentTranscriptDump(bcId: wake.workId)"))
        XCTAssertTrue(source.contains("a transcript dump failure must"))
        XCTAssertTrue(source.contains("Full transcript dumped to"))
        XCTAssertTrue(source.contains("tail -n 1"))
    }

}
