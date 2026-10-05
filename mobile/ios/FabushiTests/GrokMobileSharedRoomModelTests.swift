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
}
