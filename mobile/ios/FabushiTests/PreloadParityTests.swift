import XCTest
@testable import Fabushi

final class PreloadParityTests: XCTestCase {
    func testClipboardPasteScriptEscapesPayloadAndRequiresSuccess() {
        let text = "hello \"world\"\nnext"
        let script = IOSVNCClipboardPaste.buildTrustedNoVNCPasteScript(text: text)
        XCTAssertTrue(script.contains("clipboardPasteFrom"))
        XCTAssertTrue(script.contains("\\\"world\\\""))
        XCTAssertEqual(IOSVNCClipboardPaste.resolveHostToBoxSync(text: text, didPaste: true), text)
        XCTAssertNil(IOSVNCClipboardPaste.resolveHostToBoxSync(text: text, didPaste: false))
    }

    func testViewerVisibilityGateOnlySignalsHiddenToVisibleTransition() {
        var gate = IOSVNCViewerVisibilityGate()
        XCTAssertTrue(gate.update(true))
        XCTAssertFalse(gate.update(true))
        XCTAssertFalse(gate.update(false))
        XCTAssertTrue(gate.update(true))
    }

    func testVNCLivenessDetectsImpactfulInputWithoutOutput() {
        var detector = IOSVNCLivenessDetector()
        XCTAssertNil(detector.sample(
            nowMilliseconds: 0,
            counters: .init(keys: 0, clicks: 0, moves: 0, drawOps: 0, inBytes: 0)
        ))
        XCTAssertNil(detector.sample(
            nowMilliseconds: 4_000,
            counters: .init(keys: 1, clicks: 0, moves: 2, drawOps: 0, inBytes: 0)
        ))
        XCTAssertNil(detector.sample(
            nowMilliseconds: 7_000,
            counters: .init(keys: 1, clicks: 1, moves: 3, drawOps: 0, inBytes: 0)
        ))
        let report = detector.sample(
            nowMilliseconds: 11_000,
            counters: .init(keys: 2, clicks: 1, moves: 4, drawOps: 0, inBytes: 0)
        )
        XCTAssertEqual(report?.phase, "post_connect")
        XCTAssertEqual(report?.keys, 2)
        XCTAssertEqual(report?.clicks, 1)
        XCTAssertEqual(report?.stallMilliseconds, 7_000)
    }

    @MainActor
    func testVNCRuntimeTracksRealRFBSessionTransitions() {
        let runtime = IOSVNCPreloadRuntime()

        XCTAssertNil(runtime.ingestRFBState(.disconnected))
        XCTAssertNil(runtime.ingestRFBState(.connecting))

        let connected = runtime.ingestRFBState(.connected)
        XCTAssertEqual(connected?.phase, .connect)
        XCTAssertEqual(connected?.clean, true)
        XCTAssertNil(runtime.ingestRFBState(.connected))

        let dropped = runtime.ingestRFBState(.reconnecting)
        XCTAssertEqual(dropped?.phase, .disconnect)
        XCTAssertEqual(dropped?.clean, false)

        XCTAssertNil(runtime.ingestRFBState(.connecting))
        let reconnected = runtime.ingestRFBState(.connected)
        XCTAssertEqual(reconnected?.phase, .reconnect)
        XCTAssertEqual(reconnected?.clean, true)

        let cleanDisconnect = runtime.ingestRFBState(.disconnecting)
        XCTAssertEqual(cleanDisconnect?.phase, .disconnect)
        XCTAssertEqual(cleanDisconnect?.clean, true)
    }

