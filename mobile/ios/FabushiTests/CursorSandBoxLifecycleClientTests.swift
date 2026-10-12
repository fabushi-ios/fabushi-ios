import Foundation
import XCTest
@testable import Fabushi

private func lifecycleCredentials() -> AccountMcpCredentials {
    AccountMcpCredentials(
        getAccessToken: { _ in "token" },
        getMachineId: { "machine-id" }
    )
}

private func lifecycleResponse(_ request: URLRequest, data: Data) -> (Data, URLResponse) {
    let http = HTTPURLResponse(
        url: request.url!,
        statusCode: 200,
        httpVersion: "HTTP/2",
        headerFields: [
            "Content-Type": request.value(forHTTPHeaderField: "Content-Type") ?? "application/proto",
        ]
    )!
    return (data, http)
}

private func lifecycleBytesField(_ field: UInt8, _ string: String) -> Data {
    let value = Data(string.utf8)
    return Data([(field << 3) | 2, UInt8(value.count)]) + value
}

private func lifecycleEnvelope(flags: UInt8, payload: Data) -> Data {
    let count = UInt32(payload.count)
    return Data([
        flags,
        UInt8((count >> 24) & 0xff),
        UInt8((count >> 16) & 0xff),
        UInt8((count >> 8) & 0xff),
        UInt8(count & 0xff),
    ]) + payload
}

final class CursorSandBoxLifecycleClientTests: XCTestCase {
    func testRecreateUsesCanonicalGrokBotUnaryConnectRPC() async throws {
        let client = IOSCursorDashboardClient(
            credentials: lifecycleCredentials(),
            backendURL: URL(string: "https://backend.example.test")!,
            requestExecutor: { request in
                XCTAssertEqual(
                    request.url?.path,
                    "/aiserver.v1.GrokBotService/RecreateSandBox"
                )
                XCTAssertEqual(
                    request.value(forHTTPHeaderField: "Content-Type"),
                    "application/proto"
                )
                XCTAssertEqual(
                    request.value(forHTTPHeaderField: "Connect-Protocol-Version"),
                    "1"
                )
                XCTAssertEqual(request.httpBody, Data([0x08, 0x01, 0x10, 0x00]))
                var body = Data([0x08, 0x01])
                body += lifecycleBytesField(3, "operation-1")
                return lifecycleResponse(request, data: body)
            }
        )

        let result = try await client.recreateSandBox(
            preserveData: true,
            force: false
        )
        XCTAssertTrue(result.started)
        XCTAssertEqual(result.operationId, "operation-1")
        XCTAssertEqual(result.reason, "")
    }

    func testForceRecreateUsesEmptyCanonicalRequest() async throws {
        let client = IOSCursorDashboardClient(
            credentials: lifecycleCredentials(),
            backendURL: URL(string: "https://backend.example.test")!,
            requestExecutor: { request in
                XCTAssertEqual(
                    request.url?.path,
                    "/aiserver.v1.GrokBotService/ForceRecreateSandBox"
                )
                XCTAssertEqual(request.httpBody, Data())
                var body = Data([0x08, 0x01])
                body += lifecycleBytesField(3, "force-operation")
                return lifecycleResponse(request, data: body)
            }
        )
        let result = try await client.forceRecreateSandBox()
        XCTAssertEqual(result.operationId, "force-operation")
    }

    func testMigrationWatchUsesStreamingEnvelopeAndProjectsOffsets() async throws {
        let client = IOSCursorDashboardClient(
            credentials: lifecycleCredentials(),
            backendURL: URL(string: "https://backend.example.test")!,
            requestExecutor: { request in
                XCTAssertEqual(
                    request.url?.path,
                    "/aiserver.v1.GrokBotService/WatchSandBoxMigration"
                )
                XCTAssertEqual(
                    request.value(forHTTPHeaderField: "Content-Type"),
                    "application/connect+proto"
                )
                let requestBody = try XCTUnwrap(request.httpBody)
                XCTAssertEqual(requestBody.prefix(5), Data([0, 0, 0, 0, 12]))
                XCTAssertEqual(
                    Data(requestBody.dropFirst(5)),
                    lifecycleBytesField(1, "offset-1") + Data([0x10, 0x01])
                )

                var event = Data([0x08, 0x06])
                event += lifecycleBytesField(2, "done")
                event += Data([0x18, 0x7b])
                event += lifecycleBytesField(4, "offset-2")
                event += lifecycleBytesField(5, "operation-2")
                let stream = lifecycleEnvelope(flags: 0, payload: event)
                    + lifecycleEnvelope(flags: 2, payload: Data("{}".utf8))
                return lifecycleResponse(request, data: stream)
            }
        )

        var events: [IOSCursorSandBoxMigrationEvent] = []
        for try await event in client.watchSandBoxMigration(
            fromOffsetKey: "offset-1",
            includeFinished: true
        ) {
            events.append(event)
        }
        XCTAssertEqual(events.count, 1)
        XCTAssertEqual(events[0].phaseName, "done")
        XCTAssertEqual(events[0].detail, "done")
        XCTAssertEqual(events[0].atMs, 123)
        XCTAssertEqual(events[0].offsetKey, "offset-2")
        XCTAssertEqual(events[0].operationId, "operation-2")
    }

