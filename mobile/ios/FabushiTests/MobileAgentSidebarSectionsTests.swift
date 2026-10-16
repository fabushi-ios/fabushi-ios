import XCTest
@testable import Fabushi

final class MobileAgentSidebarSectionsTests: XCTestCase {
    func testProjectionKeepsPinnedNamedAndUnassignedSectionsInStableOrder() {
        let sections = [
            MobileAgentSidebarSection(id: "ops", name: "运营部", agentIds: ["b"], isCollapsed: false),
            MobileAgentSidebarSection(id: "qa", name: "测试部", agentIds: ["c"], isCollapsed: true),
        ]

        let projected = MobileAgentSidebarSections.projected(
            agentIds: ["a", "b", "c", "d"],
            pinnedAgentIds: ["a"],
            sections: sections,
            searching: false
        )

        XCTAssertEqual(
            projected.map(\.id),
            [
                MobileAgentSidebarSections.pinnedSectionID,
                "ops",
                "qa",
                MobileAgentSidebarSections.unassignedSectionID,
            ]
        )
        XCTAssertEqual(projected[0].agentIds, ["a"])
        XCTAssertEqual(projected[1].agentIds, ["b"])
        XCTAssertTrue(projected[2].isCollapsed)
        XCTAssertEqual(projected[3].agentIds, ["d"])
    }

    func testProjectionMakesPinnedOwnershipWinAndSearchExpandsMatches() {
        let sections = [
            MobileAgentSidebarSection(id: "ops", name: "运营部", agentIds: ["a", "b"], isCollapsed: true),
            MobileAgentSidebarSection(id: "empty", name: "空", agentIds: ["z"], isCollapsed: true),
        ]

        let projected = MobileAgentSidebarSections.projected(
            agentIds: ["a", "b"],
            pinnedAgentIds: ["a"],
            sections: sections,
            searching: true
        )

        XCTAssertEqual(
            projected.map(\.id),
            [MobileAgentSidebarSections.pinnedSectionID, "ops"]
        )
        XCTAssertEqual(projected[0].agentIds, ["a"])
        XCTAssertEqual(projected[1].agentIds, ["b"])
        XCTAssertFalse(projected[1].isCollapsed)
    }

    func testNormalizationRejectsAllSyntheticSectionIDsAndDuplicateOwnership() {
        let normalized = MobileAgentSidebarSections.normalized([
            .init(id: MobileAgentSidebarSections.pinnedSectionID, name: "fake pin", agentIds: ["a"]),
            .init(id: "ops", name: "Ops", agentIds: ["a", "a", "b"]),
            .init(id: "qa", name: "QA", agentIds: ["b", "c"]),
            .init(id: MobileAgentSidebarSections.unassignedSectionID, name: "fake unassigned", agentIds: ["d"]),
            .init(id: "__agents__", name: "fake agents", agentIds: ["e"]),
        ])

        XCTAssertEqual(normalized.map(\.id), ["ops", "qa"])
        XCTAssertEqual(normalized[0].agentIds, ["a", "b"])
        XCTAssertEqual(normalized[1].agentIds, ["c"])
    }

    func testCollapseToggleChangesOnlyTargetSectionAndRoundTripsFoundationValue() {
        let input = [
            MobileAgentSidebarSection(id: "one", name: "One", agentIds: ["a"], isCollapsed: false),
            MobileAgentSidebarSection(id: "two", name: "Two", agentIds: ["b"], isCollapsed: true),
        ]

        let collapsed = MobileAgentSidebarSections.togglingCollapsed(input, sectionId: "one")
        XCTAssertEqual(collapsed[0].isCollapsed, true)
        XCTAssertEqual(collapsed[1].isCollapsed, true)

        let restored = MobileAgentSidebarSections.canonical(
            from: MobileAgentSidebarSections.foundationValue(collapsed)
        )
        XCTAssertEqual(restored, collapsed)

        XCTAssertEqual(
            MobileAgentSidebarSections.togglingCollapsed(collapsed, sectionId: "missing"),
            collapsed
        )
    }

    func testFallbackCollapseStateIsAccountScoped() {
        let suite = "FabushiTests.MobileAgentSidebarSections.\(UUID().uuidString)"
        guard let defaults = UserDefaults(suiteName: suite) else {
            XCTFail("Failed to create isolated UserDefaults")
            return
        }
        defer { defaults.removePersistentDomain(forName: suite) }

        let accountA = [
            MobileAgentSidebarSection(id: "one", name: "One", agentIds: ["a"], isCollapsed: true)
        ]
        MobileAgentSidebarSections.persistFallback(
            accountA,
            accountScopeKey: "account-a",
            defaults: defaults
        )

        XCTAssertEqual(
            MobileAgentSidebarSections.loadFallback(
                accountScopeKey: "account-a",
                defaults: defaults
            ),
            accountA
        )
        XCTAssertEqual(
            MobileAgentSidebarSections.loadFallback(
                accountScopeKey: "account-b",
                defaults: defaults
            ),
            []
        )
    }
    func testSidebarVisualProjectionMatchesDesktopStatusPriorityAndIdentity() {
        let blocked = MobileBotSummary(
            id: "blocked",
            name: "Blocked",
            description: "",
            title: "  Reviewer  ",
            unread: true,
            isComposingMessage: true,
            waitingReason: "Approval required",
            isRunning: true
        )
        let blockedProjection = projectMobileSidebarAgentVisual(blocked, isPinned: true)
        XCTAssertEqual(blockedProjection.statusBadge, .blocked)
        XCTAssertEqual(blockedProjection.statusLabel, "Needs attention")
        XCTAssertFalse(blockedProjection.isTyping)
        XCTAssertFalse(blockedProjection.isWorking)
        XCTAssertEqual(blockedProjection.title, "Reviewer")
        XCTAssertTrue(blockedProjection.isPinned)
        XCTAssertEqual(mobileBotHomeSubtitle(blocked), "Approval required")

        let typing = MobileBotSummary(
            id: "typing",
            name: "Typing",
            description: "",
            unread: false,
            isComposingMessage: true,
            waitingReason: nil,
            isRunning: false
        )
        let typingProjection = projectMobileSidebarAgentVisual(typing, isPinned: false)
        XCTAssertEqual(typingProjection.statusBadge, .working)
        XCTAssertEqual(typingProjection.statusLabel, "Working")
        XCTAssertTrue(typingProjection.isTyping)
        XCTAssertTrue(typingProjection.isWorking)
        XCTAssertEqual(mobileBotHomeSubtitle(typing), "正在输入…")

        let unread = MobileBotSummary(
            id: "unread",
            name: "Unread",
            description: "",
            unread: true,
            isRunning: true
        )
        XCTAssertEqual(
            projectMobileSidebarAgentVisual(unread, isPinned: false).statusBadge,
            .unread,
            "Unread marker must win over the working presence marker"
        )

        let group = MobileBotSummary(
            id: "group",
            name: "Group",
            description: "",
            title: "must-not-render",
            isGroup: true
        )
        XCTAssertNil(projectMobileSidebarAgentVisual(group, isPinned: false).title)

        let idle = MobileBotSummary(id: "idle", name: "Idle", description: "")
        XCTAssertNil(projectMobileSidebarAgentVisual(idle, isPinned: false).statusBadge)
    }

}