    @MainActor
    func testVNCRuntimeParsesTrustedNativeBridgeMessages() {
        XCTAssertEqual(
            IOSVNCPreloadRuntime.rfbState(from: [
                "kind": "rfb_state",
                "state": "connected",
            ]),
            .connected
        )
        XCTAssertNil(IOSVNCPreloadRuntime.rfbState(from: [
            "kind": "rfb_state",
            "state": "unknown",
        ]))

        let counters = IOSVNCPreloadRuntime.livenessCounters(from: [
            "kind": "liveness",
            "counters": [
                "keys": 3,
                "clicks": 2,
                "moves": 9,
                "drawOps": 7,
                "inBytes": 1024,
            ],
        ])
        XCTAssertEqual(
            counters,
            .init(keys: 3, clicks: 2, moves: 9, drawOps: 7, inBytes: 1024)
        )

        let cursor = IOSVNCPreloadRuntime.cursorTelemetry(from: [
            "kind": "cursor",
            "x": 10.5,
            "y": 20.25,
            "type": "drag",
        ])
        XCTAssertEqual(cursor?.x, 10.5)
        XCTAssertEqual(cursor?.y, 20.25)
        XCTAssertEqual(cursor?.kind, .drag)
        XCTAssertNil(IOSVNCPreloadRuntime.cursorTelemetry(from: [
            "kind": "cursor",
            "x": -1,
            "y": 2,
            "type": "move",
        ]))

        XCTAssertEqual(
            IOSVNCPreloadRuntime.hostKey(from: [
                "kind": "host_key",
                "key": "ArrowRight",
            ]),
            .arrowRight
        )
        XCTAssertEqual(
            IOSVNCPreloadRuntime.hostKey(from: [
                "kind": "host_key",
                "key": "ArrowUp",
            ]),
            .arrowUp
        )
        XCTAssertNil(IOSVNCPreloadRuntime.hostKey(from: [
            "kind": "host_key",
            "key": "Escape",
        ]))
        XCTAssertNil(IOSVNCPreloadRuntime.hostKey(from: [
            "kind": "cursor",
            "key": "ArrowLeft",
        ]))
    }

    @MainActor
    func testVNCBootstrapScriptUsesRealNoVNCSignalsAndCounters() {
        let script = IOSVNCPreloadRuntime.bootstrapScript
        XCTAssertTrue(script.contains("noVNC_connected"))
        XCTAssertTrue(script.contains("noVNC_reconnecting"))
        XCTAssertTrue(script.contains("rfb_state"))
        XCTAssertTrue(script.contains("core/rfb.js"))
        XCTAssertTrue(script.contains("core/display.js"))
        XCTAssertTrue(script.contains("core/websock.js"))
        XCTAssertTrue(script.contains("fabushiVNC"))
        XCTAssertTrue(script.contains("sandInteractive"))
        XCTAssertTrue(script.contains("host_key"))
        XCTAssertTrue(script.contains("ArrowUp"))
        XCTAssertTrue(script.contains("ArrowDown"))
        XCTAssertTrue(script.contains("ArrowLeft"))
        XCTAssertTrue(script.contains("ArrowRight"))
        XCTAssertFalse(
            script.contains("preventDefault"),
            "Trusted host-key forwarding must not steal arrows from noVNC"
        )
        XCTAssertFalse(
            script.contains("didFinish"),
            "RFB connectivity must never be inferred from WebKit navigation completion"
        )
    }

    func testBrowserIdentityProviderAllowlistAndNavigationPolicy() throws {
        XCTAssertTrue(IOSBrowserPreloadPolicy.isAllowlistedIdentityProvider(hostname: "login.microsoftonline.com"))
        XCTAssertTrue(IOSBrowserPreloadPolicy.isAllowlistedIdentityProvider(hostname: "tenant.okta.com"))
        XCTAssertFalse(IOSBrowserPreloadPolicy.isAllowlistedIdentityProvider(hostname: "example.com"))
        let trusted = try XCTUnwrap(URL(string: "https://chat.openai.com/path"))
        XCTAssertEqual(
            IOSWebViewPreloadPolicy.decision(for: trusted, trustedHosts: ["chat.openai.com"]),
            .allowInView
        )
        let external = try XCTUnwrap(URL(string: "https://example.com"))
        XCTAssertEqual(
            IOSWebViewPreloadPolicy.decision(for: external, trustedHosts: ["chat.openai.com"]),
            .openExternally
        )
        let callback = try XCTUnwrap(URL(string: "fabushi://auth/callback"))
        XCTAssertEqual(
            IOSWebViewPreloadPolicy.decision(for: callback, trustedHosts: []),
            .handToApp
        )
    }


