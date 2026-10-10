import XCTest
@testable import Fabushi

final class ForwardMessageParityTests: XCTestCase {
    func testRecipientNavigationStaysBoundedToEligibleResults() {
        XCTAssertNil(
            ForwardRecipientNavigation.nextIndex(
                currentIndex: 0,
                recipientCount: 0,
                key: .arrowDown
            )
        )

        XCTAssertEqual(
            ForwardRecipientNavigation.nextIndex(
                currentIndex: 0,
                recipientCount: 3,
                key: .arrowUp
            ),
            0
        )
        XCTAssertEqual(
            ForwardRecipientNavigation.nextIndex(
                currentIndex: 2,
                recipientCount: 3,
                key: .arrowDown
            ),
            2
        )
        XCTAssertEqual(
            ForwardRecipientNavigation.nextIndex(
                currentIndex: 1,
                recipientCount: 3,
                key: .home
            ),
            0
        )
        XCTAssertEqual(
            ForwardRecipientNavigation.nextIndex(
                currentIndex: 1,
                recipientCount: 3,
                key: .end
            ),
            2
        )
    }

    func testSubmitShortcutAcceptsDesktopCommandAndControlVariants() {
        XCTAssertTrue(
            ForwardRecipientNavigation.isSubmitShortcut(
                isReturn: true,
                command: true,
                control: false
            )
        )
        XCTAssertTrue(
            ForwardRecipientNavigation.isSubmitShortcut(
                isReturn: true,
                command: false,
                control: true
            )
        )
        XCTAssertFalse(
            ForwardRecipientNavigation.isSubmitShortcut(
                isReturn: true,
                command: false,
                control: false
            )
        )
        XCTAssertFalse(
            ForwardRecipientNavigation.isSubmitShortcut(
                isReturn: false,
                command: true,
                control: true
            )
        )
    }

    func testPageNavigationMatchesDesktopSixRowStepAndClamps() {
        XCTAssertEqual(
            ForwardRecipientNavigation.nextIndex(
                currentIndex: 2,
                recipientCount: 20,
                key: .pageDown
            ),
            8
        )
        XCTAssertEqual(
            ForwardRecipientNavigation.nextIndex(
                currentIndex: 8,
                recipientCount: 20,
                key: .pageUp
            ),
            2
        )
        XCTAssertEqual(
            ForwardRecipientNavigation.nextIndex(
                currentIndex: 18,
                recipientCount: 20,
                key: .pageDown
            ),
            19
        )
        XCTAssertEqual(
            ForwardRecipientNavigation.nextIndex(
                currentIndex: 2,
                recipientCount: 20,
                key: .pageUp,
                pageSize: 6
            ),
            0
        )
    }

    func testInvalidCurrentIndexIsClampedBeforeMovement() {
        XCTAssertEqual(
            ForwardRecipientNavigation.nextIndex(
                currentIndex: -50,
                recipientCount: 4,
                key: .arrowDown
            ),
            1
        )
        XCTAssertEqual(
            ForwardRecipientNavigation.nextIndex(
                currentIndex: 50,
                recipientCount: 4,
                key: .arrowUp
            ),
            2
        )
    }

    func testAgentForwardRequiresSettledCanonicalMessageIdentity() {
        var message = MobileChatMessage(
            id: "local-row",
            role: .assistant,
            text: "Ready"
        )

        XCTAssertNil(
            mobileBotForwardMessageId(
                message,
                sourceConversationId: "conversation-1"
            )
        )

        message.canonicalMessageId = "message-1"
        message.streaming = true
        XCTAssertNil(
            mobileBotForwardMessageId(
                message,
                sourceConversationId: "conversation-1"
            )
        )

        message.streaming = false
        message.optimisticDeliveryPhase = .pending
        XCTAssertNil(
            mobileBotForwardMessageId(
                message,
                sourceConversationId: "conversation-1"
            )
        )

        message.optimisticDeliveryPhase = .acceptedAwaitingEcho
        XCTAssertNil(
            mobileBotForwardMessageId(
                message,
                sourceConversationId: "conversation-1"
            )
        )

        message.optimisticDeliveryPhase = .failed
        XCTAssertNil(
            mobileBotForwardMessageId(
                message,
                sourceConversationId: "conversation-1"
            )
        )

        message.optimisticDeliveryPhase = nil
        XCTAssertEqual(
            mobileBotForwardMessageId(
                message,
                sourceConversationId: "conversation-1"
            ),
            "message-1"
        )

        message.text = "   "
        XCTAssertNil(
            mobileBotForwardMessageId(
                message,
                sourceConversationId: "conversation-1"
            )
        )
        message.attachmentURL = "https://example.invalid/report.pdf"
        XCTAssertEqual(
            mobileBotForwardMessageId(
                message,
                sourceConversationId: "conversation-1"
            ),
            "message-1"
        )
        message.attachmentURL = nil
        message.text = "Ready"

        XCTAssertNil(
            mobileBotForwardMessageId(
                message,
                sourceConversationId: nil
            )
        )

        message.kind = .notice
        XCTAssertNil(
            mobileBotForwardMessageId(
                message,
                sourceConversationId: "conversation-1"
            )
        )
    }

}