    func testMigrationEnvelopeDecoderRejectsTruncation() throws {
        var decoder = IOSCursorConnectEnvelopeDecoder()
        _ = try decoder.append(Data([0, 0, 0, 0, 4, 1, 2]))
        XCTAssertThrowsError(try decoder.requireEmptyAtEOF())
    }

    func testEnsureSandBoxPreservesBrokerFailureContracts() async throws {
        struct Case: Sendable {
            let status: Int
            let hint: String?
            let retryAfter: String?
            let expected: IOSCursorSandBoxBrokerError.Kind
        }
        let cases: [Case] = [
            .init(
                status: 403,
                hint: CLOUD_AGENT_STORAGE_DISABLED,
                retryAfter: nil,
                expected: .noStorage
            ),
            .init(
                status: 503,
                hint: SAND_BOX_BLOCKED,
                retryAfter: "7",
                expected: .blocked(retryAfterMilliseconds: 7_000)
            ),
            .init(
                status: 409,
                hint: "SAND_CLIENT_UPDATE_REQUIRED",
                retryAfter: nil,
                expected: .updateRequired
            ),
            .init(
                status: 401,
                hint: nil,
                retryAfter: nil,
                expected: .accessDenied
            ),
            .init(
                status: 500,
                hint: nil,
                retryAfter: nil,
                expected: .http(status: 500)
            ),
        ]

        for value in cases {
            let client = IOSCursorDashboardClient(
                credentials: lifecycleCredentials(),
                backendURL: URL(string: "https://backend.example.test")!,
                requestExecutor: { request in
                    var headers = ["Content-Type": "application/proto"]
                    if let hint = value.hint {
                        headers["x-automation-failure-hint"] = hint
                    }
                    if let retryAfter = value.retryAfter {
                        headers["retry-after"] = retryAfter
                    }
                    let response = HTTPURLResponse(
                        url: request.url!,
                        statusCode: value.status,
                        httpVersion: "HTTP/2",
                        headerFields: headers
                    )!
                    return (Data("backend detail".utf8), response)
                }
            )

            do {
                _ = try await client.ensureSandBox()
                XCTFail("broker failure must not decode as a successful descriptor")
            } catch let error as IOSCursorSandBoxBrokerError {
                XCTAssertEqual(error.kind, value.expected)
            } catch {
                XCTFail("unexpected error type: \(error)")
            }
        }
    }

    func testEnsureSandBoxNoStorageAndAccessDeniedInvalidateDescriptorEligibility() {
        XCTAssertTrue(
            IOSCursorSandBoxBrokerError(
                kind: .noStorage,
                message: GATEWAY_NO_STORAGE_MESSAGE_MARKER
            ).deniesCachedDescriptor
        )
        XCTAssertTrue(
            IOSCursorSandBoxBrokerError(
                kind: .accessDenied,
                message: GATEWAY_ACCESS_DENIED_MESSAGE_MARKER
            ).deniesCachedDescriptor
        )
        XCTAssertFalse(
            IOSCursorSandBoxBrokerError(
                kind: .blocked(retryAfterMilliseconds: 1_000),
                message: "blocked"
            ).deniesCachedDescriptor
        )
        XCTAssertFalse(
            IOSCursorSandBoxBrokerError(
                kind: .updateRequired,
                message: "update"
            ).deniesCachedDescriptor
        )
    }


    @MainActor
    func testEnsureSandBoxProjectsBackendUpdateRequirementAndClearsOnSuccess() async throws {
        var requiredEvents: [Bool] = []
        let requiredClient = IOSCursorDashboardClient(
            credentials: lifecycleCredentials(),
            backendURL: URL(string: "https://backend.example.test")!,
            requestExecutor: { request in
                let response = HTTPURLResponse(
                    url: request.url!,
                    statusCode: 409,
                    httpVersion: "HTTP/2",
                    headerFields: [
                        "Content-Type": "application/proto",
                        "x-automation-failure-hint": "SAND_CLIENT_UPDATE_REQUIRED",
                    ]
                )!
                return (Data(), response)
            },
            backendUpdateRequirementSink: { requiredEvents.append($0) }
        )
        do {
            _ = try await requiredClient.ensureSandBox()
            XCTFail("update-required broker response must fail the connection")
        } catch let error as IOSCursorSandBoxBrokerError {
            XCTAssertEqual(error.kind, .updateRequired)
        }
        XCTAssertEqual(requiredEvents, [true])

        requiredEvents.removeAll()
        var body = Data()
        body += lifecycleBytesField(5, "exec-auth")
        body += lifecycleBytesField(6, "https://exec.example.test")
        let successfulClient = IOSCursorDashboardClient(
            credentials: lifecycleCredentials(),
            backendURL: URL(string: "https://backend.example.test")!,
            requestExecutor: { request in
                lifecycleResponse(request, data: body)
            },
            backendUpdateRequirementSink: { requiredEvents.append($0) }
        )
        _ = try await successfulClient.ensureSandBox()
        XCTAssertEqual(requiredEvents, [false])
    }

}