    @MainActor
    func testNativeBrowserAuthPlanPreservesIdPCompatibilityAndSystemLifecycle() throws {
        let idp = try XCTUnwrap(URL(string: "https://tenant.okta.com/login"))
        let plan = try XCTUnwrap(IOSWebViewPreloadEntrypoint.browserAuthPlan(for: idp))
        XCTAssertEqual(plan.originalOrigin, "https://tenant.okta.com")
        XCTAssertEqual(plan.localNetworkCompatibility, .systemBrowserOwnedIdentityProvider)
        XCTAssertTrue(plan.usesSystemAuthenticationSession)
        XCTAssertTrue(plan.usesSystemDialogLifecycle)
        XCTAssertTrue(plan.usesSystemPasskeyPresentation)
        XCTAssertTrue(plan.callbackReturnsToApp)

        let ordinary = try XCTUnwrap(URL(string: "https://cursor.com/login"))
        XCTAssertEqual(
            IOSWebViewPreloadEntrypoint.browserAuthPlan(for: ordinary)?.localNetworkCompatibility,
            .notRequired
        )
        XCTAssertNil(
            IOSWebViewPreloadEntrypoint.browserAuthPlan(
                for: try XCTUnwrap(URL(string: "javascript:alert(1)"))
            )
        )
    }

    @MainActor
    func testNativeBrowserAuthFenceHappyCancelReloadAndAccountSwitch() throws {
        let fence = IOSBrowserAuthSessionFence()
        let loginURL = try XCTUnwrap(URL(string: "https://tenant.okta.com/login"))
        let callback = try XCTUnwrap(URL(string: "fabushi://auth/complete?attemptId=attempt-1"))
        let hostileCallback = try XCTUnwrap(URL(string: "https://example.com/callback"))
        let wrongAppCallback = try XCTUnwrap(URL(string: "fabushi://other/complete"))

        let first = try XCTUnwrap(fence.begin(
            attemptID: "attempt-1",
            accountScope: "logged-out",
            url: loginURL
        ))
        XCTAssertTrue(fence.accepts(
            first,
            currentAttemptID: "attempt-1",
            currentAccountScope: "logged-out"
        ))
        XCTAssertFalse(fence.accepts(
            first,
            currentAttemptID: "attempt-1",
            currentAccountScope: "account-b"
        ), "account switch must fence the old browser callback")
        XCTAssertFalse(fence.accepts(
            first,
            currentAttemptID: "attempt-1",
            currentAccountScope: "logged-out",
            callbackURL: hostileCallback
        ))
        XCTAssertFalse(fence.accepts(
            first,
            currentAttemptID: "attempt-1",
            currentAccountScope: "logged-out",
            callbackURL: wrongAppCallback
        ), "only the browser-auth app callback is an origin return")

        let reloaded = try XCTUnwrap(fence.begin(
            attemptID: "attempt-1",
            accountScope: "logged-out",
            url: loginURL
        ))
        XCTAssertFalse(fence.accepts(
            first,
            currentAttemptID: "attempt-1",
            currentAccountScope: "logged-out"
        ), "reopening the system browser must stale the previous generation")
        XCTAssertTrue(fence.settleCallback(
            reloaded,
            currentAttemptID: "attempt-1",
            currentAccountScope: "logged-out",
            callbackURL: callback
        ))
        XCTAssertFalse(fence.accepts(
            reloaded,
            currentAttemptID: "attempt-1",
            currentAccountScope: "logged-out"
        ), "settled callbacks must not commit twice")

        let cancelled = try XCTUnwrap(fence.begin(
            attemptID: "attempt-2",
            accountScope: "logged-out",
            url: loginURL
        ))
        fence.cancel()
        XCTAssertFalse(fence.accepts(
            cancelled,
            currentAttemptID: "attempt-2",
            currentAccountScope: "logged-out"
        ))
    }

    @MainActor
    func testFeatureEventBrokerBuffersUnmatchedEventsWithoutDroppingThem() async throws {
        var receiveCount = 0
        var events: [CoordinatorPayload] = [
            .object(["type": .string("group.listed"), "groups": .array([])]),
            .object(["type": .string("bot.listed"), "bots": .array([])]),
        ]
        let broker = IOSFeatureEventBroker { _ in
            receiveCount += 1
            return events.isEmpty ? nil : events.removeFirst()
        }

        let botPayload = try await broker.next(deadlineMilliseconds: 1_000) {
            $0["type"] as? String == "bot.listed"
        }
        let bot = try XCTUnwrap(botPayload.foundationValue as? [String: Any])
        XCTAssertEqual(bot["type"] as? String, "bot.listed")
        XCTAssertEqual(receiveCount, 2)

        let groupPayload = try await broker.next(deadlineMilliseconds: 1_000) {
            $0["type"] as? String == "group.listed"
        }
        let group = try XCTUnwrap(groupPayload.foundationValue as? [String: Any])
        XCTAssertEqual(group["type"] as? String, "group.listed")
        XCTAssertEqual(receiveCount, 2, "buffered events must not trigger a second receive")
        broker.dispose()
    }



