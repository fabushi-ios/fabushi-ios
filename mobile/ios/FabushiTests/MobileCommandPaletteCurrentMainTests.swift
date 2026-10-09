import XCTest
@testable import Fabushi

final class MobileCommandPaletteCurrentMainTests: XCTestCase {
    private func bot(
        _ id: String,
        name: String,
        hidden: Bool = false,
        isGroup: Bool = false
    ) -> MobileBotSummary {
        MobileBotSummary(
            id: id,
            name: name,
            description: "",
            hidden: hidden,
            isGroup: isGroup
        )
    }

    func testDefaultAllOmitsHiddenBots() {
        let entries = GrokMobileCommandPaletteModel.entries(
            bots: [
                bot("visible", name: "Visible"),
                bot("hidden", name: "Hidden", hidden: true),
            ],
            conversations: [],
            messagesByConversation: [:],
            actions: [],
            query: "",
            tab: .all
        )

        XCTAssertEqual(entries.map(\.id), ["bot:visible"])
    }

    func testSearchReturnsVisibleMatchesBeforeHiddenMatches() {
        let entries = GrokMobileCommandPaletteModel.entries(
            bots: [
                bot("visible", name: "Agent Alpha"),
                bot("hidden", name: "Agent Archive", hidden: true),
            ],
            conversations: [],
            messagesByConversation: [:],
            actions: [],
            query: "agent",
            tab: .agents
        )

        XCTAssertEqual(entries.map(\.id), ["bot:visible", "bot:hidden"])
    }

    func testDuplicateRosterIdentityPreservesFirstSlotButUsesNewestRow() {
        let entries = GrokMobileCommandPaletteModel.entries(
            bots: [
                bot("a", name: "Old A"),
                bot("b", name: "B"),
                bot("a", name: "Newest A"),
            ],
            conversations: [],
            messagesByConversation: [:],
            actions: [],
            query: "",
            tab: .all
        )

        XCTAssertEqual(entries.map(\.id), ["bot:a", "bot:b"])
        XCTAssertEqual(entries.map(\.label), ["Newest A", "B"])
    }

    func testHiddenGroupOnlyAppearsInGroupSearch() {
        let entries = GrokMobileCommandPaletteModel.entries(
            bots: [bot("g", name: "Hidden Group", hidden: true, isGroup: true)],
            conversations: [],
            messagesByConversation: [:],
            actions: [],
            query: "hidden",
            tab: .groups
        )

        XCTAssertEqual(entries.map(\.id), ["bot:g"])
    }
}
