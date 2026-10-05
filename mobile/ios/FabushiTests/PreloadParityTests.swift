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

    func testPinnedMainRPCSurfaceAndEdgeChannelNames() {
        XCTAssertTrue(IOSMainRPCRuntime.isMethod("openExternal"))
        XCTAssertTrue(IOSMainRPCRuntime.isMethod("authenticateMcpServer"))
        XCTAssertTrue(IOSMainRPCRuntime.isMethod("getLinkMetadata"))
        XCTAssertEqual(IOSMainRPCRuntime.methodTable["getLinkMetadata"], .object)
        XCTAssertTrue(IOSMainRPCRuntime.isMethod("listAllAutomations"))
        XCTAssertEqual(IOSMainRPCRuntime.methodTable["listAllAutomations"], .none)
        XCTAssertFalse(IOSMainRPCRuntime.isMethod("totallyUnknownMethod"))
        XCTAssertEqual(
            IOSRPCEdgeRuntime.methodChannel(edge: "main", method: "openExternal"),
            "sand-rpc:main:m:openExternal"
        )
        XCTAssertEqual(
            IOSRPCEdgeRuntime.eventChannel(edge: "main", event: "deep-link"),
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
    func testVNCRuntimeEntrypointUsesSharedNativePolicy() {
        let runtime = IOSVNCPreloadEntrypoint.install()
        XCTAssertTrue(runtime.updateViewerVisibility(true))
        XCTAssertTrue(runtime.viewerIsVisible)
    }
}
