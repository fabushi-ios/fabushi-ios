import XCTest
@testable import Fabushi

final class AutoReviewApprovalParityTests: XCTestCase {
    func testAlwaysAllowRedactsNormalizesBoundsAndKeepsNewestTwenty() throws {
        let current = normalizeSandAutoReviewInstructions(
            isEnabled: true,
            allowInstructions: (1...20).map { "rule-\($0)" },
            blockInstructions: ["ask-first"]
        )
        let next = try XCTUnwrap(appendMobileAutoReviewAllowRule(
            current,
            proposedRule: "curl   https://user:pass@example.com/path?token=secret#frag   Authorization: Bearer abc123"
        ))
        XCTAssertEqual(next.allowInstructions.count, 20)
        XCTAssertFalse(next.allowInstructions.contains("rule-1"))
        let added = try XCTUnwrap(next.allowInstructions.last)
        XCTAssertFalse(added.contains("user:pass"))
        XCTAssertFalse(added.contains("token=secret"))
        XCTAssertFalse(added.contains("#frag"))
        XCTAssertFalse(added.contains("abc123"))
        XCTAssertFalse(added.contains("  "))
        XCTAssertEqual(next.blockInstructions, ["ask-first"])
    }

    func testAlwaysAllowDeduplicatesAndDecoderFailsClosed() throws {
        let current = normalizeSandAutoReviewInstructions(
            isEnabled: true, allowInstructions: ["git status"], blockInstructions: []
        )
        let next = try XCTUnwrap(appendMobileAutoReviewAllowRule(current, proposedRule: " git   status "))
        XCTAssertEqual(next.allowInstructions, ["git status"])
        XCTAssertThrowsError(try decodeMobileAutoReviewInstructions([
            "isEnabled": true, "allowInstructions": ["ok"],
        ]))
    }

    func testApprovalProjectionRequiresExactOperationOwnership() throws {
        let event: [String: Any] = [
            "type": "approval.requested", "operationId": "operation-a",
            "approvalId": "approval-1", "kind": "command",
            "subject": "git status", "proposedRule": "git status",
        ]
        XCTAssertNil(projectMobileApprovalRequest(event, operationId: "operation-b"))
        let row = try XCTUnwrap(projectMobileApprovalRequest(event, operationId: "operation-a"))
        XCTAssertEqual(row.approvalId, "approval-1")
        XCTAssertEqual(row.approvalProposedRule, "git status")
        XCTAssertEqual(row.actionStatus, "pending")
    }

    func testHostDecisionKeepsAlwaysAsOneTimeAfterRulePersistence() {
        XCTAssertEqual(MobileAutoReviewResolution.approved.hostDecision, "allow-once")
        XCTAssertEqual(MobileAutoReviewResolution.always.hostDecision, "allow-once")
        XCTAssertEqual(MobileAutoReviewResolution.denied.hostDecision, "deny")
    }
}


private func autoReviewProtoBytesField(_ field: UInt8, _ value: Data) -> Data {
    precondition(value.count < 128)
    return Data([(field << 3) | 2, UInt8(value.count)]) + value
}

private func autoReviewProtoStringField(_ field: UInt8, _ value: String) -> Data {
    autoReviewProtoBytesField(field, Data(value.utf8))
}

extension AutoReviewApprovalParityTests {
    func testClassifierDecisionProjectionFailsClosedForNonDecisions() {
        XCTAssertEqual(mobileAutoReviewResolutionForClassifier(["kind": "allow"]), .approved)
        XCTAssertEqual(mobileAutoReviewResolutionForClassifier(["kind": "block"]), .denied)
        XCTAssertNil(mobileAutoReviewResolutionForClassifier(["kind": "error"]))
        XCTAssertNil(mobileAutoReviewResolutionForClassifier(["kind": "missing"]))
        XCTAssertNil(mobileAutoReviewResolutionForClassifier(["kind": "unspecified"]))
        XCTAssertNil(mobileAutoReviewResolutionForClassifier(["kind": "unknown:7"]))
    }

