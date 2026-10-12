import XCTest
@testable import Fabushi

final class AppVersionPolicyParityTests: XCTestCase {
    func testPolicyRequestUsesCanonicalProductionRouteAndSignedMetadata() throws {
        let metadata = IOSReleaseMetadata(
            version: "1.2.65",
            buildNumber: "34",
            bundleIdentifier: "com.ombhrum.fabushi",
            updateMechanism: "app-store-connect"
        )
        let url = try XCTUnwrap(
            IOSAppVersionPolicyClient.requestURL(metadata: metadata)
        )
        XCTAssertEqual(url.host, "api.ombhrum.com")
        XCTAssertEqual(url.path, "/api/app/version-policy")
        let components = try XCTUnwrap(
            URLComponents(url: url, resolvingAgainstBaseURL: false)
        )
        let query = Dictionary(
            uniqueKeysWithValues: (components.queryItems ?? []).map {
                ($0.name, $0.value ?? "")
            }
        )
        XCTAssertEqual(query["platform"], "ios")
        XCTAssertEqual(query["channel"], "stable")
        XCTAssertEqual(query["version"], "1.2.65")
        XCTAssertEqual(query["buildNumber"], "34")
    }

    func testPolicyDecoderDistinguishesForceOptionalAndRejectsInvalidURL() throws {
        func data(
            strategy: String,
            force: Bool,
            update: Bool,
            download: String = "https://apps.apple.com/app/id123"
        ) -> Data {
            Data("""
            {
              "enabled": true,
              "platform": "ios",
              "channel": "stable",
              "latestVersion": "2.0.0",
              "latestBuildNumber": 40,
              "minSupportedBuildNumber": 35,
              "forceUpdate": \(force),
              "allowSkip": \(!force),
              "rolloutPercentage": 100,
              "promptIntervalHours": 24,
              "title": "Update Fabushi",
              "message": "A newer version is available.",
              "releaseNotes": ["Fixes"],
              "downloadUrl": "\(download)",
              "updateAvailable": \(update),
              "strategy": "\(strategy)"
            }
            """.utf8)
        }

        let forced = try IOSAppVersionPolicyClient.decodePolicy(
            data(strategy: "force", force: true, update: true)
        )
        XCTAssertTrue(forced.isRequired)
        XCTAssertEqual(forced.strategy, .force)

        let optional = try IOSAppVersionPolicyClient.decodePolicy(
            data(strategy: "optional", force: false, update: true)
        )
        XCTAssertFalse(optional.isRequired)
        XCTAssertEqual(optional.strategy, .optional)

        XCTAssertThrowsError(
            try IOSAppVersionPolicyClient.decodePolicy(
                data(
                    strategy: "force",
                    force: true,
                    update: true,
                    download: "http://insecure.example/update"
                )
            )
        )
        XCTAssertThrowsError(
            try IOSAppVersionPolicyClient.decodePolicy(
                data(strategy: "force", force: false, update: true)
            )
        )
    }

    func testFailedPolicyLoadRetainsLastKnownBlockingPolicy() throws {
        let data = Data("""
        {
          "enabled": true,
          "platform": "ios",
          "channel": "stable",
          "latestVersion": "2.0.0",
          "latestBuildNumber": 40,
          "minSupportedBuildNumber": 35,
          "forceUpdate": true,
          "allowSkip": false,
          "rolloutPercentage": 100,
          "promptIntervalHours": 24,
          "title": "Update Fabushi",
          "message": "This version is no longer supported.",
          "releaseNotes": [],
          "downloadUrl": "https://apps.apple.com/app/id123",
          "updateAvailable": true,
          "strategy": "force"
        }
        """.utf8)
        let policy = try IOSAppVersionPolicyClient.decodePolicy(data)
        let state = IOSAppVersionPolicyLoadState.failed(
            message: "offline",
            retained: policy
        )
        XCTAssertEqual(state.policy, policy)
        XCTAssertTrue(state.policy?.isRequired == true)
    }

    func testInvalidBuildMetadataFailsClosedBeforeRequestConstruction() {
        let metadata = IOSReleaseMetadata(
            version: "1.2.65",
            buildNumber: "not-a-number",
            bundleIdentifier: "com.ombhrum.fabushi",
            updateMechanism: "app-store-connect"
        )
        XCTAssertNil(IOSAppVersionPolicyClient.requestURL(metadata: metadata))
    }
}
