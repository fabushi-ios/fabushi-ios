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

    func testOverlappingChecksFenceStalePolicyResults() async throws {
        let expectedMetadata = metadata()
        let olderPolicy = policy(latestVersion: "1.2.66", latestBuildNumber: 35)
        let newerPolicy = policy(latestVersion: "1.2.67", latestBuildNumber: 36)
        var continuations: [CheckedContinuation<IOSAppVersionPolicy, Error>] = []
        let service = IOSAppStoreUpdateService(
            metadataProvider: { expectedMetadata },
            policyFetcher: { _ in
                try await withCheckedThrowingContinuation { continuation in
                    continuations.append(continuation)
                }
            }
        )

        let first = Task { try await service.checkForUpdates() }
        while continuations.count < 1 { await Task.yield() }
        let second = Task { try await service.checkForUpdates() }
        while continuations.count < 2 { await Task.yield() }

        continuations[1].resume(returning: newerPolicy)
        _ = try await second.value
        continuations[0].resume(returning: olderPolicy)
        _ = try await first.value

        guard case .ready(let authoritative) = service.policyLoadState else {
            return XCTFail("newest refresh must remain authoritative")
        }
        XCTAssertEqual(authoritative.latestVersion, "1.2.67")
        XCTAssertEqual(authoritative.latestBuildNumber, 36)
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
    func testUpdateWiringEmitsRealCheckAndStoreManagedTelemetry() async throws {
        let expectedMetadata = metadata()
        let offeredPolicy = policy()
        let service = IOSAppStoreUpdateService(
            metadataProvider: { expectedMetadata },
            policyFetcher: { _ in offeredPolicy }
        )
        var projections: [IOSUpdateTelemetryProjection] = []
        let wiring = IOSUpdateServiceWiring(
            service: service,
            telemetry: { projections.append($0) }
        )

        _ = await wiring.route(method: "checkForUpdates", args: .object([:]))
        _ = await wiring.route(method: "quitAndInstallUpdate", args: .object([:]))

        XCTAssertEqual(projections.count, 2)
        XCTAssertEqual(projections[0].level, "info")
        XCTAssertEqual(projections[0].metadata["action"], "check")
        XCTAssertEqual(projections[0].metadata["trigger"], "rpc")
        XCTAssertEqual(projections[0].metadata["result"], "available")
        XCTAssertEqual(projections[0].metadata["direction"], "upgrade")
        XCTAssertEqual(projections[0].metadata["target_version"], "1.2.66")
        XCTAssertNil(projections[0].metadata["error_class"])
        XCTAssertEqual(projections[1], IOSUpdateTelemetry.storeManaged(
            action: "quitAndInstallUpdate",
            accepted: false
        ))
    }

    func testUpdateCheckTelemetryUsesPrivacySafeErrorClass() {
        let projection = IOSUpdateTelemetry.check(
            trigger: "runtime_refresh",
            result: "error",
            currentVersion: "1.2.65",
            targetVersion: nil,
            errorClass: "policy_fetch_failed"
        )
        XCTAssertEqual(projection.level, "warn")
        XCTAssertEqual(projection.metadata["result"], "error")
        XCTAssertEqual(projection.metadata["direction"], "unknown")
        XCTAssertEqual(projection.metadata["error_class"], "policy_fetch_failed")
    }



    func testBackendUpdateRequirementImmediatelyProjectsMinimumVersionGate() throws {
        let service = IOSAppStoreUpdateService(
            metadataProvider: { self.metadata() }
        )

        XCTAssertTrue(service.noteBackendUpdateRequirement(true))
        let required = try service.statusPayload()
        let requiredObject = try XCTUnwrap(required.foundationValue as? [String: Any])
        XCTAssertEqual(requiredObject["isBelowMinimumVersion"] as? Bool, true)

        XCTAssertFalse(service.noteBackendUpdateRequirement(true))
        XCTAssertTrue(service.noteBackendUpdateRequirement(false))
        let cleared = try service.statusPayload()
        let clearedObject = try XCTUnwrap(cleared.foundationValue as? [String: Any])
        XCTAssertEqual(clearedObject["isBelowMinimumVersion"] as? Bool, false)
    }

}
