import XCTest
@testable import Fabushi

final class FabushiRemoteDeviceGatewayTests: XCTestCase {
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
            "accessTokenExpiresAt": 2_000_000_000,
            "sessionId": "ci-runner:12345:1",
            "deviceId": "gha-12345-1-interactive",
            "username": "fabushi_mcp_ci_test",
            "user": ["id": "42", "username": "fabushi_mcp_ci_test"],
            "provider": "github-actions",
            "ciRunner": true,
        ])
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
}
