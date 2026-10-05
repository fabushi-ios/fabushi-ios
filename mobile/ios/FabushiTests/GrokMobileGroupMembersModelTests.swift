import XCTest
@testable import Fabushi

final class GrokMobileGroupMembersModelTests: XCTestCase {
    func testMutationFenceRejectsAccountOrGenerationDrift() {
        let fence = GrokMobileGroupMembersModel.MutationFence(
            accountScopeKey: "account-a",
            generation: 3
        )
        XCTAssertTrue(GrokMobileGroupMembersModel.accepts(
            fence,
            accountScopeKey: "account-a",
            generation: 3
        ))
        XCTAssertFalse(GrokMobileGroupMembersModel.accepts(
            fence,
            accountScopeKey: "account-b",
            generation: 3
        ))
        XCTAssertFalse(GrokMobileGroupMembersModel.accepts(
            fence,
            accountScopeKey: "account-a",
            generation: 4
        ))
    }

    func testCandidatesAreIndividualOnlyAndExcludeExistingMembers() {
        let group = MobileBotSummary(
            id: "group-1",
            name: "Room",
            description: "",
            isGroup: true,
            memberIds: ["bot-a"]
        )
        let roster = [
            group,
            MobileBotSummary(id: "bot-a", name: "A", description: ""),
            MobileBotSummary(id: "bot-b", name: "B", description: ""),
            MobileBotSummary(
                id: "group-2",
                name: "Nested",
                description: "",
                isGroup: true,
                memberIds: ["bot-b"]
            ),
        ]
        XCTAssertEqual(
            GrokMobileGroupMembersModel.candidates(group: group, roster: roster).map(\.id),
            ["bot-b"]
        )
        XCTAssertEqual(
            GrokMobileGroupMembersModel.adding(memberId: "bot-b", to: group, roster: roster),
            ["bot-a", "bot-b"]
        )
        XCTAssertNil(
            GrokMobileGroupMembersModel.adding(memberId: "group-2", to: group, roster: roster)
        )
    }

    func testMaximumSixAndMinimumOneAreHardInteractionBounds() {
        let members = (0..<6).map { "bot-\($0)" }
        let roster = members.map { MobileBotSummary(id: $0, name: $0, description: "") }
            + [MobileBotSummary(id: "bot-6", name: "bot-6", description: "")]
        let full = MobileBotSummary(
            id: "group-full",
            name: "Full",
            description: "",
            isGroup: true,
            memberIds: members
        )
        XCTAssertFalse(GrokMobileGroupMembersModel.canAdd(group: full, roster: roster, pending: false))
        XCTAssertNil(GrokMobileGroupMembersModel.adding(memberId: "bot-6", to: full, roster: roster))

        let one = MobileBotSummary(
            id: "group-one",
            name: "One",
            description: "",
            isGroup: true,
            memberIds: ["bot-0"]
        )
        XCTAssertFalse(GrokMobileGroupMembersModel.canRemove(group: one, pending: false))
        XCTAssertNil(GrokMobileGroupMembersModel.removing(memberId: "bot-0", from: one))
    }

    func testSharedRoomAndPendingStatesFailClosed() {
        let shared = MobileBotSummary(
            id: "shared",
            name: "Shared",
            description: "",
            isGroup: true,
            memberIds: ["bot-a", "bot-b"],
            isSharedRoom: true
        )
        let roster = [
            MobileBotSummary(id: "bot-a", name: "A", description: ""),
            MobileBotSummary(id: "bot-b", name: "B", description: ""),
            MobileBotSummary(id: "bot-c", name: "C", description: ""),
        ]
        XCTAssertNil(GrokMobileGroupMembersModel.group(id: shared.id, fallback: shared, roster: [shared] + roster))
        XCTAssertNil(GrokMobileGroupMembersModel.adding(memberId: "bot-c", to: shared, roster: roster))

        let editable = MobileBotSummary(
            id: "group",
            name: "Group",
            description: "",
            isGroup: true,
            memberIds: ["bot-a", "bot-b"]
        )
        XCTAssertFalse(GrokMobileGroupMembersModel.canAdd(group: editable, roster: roster, pending: true))
        XCTAssertFalse(GrokMobileGroupMembersModel.canRemove(group: editable, pending: true))
    }
}
