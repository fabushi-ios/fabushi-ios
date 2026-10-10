import XCTest
@testable import Fabushi

final class MobileConversationOutlineParityTests: XCTestCase {
    func testStrictOutlineProjectionCoversDesktopKinds() {
        let raw: [[String: Any]] = [
            ["kind": "user", "id": "u1", "text": "hello\nsecond"],
            ["kind": "assistant-text", "id": "a1", "text": "answer"],
            ["kind": "thinking", "id": "t1", "text": "reasoning", "durationMs": 42],
            ["kind": "send-message", "id": "m1", "message": ["type": "attachment", "url": "sand://box/a", "alt": "A"]],
            ["kind": "tool-call", "id": "c1", "name": "computerUseToolCall", "status": "pending", "summary": "open app"],
        ]
        let decoded = decodeMobileConversationOutline(raw)
        XCTAssertEqual(decoded?.count, 5)
        XCTAssertEqual(decoded?.first?.preview, "hello")
        XCTAssertEqual(decoded?.last?.label, "Computer Use")
        XCTAssertEqual(decoded?.last?.status, "pending")
    }

    func testMalformedOutlineFailsClosed() {
        XCTAssertNil(decodeMobileConversationOutline([
            ["kind": "tool-call", "id": "bad", "name": "Task", "status": "unknown"]
        ]))
        XCTAssertNil(decodeMobileConversationOutline([
            ["kind": "send-message", "id": "bad", "message": ["type": "attachment"]]
        ]))
    }

    func testOutlineScopeFencesAccountAgentAndReconnectGeneration() {
        let scope = MobileConversationOutlineScope(
            accountKey: "acct-a",
            parentAgentId: "agent-a",
            selectedAgentId: "sub-a",
            reconnectGeneration: 7,
            generation: 3
        )
        XCTAssertTrue(scope.accepts(
            accountKey: "acct-a",
            parentAgentId: "agent-a",
            selectedAgentId: "sub-a",
            reconnectGeneration: 7,
            generation: 3
        ))
        XCTAssertFalse(scope.accepts(
            accountKey: "acct-b",
            parentAgentId: "agent-a",
            selectedAgentId: "sub-a",
            reconnectGeneration: 7,
            generation: 3
        ))
        XCTAssertFalse(scope.accepts(
            accountKey: "acct-a",
            parentAgentId: "agent-a",
            selectedAgentId: "sub-a",
            reconnectGeneration: 8,
            generation: 3
        ))
        XCTAssertFalse(scope.accepts(
            accountKey: "acct-a",
            parentAgentId: "agent-b",
            selectedAgentId: "sub-a",
            reconnectGeneration: 7,
            generation: 3
        ))
        XCTAssertFalse(scope.accepts(
            accountKey: "acct-a",
            parentAgentId: "agent-a",
            selectedAgentId: "sub-b",
            reconnectGeneration: 7,
            generation: 3
        ))
        XCTAssertFalse(scope.accepts(
            accountKey: "acct-a",
            parentAgentId: "agent-a",
            selectedAgentId: "sub-a",
            reconnectGeneration: 7,
            generation: 4
        ))
    }
}
