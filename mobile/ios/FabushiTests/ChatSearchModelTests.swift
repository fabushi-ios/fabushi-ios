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

    @MainActor
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

    @MainActor
    func testConversationSearchCommandTargetsDurableConversationScope() {
        let command = MessagingModel.conversationSearchCommand(
            conversationId: " human-1 ",
            query: "  dharma  ",
            limit: 999
        )
        XCTAssertEqual(command?["type"] as? String, "search")
        let query = command?["query"] as? [String: Any]
        XCTAssertEqual(query?["text"] as? String, "dharma")
        XCTAssertEqual(query?["scope"] as? String, "conversation")
        XCTAssertEqual(query?["conversationId"] as? String, "human-1")
        XCTAssertEqual(query?["limit"] as? Int, 200)
        XCTAssertTrue(query?["senderId"] is NSNull)
        XCTAssertNil(
            MessagingModel.conversationSearchCommand(
                conversationId: "human-1",
                query: "   "
            )
        )
    }

    @MainActor
    func testConversationSearchResultsRemainConversationScoped() {
        let results = MessagingModel.conversationSearchResults(
            from: [[
                "event": [
                    "type": "searchResults",
                    "results": [
                        [
                            "kind": "message",
                            "id": "m-1",
                            "conversationId": "human-1",
                            "title": "Alice",
                            "snippet": "Global Dharma durable result",
                            "timestampMs": NSNumber(value: 1_700_000_000_000 as Int64),
                            "score": NSNumber(value: 10),
                        ],
                        [
                            "kind": "message",
                            "id": "m-other",
                            "conversationId": "human-2",
                            "snippet": "must not leak",
                            "score": NSNumber(value: 10),
                        ],
                        [
                            "kind": "conversation",
                            "id": "human-1",
                            "conversationId": "human-1",
                            "snippet": "not a message",
                            "score": NSNumber(value: 10),
                        ],
                    ],
                ],
            ]],
            conversationId: "human-1"
        )
        XCTAssertEqual(
            results,
            [
                MessagingConversationSearchResult(
                    id: "m-1",
                    conversationId: "human-1",
                    snippet: "Global Dharma durable result",
                    timestampMs: 1_700_000_000_000,
                    score: 10
                ),
            ]
        )
    }
}
