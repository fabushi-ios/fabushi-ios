import XCTest
@testable import Fabushi

final class MobileConversationPaginationParityTests: XCTestCase {
    func testOlderPagePrependsWithoutDuplicatingCurrentEntries() {
        let current = [
            MobileChatMessage(id: "m3", role: .user, text: "3"),
            MobileChatMessage(id: "m4", role: .assistant, text: "4"),
        ]
        let older = [
            MobileChatMessage(id: "m1", role: .user, text: "1"),
            MobileChatMessage(id: "m2", role: .assistant, text: "2"),
            MobileChatMessage(id: "m3", role: .user, text: "duplicate"),
        ]
        XCTAssertEqual(
            mergeMobileConversationOlderPage(older: older, current: current).map(\.id),
            ["m1", "m2", "m3", "m4"]
        )
    }

    func testOlderPagePreservesProjectedOrderWithinPage() {
        let current = [MobileChatMessage(id: "m3", role: .user, text: "3")]
        let older = [
            MobileChatMessage(id: "m1", role: .user, text: "1"),
            MobileChatMessage(id: "m2", role: .assistant, text: "2"),
        ]
        XCTAssertEqual(
            mergeMobileConversationOlderPage(older: older, current: current).map(\.id),
            ["m1", "m2", "m3"]
        )
    }
}
