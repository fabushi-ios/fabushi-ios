import XCTest
@testable import Fabushi

final class SharedMarketplaceClientParityTests: XCTestCase {
    override func setUp() {
        super.setUp()
        CursorMarketplaceLogoRegistry.shared.resetForTesting()
    }

    func testLogoRegistryRemembersOnlyExplicitUrls() {
        XCTAssertFalse(isKnownPluginLogoUrl("https://cdn.example/a.png"))
        rememberPluginLogoUrl("https://cdn.example/a.png")
        XCTAssertTrue(isKnownPluginLogoUrl("https://cdn.example/a.png"))
        XCTAssertFalse(isKnownPluginLogoUrl("https://cdn.example/b.png"))
    }

    func testBestEffortTokenDropsEmptyAndErrors() async {
        let token = await bestEffortToken { "abc" }
        XCTAssertEqual(token, "abc")
        let empty = await bestEffortToken { "" }
        XCTAssertNil(empty)
        let failed = await bestEffortToken { throw URLError(.notConnectedToInternet) }
        XCTAssertNil(failed)
    }

    func testMarketplaceContextAppliesGhostRequestIdentityChecksumAndAuthorization() async throws {
        let env = [
            "SAND_CLIENT_APP_VERSION": "1.2.3",
            "SAND_PACKAGED": "1",
            "CURSOR_API_BASE_URL": "https://api.example.test/",
        ]
        let context = await createMarketplaceRequestContext(
            getAccessToken: { "token-1" },
            getMachineId: { "machine-1" },
            env: env,
            uuid: { "request-1" }
        )
        let resolved = try XCTUnwrap(context)
        XCTAssertEqual(resolved.headers["x-ghost-mode"], "true")
        XCTAssertEqual(resolved.headers["x-request-id"], "request-1")
        XCTAssertEqual(resolved.headers["authorization"], "Bearer token-1")
        XCTAssertTrue(resolved.headers["x-cursor-checksum"]?.hasSuffix("machine-1") == true)
        XCTAssertEqual(resolved.timeoutSeconds, 12)
    }

    func testDashboardClientBuildsPostRequestWithoutDesktopConnectTransport() async throws {
        let client = await createDashboardClient(
            getAccessToken: { nil },
            env: [
                "CURSOR_API_BASE_URL": "https://api.example.test/",
                "SAND_CLIENT_APP_VERSION": "1.0.0",
                "SAND_PACKAGED": "1",
            ],
            uuid: { "request-2" }
        )
        let request = try XCTUnwrap(client?.makeRequest(
            path: "aiserver.v1.DashboardService/ListMarketplacePlugins",
            body: Data([1, 2, 3])
        ))
        XCTAssertEqual(request.httpMethod, "POST")
        XCTAssertEqual(request.timeoutInterval, 12)
        XCTAssertEqual(request.value(forHTTPHeaderField: "x-ghost-mode"), "true")
        XCTAssertNil(request.value(forHTTPHeaderField: "authorization"))
    }
}
