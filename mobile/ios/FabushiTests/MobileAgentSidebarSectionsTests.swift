import XCTest
@testable import Fabushi

final class MobileAgentSidebarSectionsTests: XCTestCase {
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