    func testSSEDecoderPreservesPartialFrameAndSplitUTF8() {
        var decoder = SSEBlockDecoder()
        let payload = Data("法🙏".utf8)
        let split = payload.count - 1

        var first = Data("event: message\ndata: ".utf8)
        first.append(payload.subdata(in: 0..<split))
        XCTAssertTrue(decoder.append(first).isEmpty)

        var second = payload.subdata(in: split..<payload.count)
        second.append(Data("\n\n".utf8))
        XCTAssertEqual(
            decoder.append(second),
            [ServerSentEventBlock(event: "message", id: nil, data: "法🙏")]
        )
        XCTAssertNil(decoder.finish())
    }

    @MainActor
    func testFeatureEventBrokerSurvivesEmptyStallAndRetryableDisconnect() async throws {
        var count = 0
        var delays: [Int] = []
        let broker = IOSFeatureEventBroker(
            receiveTimeoutMilliseconds: 5,
            sleeper: { delays.append($0) }
        ) { _ in
            count += 1
            if count == 1 { return nil }
            if count == 2 {
                throw IOSCoordinatorPortClient.PortError(
                    code: "gateway-unreachable",
                    message: "transient",
                    transportKind: "network"
                )
            }
            return .object(["type": .string("recovered")])
        }

        let payload = try await broker.next(deadlineMilliseconds: 5_000) {
            $0["type"] as? String == "recovered"
        }
        XCTAssertEqual(
            (payload.foundationValue as? [String: Any])?["type"] as? String,
            "recovered"
        )
        XCTAssertEqual(count, 3)
        XCTAssertEqual(delays, [1_000])
        broker.dispose()
    }

    @MainActor
    func testFeatureEventBrokerPermanentFailureRestartsForNextWaiter() async throws {
        var count = 0
        let broker = IOSFeatureEventBroker { _ in
            count += 1
            if count == 1 {
                throw IOSCoordinatorPortClient.PortError(
                    code: "gateway-command-failed",
                    message: "permanent"
                )
            }
            return .object(["type": .string("fresh")])
        }

        do {
            _ = try await broker.next(deadlineMilliseconds: 1_000) { _ in true }
            XCTFail("expected permanent failure")
        } catch let error as IOSCoordinatorPortClient.PortError {
            XCTAssertEqual(error.code, "gateway-command-failed")
        }

        let payload = try await broker.next(deadlineMilliseconds: 1_000) {
            $0["type"] as? String == "fresh"
        }
        XCTAssertEqual(
            (payload.foundationValue as? [String: Any])?["type"] as? String,
            "fresh"
        )
        broker.dispose()
    }

    @MainActor
    func testFeatureEventBrokerCancellationFencesLateReceive() async throws {
        var block = true
        var receiveStarted = false
        let broker = IOSFeatureEventBroker { _ in
            receiveStarted = true
            if block {
                try await Task<Never, Never>.sleep(
                    nanoseconds: 5_000_000_000
                )
            }
            return .object(["type": .string("fresh")])
        }

        let pending = Task { @MainActor in
            try await broker.next(deadlineMilliseconds: 10_000) { _ in true }
        }
        for _ in 0..<64 {
            if receiveStarted { break }
            await Task.yield()
        }
        XCTAssertTrue(receiveStarted)

        pending.cancel()
        do {
            _ = try await pending.value
            XCTFail("expected cancellation")
        } catch is CancellationError {
        }

        block = false
        receiveStarted = false
        let payload = try await broker.next(deadlineMilliseconds: 1_000) {
            $0["type"] as? String == "fresh"
        }
        XCTAssertTrue(receiveStarted)
        XCTAssertEqual(
            (payload.foundationValue as? [String: Any])?["type"] as? String,
            "fresh"
        )
        broker.dispose()
    }

