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
