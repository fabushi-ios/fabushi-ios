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
    func testParseBotProjectsCanonicalAgentRowState() throws {
        let bot = try XCTUnwrap(GrokMobileBotService.parseBot([
            "id": "agent-1",
            "name": "Research",
            "description": "Verify",
            "hidden": true,
            "unread": true,
            "conversationId": "conversation:agent-1",
        ]))
        XCTAssertTrue(bot.hidden)
        XCTAssertTrue(bot.unread)
        XCTAssertEqual(bot.conversationId, "conversation:agent-1")
    }

    @MainActor
    func testAgentRowMutationsUseCanonicalHostCommands() {
        let hidden = GrokMobileBotService.setHiddenCommand(
            id: "agent-1",
            hidden: true,
            requestId: "hidden-1"
        )
        XCTAssertEqual(hidden["type"] as? String, "bot.setHidden")
        XCTAssertEqual(hidden["id"] as? String, "agent-1")
        XCTAssertEqual(hidden["hidden"] as? Bool, true)

        let unread = GrokMobileBotService.setUnreadCommand(
            id: "agent-1",
            unread: true,
            requestId: "unread-1"
        )
        XCTAssertEqual(unread["type"] as? String, "bot.update")
        XCTAssertEqual(unread["id"] as? String, "agent-1")
        XCTAssertEqual(unread["unread"] as? Bool, true)
    }

    @MainActor
    func testAsyncTaskProjectionPreservesHostIdentity() throws {
        let task = try XCTUnwrap(GrokMobileBotService.parseAsyncTask([
            "id": "shell-1",
            "kind": "shell",
            "label": "Run validation",
            "detail": "cargo test",
            "resourceId": "process-1",
        ]))
        XCTAssertEqual(task.id, "shell-1")
        XCTAssertEqual(task.kind, "shell")
        XCTAssertEqual(task.label, "Run validation")
        XCTAssertEqual(task.detail, "cargo test")
        XCTAssertEqual(task.resourceId, "process-1")
    }

    func testNativeAgentSectionsMoveMembershipWithoutDuplicateOwnership() throws {
        let initial = [
            MobileAgentSidebarSection(id: "one", name: "One", agentIds: ["agent-1", "agent-2"]),
            MobileAgentSidebarSection(id: "two", name: "Two", agentIds: ["agent-3"]),
        ]
        let moved = MobileAgentSidebarSections.assigning(
            agentId: "agent-1",
            to: "two",
            in: initial
        )
        XCTAssertEqual(moved[0].agentIds, ["agent-2"])
        XCTAssertEqual(moved[1].agentIds, ["agent-3", "agent-1"])

        let unassigned = MobileAgentSidebarSections.assigning(
            agentId: "agent-1",
            to: nil,
            in: moved
        )
        XCTAssertFalse(unassigned.flatMap(\.agentIds).contains("agent-1"))
    }

    func testNativeAgentSectionsCreateAndNormalizeLikeDesktopSidebar() throws {
        let created = try XCTUnwrap(MobileAgentSidebarSections.creating(
            name: "  Research  ",
            with: "agent-1",
            in: [
                MobileAgentSidebarSection(id: "old", name: "Old", agentIds: ["agent-1", "agent-2"])
            ],
            id: "stable"
        ))
        XCTAssertEqual(created.first?.id, "section-stable")
        XCTAssertEqual(created.first?.name, "Research")
        XCTAssertEqual(created.first?.agentIds, ["agent-1"])
        XCTAssertEqual(created[1].agentIds, ["agent-2"])

        let normalized = MobileAgentSidebarSections.normalized([
            MobileAgentSidebarSection(id: " one ", name: "One", agentIds: ["a", "a", "b"]),
            MobileAgentSidebarSection(id: "one", name: "Duplicate", agentIds: ["c"]),
            MobileAgentSidebarSection(id: "two", name: "Two", agentIds: ["b", "c"]),
        ])
        XCTAssertEqual(normalized.map(\.id), ["one", "two"])
        XCTAssertEqual(normalized[0].agentIds, ["a", "b"])
        XCTAssertEqual(normalized[1].agentIds, ["c"])
    }

    @MainActor
    func testParseBotRejectsMissingIdentity() {
        XCTAssertNil(GrokMobileBotService.parseBot(["name": "Missing id"]))
    }
    @MainActor
    func testDeleteConfirmationCopyDistinguishesBotAndGroupDestruction() {
        let bot = MobileBotSummary(id: "bot-1", name: "Research", description: "")
        let group = MobileBotSummary(
            id: "group-1",
            name: "Study Group",
            description: "",
            isGroup: true,
            memberIds: ["bot-1"]
        )

        XCTAssertEqual(
            mobileBotDeleteDescription(bot),
            "这会永久删除该 Bot 及其聊天记录。此操作无法撤销。"
        )
        XCTAssertEqual(
            mobileBotDeleteDescription(group),
            "这会永久删除该群组及其聊天记录。群组中的 Bots 不会被删除，仍可单独使用。此操作无法撤销。"
        )
    }

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

    @MainActor
    func testSettingsProjectionAndCommandsUseCanonicalHostFields() throws {
        let bot = try XCTUnwrap(GrokMobileBotService.parseBot([
            "id": "agent-1",
            "name": "Research",
            "description": "Verify sources",
            "title": "Research assistant",
            "notifyOnUpdates": false,
        ]))
        XCTAssertEqual(bot.title, "Research assistant")
        XCTAssertFalse(bot.notifyOnUpdatesEnabled)

        let individual = GrokMobileBotService.agentProfileUpdateCommand(
            id: "agent-1",
            isGroup: false,
            name: "Renamed",
            title: "New title",
            description: "New description",
            requestId: "profile-1"
        )
        XCTAssertEqual(individual["type"] as? String, "bot.update")
        XCTAssertEqual(individual["name"] as? String, "Renamed")
        XCTAssertEqual(individual["title"] as? String, "New title")
        XCTAssertEqual(individual["description"] as? String, "New description")

        let group = GrokMobileBotService.agentProfileUpdateCommand(
            id: "group-1",
            isGroup: true,
            name: "Room",
            title: "Must not cross the group boundary",
            description: "Coordination",
            requestId: "profile-2"
        )
        XCTAssertEqual(group["type"] as? String, "group.update")
        XCTAssertNil(group["title"])

        let notifications = GrokMobileBotService.agentNotificationUpdateCommand(
            id: "agent-1",
            isEnabled: true,
            requestId: "notify-1"
        )
        XCTAssertEqual(notifications["type"] as? String, "bot.update")
        XCTAssertEqual(notifications["notifyOnUpdates"] as? Bool, true)
    }

    @MainActor
    func testInstalledMiniAppMergePreservesCanonicalSettingsProjection() throws {
        let surface = MobileBotSummary(
            id: "global-dharma-bot",
            name: "Host name",
            description: "Host description",
            title: "Canonical title",
            notifyOnUpdatesEnabled: false
        )
        let installed = MobileBotSummary(
            id: "global-dharma-bot",
            name: "全球法布施",
            description: "Installed metadata",
            miniAppId: GlobalDharmaMiniAppBridge.globalDharmaId,
            menuButtonText: "打开应用"
        )

        let merged = try XCTUnwrap(GrokMobileBotService.mergeBots([installed], [surface]).first)
        XCTAssertEqual(merged.name, "全球法布施")
        XCTAssertEqual(merged.title, "Canonical title")
        XCTAssertFalse(merged.notifyOnUpdatesEnabled)
        XCTAssertEqual(merged.miniAppId, GlobalDharmaMiniAppBridge.globalDharmaId)
    }


}
