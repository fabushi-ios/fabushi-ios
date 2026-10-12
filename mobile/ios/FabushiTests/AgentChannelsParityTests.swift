import XCTest
@testable import Fabushi

final class AgentChannelsParityTests: XCTestCase {
    func testConnectionDecoderIsStrictAndNeverRequiresCredentialPayload() throws {
        let rows = try MobileAgentChannelsModel.decodeConnections([
            [
                "platform": "slack",
                "label": "Slack",
                "status": "connected",
                "detail": "ready",
            ],
        ])
        XCTAssertEqual(rows.count, 1)
        XCTAssertEqual(rows[0].platform, "slack")
        XCTAssertEqual(rows[0].label, "Slack")
        XCTAssertEqual(rows[0].status, "connected")
        XCTAssertEqual(rows[0].detail, "ready")

        XCTAssertThrowsError(try MobileAgentChannelsModel.decodeConnections([
            ["platform": "slack", "label": "Slack"],
        ]))
        XCTAssertThrowsError(try MobileAgentChannelsModel.decodeConnections([
            ["platform": "slack", "label": "Slack", "status": "connected", "detail": NSNull()],
        ]))
    }

    func testRowStatusPriorityMatchesDesktopController() {
        let available = ConnectorManifest(
            platform: "test",
            displayName: "Test",
            blurb: "Test connector",
            credentialLabel: "token",
            availability: .available,
            connectGuide: ""
        )
        let comingSoon = ConnectorManifest(
            platform: "test",
            displayName: "Test",
            blurb: "Test connector",
            credentialLabel: "token",
            availability: .comingSoon,
            connectGuide: ""
        )
        let configured = try! MobileAgentChannelConnection(json: [
            "platform": "test",
            "label": "Account",
            "status": "configured",
        ])
        let connected = try! MobileAgentChannelConnection(json: [
            "platform": "test",
            "label": "Account",
            "status": "connected",
        ])
        let failed = try! MobileAgentChannelConnection(json: [
            "platform": "test",
            "label": "Account",
            "status": "error",
        ])

        XCTAssertEqual(
            MobileAgentChannelsModel.rowStatus(manifest: comingSoon, connection: connected),
            .comingSoon
        )
        XCTAssertEqual(
            MobileAgentChannelsModel.rowStatus(manifest: available, connection: connected),
            .connected
        )
        XCTAssertEqual(
            MobileAgentChannelsModel.rowStatus(manifest: available, connection: failed),
            .error
        )
        XCTAssertEqual(
            MobileAgentChannelsModel.rowStatus(manifest: available, connection: configured),
            .connecting
        )
        XCTAssertEqual(
            MobileAgentChannelsModel.rowStatus(manifest: available, connection: nil),
            .available
        )
    }

    func testSharedDesktopManifestsRemainFailClosedUntilConnectorShips() {
        XCTAssertEqual(CONNECTOR_MANIFESTS.map(\.platform), ["discord", "slack"])
        XCTAssertTrue(CONNECTOR_MANIFESTS.allSatisfy { $0.availability == .comingSoon })
    }
}
