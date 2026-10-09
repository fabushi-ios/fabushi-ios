import XCTest
@testable import Fabushi

@MainActor
final class MobileSignedInOnboardingParityTests: XCTestCase {
    func testSignedInRouteMatchesDesktopFirstRunContract() {
        XCTAssertEqual(
            MobileSignedInOnboardingContract.resolveRoute(
                isSignedIn: false,
                hasSeenOnboarding: false,
                agentCount: 0
            ),
            .signIn
        )
        XCTAssertEqual(
            MobileSignedInOnboardingContract.resolveRoute(
                isSignedIn: true,
                hasSeenOnboarding: false,
                agentCount: nil
            ),
            .onboarding
        )
        XCTAssertEqual(
            MobileSignedInOnboardingContract.resolveRoute(
                isSignedIn: true,
                hasSeenOnboarding: false,
                agentCount: 0
            ),
            .onboarding
        )
        XCTAssertEqual(
            MobileSignedInOnboardingContract.resolveRoute(
                isSignedIn: true,
                hasSeenOnboarding: true,
                agentCount: 0
            ),
            .shell
        )
        XCTAssertEqual(
            MobileSignedInOnboardingContract.resolveRoute(
                isSignedIn: true,
                hasSeenOnboarding: false,
                agentCount: 1
            ),
            .shell
        )
    }

    func testSignedInStepOrderAndBackNavigationStayDeterministic() {
        XCTAssertEqual(MobileSignedInOnboardingStep.meet.next, .computerDemo)
        XCTAssertEqual(MobileSignedInOnboardingStep.computerDemo.next, .jobs)
        XCTAssertEqual(MobileSignedInOnboardingStep.jobs.next, .tools)
        XCTAssertEqual(MobileSignedInOnboardingStep.tools.next, .create)
        XCTAssertEqual(MobileSignedInOnboardingStep.create.next, .handOff)
        XCTAssertEqual(MobileSignedInOnboardingStep.handOff.next, .completed)
        XCTAssertNil(MobileSignedInOnboardingStep.meet.previous)
        XCTAssertEqual(MobileSignedInOnboardingStep.create.previous, .tools)
    }

    func testOnboardingCharacterCatalogExcludesGeneralEditorBlackAndMatchesDesktopOrder() {
        XCTAssertEqual(
            MobileOnboardingCharacterCatalog.colorIds,
            ["brown", "red", "orange", "yellow", "green", "cyan", "blue", "violet", "magenta", "gray"]
        )
        XCTAssertEqual(
            MobileOnboardingCharacterCatalog.shapeIds,
            ["blob", "pebble", "squircle", "tablet", "wedge", "hex", "cloud", "teardrop"]
        )
        XCTAssertFalse(MobileOnboardingCharacterCatalog.colorIds.contains("black"))

        var draft = MobileSignedInOnboardingDraft()
        draft.color = "black"
        draft.shape = "unknown"
        XCTAssertEqual(draft.normalized.color, "blue")
        XCTAssertEqual(draft.normalized.shape, "blob")
        XCTAssertFalse(draft.canSubmit)
        draft.name = "  Researcher  "
        XCTAssertTrue(draft.canSubmit)
    }

    func testDailyToolCatalogMatchesDesktopSourceOrderAndFiltering() {
        XCTAssertEqual(MobileOnboardingTool.all.count, 45)
        XCTAssertEqual(Array(MobileOnboardingTool.all.prefix(5).map(\.label)), [
            "Workspace", "Slack", "Notion", "Salesforce", "Microsoft 365",
        ])
        XCTAssertEqual(Array(MobileOnboardingTool.all.suffix(5).map(\.label)), [
            "Mixpanel", "Snowflake", "Databricks", "Mailchimp",
        ].suffix(5))
        XCTAssertEqual(MobileOnboardingTool.filtered("micro").map(\.label), ["Microsoft 365"])
        XCTAssertEqual(MobileOnboardingTool.filtered("  slack ").map(\.label), ["Slack"])
    }

    func testSuggestionRankingAndPersonaIdentityMatchDesktopContract() {
        let selected = MobileOnboardingSuggestion.selected(for: ["Slack", "GitHub"])
        XCTAssertEqual(selected.count, 10)
        XCTAssertEqual(selected[0].suggestion.id, "channel-digest")
        XCTAssertEqual(selected[0].renderedDescription, "Summarizes your Slack channels and flags what needs you")
        XCTAssertEqual(selected[1].suggestion.id, "qa-engineer")
        XCTAssertEqual(selected[1].renderedDescription, "Clicks through every new GitHub deploy and reports what breaks")
        XCTAssertEqual(Set(selected.map(\.id)).count, selected.count)

        let universal = MobileOnboardingSuggestion.selected(for: [])
        XCTAssertEqual(universal.first?.suggestion.id, "night-shift")
        let identities = MobileOnboardingSuggestion.identities(for: universal)
        XCTAssertEqual(identities[0], .init(color: "orange", shape: "hex"))
        XCTAssertEqual(identities[1], .init(color: "magenta", shape: "cloud"))
        XCTAssertEqual(Set(identities.prefix(8).map(\.shape)).count, 8)
    }

    func testDescriptionAndCreateCommandPreserveOnboardingFieldsAndHostLimits() throws {
        let description = MobileSignedInOnboardingContract.descriptionWithDailyTools(
            "Tracks invoices.",
            tools: ["QuickBooks", "Slack"]
        )
        XCTAssertEqual(
            description,
            "Tracks invoices. The user works with QuickBooks, Slack every day — start with those tools when suggesting connectors or taking on work."
        )

        let command = try GrokMobileBotService.createCommand(
            name: "  Invoice Chaser  ",
            description: String(repeating: "x", count: 2_200),
            avatarShape: "blob",
            avatarColor: "blue",
            requestId: "stable-onboarding-request",
            origin: "user",
            isKickstartRequested: true,
            templateId: "invoice-chaser"
        )
        XCTAssertEqual(command["type"] as? String, "bot.create")
        XCTAssertEqual(command["requestId"] as? String, "stable-onboarding-request")
        XCTAssertEqual(command["name"] as? String, "Invoice Chaser")
        XCTAssertEqual((command["description"] as? String)?.count, 2_000)
        XCTAssertEqual(command["avatarShape"] as? String, "blob")
        XCTAssertEqual(command["avatarColor"] as? String, "blue")
        XCTAssertEqual(command["origin"] as? String, "user")
        XCTAssertEqual(command["isKickstartRequested"] as? Bool, true)
        XCTAssertEqual(command["templateId"] as? String, "invoice-chaser")
    }

    func testTransportFailureMapsToRecoverableDesktopMessage() {
        let error = IOSCoordinatorPortClient.PortError(
            code: "source/transport-failure",
            message: "offline"
        )
        XCTAssertEqual(
            MobileSignedInOnboardingContract.createErrorMessage(error),
            "Can't reach your computer right now. Check your connection and try again."
        )
    }
}
