import XCTest
@testable import Fabushi

final class AboutOverlayParityTests: XCTestCase {
    func testProjectionUsesNativeBundleVersionTrackAndPlatform() {
        let info = FabushiAboutInfo.project(
            infoDictionary: [
                "CFBundleShortVersionString": "1.2.65",
                "CFBundleVersion": "34",
            ],
            receiptLastPathComponent: "sandboxReceipt",
            isDebug: false
        )

        XCTAssertEqual(info.version, "1.2.65")
        XCTAssertEqual(info.build, "34")
        XCTAssertEqual(info.displayVersion, "1.2.65 (34)")
        XCTAssertEqual(info.releaseTrack, .testFlight)
        XCTAssertEqual(info.platform, "iOS")
        XCTAssertEqual(
            info.copyText,
            "Version: 1.2.65\nBuild: 34\nRelease Track: TestFlight\nOS: iOS"
        )
    }

    func testReleaseTrackFailsClosedAcrossDevelopmentArchiveAndStore() {
        XCTAssertEqual(
            FabushiAboutInfo.project(
                infoDictionary: [:],
                receiptLastPathComponent: nil,
                isDebug: true
            ).releaseTrack,
            .development
        )
        XCTAssertEqual(
            FabushiAboutInfo.project(
                infoDictionary: [:],
                receiptLastPathComponent: nil,
                isDebug: false
            ).releaseTrack,
            .archive
        )
        XCTAssertEqual(
            FabushiAboutInfo.project(
                infoDictionary: [:],
                receiptLastPathComponent: "receipt",
                isDebug: false
            ).releaseTrack,
            .appStore
        )
    }

    func testCopiedConfirmationMatchesDesktopTwelveHundredMillisecondContract() {
        XCTAssertEqual(FabushiAboutPresentationPolicy.copyConfirmationMilliseconds, 1_200)
    }
}
