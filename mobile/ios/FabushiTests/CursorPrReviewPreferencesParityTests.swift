import Foundation
import XCTest
@testable import Fabushi

private func prReviewCredentials() -> AccountMcpCredentials {
    AccountMcpCredentials(
        getAccessToken: { _ in "token" },
        getMachineId: { "machine-id" }
    )
}

private func prReviewResponse(_ request: URLRequest, body: Data) -> (Data, URLResponse) {
    (
        body,
        HTTPURLResponse(
            url: request.url!,
            statusCode: 200,
            httpVersion: "HTTP/2",
            headerFields: ["Content-Type": "application/proto"]
        )!
    )
}

private func prReviewVarint(_ field: UInt8, _ value: UInt8) -> Data {
    Data([(field << 3), value])
}

private func prReviewBytes(_ field: UInt8, _ body: Data) -> Data {
    Data([(field << 3) | 2, UInt8(body.count)]) + body
}

final class CursorPrReviewPreferencesParityTests: XCTestCase {
    func testFetchesUserAndTeamPreferencesWithCanonicalRoutesAndTenSecondTimeout() async throws {
        actor Seen {
            var requests: [URLRequest] = []
            func append(_ request: URLRequest) { requests.append(request) }
            func all() -> [URLRequest] { requests }
        }
        let seen = Seen()
        let client = IOSCursorDashboardClient(
            credentials: prReviewCredentials(),
            backendURL: URL(string: "https://backend.example.test")!,
            requestExecutor: { request in
                await seen.append(request)
                switch request.url?.path {
                case "/aiserver.v1.BackgroundComposerService/GetBackgroundComposerUserSettings":
                    XCTAssertEqual(request.timeoutInterval, 10)
                    XCTAssertEqual(request.httpBody, Data())
                    return prReviewResponse(request, body: prReviewVarint(6, 2))
                case "/aiserver.v1.DashboardService/GetTeamAdminSettingsOrEmptyIfNotInTeam":
                    XCTAssertEqual(request.timeoutInterval, 10)
                    XCTAssertEqual(request.httpBody, Data())
                    let pullRequestPreferences = prReviewVarint(1, 3)
                    return prReviewResponse(
                        request,
                        body: prReviewBytes(37, pullRequestPreferences)
                    )
                default:
                    XCTFail("Unexpected PR review preference RPC")
                    return prReviewResponse(request, body: Data())
                }
            }
        )

        let preferences = try await client.getPrReviewPreferences()
        XCTAssertEqual(preferences.user, .graphite)
        XCTAssertEqual(preferences.team, .reviewCursor)
        XCTAssertEqual(await seen.all().count, 2)
    }

    func testTeamPreferenceFallsBackToBackgroundAgentSettingsAndUnknownModesStayUnset() throws {
        let team = prReviewBytes(7, prReviewVarint(5, 1))
        XCTAssertEqual(
            try IOSCursorDashboardProtoForTests.decodeTeam(team),
            .github
        )
        XCTAssertNil(
            try IOSCursorDashboardProtoForTests.decodeUser(prReviewVarint(6, 9))
        )
    }
}

private enum IOSCursorDashboardProtoForTests {
    static func decodeUser(_ data: Data) throws -> SandPrReviewDestination? {
        try IOSCursorDashboardClient.decodePrReviewUserDestinationForTests(data)
    }

    static func decodeTeam(_ data: Data) throws -> SandPrReviewDestination? {
        try IOSCursorDashboardClient.decodePrReviewTeamDestinationForTests(data)
    }
}
