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

    func testComputerUpdateProjectionUsesAccountWideRosterAndFailsClosed() {
        let agent = bot("agent", name: "Agent")
        let available = RemoteComputerAgentBoxSnapshot(
            agentID: agent.id,
            state: "running",
            vncURL: nil,
            imageUpdateAvailable: true
        )

        XCTAssertEqual(
            MobileCommandPaletteComputerUpdateProjection.action(
                agent: agent,
                status: available,
                workingAgentNames: [],
                isPending: false,
                isQueued: false
            ),
            .ready
        )
        XCTAssertEqual(
            MobileCommandPaletteComputerUpdateProjection.action(
                agent: agent,
                status: available,
                workingAgentNames: ["Writer"],
                isPending: false,
                isQueued: false
            ),
            .busyOverride
        )
        XCTAssertNil(
            MobileCommandPaletteComputerUpdateProjection.action(
                agent: agent,
                status: available,
                workingAgentNames: [],
                isPending: true,
                isQueued: false
            )
        )
        XCTAssertNil(
            MobileCommandPaletteComputerUpdateProjection.action(
                agent: agent,
                status: available,
                workingAgentNames: [],
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
                workingAgentNames: [],
                isPending: false,
                isQueued: false
            )
        )
        XCTAssertNil(
            MobileCommandPaletteComputerUpdateProjection.action(
                agent: bot("group", name: "Group", isGroup: true),
                status: available,
                workingAgentNames: [],
                isPending: false,
                isQueued: false
            )
        )

        let idle = [
            bot("idle", name: "Idle"),
            bot("group", name: "Group", isGroup: true),
        ]
        XCTAssertTrue(
            MobileCommandPaletteComputerUpdateProjection.workingAgentNames(idle).isEmpty
        )

        let running = MobileBotSummary(
            id: "running",
            name: "Writer",
            description: "",
            isRunning: true
        )
        let runningTwo = MobileBotSummary(
            id: "running-two",
            name: "Researcher",
            description: "",
            isRunning: true
        )
        XCTAssertEqual(
            MobileCommandPaletteComputerUpdateProjection.workingAgentNames(
                [agent, running, runningTwo]
            ),
            ["Writer", "Researcher"]
        )
        XCTAssertEqual(
            MobileCommandPaletteComputerUpdateProjection.confirmationDelaySeconds,
            3
        )
        XCTAssertEqual(
            MobileCommandPaletteComputerUpdateProjection.workingTitle(["Writer", "Researcher"]),
            "Update while agents are working?"
        )
        XCTAssertTrue(
            MobileCommandPaletteComputerUpdateProjection
                .workingDescription(["Writer", "Researcher"])
                .contains("are working on Fabushi's computer")
        )
    }

    func testRootNotificationProjectionValidatesActionsAndDedupeCount() {
        let tray = projectMobileRootNotificationTray([
            "kind": "error",
            "id": "tray-1",
            "title": "Provider busy",
            "detail": "Retry later",
            "requestId": "request-1",
            "errorKind": "provider_overloaded",
            "count": 3,
            "actions": [
                [
                    "kind": "open-url",
                    "label": "Status",
                    "url": "https://status.example.com",
                ],
                [
                    "kind": "open-url",
                    "label": "Blocked",
                    "url": "file:///private/secret",
                ],
                [
                    "kind": "dashboard-action",
                    "label": "Retry",
                    "action": "retry-provider",
                    "args": ["provider": "cursor"],
                    "successMessage": "Retry requested",
                ],
            ],
        ])

        XCTAssertEqual(tray?.id, "tray-1")
        XCTAssertEqual(tray?.requestID, "request-1")
        XCTAssertEqual(tray?.errorKind, "provider_overloaded")
        XCTAssertEqual(tray?.count, 3)
        XCTAssertEqual(tray?.actions.map(\.label), ["Status", "Retry"])

        guard let first = tray?.actions.first else {
            return XCTFail("expected validated open-url action")
        }
        if case .openURL(let url) = first.kind {
            XCTAssertEqual(url.absoluteString, "https://status.example.com")
        } else {
            XCTFail("expected open-url action")
        }

        guard let last = tray?.actions.last else {
            return XCTFail("expected validated dashboard action")
        }
        if case .dashboard(let action, let args, let successMessage) = last.kind {
            XCTAssertEqual(action, "retry-provider")
            XCTAssertEqual(args["provider"] as? String, "cursor")
            XCTAssertEqual(successMessage, "Retry requested")
        } else {
            XCTFail("expected dashboard action")
        }
    }

    func testRootNotificationReducerUsesHostChangedEventsAsSourceOfTruth() {
        let initial = projectMobileRootNotificationTrays([
            [
                "kind": "error",
                "id": "tray-1",
                "title": "First",
            ],
        ])
        XCTAssertEqual(initial.map(\.id), ["tray-1"])

        let pushed = reduceMobileRootNotificationEvent(
            initial,
            event: [
                "type": "tray.changed",
                "action": "pushed",
                "tray": [
                    "kind": "error",
                    "id": "tray-2",
                    "title": "Second",
                ],
            ]
        )
        XCTAssertEqual(pushed.map(\.id), ["tray-1", "tray-2"])

        let updated = reduceMobileRootNotificationEvent(
            pushed,
            event: [
                "type": "tray.changed",
                "action": "pushed",
                "tray": [
                    "kind": "error",
                    "id": "tray-2",
                    "title": "Second updated",
                    "count": 2,
                ],
            ]
        )
        XCTAssertEqual(updated.count, 2)
        XCTAssertEqual(updated.last?.title, "Second updated")
        XCTAssertEqual(updated.last?.count, 2)

        let dismissed = reduceMobileRootNotificationEvent(
            updated,
            event: [
                "type": "tray.changed",
                "action": "dismissed",
                "id": "tray-1",
            ]
        )
        XCTAssertEqual(dismissed.map(\.id), ["tray-2"])

        let cleared = reduceMobileRootNotificationEvent(
            dismissed,
            event: [
                "type": "tray.changed",
                "action": "cleared",
            ]
        )
        XCTAssertTrue(cleared.isEmpty)
    }

}
