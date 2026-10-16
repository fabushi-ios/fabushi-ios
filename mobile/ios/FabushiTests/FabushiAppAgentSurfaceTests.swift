import XCTest
@testable import Fabushi

final class FabushiAppAgentSurfaceTests: XCTestCase {
    @MainActor
    func testSurfacePublishesBoundedGenerationSafeSnapshot() throws {
        let surface = FabushiAppAgentSurface()
        let first = try surface.publish(
            screen: "home",
            elements: [
                .init(agentId: "home.open", role: "button", name: "Open"),
                .init(
                    agentId: "account.secret",
                    role: "secure-textbox",
                    name: "Credential",
                    sensitive: true,
                    valuePresent: true,
                    valueLength: 12
                ),
            ]
        )
        XCTAssertEqual(first.version, 1)
        XCTAssertEqual(first.appId, "fabushi.ios")
        XCTAssertEqual(first.platform, "ios")
        XCTAssertEqual(first.screen, "home")
        XCTAssertEqual(first.elements.count, 2)
        XCTAssertTrue(first.elements[1].sensitive)
        XCTAssertEqual(first.elements[1].valuePresent, true)
        XCTAssertEqual(first.elements[1].valueLength, 12)

        let second = try surface.publish(
            screen: "details",
            elements: [.init(agentId: "details.close", role: "button", name: "Close")]
        )
        XCTAssertGreaterThan(second.generation, first.generation)
        XCTAssertEqual(surface.status().generation, second.generation)
    }

    @MainActor
    func testSurfaceRejectsStaleOrUnauthorizedActions() throws {
        let surface = FabushiAppAgentSurface()
        var invokedValue: String?
        let first = try surface.publish(
            screen: "compose",
            elements: [.init(agentId: "composer", role: "textbox", name: "Message")],
            actions: [
                "composer": .init(allowed: ["setValue"]) { value in
                    invokedValue = value
                },
            ]
        )
        let completed = try surface.perform(
            expectedGeneration: first.generation,
            agentId: "composer",
            action: "setValue",
            value: "hello"
        )
        XCTAssertEqual(invokedValue, "hello")
        XCTAssertGreaterThan(completed.generation, first.generation)

        XCTAssertThrowsError(
            try surface.perform(
                expectedGeneration: first.generation,
                agentId: "composer",
                action: "setValue",
                value: "stale"
            )
        ) { error in
            XCTAssertEqual(error as? FabushiAppAgentSurface.SurfaceError, .staleGeneration)
        }

        XCTAssertThrowsError(
            try surface.perform(
                expectedGeneration: completed.generation,
                agentId: "composer",
                action: "invoke"
            )
        ) { error in
            XCTAssertEqual(error as? FabushiAppAgentSurface.SurfaceError, .unsupportedAction)
        }
    }

    @MainActor
    func testSurfaceNeverWritesSensitiveValuesThroughSemanticAction() throws {
        let surface = FabushiAppAgentSurface()
        let snapshot = try surface.publish(
            screen: "login",
            elements: [
                .init(agentId: "password", role: "secure-textbox", name: "Password", sensitive: true),
            ],
            actions: [
                "password": .init(allowed: ["setValue"]) { _ in
                    XCTFail("Sensitive semantic action must fail before invocation")
                },
            ]
        )
        XCTAssertThrowsError(
            try surface.perform(
                expectedGeneration: snapshot.generation,
                agentId: "password",
                action: "setValue",
                value: "secret"
            )
        ) { error in
            XCTAssertEqual(
                error as? FabushiAppAgentSurface.SurfaceError,
                .sensitiveInputRequiresSecureInput
            )
        }
    }

    @MainActor
    func testSurfaceBoundsLargeSnapshotsAndSupportsDeterministicFindAssert() throws {
        let surface = FabushiAppAgentSurface()
        let elements = (0...FabushiAppAgentSurface.maximumElementCount).map { index in
            FabushiAppAgentSurface.Element(
                agentId: "item.\(index)",
                role: "button",
                name: "Item \(index)"
            )
        }
        let snapshot = try surface.publish(screen: "list", elements: elements)
        XCTAssertEqual(snapshot.elements.count, FabushiAppAgentSurface.maximumElementCount)
        XCTAssertEqual(snapshot.elements.last?.agentId, FabushiAppAgentSurface.truncationAgentId)
        XCTAssertEqual(surface.find(agentId: "item.42", limit: 1).first?.name, "Item 42")
        XCTAssertTrue(surface.assertState(agentId: "item.42", state: "visible").passed)
        XCTAssertTrue(surface.assertState(agentId: "missing", state: "absent").passed)
    }
}
