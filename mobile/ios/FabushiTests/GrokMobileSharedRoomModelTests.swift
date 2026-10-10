import XCTest
@testable import Fabushi

final class GrokMobileSharedRoomModelTests: XCTestCase {
    func testProjectsDesktopSharingContractAndDerivesHostState() {
        let state = GrokMobileSharedRoomModel.projectSharingState([
            "isEnabled": true,
            "selfAuthId": "auth-self",
            "pendingJoinRequests": [[
                "requestId": "request-1",
                "roomId": "room-1",
                "requesterAuthId": "guest-1",
                "requesterName": "Guest",
            ]],
            "rooms": [[
                "roomId": "room-1",
                "name": "Shared room",
                "hostAuthId": "auth-self",
                "members": [
                    [
                        "kind": "agent",
                        "authId": "auth-self",
                        "agentId": "agent-1",
                        "displayName": "Research",
                    ],
                    [
                        "kind": "human",
                        "authId": "guest-1",
                        "displayName": "Guest",
                    ],
                ],
            ]],
            "typingUsers": [],
        ])
        XCTAssertNotNil(state)

        let agent = MobileBotSummary(
            id: "agent-1",
            name: "Research",
            description: "Verify"
        )
        let candidate = MobileBotSummary(
            id: "agent-2",
            name: "Writer",
            description: "Draft"
        )
        let snapshot = GrokMobileSharedRoomModel.snapshot(
            agent: agent,
            roster: [agent, candidate],
            state: state!
        )
        XCTAssertEqual(snapshot.room?.roomId, "room-1")
        XCTAssertTrue(snapshot.isHost)
        XCTAssertEqual(snapshot.selfAgentIds, ["agent-1"])
        XCTAssertEqual(snapshot.candidates.map(\.id), ["agent-2"])
        XCTAssertEqual(snapshot.requests.map(\.requestId), ["request-1"])
    }

    func testRejectsMalformedSharingRowsInsteadOfSilentlyDroppingThem() {
        XCTAssertNil(
            GrokMobileSharedRoomModel.projectSharingState([
                "isEnabled": true,
                "selfAuthId": NSNull(),
                "pendingJoinRequests": [],
                "rooms": [[
                    "roomId": "room-1",
                    "name": "Shared room",
                    "hostAuthId": "host-1",
                    "members": [["kind": "agent"]],
                ]],
                "typingUsers": [],
            ])
        )
    }

    func testInviteProjectionRequiresStableDesktopFields() {
        XCTAssertEqual(
            GrokMobileSharedRoomModel.projectInviteResult([
                "status": "ok",
                "shareUrl": "https://example.test/share",
                "expiresAtMs": 123.0,
                "roomId": "room-1",
            ]),
            .ok(
                shareURL: "https://example.test/share",
                expiresAtMs: 123.0,
                roomId: "room-1"
            )
        )
        XCTAssertNil(
            GrokMobileSharedRoomModel.projectInviteResult([
                "status": "ok",
                "shareUrl": "https://example.test/share",
            ])
        )
    }

    func testLifecycleFenceRejectsAccountAgentAndGenerationDrift() {
        let fence = GrokMobileSharedRoomModel.LifecycleFence(
            accountScopeKey: "account-a",
            agentId: "agent-1",
            generation: 3
        )
        XCTAssertTrue(
            GrokMobileSharedRoomModel.accepts(
                fence,
                accountScopeKey: "account-a",
                agentId: "agent-1",
                generation: 3
            )
        )
        XCTAssertFalse(
            GrokMobileSharedRoomModel.accepts(
                fence,
                accountScopeKey: "account-b",
                agentId: "agent-1",
                generation: 3
            )
        )
        XCTAssertFalse(
            GrokMobileSharedRoomModel.accepts(
                fence,
                accountScopeKey: "account-a",
                agentId: "agent-2",
                generation: 3
            )
        )
        XCTAssertFalse(
            GrokMobileSharedRoomModel.accepts(
                fence,
                accountScopeKey: "account-a",
                agentId: "agent-1",
                generation: 4
            )
        )
    }

