import XCTest
@testable import Fabushi

final class SharedPolicyParityTests: XCTestCase {
    func testDesktopAndFabushiDeepLinks() {
        XCTAssertEqual(FabushiDesktopPolicy.buildPluginDeepLink(pluginID: "123"), "fabushi://app/v1/plugin/add?id=123")
        XCTAssertNil(FabushiDesktopPolicy.buildPluginDeepLink(pluginID: "abc"))
        let parsed = FabushiDeepLinkParser.parse("fabushi://app/v1/info?topic=deep-links")
        XCTAssertEqual(parsed?.route, .info(topic: "deep-links"))
        XCTAssertEqual(parsed?.source, .customScheme)
        XCTAssertEqual(parsed?.canonicalURL.absoluteString, "fabushi://app/v1/info?topic=deep-links")
        XCTAssertNil(FabushiDeepLinkParser.parse("fabushi://user:pass@app/v1/open"))
        XCTAssertNil(FabushiDeepLinkParser.parse("fabushi://app/v1/../open"))
        XCTAssertNil(FabushiDeepLinkParser.parse("fabushi://app/v1/info?topic=deep-links&topic=deep-links"))
    }

    func testExternalAndLinkPreviewPolicies() {
        XCTAssertEqual(ExternalURLPolicy.parseAllowed("https://example.com/a"), "https://example.com/a")
        XCTAssertNil(ExternalURLPolicy.parseAllowed("javascript:alert(1)"))
        XCTAssertTrue(ExternalURLPolicy.isHTTP("http://example.com"))
        XCTAssertTrue(LinkPreviewPolicy.hasNonPublicHostnameSuffix("api.internal"))
        XCTAssertTrue(LinkPreviewPolicy.hasNonPublicHostnameSuffix("localhost"))
        XCTAssertFalse(LinkPreviewPolicy.hasNonPublicHostnameSuffix("openai.com"))
    }

    func testRetryAfterNumericAndDate() {
        let now = Date(timeIntervalSince1970: 0)
        XCTAssertEqual(RetryAfter.parseMilliseconds("1.5", now: now), 1_500)
        XCTAssertEqual(RetryAfter.parseMilliseconds("-1", now: now), 0)
        XCTAssertEqual(RetryAfter.parseMilliseconds("Thu, 01 Jan 1970 00:00:05 GMT", now: now), 5_000)
        XCTAssertNil(RetryAfter.parseMilliseconds("not-a-date", now: now))
    }

    func testUpdateTrackGate() {
        XCTAssertEqual(UpdateTrackPolicy.selectable(unlockInternalTracks: false), [.stable])
        XCTAssertEqual(UpdateTrackPolicy.selectable(unlockInternalTracks: true), [.stable, .dogfood])
        XCTAssertNil(UpdateTrackPolicy.managedTrack("nightly"))
        XCTAssertEqual(UpdateTrackPolicy.effectiveTrack(managed: .nightly, userOverride: nil, buildDefault: nil), .stable)
    }

    func testTimezoneAndErrno() {
        XCTAssertTrue(TimeZonePolicy.isValidIANA("America/Los_Angeles"))
        XCTAssertFalse(TimeZonePolicy.isValidIANA("Mars/Olympus"))
        XCTAssertEqual(TimeZonePolicy.formatUTCOffset(at: Date(timeIntervalSince1970: 0), timeZone: "UTC"), "UTC+0")
        XCTAssertEqual(SystemErrno.find(in: POSIXError(.ECONNREFUSED)), "ECONNREFUSED")
        let nested = SystemErrnoError(systemErrnoCode: nil, underlyingSystemError: SystemErrnoError(systemErrnoCode: "ETIMEDOUT", underlyingSystemError: nil))
        XCTAssertEqual(SystemErrno.find(in: nested), "ETIMEDOUT")
    }

    func testVNCContractsAndIdentity() {
        XCTAssertEqual(VNCLivenessContract.windowMilliseconds, 10_000)
        XCTAssertEqual(VNCLivenessContract.minimumImpactfulInputs, 3)
        XCTAssertTrue(VNCLivenessContract.isValid(.init(phase: "post_connect", stallMilliseconds: 5, keys: 1, clicks: 2, moves: 3, inBytes: 0)))
        XCTAssertFalse(VNCLivenessContract.isValid(.init(phase: "post_connect", stallMilliseconds: -1, keys: 1, clicks: 2, moves: 3, inBytes: 0)))
        XCTAssertEqual(VNCViewerVisibilityContract.channel, "sand:vnc-viewer-visible")
        XCTAssertEqual(FabushiProductIdentity.displayName, "Fabushi")
    }
}
