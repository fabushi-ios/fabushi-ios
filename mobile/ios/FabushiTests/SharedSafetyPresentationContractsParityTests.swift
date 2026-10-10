import XCTest
@testable import Fabushi

final class SharedSafetyPresentationContractsParityTests: XCTestCase {
    func testAutoReviewInstructionNormalizationDeduplicatesClampsAndDefaultsEnabled() {
        let long = String(repeating: "x", count: 1_200)
        let normalized = normalizeSandAutoReviewInstructions(
            allowInstructions: ["  allow this  ", "allow this", "", long],
            blockInstructions: ["block"]
        )
        XCTAssertTrue(normalized.isEnabled)
        XCTAssertEqual(normalized.allowInstructions.count, 2)
        XCTAssertEqual(normalized.allowInstructions[0], "allow this")
        XCTAssertEqual(normalized.allowInstructions[1].count, 1_000)

        let emojiLimited = normalizeSandAutoReviewInstructions(
            allowInstructions: [String(repeating: "😀", count: 600)]
        )
        XCTAssertEqual(emojiLimited.allowInstructions[0].utf16.count, 1_000)
        XCTAssertEqual(emojiLimited.allowInstructions[0].count, 500)
        XCTAssertEqual(normalized.blockInstructions, ["block"])
    }

    func testAutoReviewRedactionRemovesUrlCredentialsQueryAndInlineSecrets() {
        let raw = "see https://user:pass@example.com/path?token=abc#frag authorization=BearerSecret Bearer verysecret sk-testsecret"
        let redacted = redactSandAutoReviewInlineSecrets(raw)
        XCTAssertFalse(redacted.contains("user:pass"))
        XCTAssertFalse(redacted.contains("token=abc"))
        XCTAssertFalse(redacted.contains("verysecret"))
        XCTAssertFalse(redacted.contains("sk-testsecret"))
        XCTAssertTrue(redacted.contains("https://example.com/path"))

        let shaped = redactSandAutoReviewInlineSecrets("line1\n  token=secret\nline3")
        XCTAssertEqual(shaped, "line1\n  token=…\nline3")
    }

    func testSharedTextAndSidebarParsingMatchDesktopBounds() {
        XCTAssertEqual(SandText.clampLine("  hi\nthere  ", maxLength: 20), "hi there")
        XCTAssertEqual(SandText.clampBlock(String(repeating: "😀", count: 3), maxLength: 4).utf16.count, 4)

        let parsed = SidebarSections.parse([
            ["id": " first ", "name": "One", "agentIds": ["a", 42, "a"]],
            ["id": "", "name": "drop", "agentIds": ["b"]],
            ["id": "second", "agentIds": ["a", "b"]],
            "invalid",
        ])
        XCTAssertEqual(parsed.map(\.id), ["first", "second", SidebarSections.agentsSectionID])
        XCTAssertEqual(parsed[0].agentIDs, ["a"])
        XCTAssertEqual(parsed[1].agentIDs, ["b"])
    }

    func testSpotlightFencesOutsideTextAndStripsForgedMarkers() {
        let content = spotlightToolResultContent(
            source: "web<source>",
            content: [
                .init(type: "text", text: "a"),
                .init(type: "text", text: "b \(SPOTLIGHT_TAG)"),
                .init(type: "image", metadata: ["id": "1"]),
            ]
        )
        XCTAssertEqual(content.first?.text, "<\(SPOTLIGHT_TAG) source=\"websource\">")
        XCTAssertEqual(content.last?.text, "</\(SPOTLIGHT_TAG)>")
        XCTAssertTrue(content[1].text?.contains(SPOTLIGHT_TAG_REDACTION) == true)
        XCTAssertFalse(resolveSpotlightEnabled(envOverride: "false", checkStatsigGate: { true }))
        XCTAssertTrue(resolveSpotlightEnabled(envOverride: nil, checkStatsigGate: { true }))
    }

    func testTimelineEventsDescribeKnownChangesAndBuildWakeCue() {
        XCTAssertEqual(describeTimelineEvent(.nameChanged(to: "Builder")), "Renamed to Builder")
        XCTAssertEqual(
            describeTimelineEvent(.automationChanged(action: "enabled", automationName: "Morning")),
            "Enabled automation \"Morning\""
        )
        XCTAssertEqual(describeTimelineEvent(.unknown(type: "future")), "Updated this conversation")
        let wake = buildTimelineEventWakePrompt([.channelConnected(label: "Slack")])
        XCTAssertTrue(wake.hasPrefix(TIMELINE_EVENT_WAKE_CUE))
        XCTAssertTrue(wake.contains("Connected to Slack"))
        XCTAssertTrue(wake.contains("reply with SendMessage"))
    }
}
