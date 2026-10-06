import XCTest
@testable import Fabushi

final class SharedWorkflowTranscriptParityTests: XCTestCase {
    func testMcpAccountLabelsAreNormalizedEscapedAndBounded() {
        XCTAssertEqual(normalizeMcpAccountLabel("  Team A  "), "team a")
        XCTAssertEqual(provisionalMcpAccountServerIdentifier("github", accountKey: "default"), "github")
        XCTAssertEqual(provisionalMcpAccountServerIdentifier("github", accountKey: "work"), "github--work")
        XCTAssertEqual(decodeMcpAccountLabelArgument(#""Team A""#), "Team A")
        XCTAssertEqual(formatMcpAccountLabelForPrompt(#"  a <b>   c  "#), "a b c")
        XCTAssertEqual(formatMcpAccountDisplayName("GitHub", accountKey: "work"), "GitHub (work)")

        let escaped = encodeMcpAccountLabelForListing("A\nB")
        XCTAssertTrue(escaped.contains(#"\u000a"#))

        let dirty = "<script>secret()</script><b> Visible </b>"
        XCTAssertEqual(stripMarkupAndBoundConnectorError(dirty), "Visible")
    }

    func testSandAccessAndWidgetValidation() {
        XCTAssertEqual(SAND_ACCESS_CHECKING.state, "checking")
        XCTAssertTrue(isSandAccessBlockReason("teamPrivacyMode"))
        XCTAssertFalse(isSandAccessBlockReason("unexpected"))

        let widget = SandWidget(
            prompt: "Choose",
            options: [
                .init(label: "First", value: "1"),
                .init(label: "Second"),
            ],
            allowCustom: true
        )
        XCTAssertTrue(widget.isValid)
        XCTAssertEqual(summarizeWidget(widget), "Choose — First / Second")
        XCTAssertEqual(getWidgetAnswerLabel(widget, answer: "1"), "First")
        XCTAssertEqual(getWidgetAnswerLabel(widget, answer: "Second"), "Second")
    }

    func testPendingTranscriptApprovalsSettleOnlyMatchingPendingRequest() {
        let auto = TranscriptEntry(
            kind: "send-message",
            message: .init(
                type: "auto-review-approval",
                approval: .init(requestId: "r1", status: "pending")
            )
        )
        XCTAssertEqual(
            settlePendingAutoReviewApprovalEntry(auto, status: "approved", requestId: "r1")?.message?.approval?.status,
            "approved"
        )
        XCTAssertNil(settlePendingAutoReviewApprovalEntry(auto, status: "approved", requestId: "other"))

        let local = TranscriptEntry(
            kind: "send-message",
            message: .init(
                type: "local-tool-permission",
                ask: .init(requestId: "l1", status: "pending")
            )
        )
        XCTAssertEqual(
            settlePendingLocalToolPermissionEntry(local, status: "denied")?.message?.ask?.status,
            "denied"
        )
    }

    func testTranscriptMainAndThreadProjection() {
        let entries: [TranscriptEntry] = [
            .init(kind: "message", id: "root"),
            .init(kind: "message", id: "b1", replyTo: "root", branched: true),
            .init(kind: "message", id: "b2", replyTo: "b1", branched: true),
            .init(kind: "message", id: "main2"),
        ]
        XCTAssertEqual(getMainTranscriptEntries(entries).compactMap(\.id), ["root", "main2"])
        XCTAssertEqual(getThreadTranscriptEntries(entries, rootId: "root").compactMap(\.id), ["root", "b1", "b2"])

        let branched = [
            BranchedTranscriptEntry(id: "b1", replyTo: "root"),
            BranchedTranscriptEntry(id: "b2", replyTo: "b1"),
            BranchedTranscriptEntry(id: "other", replyTo: "root2"),
        ]
        XCTAssertEqual(branchReplyCounts(branched)["root"], 2)
        XCTAssertEqual(threadDescendants("root", branched: branched).map(\.id), ["b1", "b2"])
    }

    func testAgentPeerVisibilityAndWorkflowConstants() {
        let peer = TranscriptEntry(kind: "message", id: "m1", toAgent: .init(kind: "agent"))
        let hidden = TranscriptEntry(kind: "message", id: "m2", toAgent: .init(kind: "system"))
        XCTAssertTrue(isAgentPeerMessageEntry(peer))
        XCTAssertTrue(isOutboundAgentPeerMessageEntry(peer))
        XCTAssertFalse(isHiddenOutboundAgentPeerMessageEntry(peer))
        XCTAssertTrue(isHiddenOutboundAgentPeerMessageEntry(hidden))
        XCTAssertFalse(entryRaisesUserActivitySignal(peer))

        XCTAssertEqual(WORKFLOW_REFERENCE_NODE_TYPE, "workflowReference")
        XCTAssertEqual(SandSkillPublishError(reason: "private").errorDescription, "skill-publish/refused: private")
        XCTAssertTrue(isSandUpdateTrack("stable"))
        XCTAssertFalse(isSandUpdateTrack("beta"))
    }
    func testNativeTranscriptCardsProjectRecoveredNoticePermissionAndTimelineSemantics() throws {
        let notice = try XCTUnwrap(projectMobileTranscriptCard(
            event: [
                "entryId": "notice-1",
                "timestampMs": 1_000,
                "card": ["kind": "notice", "text": "Workspace updated"],
            ],
            operationId: "op-1"
        ))
        XCTAssertEqual(notice.kind, .notice)
        XCTAssertEqual(notice.text, "Workspace updated")
        XCTAssertEqual(notice.createdAt.timeIntervalSince1970, 1, accuracy: 0.001)

        let permission = try XCTUnwrap(projectMobileTranscriptCard(
            event: [
                "entryId": "permission-1",
                "card": [
                    "kind": "permission-request",
                    "permission": ["title": "Camera"],
                ],
            ],
            operationId: nil
        ))
        XCTAssertEqual(permission.kind, .permissionRequest)
        XCTAssertEqual(permission.text, "Camera")

        let renamed = try XCTUnwrap(projectMobileTranscriptCard(
            event: [
                "entryId": "event-1",
                "card": [
                    "kind": "timeline-event",
                    "event": ["type": "name-changed", "to": "Research"],
                ],
            ],
            operationId: nil
        ))
        XCTAssertEqual(renamed.kind, .timelineEvent)
        XCTAssertEqual(renamed.text, "Renamed to Research")
        XCTAssertNil(renamed.timelineAutomationId)

        let automation = try XCTUnwrap(projectMobileTranscriptCard(
            event: [
                "entryId": "event-2",
                "card": [
                    "kind": "timelineEvent",
                    "event": [
                        "type": "automation-changed",
                        "automationId": "routine-7",
                        "action": "enabled",
                        "automationName": "Morning brief",
                    ],
                ],
            ],
            operationId: nil
        ))
        XCTAssertEqual(automation.kind, .timelineEvent)
        XCTAssertEqual(automation.text, "Enabled automation \"Morning brief\"")
        XCTAssertEqual(automation.timelineAutomationId, "routine-7")
    }

    func testNativeTranscriptCardsRejectMalformedOrUnknownRecoveredCards() {
        XCTAssertNil(projectMobileTranscriptCard(event: ["card": ["kind": "notice"]], operationId: nil))
        XCTAssertNil(projectMobileTranscriptCard(event: ["card": ["kind": "permission-request", "permission": [:]]], operationId: nil))
        XCTAssertNil(projectMobileTranscriptCard(event: ["card": ["kind": "timeline-event", "event": ["type": "automation-changed", "automationName": "Missing id"]]], operationId: nil))
        XCTAssertNil(projectMobileTranscriptCard(event: ["card": ["kind": "unknown"]], operationId: nil))
    }

}