    func testClassifierCodecPreservesCanonicalDecisionAndFailureMetadata() throws {
        var success = Data([0x08, 0x01])
        success += autoReviewProtoStringField(2, "safe")
        success += autoReviewProtoStringField(3, "git status")
        let successResult = autoReviewProtoBytesField(1, success)
        XCTAssertEqual(
            try IOSCursorDashboardClient.decodeAutoReviewClassifierResponseForTests(
                autoReviewProtoBytesField(1, successResult)
            ),
            .success(.init(
                decision: .allow,
                blockReason: "safe",
                proposedAllowRule: "git status"
            ))
        )

        let metadata = #"{"failureReason":"rate_limit","retryable":true}"#
        let backendMessage = "temporarily unavailable\n\nSmartModeClassifierFailureMetadata:\(metadata)"
        let errorResult = autoReviewProtoBytesField(
            2,
            autoReviewProtoStringField(1, backendMessage)
        )
        XCTAssertEqual(
            try IOSCursorDashboardClient.decodeAutoReviewClassifierResponseForTests(
                autoReviewProtoBytesField(1, errorResult)
            ),
            .backendError(
                message: "temporarily unavailable",
                failureReason: "rate_limit",
                retryable: true
            )
        )
        XCTAssertEqual(
            try IOSCursorDashboardClient.decodeAutoReviewClassifierResponseForTests(Data()),
            .missing
        )
    }

    func testClassifierRequestCarriesTargetContextAttemptAndMode() throws {
        let body = try IOSCursorDashboardClient.autoReviewClassifierRequestForTests(
            .init(
                parentConversationId: "conversation-1",
                action: "git status",
                arguments: [
                    "force": .bool(false),
                    "path": .string("/repo"),
                ],
                conversationContext: [
                    .init(role: "user", content: "check status"),
                    .init(role: "assistant", content: "I need approval"),
                ],
                attemptIndex: 0,
                mode: "enforce"
            )
        )
        XCTAssertFalse(body.isEmpty)
        XCTAssertNotNil(body.range(of: Data("conversation-1".utf8)))
        XCTAssertNotNil(body.range(of: Data("git status".utf8)))
        XCTAssertNotNil(body.range(of: Data("check status".utf8)))
        XCTAssertNotNil(body.range(of: Data("enforce".utf8)))
        XCTAssertNotNil(body.range(of: Data([0x10, 0x00])))
    }

    func testClassifierShippingCallerUsesAuthenticatedProtoTransportAndTenSecondTimeout() async throws {
        var success = Data([0x08, 0x02])
        success += autoReviewProtoStringField(2, "blocked by policy")
        let responseBody = autoReviewProtoBytesField(
            1,
            autoReviewProtoBytesField(1, success)
        )
        let credentials = AccountMcpCredentials(
            getAccessToken: { _ in "secret-token" },
            getMachineId: { "machine-1" }
        )
        let client = IOSCursorDashboardClient(
            credentials: credentials,
            backendURL: URL(string: "https://backend.example.test")!,
            requestExecutor: { request in
                XCTAssertEqual(
                    request.url?.path,
                    "/aiserver.v1.DashboardService/ClassifySandAutoReview"
                )
                XCTAssertEqual(
                    request.value(forHTTPHeaderField: "Content-Type"),
                    "application/proto"
                )
                XCTAssertEqual(
                    request.value(forHTTPHeaderField: "Connect-Protocol-Version"),
                    "1"
                )
                XCTAssertEqual(
                    request.value(forHTTPHeaderField: "authorization"),
                    "Bearer secret-token"
                )
                XCTAssertEqual(
                    request.value(forHTTPHeaderField: "x-ghost-mode"),
                    "true"
                )
                XCTAssertTrue(
                    request.value(forHTTPHeaderField: "x-cursor-checksum")?
                        .hasSuffix("machine-1") == true
                )
                XCTAssertEqual(request.timeoutInterval, 10, accuracy: 0.001)
                let http = HTTPURLResponse(
                    url: request.url!,
                    statusCode: 200,
                    httpVersion: "HTTP/2",
                    headerFields: ["Content-Type": "application/proto"]
                )!
                return (responseBody, http)
            }
        )
        XCTAssertEqual(
            try await client.classifySandAutoReview(
                .init(
                    parentConversationId: "conversation-1",
                    action: "rm file",
                    arguments: [:],
                    conversationContext: []
                )
            ),
            .success(.init(
                decision: .block,
                blockReason: "blocked by policy",
                proposedAllowRule: nil
            ))
        )
    }
}
