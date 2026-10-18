import XCTest
@testable import Fabushi

final class FabushiRemoteDeviceGatewayTests: XCTestCase {
    @MainActor
    func testGatewayRedactsBearerMaterialFromNestedStructuredResult() throws {
        let accessToken = String(repeating: "s", count: 64)
        let raw: [String: Any] = [
            "tokenEcho": "prefix \(accessToken) suffix",
            "nested": ["value": accessToken],
            "array": [accessToken, ["deeper": accessToken]],
            accessToken: "token-as-key",
        ]

        let safe = try FabushiRemoteDeviceGateway.boundedResult(raw, accessToken: accessToken)
        let encoded = try JSONSerialization.data(withJSONObject: safe, options: [.sortedKeys])
        let text = try XCTUnwrap(String(data: encoded, encoding: .utf8))

        XCTAssertFalse(text.contains(accessToken))
        XCTAssertEqual(safe["tokenEcho"] as? String, "prefix [redacted] suffix")
        XCTAssertEqual(safe["[redacted]"] as? String, "token-as-key")
        XCTAssertEqual((safe["nested"] as? [String: Any])?["value"] as? String, "[redacted]")
        XCTAssertEqual((safe["array"] as? [Any])?.first as? String, "[redacted]")
    }

    @MainActor
    func testGatewayRejectsOversizedStructuredResultFailClosed() {
        let accessToken = String(repeating: "t", count: 64)
        let raw: [String: Any] = [
            "text": String(repeating: "x", count: FabushiRemoteDeviceGateway.maximumResultBytes),
        ]

        XCTAssertThrowsError(try FabushiRemoteDeviceGateway.boundedResult(raw, accessToken: accessToken)) { error in
            XCTAssertEqual(error as? FabushiRemoteDeviceGateway.GatewayError, .resultTooLarge)
        }
    }

    @MainActor
    func testGatewayPublishesOnlyCanonicalSemanticAppTools() throws {
        XCTAssertEqual(
            FabushiRemoteDeviceGateway.toolDescriptors.compactMap { $0["name"] as? String },
            FabushiAppAgentSurface.toolNames
        )
        XCTAssertEqual(FabushiRemoteDeviceGateway.officialGatewayURL.scheme, "wss")
        XCTAssertEqual(FabushiRemoteDeviceGateway.officialGatewayURL.host, "fabushi-mcp.ombhrum.com")
        XCTAssertEqual(FabushiRemoteDeviceGateway.officialGatewayURL.path, "/agent")
        XCTAssertTrue(JSONSerialization.isValidJSONObject(FabushiRemoteDeviceGateway.toolDescriptors))
    }

    @MainActor
    func testGatewayAcceptsBoundedRefreshTokenFreeAppSessionShape() throws {
        let session = try FabushiRemoteDeviceGateway.parseAgentSession([
            "accessToken": String(repeating: "a", count: 64),
            "accessTokenExpiresAt": 2_000_000_000_000,
            "sessionId": "ci-runner:12345:1",
            "deviceId": "gha-12345-1-interactive",
            "username": "fabushi_mcp_ci_test",
            "user": ["id": "42", "username": "fabushi_mcp_ci_test"],
            "provider": "github-actions",
            "ciRunner": true,
        ], nowMilliseconds: 1_900_000_000_000)
        XCTAssertEqual(session.deviceId, "gha-12345-1-interactive")
        XCTAssertEqual(session.sessionId, "ci-runner:12345:1")
        XCTAssertEqual(session.username, "fabushi_mcp_ci_test")
    }

    @MainActor
    func testGatewayRejectsInvalidCredentialsAndDeviceIds() {
        XCTAssertThrowsError(try FabushiRemoteDeviceGateway.parseAgentSession([
            "accessToken": "short",
            "sessionId": "ci-runner:1:1",
            "deviceId": "gha-1-1-interactive",
        ]))
        XCTAssertThrowsError(try FabushiRemoteDeviceGateway.parseAgentSession([
            "accessToken": String(repeating: "a", count: 64),
            "sessionId": "ci-runner:1:1",
            "deviceId": "bad device id",
        ]))
    }
    @MainActor
    func testGatewayRejectsExpiredOrNearlyExpiredAgentSession() {
        let base: [String: Any] = [
            "accessToken": String(repeating: "a", count: 64),
            "sessionId": "ci-runner:1:1",
            "deviceId": "gha-1-1-interactive",
        ]
        var expired = base
        expired["accessTokenExpiresAt"] = 1_999_999_999_999 as Int64
        XCTAssertThrowsError(
            try FabushiRemoteDeviceGateway.parseAgentSession(expired, nowMilliseconds: 2_000_000_000_000)
        )

        var withinLeeway = base
        withinLeeway["accessTokenExpiresAt"] = 2_000_000_020_000 as Int64
        XCTAssertThrowsError(
            try FabushiRemoteDeviceGateway.parseAgentSession(withinLeeway, nowMilliseconds: 2_000_000_000_000)
        )
    }