    func testRejectsMissingOrWrongTopLevelSharingFields() {
        let valid: [String: Any] = [
            "isEnabled": true,
            "selfAuthId": "auth-self",
            "pendingJoinRequests": [],
            "rooms": [],
            "typingUsers": [],
        ]

        var missingEnabled = valid
        missingEnabled.removeValue(forKey: "isEnabled")
        XCTAssertNil(GrokMobileSharedRoomModel.projectSharingState(missingEnabled))

        var wrongRequests = valid
        wrongRequests["pendingJoinRequests"] = ["not": "an array"]
        XCTAssertNil(GrokMobileSharedRoomModel.projectSharingState(wrongRequests))

        var wrongRooms = valid
        wrongRooms["rooms"] = "rooms"
        XCTAssertNil(GrokMobileSharedRoomModel.projectSharingState(wrongRooms))

        var wrongTyping = valid
        wrongTyping["typingUsers"] = NSNull()
        XCTAssertNil(GrokMobileSharedRoomModel.projectSharingState(wrongTyping))
    }

    func testControllerPendingPolicyDeduplicatesOnlyMatchingKeys() {
        var pending = Set(["agent:agent-1"])

        XCTAssertFalse(
            MobileBotSharedRoomPendingPolicy.canBegin(
                "agent:agent-1",
                pending: pending
            )
        )
        XCTAssertTrue(
            MobileBotSharedRoomPendingPolicy.canBegin(
                "request:request-1",
                pending: pending
            )
        )

        pending = MobileBotSharedRoomPendingPolicy.adding(
            "request:request-1",
            to: pending
        )
        XCTAssertEqual(
            pending,
            Set(["agent:agent-1", "request:request-1"])
        )

        pending = MobileBotSharedRoomPendingPolicy.removing(
            "agent:agent-1",
            from: pending
        )
        XCTAssertEqual(pending, Set(["request:request-1"]))
    }

    func testUIActionPolicyShowsPeopleToEveryoneAndKeepsDesktopRemovalRules() {
        let host = GrokMobileSharedRoomModel.Member(
            kind: .human,
            authId: "auth-host",
            agentId: nil,
            displayName: "Host",
            avatarURL: nil
        )
        let guest = GrokMobileSharedRoomModel.Member(
            kind: .human,
            authId: "auth-guest",
            agentId: nil,
            displayName: "Guest",
            avatarURL: nil
        )
        let selfAgent = GrokMobileSharedRoomModel.Member(
            kind: .agent,
            authId: "auth-guest",
            agentId: "agent-1",
            displayName: "Agent",
            avatarURL: nil
        )
        let room = GrokMobileSharedRoomModel.Room(
            roomId: "room-1",
            name: "Shared room",
            hostAuthId: "auth-host",
            members: [host, guest, selfAgent],
            avatarDataURL: nil
        )

        XCTAssertEqual(
            MobileBotSharedRoomActionPolicy.people(in: room).map(\.authId),
            ["auth-host", "auth-guest"]
        )
        XCTAssertFalse(
            MobileBotSharedRoomActionPolicy.canRemoveHuman(
                guest,
                room: room,
                isHost: false
            )
        )
        XCTAssertTrue(
            MobileBotSharedRoomActionPolicy.canRemoveHuman(
                guest,
                room: room,
                isHost: true
            )
        )
        XCTAssertFalse(
            MobileBotSharedRoomActionPolicy.canRemoveHuman(
                host,
                room: room,
                isHost: true
            )
        )
        XCTAssertTrue(
            MobileBotSharedRoomActionPolicy.canRemoveOwnAgent(
                "agent-1",
                selfAgentIds: ["agent-1"]
            )
        )
    }

    func testHeaderPresentationUsesSameSharedRoomLifecycleOwner() {
        XCTAssertEqual(
            String(describing: MobileBotSharedRoomTriggerPresentation.header),
            "header"
        )
        XCTAssertEqual(
            String(describing: MobileBotSharedRoomTriggerPresentation.settings),
            "settings"
        )
    }

}
