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
}
