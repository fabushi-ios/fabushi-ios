import XCTest
@testable import Fabushi

@MainActor
final class UpdateParityTests: XCTestCase {
    private func metadata() -> IOSReleaseMetadata {
        .init(
            version: "1.2.65",
            buildNumber: "34",
            bundleIdentifier: "com.ombhrum.fabushi",
            updateMechanism: "app-store-connect"
        )
    }

    private func policy(
        strategy: IOSAppVersionPolicyStrategy = .optional,
        latestVersion: String = "1.2.66",
        latestBuildNumber: Int = 35,
        minSupportedBuildNumber: Int = 34,
        forceUpdate: Bool = false,
        updateAvailable: Bool = true
    ) -> IOSAppVersionPolicy {
        .init(
            enabled: true,
            platform: "ios",
            channel: "stable",
            latestVersion: latestVersion,
            latestBuildNumber: latestBuildNumber,
            minSupportedBuildNumber: minSupportedBuildNumber,
            forceUpdate: forceUpdate,
            allowSkip: !forceUpdate,
            rolloutPercentage: 100,
            promptIntervalHours: 24,
            title: "Update Fabushi",
            message: "A newer version is available.",
            releaseNotes: ["Parity update"],
            downloadUrl: "https://apps.apple.com/app/id123456789",
            updateAvailable: updateAvailable,
            strategy: strategy
        )
    }

    func testExplicitCheckUsesVersionPolicyAndProjectsDesktopCompatibleStatus() async throws {
        let expectedMetadata = metadata()
        let offeredPolicy = policy()
        let service = IOSAppStoreUpdateService(
            metadataProvider: { expectedMetadata },
            policyFetcher: { metadata in
                XCTAssertEqual(metadata, expectedMetadata)
                return offeredPolicy
            }
        )

        let checked = try await service.checkForUpdates()
        let object = try XCTUnwrap(checked.foundationValue as? [String: Any])
        let state = try XCTUnwrap(object["state"] as? [String: Any])

        XCTAssertEqual(state["type"] as? String, "available")
        XCTAssertEqual(state["version"] as? String, "1.2.66")
        XCTAssertEqual(object["currentVersion"] as? String, "1.2.65")
        XCTAssertEqual(object["currentTrack"] as? String, "stable")
        XCTAssertEqual(object["isTrackManagedByPolicy"] as? Bool, true)
        XCTAssertEqual(object["selfUpdateSupported"] as? Bool, false)
        XCTAssertEqual(object["storeURL"] as? String, "https://apps.apple.com/app/id123456789")
    }

    func testForcePolicyProjectsMinimumVersionBlocker() async throws {
        let expectedMetadata = metadata()
        let forcedPolicy = policy(
            strategy: .force,
            minSupportedBuildNumber: 35,
            forceUpdate: true
        )
        let service = IOSAppStoreUpdateService(
            metadataProvider: { expectedMetadata },
            policyFetcher: { _ in forcedPolicy }
        )

        let checked = try await service.checkForUpdates()
        let object = try XCTUnwrap(checked.foundationValue as? [String: Any])
        XCTAssertEqual(object["isBelowMinimumVersion"] as? Bool, true)
        XCTAssertEqual(object["policyStrategy"] as? String, "force")
        XCTAssertEqual(object["allowSkip"] as? Bool, false)
    }

    func testFailedRefreshRetainsLastOfferAndReportsError() async throws {
        struct Offline: LocalizedError {
            var errorDescription: String? { "offline" }
        }
        let expectedMetadata = metadata()
        let offeredPolicy = policy()
        var attempt = 0
        let service = IOSAppStoreUpdateService(
            metadataProvider: { expectedMetadata },
            policyFetcher: { _ in
                attempt += 1
                if attempt == 1 { return offeredPolicy }
                throw Offline()
            }
        )

        _ = try await service.checkForUpdates()
        let retained = try await service.checkForUpdates()
        let object = try XCTUnwrap(retained.foundationValue as? [String: Any])
        let state = try XCTUnwrap(object["state"] as? [String: Any])
        let lastCheck = try XCTUnwrap(state["lastCheck"] as? [String: Any])

        XCTAssertEqual(state["type"] as? String, "available")
        XCTAssertEqual(state["version"] as? String, "1.2.66")
        XCTAssertEqual(lastCheck["result"] as? String, "error")
        XCTAssertEqual(lastCheck["errorMessage"] as? String, "offline")
    }

    func testPolicyDecoderRejectsOptionalForceMismatch() throws {
        let payload: [String: Any] = [
            "enabled": true,
            "platform": "ios",
            "channel": "stable",
            "latestVersion": "1.2.66",
            "latestBuildNumber": 35,
            "minSupportedBuildNumber": 34,
            "forceUpdate": true,
            "allowSkip": true,
            "rolloutPercentage": 100,
            "promptIntervalHours": 24,
            "title": "Update",
            "message": "Update available",
            "releaseNotes": [],
            "downloadUrl": "https://apps.apple.com/app/id123456789",
            "updateAvailable": true,
            "strategy": "optional",
        ]
        let data = try JSONSerialization.data(withJSONObject: payload)
        XCTAssertThrowsError(try IOSAppVersionPolicyClient.decodePolicy(data)) { error in
            XCTAssertEqual(error as? IOSAppVersionPolicyClient.ClientError, .invalidPolicy)
        }
    }

    func testWiringFailsClosedForDesktopOnlyUpdateControls() async throws {
        let expectedMetadata = metadata()
        let service = IOSAppStoreUpdateService(metadataProvider: { expectedMetadata })
        let wiring = IOSUpdateServiceWiring(service: service)

        let stable = await wiring.route(
            method: "setUpdateTrack",
            args: .object(["track": .string("stable")])
        )
        guard case .ok(let stablePayload)? = stable else {
            return XCTFail("stable App Store track should return current status")
        }
        let stableObject = try XCTUnwrap(stablePayload.foundationValue as? [String: Any])
        XCTAssertEqual(stableObject["currentTrack"] as? String, "stable")

        let nightly = await wiring.route(
            method: "setUpdateTrack",
            args: .object(["track": .string("nightly")])
        )
        guard case .failed(let nightlyFailure)? = nightly else {
            return XCTFail("nightly must fail closed on iOS")
        }
        XCTAssertEqual(nightlyFailure.code, "ios-update-track-managed-by-app-store")

        let idleApply = await wiring.route(
            method: "setAutoUpdateWhenIdleOptIn",
            args: .object(["enabled": .bool(true)])
        )
        guard case .failed(let idleFailure)? = idleApply else {
            return XCTFail("idle apply must fail closed on iOS")
        }
        XCTAssertEqual(idleFailure.code, "ios-update-idle-apply-managed-by-app-store")
    }
}
