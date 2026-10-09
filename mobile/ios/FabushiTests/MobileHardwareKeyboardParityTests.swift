import XCTest
@testable import Fabushi

final class MobileHardwareKeyboardParityTests: XCTestCase {
    func testRootShortcutContractMatchesDesktopHotkeysThatHaveNativeOwners() {
        let byID = Dictionary(uniqueKeysWithValues: MobileHardwareKeyboardContract.rootSpecs.map { ($0.id, $0.hotkeyDescription) })

        XCTAssertEqual(byID["sand.newAgent"], "cmd+n")
        XCTAssertEqual(byID["sand.commandPalette"], "cmd+k")
        XCTAssertEqual(byID["sand.openSettings"], "cmd+,")
        XCTAssertEqual(byID["sand.openTools"], "cmd+shift+m")
        XCTAssertEqual(byID["sand.focusSearch"], "cmd+shift+f")
        XCTAssertEqual(byID["sand.previousAgent"], "alt+up")
        XCTAssertEqual(byID["sand.nextAgent"], "alt+down")
        XCTAssertEqual(byID["sand.navigateBack"], "cmd+[")
        XCTAssertEqual(byID["sand.toggleSidebar"], "cmd+b")
        XCTAssertEqual(byID["sand.escape"], "escape")
        XCTAssertEqual(byID["sand.focusAgent1"], "cmd+1")
        XCTAssertEqual(byID["sand.focusAgent9"], "cmd+9")
        XCTAssertEqual(MobileHardwareKeyboardContract.platformNotApplicableHotkeys, ["cmd+]"])
    }

    func testPromptShortcutsStayScopedToActiveChatOwner() {
        XCTAssertEqual(
            MobileHardwareKeyboardContract.promptSpecs.map(\.hotkeyDescription),
            ["cmd+i", "cmd+l"]
        )
        XCTAssertTrue(
            MobileHardwareKeyboardContract.promptSpecs.allSatisfy { $0.action == .focusPrompt }
        )
    }

    func testAgentProjectionExcludesGroupsMiniAppsAndHiddenBots() {
        let visible = MobileBotSummary(id: "a", name: "A", description: "")
        let hidden = MobileBotSummary(id: "b", name: "B", description: "", hidden: true)
        let group = MobileBotSummary(id: "g", name: "G", description: "", isGroup: true)
        let miniApp = MobileBotSummary(id: "m", name: "M", description: "", miniAppId: "app")
        let second = MobileBotSummary(id: "c", name: "C", description: "")

        XCTAssertEqual(
            MobileHardwareKeyboardProjection.navigableAgents([visible, hidden, group, miniApp, second]).map(\.id),
            ["a", "c"]
        )
    }

    func testNumberAndCycleShortcutsReuseCanonicalRosterOrder() {
        let a = MobileBotSummary(id: "a", name: "A", description: "")
        let b = MobileBotSummary(id: "b", name: "B", description: "")
        let c = MobileBotSummary(id: "c", name: "C", description: "")
        let bots = [a, b, c]

        XCTAssertEqual(
            MobileHardwareKeyboardProjection.target(
                for: .focusAgent(2),
                currentAgentID: "a",
                bots: bots
            )?.id,
            "b"
        )
        XCTAssertEqual(
            MobileHardwareKeyboardProjection.target(
                for: .previousAgent,
                currentAgentID: "a",
                bots: bots
            )?.id,
            "c"
        )
        XCTAssertEqual(
            MobileHardwareKeyboardProjection.target(
                for: .nextAgent,
                currentAgentID: "c",
                bots: bots
            )?.id,
            "a"
        )
    }
}
