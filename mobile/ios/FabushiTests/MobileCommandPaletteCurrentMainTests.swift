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

    func testComputerUpdateProjectionFailsClosedAndPreservesBusyOverride() {
        let agent = bot("agent", name: "Agent")
        let available = RemoteComputerAgentBoxSnapshot(
            agentID: agent.id,
            state: "running",
            vncURL: nil,
            imageUpdateAvailable: true
        )
        let idle = RemoteComputerHostActivitySnapshot(
            agentID: agent.id,
            runningComputerSubagentIDs: [],
            isComputerUseTaskActive: false
        )
        let busy = RemoteComputerHostActivitySnapshot(
            agentID: agent.id,
            runningComputerSubagentIDs: ["computer-subagent"],
            isComputerUseTaskActive: true
        )

        XCTAssertEqual(
            MobileCommandPaletteComputerUpdateProjection.action(
                agent: agent,
                status: available,
                activity: idle,
                isPending: false,
                isQueued: false
            ),
            .ready
        )
        XCTAssertEqual(
            MobileCommandPaletteComputerUpdateProjection.action(
                agent: agent,
                status: available,
                activity: busy,
                isPending: false,
                isQueued: false
            ),
            .busyOverride
        )
        XCTAssertNil(
            MobileCommandPaletteComputerUpdateProjection.action(
                agent: agent,
                status: available,
                activity: idle,
                isPending: true,
                isQueued: false
            )
        )
        XCTAssertNil(
            MobileCommandPaletteComputerUpdateProjection.action(
                agent: agent,
                status: available,
                activity: idle,
                isPending: false,
                isQueued: true
            )
        )
        XCTAssertNil(
            MobileCommandPaletteComputerUpdateProjection.action(
                agent: agent,
                status: .init(
                    agentID: agent.id,
                    state: "running",
                    vncURL: nil,
                    imageUpdateAvailable: false
                ),
                activity: idle,
                isPending: false,
                isQueued: false
            )
        )
        XCTAssertNil(
            MobileCommandPaletteComputerUpdateProjection.action(
                agent: bot("group", name: "Group", isGroup: true),
                status: available,
                activity: idle,
                isPending: false,
                isQueued: false
            )
        )
        XCTAssertNil(
            MobileCommandPaletteComputerUpdateProjection.action(
                agent: agent,
                status: available,
                activity: .init(
                    agentID: "different-agent",
                    runningComputerSubagentIDs: [],
                    isComputerUseTaskActive: false
                ),
                isPending: false,
                isQueued: false
            )
        )
    }

}
