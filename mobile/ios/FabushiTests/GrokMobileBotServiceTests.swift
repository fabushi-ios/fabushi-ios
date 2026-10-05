import XCTest
@testable import Fabushi

final class GrokMobileBotServiceTests: XCTestCase {
    @MainActor
    func testMergePrefersInstalledMiniAppProjectionForSameBot() {
        let surface = [
            MobileBotSummary(
                id: "global-dharma-bot",
                name: "Surface",
                description: "surface",
                miniAppId: nil,
                menuButtonText: nil
            ),
            MobileBotSummary(
                id: "plain-bot",
                name: "Plain",
                description: "plain"
            ),
        ]
        let installed = [
            MobileBotSummary(
                id: "global-dharma-bot",
                name: "全球法布施",
                description: "installed",
                miniAppId: GlobalDharmaMiniAppBridge.globalDharmaId,
                menuButtonText: "打开应用"
            ),
        ]

        let merged = GrokMobileBotService.mergeBots(installed, surface)

        XCTAssertEqual(merged.first?.id, "global-dharma-bot")
        XCTAssertEqual(merged.first?.name, "全球法布施")
        XCTAssertEqual(merged.first?.miniAppId, GlobalDharmaMiniAppBridge.globalDharmaId)
        XCTAssertEqual(merged.count, 2)
    }

    @MainActor
    func testParseBotAppliesGlobalDharmaMiniAppFallback() throws {
        let bot = try XCTUnwrap(GrokMobileBotService.parseBot([
            "id": "global-dharma-bot",
            "displayName": "全球法布施",
            "description": "Dharma",
        ]))

        XCTAssertEqual(bot.name, "全球法布施")
        XCTAssertEqual(bot.miniAppId, GlobalDharmaMiniAppBridge.globalDharmaId)
        XCTAssertEqual(bot.menuButtonText, "打开应用")
    }

    @MainActor
    func testParseBotRejectsMissingIdentity() {
        XCTAssertNil(GrokMobileBotService.parseBot(["name": "Missing id"]))
    }
    @MainActor
    func testCommittedMobileBotNameMatchesDesktopRenameRule() {
        XCTAssertNil(committedMobileBotName(initialValue: "Research", draftValue: " Research "))
        XCTAssertNil(committedMobileBotName(initialValue: "Research", draftValue: "   "))
        XCTAssertEqual(
            committedMobileBotName(initialValue: "Research", draftValue: "  Release Bot  "),
            "Release Bot"
        )
    }

    @MainActor
    func testBotMutationCommandsUseCanonicalFeatureHostContracts() {
        let longName = String(repeating: "a", count: 90)
        let rename = GrokMobileBotService.renameCommand(
            id: "agent-1",
            name: longName,
            requestId: "rename-1"
        )
        XCTAssertEqual(rename["type"] as? String, "bot.update")
        XCTAssertEqual(rename["requestId"] as? String, "rename-1")
        XCTAssertEqual(rename["id"] as? String, "agent-1")
        XCTAssertEqual((rename["name"] as? String)?.count, 72)

        let duplicate = GrokMobileBotService.duplicateCommand(
            id: "agent-1",
            requestId: "clone-1"
        )
        XCTAssertEqual(duplicate["type"] as? String, "bot.clone")
        XCTAssertEqual(duplicate["id"] as? String, "agent-1")

        let delete = GrokMobileBotService.deleteCommand(
            id: "agent-1",
            requestId: "delete-1"
        )
        XCTAssertEqual(delete["type"] as? String, "bot.delete")
        XCTAssertEqual(delete["id"] as? String, "agent-1")
    }

    @MainActor
    func testParseGroupProjectsCanonicalMembershipFields() throws {
        let group = try XCTUnwrap(GrokMobileBotService.parseGroup([
            "id": "group-1",
            "name": "Research Room",
            "description": "Cross-check",
            "memberIds": ["bot-a", "bot-b"],
        ]))
        XCTAssertTrue(group.isGroup)
        XCTAssertFalse(group.isSharedRoom)
        XCTAssertEqual(group.memberIds, ["bot-a", "bot-b"])
        XCTAssertNil(group.miniAppId)
    }

    @MainActor
    func testParseGroupFailsClosedForInvalidMembership() {
        XCTAssertNil(GrokMobileBotService.parseGroup([
            "id": "group-empty", "name": "Empty", "memberIds": [],
        ]))
        XCTAssertNil(GrokMobileBotService.parseGroup([
            "id": "group-too-large",
            "name": "Too large",
            "memberIds": (0...6).map { "bot-\($0)" },
        ]))
        XCTAssertNil(GrokMobileBotService.parseGroup([
            "id": "group-duplicate",
            "name": "Duplicate",
            "memberIds": ["bot-a", "bot-a"],
        ]))
    }

    @MainActor
    func testGroupUpdateCommandUsesCanonicalHostContract() {
        let command = GrokMobileBotService.groupUpdateCommand(
            id: "group-1",
            memberIds: ["bot-a", "bot-b"],
            requestId: "group-update-1"
        )
        XCTAssertEqual(command["type"] as? String, "group.update")
        XCTAssertEqual(command["id"] as? String, "group-1")
        XCTAssertEqual(command["requestId"] as? String, "group-update-1")
        XCTAssertEqual(command["memberIds"] as? [String], ["bot-a", "bot-b"])
    }

}