    @MainActor
    func testGatewayRejectsSecondEpochExpiryRegressionFromRustHost() {
        XCTAssertThrowsError(
            try FabushiRemoteDeviceGateway.parseAgentSession([
                "accessToken": String(repeating: "c", count: 64),
                "accessTokenExpiresAt": 2_000_000_120 as Int64,
                "sessionId": "session-generation-42",
                "deviceId": "ios-device-42",
                "username": "device@example.com",
            ], nowMilliseconds: 2_000_000_000_000)
        )
    }

    @MainActor
    func testRegistrationFenceRejectsTokenDeviceOrSessionIdentityChanges() {
        let original = FabushiRemoteDeviceGateway.AgentSession(
            accessToken: String(repeating: "a", count: 64),
            deviceId: "ios-device-a",
            sessionId: "session-a",
            username: "device@example.com",
            accessTokenExpiresAt: 2_000_000_120_000
        )
        let tokenChanged = FabushiRemoteDeviceGateway.AgentSession(
            accessToken: String(repeating: "b", count: 64),
            deviceId: original.deviceId,
            sessionId: original.sessionId,
            username: original.username,
            accessTokenExpiresAt: original.accessTokenExpiresAt
        )
        let deviceChanged = FabushiRemoteDeviceGateway.AgentSession(
            accessToken: original.accessToken,
            deviceId: "ios-device-b",
            sessionId: original.sessionId,
            username: original.username,
            accessTokenExpiresAt: original.accessTokenExpiresAt
        )
        let sessionChanged = FabushiRemoteDeviceGateway.AgentSession(
            accessToken: original.accessToken,
            deviceId: original.deviceId,
            sessionId: "session-b",
            username: original.username,
            accessTokenExpiresAt: original.accessTokenExpiresAt
        )

        let identity = FabushiRemoteDeviceGateway.sessionIdentity(original)
        XCTAssertEqual(identity, FabushiRemoteDeviceGateway.sessionIdentity(original))
        XCTAssertNotEqual(identity, FabushiRemoteDeviceGateway.sessionIdentity(tokenChanged))
        XCTAssertNotEqual(identity, FabushiRemoteDeviceGateway.sessionIdentity(deviceChanged))
        XCTAssertNotEqual(identity, FabushiRemoteDeviceGateway.sessionIdentity(sessionChanged))
    }

    @MainActor
    func testOfficialRegistrationBindsSessionGenerationSchemaAndCanonicalOwner() throws {
        let session = try FabushiRemoteDeviceGateway.parseAgentSession([
            "accessToken": String(repeating: "b", count: 64),
            "accessTokenExpiresAt": 2_000_000_120_000 as Int64,
            "sessionId": "session-generation-42",
            "deviceId": "ios-device-42",
            "username": "fabushi_mcp_ci_test",
        ], nowMilliseconds: 2_000_000_000_000)
        let message = FabushiRemoteDeviceGateway.registrationMessage(session)
        XCTAssertEqual(message["generation"] as? String, "session-generation-42")
        XCTAssertEqual(message["toolSchemaVersion"] as? String, "fabushi-app-surface-v1")
        XCTAssertEqual((message["metadata"] as? [String: Any])?["owner"] as? String, "canonical-host")
        XCTAssertEqual(message["leaseSeconds"] as? Int, FabushiRemoteDeviceGateway.leaseSeconds)
    }

    @MainActor
    func testStatusToolRequiresReadOnlyDiskPreflight() {
        XCTAssertTrue(FabushiRemoteDeviceGateway.isReadOnlyDiskPreflight([
            "impact": "none",
            "targets": [] as [Any],
        ]))
        XCTAssertFalse(FabushiRemoteDeviceGateway.isReadOnlyDiskPreflight([
            "impact": "write",
            "targets": [] as [Any],
        ]))
        XCTAssertFalse(FabushiRemoteDeviceGateway.isReadOnlyDiskPreflight([
            "impact": "none",
            "targets": ["/tmp/file"],
        ]))
        XCTAssertFalse(FabushiRemoteDeviceGateway.isReadOnlyDiskPreflight(nil))
    }

    @MainActor
    func testGatewayNormalizesAndValidatesVoIPRegistrationToken() {
        let suiteName = "FabushiRemoteDeviceGatewayTests.voip.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suiteName)!
        defer { defaults.removePersistentDomain(forName: suiteName) }

        XCTAssertNil(FabushiRemoteDeviceGateway.currentVoIPToken(defaults: defaults))

        defaults.set("  AABBCCDDEEFF00112233445566778899  ", forKey: HumanCallSystemCoordinator.voIPTokenDefaultsKey)
        XCTAssertEqual(
            FabushiRemoteDeviceGateway.currentVoIPToken(defaults: defaults),
            "aabbccddeeff00112233445566778899"
        )

        defaults.set("not-a-hex-token", forKey: HumanCallSystemCoordinator.voIPTokenDefaultsKey)
        XCTAssertNil(FabushiRemoteDeviceGateway.currentVoIPToken(defaults: defaults))
    }

}
