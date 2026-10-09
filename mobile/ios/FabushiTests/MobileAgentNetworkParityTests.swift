import XCTest
@testable import Fabushi

final class MobileAgentNetworkParityTests: XCTestCase {
    private func bot(
        _ id: String,
        updatedAtMs: Int64? = nil,
        composing: Bool = false,
        waiting: String? = nil,
        running: Bool = false,
        isGroup: Bool = false,
        members: [String] = [],
        partners: [String] = []
    ) -> MobileBotSummary {
        MobileBotSummary(
            id: id,
            name: id.uppercased(),
            description: "desc-\(id)",
            lastMessagePreview: "last-\(id)",
            updatedAtMs: updatedAtMs,
            isComposingMessage: composing,
            waitingReason: waiting,
            isRunning: running,
            isGroup: isGroup,
            memberIds: members,
            conversationPartnerIds: partners
        )
    }

    func testGateAvailabilityRetainsEnabledEmptyRoster() {
        XCTAssertEqual(MobileAgentNetworkAvailability.resolve(gateEnabled: false, hasAgents: true), .unavailable)
        XCTAssertEqual(MobileAgentNetworkAvailability.resolve(gateEnabled: true, hasAgents: false), .retainedEmptyRoster)
        XCTAssertEqual(MobileAgentNetworkAvailability.resolve(gateEnabled: true, hasAgents: true), .available)
    }

    func testEdgesDedupeMembershipAndMessagesAndRejectSelfOrUnknown() {
        let roster = [
            bot("a", partners: ["b", "b", "a", "missing"]),
            bot("b", partners: ["a"]),
            bot("g", isGroup: true, members: ["a", "b", "g", "missing", "a"]),
        ]
        let edges = MobileAgentNetworkModel.edges(roster)
        XCTAssertEqual(edges.filter { $0.kind == .message }.map(\.id), ["msg::a::b"])
        XCTAssertEqual(
            Set(edges.filter { $0.kind == .membership }.map(\.id)),
            Set(["member::g::a", "member::g::b"])
        )
    }

    func testRecentTalkingAndActivityPriority() {
        let now: Int64 = 1_000_000
        let a = bot("a", updatedAtMs: now - 5_000, running: true)
        let b = bot("b", updatedAtMs: now - 10_000, running: true)
        let edge = MobileAgentNetworkEdge(id: "msg::a::b", sourceId: "a", targetId: "b", kind: .message)
        XCTAssertEqual(MobileAgentNetworkModel.edgeActivity(edge, agentsById: ["a": a, "b": b], nowMs: now), .talking)

        let typing = bot("typing", composing: true, running: true)
        XCTAssertEqual(MobileAgentNetworkModel.activity(typing), .typing)
        let waiting = bot("waiting", composing: true, waiting: "Needs approval", running: true)
        XCTAssertEqual(MobileAgentNetworkModel.activity(waiting), .waiting)

        let old = bot("old", updatedAtMs: now - MobileAgentNetworkModel.recentWindowMs - 1)
        XCTAssertEqual(MobileAgentNetworkModel.edgeActivity(edge, agentsById: ["a": a, "b": old], nowMs: now), .idle)
    }

    func testSelectionReconcilesRosterChange() {
        let a = bot("a"), b = bot("b")
        let selected = MobileAgentNetworkSelection(agentId: "a")
        XCTAssertEqual(MobileAgentNetworkModel.reconcile(selected, agents: [a, b]), selected)
        XCTAssertNil(MobileAgentNetworkModel.reconcile(selected, agents: [b]))
        XCTAssertEqual(MobileAgentNetworkModel.toggle(selected, id: "a"), nil)
        XCTAssertEqual(MobileAgentNetworkModel.toggle(nil, id: "b"), .init(agentId: "b"))
    }

