import XCTest
@testable import Fabushi

private actor FeedbackRequestRecorder {
    private var request: URLRequest?

    func record(_ request: URLRequest) {
        self.request = request
    }

    func snapshot() -> URLRequest? {
        request
    }
}

final class FeedbackOverlayParityTests: XCTestCase {
    func testFeedbackCodesNormalizeDesktopContractAndUnknownFailsClosed() {
        XCTAssertEqual(AccountFeedbackCode.normalize("access-denied"), .accessDenied)
        XCTAssertEqual(AccountFeedbackCode.normalize("invalid-feedback"), .invalidFeedback)
        XCTAssertEqual(AccountFeedbackCode.normalize("not-signed-in"), .notSignedIn)
        XCTAssertEqual(AccountFeedbackCode.normalize("rate-limited"), .rateLimited)
        XCTAssertEqual(AccountFeedbackCode.normalize("subscription-required"), .subscriptionRequired)
        XCTAssertEqual(AccountFeedbackCode.normalize("unavailable"), .unavailable)
        XCTAssertEqual(AccountFeedbackCode.normalize("future-code"), .unavailable)
        XCTAssertEqual(AccountFeedbackCode.normalize(nil), .unavailable)
    }

    func testFeedbackErrorMessagesRemainRecoverableAndUserFacing() {
        for code in AccountFeedbackCode.allCases {
            XCTAssertFalse(code.localizedMessage.isEmpty)
        }
        XCTAssertEqual(AccountFeedbackError(code: .rateLimited).errorDescription, AccountFeedbackCode.rateLimited.localizedMessage)
    }

    func testFeedbackViewStateKeepsSendingAndSentDistinctFromRecoverableFailure() {
        XCTAssertNotEqual(AccountFeedbackViewState.sending, .idle)
        XCTAssertNotEqual(AccountFeedbackViewState.sent, .sending)
        XCTAssertEqual(AccountFeedbackViewState.failed(.unavailable), .failed(.unavailable))
        XCTAssertNotEqual(AccountFeedbackViewState.failed(.rateLimited), .sent)
    }
    private func feedbackTestToken(_ payload: [String: String]) throws -> String {
        let data = try JSONSerialization.data(withJSONObject: payload, options: [.sortedKeys])
        let encoded = data.base64EncodedString()
            .replacingOccurrences(of: "+", with: "-")
            .replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "=", with: "")
        return "test-header." + encoded + ".test-signature"
    }

    @MainActor
    func testFeedbackClientUsesCurrentMainSandEndpointHeadersAndPayload() async throws {
        let token = try feedbackTestToken(["sub": "user-1"])
        let requestRecorder = FeedbackRequestRecorder()
        let client = IOSFeedbackReportClient(
            getAccessToken: { _ in token },
            getMachineID: { "machine-123" },
            getBackendURL: { "https://backend.example.test/" },
            recentSentryEventIDs: {
                ["0123456789abcdef0123456789abcdef", "not-a-sentry-id"]
            },
            appVersion: { "9.9.9" },
            osVersion: { "iOS-test" },
            requestExecutor: { request in
                await requestRecorder.record(request)
                return (
                    Data("{}".utf8),
                    HTTPURLResponse(
                        url: try XCTUnwrap(request.url),
                        statusCode: 200,
                        httpVersion: nil,
                        headerFields: nil
                    )!
                )
            }
        )
        let result = await client.submit([
            "accountSlot": "user-1",
            "message": "  hello feedback  ",
            "submissionId": "550e8400-e29b-41d4-a716-446655440000",
            "conversationId": " conversation-7 ",
        ])
        XCTAssertEqual(result["ok"] as? Bool, true)
        let capturedRequest = await requestRecorder.snapshot()
        let request = try XCTUnwrap(capturedRequest)
        XCTAssertEqual(request.url?.path, "/sand/feedback")
        XCTAssertEqual(request.httpMethod, "POST")
        XCTAssertEqual(request.timeoutInterval, 15)
        XCTAssertEqual(request.value(forHTTPHeaderField: "authorization"), "Bearer " + token)
        XCTAssertNotNil(request.value(forHTTPHeaderField: "x-cursor-checksum"))
        let body = try XCTUnwrap(
            JSONSerialization.jsonObject(with: try XCTUnwrap(request.httpBody)) as? [String: Any]
        )
        XCTAssertEqual(body["message"] as? String, "hello feedback")
        XCTAssertEqual(body["conversationId"] as? String, "conversation-7")
        XCTAssertEqual(body["submissionId"] as? String, "550e8400-e29b-41d4-a716-446655440000")
        XCTAssertEqual(body["appVersion"] as? String, "9.9.9")
        XCTAssertEqual(body["platform"] as? String, "ios")
        XCTAssertEqual(body["osVersion"] as? String, "iOS-test")
        XCTAssertEqual(
            body["sentryEventIds"] as? [String],
            ["0123456789abcdef0123456789abcdef"]
        )
    }

    @MainActor
    func testFeedbackClientFencesTokenIdentityAndMapsRateLimit() async throws {
        let token = try feedbackTestToken(["email": "me@example.test"])
        let client = IOSFeedbackReportClient(
            getAccessToken: { _ in token },
            getMachineID: { "machine-123" },
            getBackendURL: { "https://backend.example.test" },
            requestExecutor: { request in
                return (
                    Data(),
                    HTTPURLResponse(
                        url: try XCTUnwrap(request.url),
                        statusCode: 429,
                        httpVersion: nil,
                        headerFields: nil
                    )!
                )
            }
        )
        let mismatch = await client.submit([
            "accountSlot": "other@example.test",
            "message": "hello",
            "submissionId": "550e8400-e29b-41d4-a716-446655440000",
        ])
        XCTAssertEqual(mismatch["code"] as? String, "not-signed-in")

        let limited = await client.submit([
            "accountSlot": "me@example.test",
            "message": "hello",
            "submissionId": "550e8400-e29b-41d4-a716-446655440000",
        ])
        XCTAssertEqual(limited["code"] as? String, "rate-limited")
    }

}
