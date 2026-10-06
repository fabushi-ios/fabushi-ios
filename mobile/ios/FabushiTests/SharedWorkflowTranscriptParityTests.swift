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

    func testSendMessageTextProjectionPreservesDesktopGuardsAndBareLinkGate() throws {
        let plain = try XCTUnwrap(projectMobileSendMessageText([
            "kind": "send-message",
            "id": "text-1",
            "message": [
                "type": "text",
                "content": "Hello",
                "channel": "slack",
            ],
            "streaming": true,
            "timestampMs": 1_500,
        ]))
        XCTAssertEqual(plain.id, "text-1")
        XCTAssertEqual(plain.content, "Hello")
        XCTAssertEqual(plain.channel, "slack")
        XCTAssertTrue(plain.streaming)
        XCTAssertEqual(plain.timestampMs, 1_500)
        XCTAssertEqual(plain.presentation, .text)

        let bareLink = try XCTUnwrap(projectMobileSendMessageText([
            "kind": "send-message",
            "id": "text-2",
            "message": [
                "type": "text",
                "content": " [Example](https://example.com/path?x=1) ",
            ],
        ]))
        XCTAssertEqual(
            bareLink.presentation,
            .urlCard("https://example.com/path?x=1")
        )

        let withImage = try XCTUnwrap(projectMobileSendMessageText([
            "kind": "send-message",
            "id": "text-3",
            "message": [
                "type": "text",
                "content": "https://example.com",
                "images": [[
                    "url": "https://example.com/a.png",
                    "alt": "A",
                ]],
            ],
        ]))
        XCTAssertEqual(withImage.presentation, .text)
        XCTAssertEqual(withImage.images, [
            .init(url: "https://example.com/a.png", alt: "A"),
        ])

        XCTAssertNil(projectMobileSendMessageText([
            "kind": "send-message",
            "id": "text-4",
            "message": ["type": "text", "content": "x", "images": "bad"],
        ]))
        XCTAssertNil(projectMobileSendMessageText([
            "kind": "send-message",
            "id": "text-5",
            "message": ["type": "text", "content": "x"],
            "streaming": "yes",
        ]))
        XCTAssertNil(projectMobileSendMessageText([
            "kind": "send-message",
            "id": "text-6",
            "message": ["type": "text", "content": "x", "channel": 7],
        ]))
    }

    func testTranscriptCardDispatchesSendMessageTextBeforeAttachmentProjection() throws {
        let row = try XCTUnwrap(projectMobileTranscriptCard(
            event: [
                "entryId": "outer",
                "card": [
                    "kind": "send-message",
                    "id": "text-7",
                    "message": [
                        "type": "text",
                        "content": "https://example.com",
                    ],
                ],
            ],
            operationId: "op-text"
        ))
        XCTAssertEqual(row.id, "text-7")
        XCTAssertEqual(row.text, "https://example.com")
        XCTAssertEqual(row.sendMessageTextProjection?.presentation, .urlCard("https://example.com/"))
        XCTAssertNil(row.attachmentProjection)
    }

    func testAttachmentDataProjectsDesktopKindsAndBoxMetadata() throws {
        let box = try XCTUnwrap(projectMobileAttachmentCard([
            "kind": "send-message",
            "id": "box-1",
            "message": [
                "type": "attachment",
                "url": "sand://box?request=1",
            ],
            "boxInstruction": "Open the browser",
            "boxRequest": "Complete checkout",
            "boxRequestId": "request-1",
            "boxResolution": "completed",
            "boxSnapshot": "data:image/png;base64,abc",
            "timestampMs": 2_500,
        ]))
        XCTAssertEqual(box.kind, .box)
        XCTAssertEqual(box.instruction, "Open the browser")
        XCTAssertEqual(box.request, "Complete checkout")
        XCTAssertEqual(box.requestId, "request-1")
        XCTAssertEqual(box.resolution, "completed")
        XCTAssertEqual(box.screenshotDataURL, "data:image/png;base64,abc")
        XCTAssertEqual(box.timestampMs, 2_500)

        let link = try XCTUnwrap(projectMobileAttachmentCard([
            "kind": "send-message",
            "id": "link-1",
            "message": [
                "type": "attachment",
                "url": " https://example.com/report ",
            ],
        ]))
        XCTAssertEqual(link.kind, .legacyLink)
        XCTAssertEqual(link.url, "https://example.com/report")

        let media = try XCTUnwrap(projectMobileAttachmentCard([
            "kind": "send-message",
            "id": "media-1",
            "message": [
                "type": "attachment",
                "url": "https://example.com/PHOTO.PNG?download=1",
                "alt": "Screenshot",
            ],
        ]))
        XCTAssertEqual(media.kind, .media)
        XCTAssertEqual(media.alt, "Screenshot")

        let file = try XCTUnwrap(projectMobileAttachmentCard([
            "kind": "send-message",
            "id": "file-1",
            "message": [
                "type": "attachment",
                "url": "/tmp/report.pdf",
            ],
        ]))
        XCTAssertEqual(file.kind, .file)
    }

    func testUserAttachmentProjectionPreservesGalleryMetadataAndFailsClosed() throws {
        let projected = try XCTUnwrap(projectMobileAttachmentCard([
            "kind": "user-attachment",
            "id": "upload-1",
            "file_path": "C:\\tmp\\photo.PNG",
            "file_name": "",
            "byteSize": 42,
            "width": 640,
            "height": 480,
            "timestampMs": 3_000,
            "batchId": "batch-7",
            "replyTo": "message-2",
            "clientNonce": "nonce-9",
        ]))
        XCTAssertEqual(projected.kind, .media)
        XCTAssertEqual(projected.name, "photo.PNG")
        XCTAssertEqual(projected.byteSize, 42)
        XCTAssertEqual(projected.width, 640)
        XCTAssertEqual(projected.height, 480)
        XCTAssertEqual(projected.timestampMs, 3_000)
        XCTAssertEqual(projected.batchId, "batch-7")
        XCTAssertEqual(projected.replyTo, "message-2")
        XCTAssertEqual(projected.clientNonce, "nonce-9")

        XCTAssertNil(projectMobileAttachmentCard([
            "kind": "user-attachment",
            "id": "upload-2",
            "file_path": "/tmp/a.txt",
            "byteSize": -1,
        ]))
        XCTAssertNil(projectMobileAttachmentCard([
            "kind": "user-attachment",
            "id": "upload-3",
            "file_path": "/tmp/a.txt",
            "batchId": "",
        ]))
        XCTAssertNil(projectMobileAttachmentCard([
            "kind": "user-attachment",
            "id": "upload-4",
            "file_path": "/tmp/a.txt",
            "file_name": 7,
        ]))
    }

    func testTranscriptAndLiveChatUseTheSameTypedAttachmentProjection() throws {
        let transcript = try XCTUnwrap(projectMobileTranscriptCard(
            event: [
                "card": [
                    "kind": "user-attachment",
                    "id": "upload-5",
                    "file_path": "/tmp/notes.txt",
                    "batchId": "batch-8",
                ],
            ],
            operationId: "op-attachment"
        ))
        XCTAssertEqual(transcript.role, .user)
        XCTAssertEqual(transcript.attachmentProjection?.kind, .file)
        XCTAssertEqual(transcript.attachmentFileName, "notes.txt")
        XCTAssertEqual(transcript.attachmentBatchId, "batch-8")

        let live = try XCTUnwrap(projectMobileChatMessageAttachment(
            id: "message-9",
            raw: [
                "url": "https://example.com/video.webm",
                "file_name": "video.webm",
                "alt": "Demo",
            ],
            batchId: "batch-9"
        ))
        XCTAssertEqual(live.kind, .media)
        XCTAssertEqual(live.name, "video.webm")
        XCTAssertEqual(live.alt, "Demo")
        XCTAssertEqual(live.batchId, "batch-9")
    }

    func testNativeTranscriptCardsRejectMalformedOrUnknownRecoveredCards() {
        XCTAssertNil(projectMobileTranscriptCard(event: ["card": ["kind": "notice"]], operationId: nil))
        XCTAssertNil(projectMobileTranscriptCard(event: ["card": ["kind": "permission-request", "permission": [:]]], operationId: nil))
        XCTAssertNil(projectMobileTranscriptCard(event: ["card": ["kind": "timeline-event", "event": ["type": "automation-changed", "automationName": "Missing id"]]], operationId: nil))
        XCTAssertNil(projectMobileTranscriptCard(event: ["card": ["kind": "unknown"]], operationId: nil))
    }

}
