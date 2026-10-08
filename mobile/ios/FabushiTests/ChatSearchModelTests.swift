import XCTest
@testable import Fabushi

final class ChatSearchModelTests: XCTestCase {
    func testSearchCountsOccurrencesWithoutFilteringEntries() {
        XCTAssertEqual(
            chatSearchMatches(
                [
                    ChatSearchEntry(id: "m1", text: "Alpha alpha"),
                    ChatSearchEntry(id: "m2", text: "other"),
                    ChatSearchEntry(id: "m3", text: "ALPHA"),
                ],
                query: "alpha"
            ),
            [
                ChatSearchMatch(entryId: "m1", occurrence: 0),
                ChatSearchMatch(entryId: "m1", occurrence: 1),
                ChatSearchMatch(entryId: "m3", occurrence: 0),
            ]
        )
    }

    func testCanonicalMessageAuthorProjectionUsesSelfAndContactsWithoutGuessingUnknownActors() {
        let contacts = [
            MessagingContact(id: "peer-1", displayName: "Alice", username: nil, kind: "human"),
        ]
        XCTAssertEqual(
            MessagingModel.searchAuthorName(
                senderId: "self",
                currentActorId: "self",
                contacts: contacts
            ),
            "You"
        )
        XCTAssertEqual(
            MessagingModel.searchAuthorName(
                senderId: "peer-1",
                currentActorId: "self",
                contacts: contacts
            ),
            "Alice"
        )
        XCTAssertNil(
            MessagingModel.searchAuthorName(
                senderId: "unknown",
                currentActorId: "self",
                contacts: contacts
            )
        )
    }

    func testSearchNavigationWrapsBothDirections() {
        XCTAssertEqual(nextChatSearchIndex(current: nil, count: 3, delta: 1), 0)
        XCTAssertEqual(nextChatSearchIndex(current: nil, count: 3, delta: -1), 2)
        XCTAssertEqual(nextChatSearchIndex(current: 2, count: 3, delta: 1), 0)
        XCTAssertEqual(nextChatSearchIndex(current: 0, count: 3, delta: -1), 2)
        XCTAssertNil(nextChatSearchIndex(current: nil, count: 0, delta: 1))
    }

    func testWhitespaceOnlyQueryHasNoMatches() {
        XCTAssertTrue(chatSearchMatches([ChatSearchEntry(id: "m1", text: "message")], query: "   ").isEmpty)
    }
}
