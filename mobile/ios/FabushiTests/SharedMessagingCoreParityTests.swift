import XCTest
@testable import Fabushi

final class SharedMessagingCoreParityTests: XCTestCase {
    func testMessageAddressAndReplicaOrdering() {
        XCTAssertTrue(MessageReference.isMessageAddress("t12ua4"))
        XCTAssertTrue(MessageReference.isMessageAddress("tbs7"))
        XCTAssertFalse(MessageReference.isMessageAddress("t12u3a4"))
        XCTAssertFalse(MessageReference.isMessageAddress("tbad"))
        XCTAssertEqual(ReplicaOrdering.transcriptReplicaKey(agentID: "a1"), "transcript:a1")
        XCTAssertEqual(ReplicaOrdering.rosterReplicaKey, "roster")
    }

    func testSendPreviewVariants() {
        XCTAssertEqual(SendMessagePreview.text(for: .cursorAgent(title: "  Build  ")), "Cursor agent: Build")
        XCTAssertEqual(SendMessagePreview.text(for: .emailDraft(subject: "", body: "Body")), "Body")
        XCTAssertEqual(SendMessagePreview.text(for: .autoReviewApproval(summary: "Ship")), "Approval required: Ship")
        XCTAssertEqual(SendMessagePreview.text(for: .connector(variant: "connected", connector: "Slack")), "Slack connected")
        XCTAssertEqual(SendMessagePreview.text(for: .connectors(["Slack", "GitHub"])), "Connect Slack, GitHub")
    }

    func testOptimisticReactionToggleMatchesDesktopReactionRootSemantics() {
        let original = [
            ChatReaction(reaction: "👍", count: 2, chosenByMe: false),
            ChatReaction(reaction: "❤️", count: 1, chosenByMe: true),
        ]

        let added = projectChatReactionToggle(original, reaction: "👍", enabled: true)
        XCTAssertEqual(
            added,
            [
                ChatReaction(reaction: "👍", count: 3, chosenByMe: true),
                ChatReaction(reaction: "❤️", count: 1, chosenByMe: true),
            ]
        )

        let removed = projectChatReactionToggle(added, reaction: "👍", enabled: false)
        XCTAssertEqual(removed, original)

        XCTAssertEqual(
            projectChatReactionToggle(
                [ChatReaction(reaction: "❤️", count: 1, chosenByMe: true)],
                reaction: "❤️",
                enabled: false
            ),
            []
        )
        XCTAssertEqual(
            projectChatReactionToggle(original, reaction: "😂", enabled: false),
            original
        )
    }

    func testSidebarNormalizationAndFolds() {
        let normalized = SidebarSections.normalize([
            .init(id: " one ", name: "One", agentIDs: ["a", "a", ""]),
            .init(id: "two", name: "Two", agentIDs: ["a", "b"]),
            .init(id: "__agents__", name: "Bad", agentIDs: ["c"]),
        ])
        XCTAssertEqual(normalized.map(\.id), ["one", "two", "__agents__"])
        XCTAssertEqual(normalized[0].agentIDs, ["a"])
        XCTAssertEqual(normalized[1].agentIDs, ["b"])
        let folded = SidebarSections.withFolds(normalized, collapsedSectionIDs: ["two"])
        XCTAssertEqual(folded[1].isCollapsed, true)
    }

    func testSandTextAndSlug() {
        XCTAssertEqual(SandText.clampLine("  hello \n world  ", maxLength: 20), "hello world")
        XCTAssertEqual(SandText.clampBlock("  a\nb  ", maxLength: 10), "a\nb")
        XCTAssertEqual(SandText.decapitalize("Hello"), "hello")
        XCTAssertEqual(SandText.slugifyName("Hello World!", fallbackPrefix: "agent", nowMilliseconds: 5), "hello-world")
        XCTAssertEqual(SandText.slugifyName("***", fallbackPrefix: "agent", nowMilliseconds: 5), "agent-5")
    }

    func testWriteEpochInvalidatesSnapshotsAndOldSettlers() {
        let epoch = WriteEpoch()
        let snapshot = epoch.snapshot()
        let settle = epoch.begin()
        XCTAssertTrue(epoch.isStale(snapshot))
        settle()
        XCTAssertTrue(epoch.isStale(snapshot))
        let after = epoch.snapshot()
        XCTAssertFalse(epoch.isStale(after))
        let staleSettler = epoch.begin()
        epoch.reset()
        staleSettler()
        XCTAssertFalse(epoch.isStale(epoch.snapshot()))
    }

    func testUsageAndSendAcceptanceContracts() {
        XCTAssertTrue(UsageContract.supportedDashboardActions.contains(.requestLimitIncrease))
        XCTAssertEqual(SendAcceptanceContract.nonceDigestMismatch, "send/nonce-digest-mismatch")
        XCTAssertEqual(SendAcceptanceContract.hostAccountSlot, "host")
    }
}
