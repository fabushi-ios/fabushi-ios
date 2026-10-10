import XCTest
@testable import Fabushi

@MainActor
final class DeepLinkProjectionParityTests: XCTestCase {
    func testDesktopModelIntentFamilyProjectsToNativeRoutes() throws {
        let agent = try XCTUnwrap(FabushiDeepLinkParser.parse("fabushi://agent/agent-42"))
        XCTAssertEqual(agent.route, .agent(id: "agent-42"))
        XCTAssertEqual(agent.source, .customScheme)
        XCTAssertEqual(agent.canonicalURL.absoluteString, "fabushi://agent/agent-42")

        let plugin = try XCTUnwrap(
            FabushiDeepLinkParser.parse("https://fabushi.app/link/v1/plugin/add?id=global-dharma")
        )
        XCTAssertEqual(plugin.route, .pluginAdd(id: "global-dharma"))
        XCTAssertEqual(plugin.source, .universalLink)
        XCTAssertEqual(
            plugin.canonicalURL.absoluteString,
            "fabushi://app/v1/plugin/add?id=global-dharma"
        )

        let open = try XCTUnwrap(FabushiDeepLinkParser.parse("fabushi://app/v1/open"))
        XCTAssertEqual(open.route, .open)

        let info = try XCTUnwrap(
            FabushiDeepLinkParser.parse(
                "https://fabushi.app/link/v1/info?topic=deep-links"
            )
        )
        XCTAssertEqual(info.route, .info(topic: "deep-links"))
        XCTAssertEqual(
            info.canonicalURL.absoluteString,
            "fabushi://app/v1/info?topic=deep-links"
        )
    }

    func testParserFailsClosedForUntrustedOrAmbiguousCandidates() {
        XCTAssertNil(FabushiDeepLinkParser.parse("http://fabushi.app/link/v1/open"))
        XCTAssertNil(FabushiDeepLinkParser.parse("https://example.com/link/v1/open"))
        XCTAssertNil(FabushiDeepLinkParser.parse("fabushi://app/v1/open?extra=1"))
        XCTAssertNil(
            FabushiDeepLinkParser.parse(
                "fabushi://app/v1/plugin/add?id=global-dharma&id=duplicate"
            )
        )
        XCTAssertNil(FabushiDeepLinkParser.parse("fabushi://agent/../settings"))
        XCTAssertNil(FabushiDeepLinkParser.parse("fabushi://agent/%2e%2e"))
        XCTAssertNil(FabushiDeepLinkParser.parse("fabushi://app/v1/info?topic=other"))
    }

    func testControllerQueuesUntilRendererReadyThenPreservesOrder() {
        var dispatched: [FabushiDeepLinkRoute] = []
        var activationCount = 0
        let controller = IOSDeepLinkController(
            dispatch: { dispatched.append($0.route) },
            requestActivation: { activationCount += 1 }
        )

        XCTAssertTrue(controller.handleCandidate("fabushi://agent/agent-a", origin: "unit"))
        XCTAssertTrue(controller.handleCandidate("fabushi://app/v1/open", origin: "unit"))
        XCTAssertTrue(controller.hasPendingActivation)
        XCTAssertEqual(dispatched, [])
        XCTAssertEqual(activationCount, 2)

        controller.markReady()
        XCTAssertFalse(controller.hasPendingActivation)
        XCTAssertEqual(dispatched, [.agent(id: "agent-a"), .open])
    }

    func testControllerCanonicalDedupesAcrossSchemeFamiliesInsideWindow() {
        var now = Date(timeIntervalSince1970: 1_000)
        var dispatched: [FabushiDeepLinkRoute] = []
        let controller = IOSDeepLinkController(
            dispatch: { dispatched.append($0.route) },
            now: { now }
        )
        controller.markReady()

        XCTAssertTrue(controller.handleCandidate("fabushi://app/v1/open", origin: "scheme"))
        XCTAssertFalse(
            controller.handleCandidate(
                "https://fabushi.app/link/v1/open",
                origin: "universal"
            )
        )
        XCTAssertEqual(dispatched, [.open])

        now = now.addingTimeInterval(IOSDeepLinkController.dedupeWindow + 0.1)
        XCTAssertTrue(
            controller.handleCandidate(
                "https://fabushi.app/link/v1/open",
                origin: "universal"
            )
        )
        XCTAssertEqual(dispatched, [.open, .open])
    }

    func testControllerBoundsPreReadyQueueWithoutDroppingAcceptedEntries() {
        var dispatched: [FabushiDeepLinkRoute] = []
        let controller = IOSDeepLinkController(dispatch: { dispatched.append($0.route) })

        for index in 0..<IOSDeepLinkController.pendingLimit {
            XCTAssertTrue(
                controller.handleCandidate(
                    "fabushi://agent/agent-\(index)",
                    origin: "unit"
                )
            )
        }
        XCTAssertFalse(
            controller.handleCandidate("fabushi://agent/overflow", origin: "unit")
        )
        controller.markReady()
        XCTAssertEqual(dispatched.count, IOSDeepLinkController.pendingLimit)
        XCTAssertEqual(dispatched.first, .agent(id: "agent-0"))
        XCTAssertEqual(
            dispatched.last,
            .agent(id: "agent-\(IOSDeepLinkController.pendingLimit - 1)")
        )
    }
}
