import Foundation
import XCTest
@testable import Fabushi

private func accessTestCredentials() -> AccountMcpCredentials {
    AccountMcpCredentials(
        getAccessToken: { _ in "access-token" },
        getMachineId: { "access-machine" }
    )
}

private func accessJSONResponse(
    _ request: URLRequest,
    _ object: [String: Any]
) throws -> (Data, URLResponse) {
    (
        try JSONSerialization.data(withJSONObject: object),
        HTTPURLResponse(
            url: request.url!,
            statusCode: 200,
            httpVersion: "HTTP/2",
            headerFields: ["Content-Type": "application/json"]
        )!
    )
}

@MainActor
final class CursorAccessParityTests: XCTestCase {
    func testWireProjectionMatchesDesktopEnums() {
        XCTAssertEqual(iosSandAccessFromWire(state: 1, blockReason: 1), .init(state: "granted", reason: "none"))
        XCTAssertEqual(iosSandAccessFromWire(state: 2, blockReason: 2), .init(state: "unavailable", reason: "teamPrivacyMode"))
        XCTAssertEqual(iosSandAccessFromWire(state: 3, blockReason: 7), .init(state: "paymentRequired", reason: "paywallIndividual"))
        XCTAssertEqual(iosSandAccessFromWire(state: 999, blockReason: 999), .init(state: "unknown", reason: "unspecified"))
    }

    func testDashboardAccessUsesCanonicalRPCAndTenSecondDeadline() async throws {
        let client = IOSCursorDashboardClient(
            credentials: accessTestCredentials(),
            backendURL: URL(string: "https://backend.example.test")!,
            requestExecutor: { request in
                XCTAssertEqual(
                    request.url?.path,
                    "/aiserver.v1.DashboardService/GetSandAccessStatus"
                )
                XCTAssertEqual(request.timeoutInterval, 10)
                return try accessJSONResponse(
                    request,
                    ["state": 3, "blockReason": 8]
                )
            }
        )
        let access = try await client.getSandAccessStatus()
        XCTAssertEqual(access, .init(state: "paymentRequired", reason: "paywallTeamMember"))
    }

    func testReadOncePreservesLoggingAndSettledAuthSlotSemantics() async {
        var accessCalls = 0
        let loggingIn = IOSCursorSandAccessReader(
            getAuthState: { .loggingIn },
            readAccess: {
                accessCalls += 1
                return .init(state: "granted", reason: "none")
            }
        )
        XCTAssertEqual(await loggingIn.readFresh(), SAND_ACCESS_CHECKING)
        XCTAssertEqual(accessCalls, 0)

        let loggedOut = IOSCursorSandAccessReader(
            getAuthState: { .loggedOut },
            readAccess: {
                accessCalls += 1
                return .init(state: "granted", reason: "none")
            }
        )
        XCTAssertEqual(await loggedOut.readFresh(), SAND_ACCESS_UNKNOWN)
        XCTAssertEqual(accessCalls, 0)

        let missingIdentity = IOSCursorSandAccessReader(
            getAuthState: { .loggedIn(authId: "", email: "fallback@example.test") },
            readAccess: {
                accessCalls += 1
                return .init(state: "granted", reason: "none")
            }
        )
        XCTAssertEqual(await missingIdentity.readFresh(), SAND_ACCESS_CHECKING)
        XCTAssertEqual(accessCalls, 0)

        let authIdWins = IOSCursorSandAccessReader(
            getAuthState: { .loggedIn(authId: "auth-slot", email: "email-slot") },
            readAccess: { .init(state: "granted", reason: "none") }
        )
        let snapshot = await authIdWins.readFreshSnapshot()
        XCTAssertEqual(snapshot.identity, "auth-slot")
        XCTAssertEqual(snapshot.access, .init(state: "granted", reason: "none"))
    }

    func testBackendFailureProjectsUnknownForCurrentIdentity() async {
        enum Failure: Error { case unavailable }
        let reader = IOSCursorSandAccessReader(
            getAuthState: { .loggedIn(authId: nil, email: "email-slot") },
            readAccess: { throw Failure.unavailable }
        )
        let snapshot = await reader.readFreshSnapshot()
        XCTAssertEqual(snapshot.identity, "email-slot")
        XCTAssertEqual(snapshot.access, SAND_ACCESS_UNKNOWN)
    }

    func testReaderGenerationPreventsOlderOverlapFromOverwritingNewerAnswer() async {
        var call = 0
        let reader = IOSCursorSandAccessReader(
            getAuthState: { .loggedIn(authId: "account-a", email: nil) },
            readAccess: {
                call += 1
                if call == 1 {
                    try await Task.sleep(nanoseconds: 80_000_000)
                    return .init(state: "unavailable", reason: "teamSetupRequired")
                }
                return .init(state: "granted", reason: "none")
            }
        )

        async let older = reader.read()
        try? await Task.sleep(nanoseconds: 10_000_000)
        async let newer = reader.read()
        let results = await (older, newer)

        XCTAssertEqual(results.1, .init(state: "granted", reason: "none"))
        XCTAssertEqual(results.0, .init(state: "granted", reason: "none"))
    }
}