    func testAppSurfacesDoNotBypassCanonicalFeatureEventBroker() throws {
        let file = URL(fileURLWithPath: #filePath)
        let repositoryRoot = file
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let roots = [
            repositoryRoot.appendingPathComponent("frontend"),
            repositoryRoot.appendingPathComponent("mobile/ios/Fabushi"),
        ]
        let manager = FileManager.default
        var bypasses: [String] = []

        for root in roots {
            guard let enumerator = manager.enumerator(
                at: root,
                includingPropertiesForKeys: nil
            ) else { continue }
            for case let url as URL in enumerator {
                guard url.pathExtension == "swift",
                      !url.path.contains("/FabushiTests/")
                else { continue }
                let source = try String(contentsOf: url, encoding: .utf8)
                if source.contains("method: \"feature.receive\"") {
                    bypasses.append(
                        url.path.replacingOccurrences(
                            of: repositoryRoot.path + "/",
                            with: ""
                        )
                    )
                }
            }
        }

        XCTAssertEqual(
            bypasses,
            [],
            "FeatureHost receive must stay behind IOSFeatureEventBroker: \(bypasses)"
        )

        let preload = try String(
            contentsOf: repositoryRoot
                .appendingPathComponent("source/ios-preload/preload.swift"),
            encoding: .utf8
        )
        XCTAssertEqual(
            preload.components(separatedBy: "method: \"feature.receive\"").count - 1,
            1,
            "IOSPreloadBridge must remain the single raw FeatureHost receive owner"
        )
    }


    func testPinnedMainRPCSurfaceAndEdgeChannelNames() {
        XCTAssertTrue(IOSMainRPCRuntime.isMethod("openExternal"))
        XCTAssertTrue(IOSMainRPCRuntime.isMethod("authenticateMcpServer"))
        XCTAssertTrue(IOSMainRPCRuntime.isMethod("getLinkMetadata"))
        XCTAssertEqual(IOSMainRPCRuntime.methodTable["getLinkMetadata"], .object)
        XCTAssertTrue(IOSMainRPCRuntime.isMethod("listAllAutomations"))
        XCTAssertEqual(IOSMainRPCRuntime.methodTable["listAllAutomations"], IOSRPCArgumentShape.none)
        XCTAssertFalse(IOSMainRPCRuntime.isMethod("totallyUnknownMethod"))
        XCTAssertEqual(IOSMainRPCRuntime.desktopAuthorityMethodTable.count, 127)
        XCTAssertTrue(IOSMainRPCRuntime.isDesktopAuthorityMethod("getUiPreferences"))
        XCTAssertEqual(IOSMainRPCRuntime.desktopAuthorityMethodTable["getUiPreferences"], .none)
        XCTAssertTrue(IOSMainRPCRuntime.isDesktopAuthorityMethod("setUiPreferences"))
        XCTAssertEqual(IOSMainRPCRuntime.desktopAuthorityMethodTable["setUiPreferences"], .object)
        XCTAssertTrue(IOSMainRPCRuntime.isDesktopAuthorityMethod("getCallMediaPreferences"))
        XCTAssertEqual(IOSMainRPCRuntime.desktopAuthorityMethodTable["getCallMediaPreferences"], .none)
        XCTAssertTrue(IOSMainRPCRuntime.isDesktopAuthorityMethod("setCallMediaPreferences"))
        XCTAssertEqual(IOSMainRPCRuntime.desktopAuthorityMethodTable["setCallMediaPreferences"], .object)
        XCTAssertTrue(IOSMainRPCRuntime.isDesktopAuthorityMethod("requestCallMediaPermissions"))
        XCTAssertEqual(IOSMainRPCRuntime.desktopAuthorityMethodTable["requestCallMediaPermissions"], .object)
        XCTAssertTrue(IOSMainRPCRuntime.isDesktopAuthorityMethod("listCallDisplaySources"))
        XCTAssertEqual(IOSMainRPCRuntime.desktopAuthorityMethodTable["listCallDisplaySources"], .none)
        XCTAssertFalse(IOSMainRPCRuntime.isDesktopAuthorityMethod("listAllAutomations"))
        XCTAssertTrue(IOSMainRPCRuntime.isMethod("listAllAutomations"))
        XCTAssertEqual(IOSMainRPCRuntime.iosExtensionMethodTable["listAllAutomations"], .none)

        XCTAssertEqual(IOSMainRPCRuntime.eventNames, [
            "box-migration",
            "cursor-auth-changed",
            "deep-link",
            "dev-box-pull-progress",
            "dev-box-rebuild",
            "egress-tunnel-changed",
            "egress-tunnel-status-changed",
            "experiments-changed",
            "focus-agent",
            "force-onboarding",
            "open-about",
            "open-feedback",
            "skip-onboarding",
            "theme-changed",
            "update-status",
            "vnc-user-presence",
            "window-state",
            "webauthn-proxy-changed",
            "zoom-factor-changed",
        ])
        XCTAssertTrue(IOSMainRPCRuntime.isEvent("deep-link"))
        XCTAssertTrue(IOSMainRPCRuntime.isEvent("zoom-factor-changed"))
        XCTAssertFalse(IOSMainRPCRuntime.isEvent("totally-unknown-event"))

        XCTAssertEqual(Set(IOSMainRPCRuntime.eventDispositions.keys), Set(IOSMainRPCRuntime.eventNames))
        XCTAssertEqual(IOSMainRPCRuntime.eventDispositions.count, 19)
        XCTAssertEqual(IOSMainRPCRuntime.eventDisposition("deep-link")?.ownerSymbol, "IOSDeepLinkController")
        XCTAssertEqual(IOSMainRPCRuntime.eventDisposition("cursor-auth-changed")?.ownerSymbol, "IOSCursorAuthService")
        XCTAssertEqual(IOSMainRPCRuntime.eventDisposition("box-migration")?.ownerSymbol, "RemoteComputerRebuildOwner")
        XCTAssertEqual(IOSMainRPCRuntime.eventDisposition("open-feedback")?.ownerSymbol, "AccountFeedbackView")
        XCTAssertEqual(IOSMainRPCRuntime.eventDisposition("window-state")?.delivery, .platformMechanismReplacement)
        XCTAssertEqual(IOSMainRPCRuntime.eventDisposition("zoom-factor-changed")?.delivery, .platformMechanismReplacement)
        for event in IOSMainRPCRuntime.eventNames {
            let disposition = IOSMainRPCRuntime.eventDisposition(event)
            XCTAssertNotNil(disposition, "Missing iOS disposition for \(event)")
            XCTAssertFalse(disposition?.ownerPath.isEmpty ?? true)
            XCTAssertFalse(disposition?.ownerSymbol.isEmpty ?? true)
            XCTAssertFalse(disposition?.productEffect.isEmpty ?? true)
        }

        XCTAssertEqual(
            IOSMainRPCRuntime.methodChannel("openExternal"),
            "sand-rpc:main:m:openExternal"
        )
        XCTAssertEqual(
            IOSMainRPCRuntime.eventChannel("deep-link"),
            "sand-rpc:main:e:deep-link"
        )
    }

    func testPasskeyDeadlineFailsClosed() async {
        do {
            _ = try await IOSPasskeyStall.run(
                method: "credentials.get",
                sinceMilliseconds: 1,
                timeoutNanoseconds: 1_000_000,
                operation: {
                    try await Task.sleep(nanoseconds: 1_000_000_000)
                    return "late"
                },
                reportStall: { _, _ in }
            )
            XCTFail("expected passkey stall")
        } catch let error as IOSPasskeyStallError {
            XCTAssertEqual(error, .stalled(method: "credentials.get"))
        } catch {
            XCTFail("unexpected error: \(error)")
        }
    }


    @MainActor
    func testVNCRuntimeVisibilityLifecycleRebaselinesLivenessOnReveal() {
        let runtime = IOSVNCPreloadRuntime()
        XCTAssertTrue(runtime.updateViewerVisibility(true))
        XCTAssertTrue(runtime.viewerIsVisible)
        XCTAssertFalse(runtime.updateViewerVisibility(true))
        XCTAssertFalse(runtime.updateViewerVisibility(false))
        XCTAssertFalse(runtime.viewerIsVisible)
        XCTAssertTrue(runtime.updateViewerVisibility(true))
        runtime.resetLiveness()
        XCTAssertNil(runtime.sampleLiveness(
            nowMilliseconds: 10_000,
            counters: .init(keys: 4, clicks: 1, moves: 2, drawOps: 0, inBytes: 0)
        ))
    }


    @MainActor
    func testVNCRuntimeEntrypointUsesSharedNativePolicy() {
        let runtime = IOSVNCPreloadEntrypoint.install()
        XCTAssertTrue(runtime.updateViewerVisibility(true))
        XCTAssertTrue(runtime.viewerIsVisible)
    }    
    @MainActor
    func testRpcEdgeRuntimeUsesDesktopChannelsEnvelopesAndNativeCancellation() async throws {
        let transport = RecordingRPCEdgeTransport()
        transport.nextReply = .ok(.string("ok"))

        let value = try await IOSRPCEdgeRuntime.call(
            edge: IOSMainRPCRuntime.contractName,
            method: "getLinkMetadata",
            payload: .object(["url": .string("https://example.com")]),
            transport: transport
        )
        XCTAssertEqual(value, .string("ok"))
        XCTAssertEqual(
            transport.invokedChannels,
            ["sand-rpc:main:m:getLinkMetadata"]
        )
        XCTAssertEqual(
            IOSRPCEdgeRuntime.methodName(
                edge: "main",
                channel: "sand-rpc:main:m:getUiPreferences"
            ),
            "getUiPreferences"
        )
        XCTAssertEqual(
            IOSRPCEdgeRuntime.eventName(
                edge: "main",
                channel: "sand-rpc:main:e:deep-link"
            ),
            "deep-link"
        )

        transport.nextReply = .failed(.init(
            code: "handler/failed",
            detail: "deterministic"
        ))
        do {
            _ = try await IOSRPCEdgeRuntime.call(
                edge: "main",
                method: "getUiPreferences",
                payload: .object([:]),
                transport: transport
            )
            XCTFail("expected edge handler failure")
        } catch let error as IOSEdgeReplyFailure {
            XCTAssertEqual(error.code, "handler/failed")
            XCTAssertEqual(error.detail, "deterministic")
        }

        transport.nextError = CancellationError()
        do {
            _ = try await IOSRPCEdgeRuntime.call(
                edge: "main",
                method: "getUiPreferences",
                payload: .object([:]),
                transport: transport
            )
            XCTFail("expected native cancellation")
        } catch is CancellationError {
        }
    }

    func testPrimaryPreloadShippingRequestsTraverseRpcEdgeRuntime() throws {
        let file = URL(fileURLWithPath: #filePath)
        let repositoryRoot = file
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let preloadSource = try String(
            contentsOf: repositoryRoot
                .appendingPathComponent("source/ios-preload/preload.swift"),
            encoding: .utf8
        )
        let requestStart = try XCTUnwrap(
            preloadSource.range(of: "private func requestPayload(")
        )
        let requestTail = String(preloadSource[requestStart.lowerBound...])
        let requestEnd = requestTail.range(of: "\n    func request(")
        let requestBody = requestEnd.map {
            String(requestTail[..<$0.lowerBound])
        } ?? requestTail

        XCTAssertTrue(requestBody.contains("IOSRPCEdgeRuntime.call("))
        XCTAssertTrue(requestBody.contains("transport: self"))
        XCTAssertFalse(
            requestBody.contains("return try await client.request("),
            "shipping requestPayload must not bypass the canonical RPC edge runtime"
        )
    }

}


@MainActor
private final class RecordingRPCEdgeTransport: IOSRPCEdgeTransport {
    private(set) var invokedChannels: [String] = []
    private(set) var subscribedChannels: [String] = []
    var nextReply: IOSEdgeReplyEnvelope = .ok(.object([:]))
    var nextError: Error?

    func invoke(
        channel: String,
        payload: CoordinatorPayload
    ) async throws -> IOSEdgeReplyEnvelope {
        invokedChannels.append(channel)
        if let nextError {
            self.nextError = nil
            throw nextError
        }
        return nextReply
    }

    func subscribe(
        channel: String,
        listener: @escaping @MainActor (CoordinatorPayload) -> Void
    ) -> () -> Void {
        subscribedChannels.append(channel)
        return {}
    }
}
