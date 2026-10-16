import XCTest
@testable import Fabushi

final class FabushiTests: XCTestCase {
    func testMarketplacePluginIdentityIsStable() {
        let plugin = MarketplacePlugin(
            pluginId: "example.plugin",
            displayName: "示例插件",
            description: "描述",
            latestVersion: "1.0.0",
            tools: []
        )
        XCTAssertEqual(plugin.id, "example.plugin")
        XCTAssertEqual(plugin.latestVersion, "1.0.0")
    }

    @MainActor
    func testAppAgentSurfaceUsesStableToolsAndRejectsStaleSensitiveWrites() throws {
        XCTAssertEqual(FabushiAppAgentSurface.toolNames, [
            "fabushi.app.status",
            "fabushi.app.snapshot",
            "fabushi.app.find",
            "fabushi.app.action",
            "fabushi.app.wait",
            "fabushi.app.assert",
        ])
        let surface = FabushiAppAgentSurface(appId: "fabushi.ios.test")
        var invoked = false
        let snapshot = try surface.publish(
            screen: "login",
            elements: [
                .init(agentId: "login-submit", role: "button", name: "登录"),
                .init(agentId: "login-password", role: "textbox", name: "密码", sensitive: true),
            ],
            actions: [
                "login-submit": .init(allowed: ["invoke"]) { _ in invoked = true },
                "login-password": .init(allowed: ["setValue"]) { _ in },
            ]
        )
        XCTAssertTrue(surface.status().available)
        XCTAssertEqual(surface.find(role: "button").map(\.agentId), ["login-submit"])
        let after = try surface.perform(expectedGeneration: snapshot.generation, agentId: "login-submit", action: "invoke")
        XCTAssertTrue(invoked)
        XCTAssertGreaterThan(after.generation, snapshot.generation)
        XCTAssertThrowsError(try surface.perform(
            expectedGeneration: snapshot.generation,
            agentId: "login-submit",
            action: "invoke"
        ))
        XCTAssertThrowsError(try surface.perform(
            expectedGeneration: after.generation,
            agentId: "login-password",
            action: "setValue",
            value: "secret"
        ))
        XCTAssertTrue(surface.assertState(screen: "login", agentId: "login-submit").passed)
    }

    @MainActor
    func testAppAgentSurfaceBoundsOversizedScreensWithoutBecomingUnavailable() throws {
        let surface = FabushiAppAgentSurface(appId: "fabushi.ios.test")
        let elements = (0..<505).map { index in
            FabushiAppAgentSurface.Element(
                agentId: "row-\(index)",
                role: "button",
                name: "Row \(index)"
            )
        }
        var retainedInvoked = false
        var droppedInvoked = false
        let snapshot = try surface.publish(
            screen: "large-list",
            elements: elements,
            actions: [
                "row-0": .init(allowed: ["invoke"]) { _ in retainedInvoked = true },
                "row-504": .init(allowed: ["invoke"]) { _ in droppedInvoked = true },
            ]
        )

        XCTAssertTrue(surface.status().available)
        XCTAssertEqual(snapshot.elements.count, FabushiAppAgentSurface.maximumElementCount)
        XCTAssertEqual(snapshot.elements.last?.agentId, FabushiAppAgentSurface.truncationAgentId)
        XCTAssertEqual(snapshot.elements.last?.role, "status")
        XCTAssertEqual(snapshot.elements.last?.enabled, false)
        XCTAssertNil(snapshot.elements.first(where: { $0.agentId == "row-504" }))

        _ = try surface.perform(
            expectedGeneration: snapshot.generation,
            agentId: "row-0",
            action: "invoke"
        )
        XCTAssertTrue(retainedInvoked)
        XCTAssertFalse(droppedInvoked)
    }

    @MainActor
    func testAppAgentSurfaceReservesItsSyntheticTruncationIdentifier() {
        let surface = FabushiAppAgentSurface(appId: "fabushi.ios.test")
        XCTAssertThrowsError(try surface.publish(
            screen: "caller-owned-marker",
            elements: [
                .init(
                    agentId: FabushiAppAgentSurface.truncationAgentId,
                    role: "button",
                    name: "Caller marker"
                ),
            ],
            actions: [
                FabushiAppAgentSurface.truncationAgentId: .init(allowed: ["invoke"]) { _ in },
            ]
        )) { error in
            XCTAssertEqual(error as? FabushiAppAgentSurface.SurfaceError, .invalidElement)
        }
    }
}
