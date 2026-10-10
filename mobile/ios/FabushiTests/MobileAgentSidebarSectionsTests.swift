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
}
