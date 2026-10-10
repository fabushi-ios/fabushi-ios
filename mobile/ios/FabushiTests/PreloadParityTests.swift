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
}
