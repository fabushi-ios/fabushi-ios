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
        for descriptor in FabushiRemoteDeviceGateway.toolDescriptors {
            let name = try XCTUnwrap(descriptor["name"] as? String)
            let annotations = try XCTUnwrap(descriptor["annotations"] as? [String: Any])
            let readOnly = name != "fabushi.app.action"
            XCTAssertEqual(annotations["readOnlyHint"] as? Bool, readOnly)
            XCTAssertEqual(annotations["destructiveHint"] as? Bool, !readOnly)
            XCTAssertEqual(annotations["idempotentHint"] as? Bool, readOnly)
            XCTAssertEqual(annotations["openWorldHint"] as? Bool, false)
            XCTAssertNotNil(descriptor["outputSchema"] as? [String: Any])
        }
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
        XCTAssertEqual(message["toolSchemaVersion"] as? String, "fabushi-app-agent-surface-v1")
        XCTAssertEqual((message["metadata"] as? [String: Any])?["owner"] as? String, "canonical-host")
        XCTAssertEqual(message["leaseSeconds"] as? Int, FabushiRemoteDeviceGateway.leaseSeconds)
    }

    @MainActor
    func testGatewayPreflightPolicyMatchesCurrentDesktopRegistry() {
        let readOnly: [String: Any] = [
            "impact": "none",
            "targets": [] as [Any],
        ]
        let writeShaped: [String: Any] = [
            "impact": "write",
            "targets": ["/tmp/file"],
        ]
        for toolName in FabushiRemoteDeviceGateway.readOnlyToolNames {
            XCTAssertTrue(FabushiRemoteDeviceGateway.acceptsDiskPreflight(toolName: toolName, value: nil))
            XCTAssertTrue(FabushiRemoteDeviceGateway.acceptsDiskPreflight(toolName: toolName, value: readOnly))
            XCTAssertFalse(FabushiRemoteDeviceGateway.acceptsDiskPreflight(toolName: toolName, value: writeShaped))
        }
        XCTAssertFalse(FabushiRemoteDeviceGateway.acceptsDiskPreflight(toolName: "fabushi.app.action", value: nil))
        XCTAssertTrue(FabushiRemoteDeviceGateway.acceptsDiskPreflight(toolName: "fabushi.app.action", value: readOnly))
        XCTAssertFalse(FabushiRemoteDeviceGateway.acceptsDiskPreflight(toolName: "fabushi.app.action", value: writeShaped))
        XCTAssertFalse(FabushiRemoteDeviceGateway.acceptsDiskPreflight(toolName: "fabushi.app.unknown", value: readOnly))
    }

    @MainActor
    func testGatewayArgumentsJsonTakesPrecedenceAndRejectsNonObjectPayloads() throws {
        let decoded = try FabushiRemoteDeviceGateway.decodeCallArguments([
            "argumentsJson": #"{"limit":3,"name":"json"}"#,
            "arguments": ["limit": 99, "name": "object"],
        ])
        XCTAssertEqual((decoded["limit"] as? NSNumber)?.intValue, 3)
        XCTAssertEqual(decoded["name"] as? String, "json")

        let direct = try FabushiRemoteDeviceGateway.decodeCallArguments([
            "arguments": ["limit": 99, "name": "object"],
        ])
        XCTAssertEqual(direct["limit"] as? Int, 99)
        XCTAssertEqual(direct["name"] as? String, "object")

        XCTAssertThrowsError(try FabushiRemoteDeviceGateway.decodeCallArguments(["argumentsJson": "[]"]))
        XCTAssertThrowsError(try FabushiRemoteDeviceGateway.decodeCallArguments(["argumentsJson": "{"]))
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


    @MainActor
    func testGatewaySnapshotHonorsIncludeTextAndNeverProjectsSensitiveText() throws {
        let surface = FabushiAppAgentSurface()
        let snapshot = try surface.publish(
            screen: "profile",
            elements: [
                .init(
                    agentId: "profile.visible",
                    role: "text",
                    name: "Visible label",
                    text: "Visible body text"
                ),
                .init(
                    agentId: "profile.secret",
                    role: "secure-textbox",
                    name: "Password",
                    text: "secret body must never leave the surface",
                    sensitive: true
                ),
            ]
        )

        let withoutText = FabushiRemoteDeviceGateway.snapshotDictionary(
            snapshot,
            maxElements: 500,
            includeText: false
        )
        let withoutTextElements = try XCTUnwrap(withoutText["elements"] as? [[String: Any]])
        XCTAssertNil(withoutTextElements[0]["text"])
        XCTAssertNil(withoutTextElements[1]["text"])
        XCTAssertEqual(withoutText["truncated"] as? Bool, false)

        let withText = FabushiRemoteDeviceGateway.snapshotDictionary(
            snapshot,
            maxElements: 500,
            includeText: true
        )
        let withTextElements = try XCTUnwrap(withText["elements"] as? [[String: Any]])
        XCTAssertEqual(withTextElements[0]["text"] as? String, "Visible body text")
        XCTAssertNil(withTextElements[1]["text"])
        XCTAssertEqual(withText["truncated"] as? Bool, false)

        let bounded = FabushiRemoteDeviceGateway.snapshotDictionary(
            snapshot,
            maxElements: 1,
            includeText: true
        )
        XCTAssertEqual(bounded["truncated"] as? Bool, true)
    }

}
