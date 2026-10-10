import XCTest
@testable import Fabushi

final class GrokMobileAgentSettingsModelTests: XCTestCase {
    func testProfileNormalizationMatchesDesktopCommitRules() {
        let profile = GrokMobileAgentSettingsModel.normalizedProfile(
            name: "  Research  ",
            title: "  Source verifier  ",
            description: "  Check citations  ",
            isGroup: false
        )
        XCTAssertEqual(profile?.name, "Research")
        XCTAssertEqual(profile?.title, "Source verifier")
        XCTAssertEqual(profile?.description, "Check citations")
        XCTAssertNil(
            GrokMobileAgentSettingsModel.normalizedProfile(
                name: "   ",
                title: "Title",
                description: "Description",
                isGroup: false
            )
        )
    }

    func testGroupProfileDropsIndividualOnlyTitle() {
        let profile = GrokMobileAgentSettingsModel.normalizedProfile(
            name: "  Research Room ",
            title: "Must not persist",
            description: "  Coordination ",
            isGroup: true
        )
        XCTAssertEqual(profile?.name, "Research Room")
        XCTAssertNil(profile?.title)
        XCTAssertEqual(profile?.description, "Coordination")
    }

    func testMutationFenceRejectsAccountAgentAndGenerationDrift() {
        let fence = GrokMobileAgentSettingsModel.MutationFence(
            accountScopeKey: "account-a",
            agentId: "agent-1",
            generation: 4
        )
        XCTAssertTrue(
            GrokMobileAgentSettingsModel.accepts(
                fence,
                accountScopeKey: "account-a",
                agentId: "agent-1",
                generation: 4
            )
        )
        XCTAssertFalse(
            GrokMobileAgentSettingsModel.accepts(
                fence,
                accountScopeKey: "account-b",
                agentId: "agent-1",
                generation: 4
            )
        )
        XCTAssertFalse(
            GrokMobileAgentSettingsModel.accepts(
                fence,
                accountScopeKey: "account-a",
                agentId: "agent-2",
                generation: 4
            )
        )
        XCTAssertFalse(
            GrokMobileAgentSettingsModel.accepts(
                fence,
                accountScopeKey: "account-a",
                agentId: "agent-1",
                generation: 5
            )
        )
    }

    func testProfileProjectionHidesGroupTitleButKeepsCanonicalIndividualTitle() {
        let individual = MobileBotSummary(
            id: "agent-1",
            name: "Research",
            description: "Verify",
            title: "",
            notifyOnUpdatesEnabled: true
        )
        XCTAssertEqual(GrokMobileAgentSettingsModel.profile(from: individual).title, "")

        let group = MobileBotSummary(
            id: "group-1",
            name: "Room",
            description: "Coordinate",
            title: "Ignored",
            isGroup: true,
            memberIds: ["agent-1"]
        )
        XCTAssertNil(GrokMobileAgentSettingsModel.profile(from: group).title)
    }
}