    func testConversationHistoryProjectsOnlyKnownPartners() {
        let rows: [[String: Any]] = [
            ["sourceAgentId": "a", "targetAgentId": "b"],
            ["conversationPartnerIds": ["b", "c", "missing", "a"]],
            ["participants": [["type": "agent", "id": "c"], ["type": "human", "id": "person"]]],
        ]
        XCTAssertEqual(
            MobileAgentNetworkModel.extractPartnerIds(
                from: rows,
                ownerId: "a",
                knownAgentIds: ["a", "b", "c"]
            ),
            ["b", "c"]
        )
    }

    func testAccountReconnectAndRosterFenceRejectsStaleResults() {
        let roster = [bot("a"), bot("b")]
        let fence = MobileAgentNetworkFence.capture(accountScopeKey: "account-1", reconnectGeneration: 4, roster: roster)
        XCTAssertTrue(fence.matches(accountScopeKey: "account-1", reconnectGeneration: 4, roster: Array(roster.reversed())))
        XCTAssertFalse(fence.matches(accountScopeKey: "account-2", reconnectGeneration: 4, roster: roster))
        XCTAssertFalse(fence.matches(accountScopeKey: "account-1", reconnectGeneration: 5, roster: roster))
        XCTAssertFalse(fence.matches(accountScopeKey: "account-1", reconnectGeneration: 4, roster: [bot("a")]))
    }

    func testGateSnapshotSupportsCanonicalBooleanAndStatsigShape() {
        XCTAssertEqual(
            MobileAgentNetworkModel.gateEnabled(from: ["featureGates": ["sand_agent_network": true]]),
            true
        )
        XCTAssertEqual(
            MobileAgentNetworkModel.gateEnabled(
                from: ["snapshot": ["featureGates": ["sand_agent_network": ["value": false]]]]
            ),
            false
        )
        XCTAssertNil(MobileAgentNetworkModel.gateEnabled(from: ["featureGates": [:]]))
    }

    func testResponsiveGeometryAndViewportBounds() {
        let small = MobileAgentNetworkLayout.positions(ids: ["a", "b", "c"], width: 320, height: 480)
        let wide = MobileAgentNetworkLayout.positions(ids: ["a", "b", "c"], width: 1024, height: 500)
        XCTAssertEqual(small.count, 3)
        XCTAssertEqual(wide.count, 3)
        XCTAssertNotEqual(small["b"], wide["b"])
        XCTAssertEqual(MobileAgentNetworkLayout.clampedScale(0.2), 1)
        XCTAssertEqual(MobileAgentNetworkLayout.clampedScale(9), 3)

        let clamped = MobileAgentNetworkLayout.clampedOffset(
            .init(width: 10_000, height: -10_000),
            scale: 2,
            size: .init(width: 320, height: 480)
        )
        XCTAssertLessThanOrEqual(clamped.width, 160)
        XCTAssertGreaterThanOrEqual(clamped.height, -720)
    }

    func testSummaryAndRelationshipReplacementPreserveCanonicalRosterFields() {
        let original = MobileBotSummary(
            id: "a",
            name: "Agent",
            description: "description",
            title: "Title",
            hidden: true,
            unread: true,
            conversationId: "conversation-a",
            lastMessagePreview: "hello",
            updatedAtMs: 123,
            isComposingMessage: true,
            waitingReason: "wait",
            isRunning: true,
            memberIds: [],
            conversationPartnerIds: ["old"]
        )
        let updated = original.replacingConversationPartnerIds(["b"])
        XCTAssertEqual(updated.conversationPartnerIds, ["b"])
        XCTAssertEqual(updated.conversationId, original.conversationId)
        XCTAssertEqual(updated.hidden, original.hidden)
        XCTAssertEqual(updated.lastMessagePreview, original.lastMessagePreview)
        XCTAssertEqual(updated.waitingReason, original.waitingReason)

        XCTAssertEqual(
            MobileAgentNetworkModel.summary(
                agents: [updated, bot("g", isGroup: true, members: ["a"])],
                edges: [MobileAgentNetworkEdge(id: "msg::a::b", sourceId: "a", targetId: "b", kind: .message)]
            ),
            "1 agent · 1 group · 1 message link"
        )
    }
}
