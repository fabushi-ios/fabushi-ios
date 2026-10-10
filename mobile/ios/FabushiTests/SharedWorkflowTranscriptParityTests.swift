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

    func testConversationWindowProjectionPreservesThreadTopologyAndRejectsMalformedRelations() throws {
        let threaded = try XCTUnwrap(projectMobileConversationWindowMessage([
            "id": "reply-1",
            "role": "assistant",
            "text": "Thread reply",
            "createdAtMs": 1_234,
            "replyToMessageId": "root-1",
            "branched": true,
            "reactions": [],
        ]))
        XCTAssertEqual(threaded.canonicalMessageId, "reply-1")
        XCTAssertEqual(threaded.replyToMessageId, "root-1")
        XCTAssertTrue(threaded.branched)

        let legacy = try XCTUnwrap(projectMobileConversationWindowMessage([
            "id": "legacy-1",
            "role": "user",
            "text": "Legacy",
            "createdAtMs": 2_000,
        ]))
        XCTAssertNil(legacy.replyToMessageId)
        XCTAssertFalse(legacy.branched)

        XCTAssertNil(projectMobileConversationWindowMessage([
            "id": "broken-1",
            "role": "assistant",
            "text": "Broken",
            "createdAtMs": 3_000,
            "replyToMessageId": 42,
        ]))
        XCTAssertNil(projectMobileConversationWindowMessage([
            "id": "broken-2",
            "role": "assistant",
            "text": "Broken",
            "createdAtMs": 3_000,
            "branched": "yes",
        ]))
    }

    func testMobileTranscriptMessageActionsProjectMainThreadCountsAndCopySemantics() throws {
        var root = MobileChatMessage(id: "root", role: .assistant, text: "Root", canonicalMessageId: "root")
        var first = MobileChatMessage(id: "first", role: .assistant, text: "First", canonicalMessageId: "first")
        first.replyToMessageId = "root"
        first.branched = true
        var nested = MobileChatMessage(id: "nested", role: .user, text: "Nested", canonicalMessageId: "nested")
        nested.replyToMessageId = "first"
        nested.branched = true
        let main = MobileChatMessage(id: "main", role: .user, text: "Main", canonicalMessageId: "main")

        let all = [root, first, nested, main]
        XCTAssertEqual(mobileMainTranscriptEntries(all).map(mobileTranscriptCanonicalId), ["root", "main"])
        XCTAssertEqual(mobileThreadEntries(all, rootId: "root").map(mobileTranscriptCanonicalId), ["root", "first", "nested"])
        XCTAssertEqual(mobileThreadReplyCounts(all)["root"], 2)
        XCTAssertEqual(mobileTranscriptCopyText(root), "Root")

        root.text = ""
        XCTAssertNil(mobileTranscriptCopyText(root))
        let urlCard = MobileChatMessage(
            id: "url",
            role: .assistant,
            text: "https://example.com",
            sendMessageTextProjection: .init(
                id: "url",
                content: "https://example.com",
                images: [],
                channel: nil,
                streaming: false,
                timestampMs: nil,
                presentation: .urlCard("https://example.com")
            )
        )
        XCTAssertNil(mobileTranscriptCopyText(urlCard))
    }

    func testBotFindSearchUsesOnlyMainTranscriptCopyableTextAndWrapsMatches() {
        let root = MobileChatMessage(
            id: "root",
            role: .assistant,
            text: "Alpha alpha",
            canonicalMessageId: "root"
        )
        var branched = MobileChatMessage(
            id: "branch",
            role: .assistant,
            text: "Alpha hidden in thread",
            canonicalMessageId: "branch"
        )
        branched.replyToMessageId = "root"
        branched.branched = true
        let notice = MobileChatMessage(
            id: "notice",
            role: .assistant,
            text: "Alpha activity",
            kind: .notice
        )
        let user = MobileChatMessage(
            id: "user",
            role: .user,
            text: "alpha",
            canonicalMessageId: "user"
        )

        let searchable = mobileBotChatSearchEntries([root, branched, notice, user], botName: "Agent")
        XCTAssertEqual(searchable.map(\.id), ["root", "user"])

        let matches = chatSearchMatches(searchable, query: "ALPHA")
        XCTAssertEqual(
            matches,
            [
                ChatSearchMatch(entryId: "root", occurrence: 0),
                ChatSearchMatch(entryId: "root", occurrence: 1),
                ChatSearchMatch(entryId: "user", occurrence: 0),
            ]
        )
        XCTAssertEqual(nextChatSearchIndex(current: 2, count: matches.count, delta: 1), 0)
        XCTAssertEqual(nextChatSearchIndex(current: 0, count: matches.count, delta: -1), 2)
    }

    func testAgentForwardingRequiresCanonicalConversationAndSettledMessageIdentity() {
        var message = MobileChatMessage(
            id: "local",
            role: .assistant,
            text: "Forward me",
            canonicalMessageId: "canonical-message"
        )

        XCTAssertEqual(
            mobileBotForwardMessageId(message, sourceConversationId: "agent-conversation"),
            "canonical-message"
        )
        XCTAssertNil(mobileBotForwardMessageId(message, sourceConversationId: nil))
        XCTAssertNil(mobileBotForwardMessageId(message, sourceConversationId: "   "))

        message.streaming = true
        XCTAssertNil(
            mobileBotForwardMessageId(message, sourceConversationId: "agent-conversation")
        )

        let notice = MobileChatMessage(
            id: "notice",
            role: .assistant,
            text: "Not a message",
            kind: .notice
        )
        XCTAssertNil(
            mobileBotForwardMessageId(notice, sourceConversationId: "agent-conversation")
        )
    }

    func testConversationHistoryMergePreservesEphemeraAndReplacesCanonicalMessages() {
        let old = MobileChatMessage(
            id: "history:reply",
            role: .assistant,
            text: "old",
            canonicalMessageId: "reply",
            replyToMessageId: "root",
            branched: true
        )
        let ephemeral = MobileChatMessage(
            id: "thinking:op",
            role: .assistant,
            text: "",
            kind: .thinking
        )
        let refreshed = MobileChatMessage(
            id: "history:reply",
            role: .assistant,
            text: "new",
            canonicalMessageId: "reply",
            replyToMessageId: "root",
            branched: true
        )
        let root = MobileChatMessage(
            id: "history:root",
            role: .user,
            text: "root",
            canonicalMessageId: "root"
        )

        let merged = mergeMobileConversationHistory(
            current: [old, ephemeral],
            fetched: [root, refreshed]
        )
        XCTAssertEqual(
            merged.filter { $0.kind == .message }.map(mobileTranscriptCanonicalId),
            ["root", "reply"]
        )
        XCTAssertEqual(
            merged.first { mobileTranscriptCanonicalId($0) == "reply" }?.text,
            "new"
        )
        XCTAssertTrue(merged.contains { $0.id == "thinking:op" })
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

    func testTranscriptDuplicateIdsFollowDesktopLastValueMapSemanticsWithoutTrapping() {
        let branched = [
            BranchedTranscriptEntry(id: "dup", replyTo: "root-a"),
            BranchedTranscriptEntry(id: "dup", replyTo: "root-b"),
            BranchedTranscriptEntry(id: "child", replyTo: "dup"),
        ]
        let counts = branchReplyCounts(branched)
        XCTAssertEqual(counts["root-a"], 1)
        XCTAssertEqual(counts["root-b"], 2)

        let entries = [
            TranscriptEntry(kind: "message", id: "root-a"),
            TranscriptEntry(kind: "message", id: "dup", replyTo: "root-a", branched: true),
            TranscriptEntry(kind: "message", id: "dup", replyTo: "root-b", branched: true),
            TranscriptEntry(kind: "message", id: "root-b"),
        ]
        XCTAssertEqual(
            getThreadTranscriptEntries(entries, rootId: "root-b").compactMap(\.id),
            ["dup", "dup", "root-b"]
        )
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
    func testWidgetTranscriptProjectionAndDurableHistoryCards() throws {
        let card: [String: Any] = [
            "kind": "widget",
            "widget": [
                "prompt": "Deploy?",
                "helpText": "Choose one",
                "options": [
                    ["label": "Ship", "value": "ship", "style": "primary"],
                    ["label": "Stop", "value": "stop", "style": "danger"],
                ],
                "allowCustom": true,
                "dismissOnMoveOn": true,
            ],
            "respondedValue": "ship",
            "widgetDismissed": false,
            "widgetSkipped": false,
        ]
        let projected = try XCTUnwrap(projectMobileCanonicalHostTranscriptCard(
            event: [
                "entryId": "message-1-card-0",
                "card": card,
            ],
            operationId: nil
        ))
        let widget = try XCTUnwrap(mobileTranscriptWidgetProjection(projected))
        XCTAssertEqual(widget.widget.prompt, "Deploy?")
        XCTAssertEqual(widget.widget.options.count, 2)
        XCTAssertEqual(widget.respondedValue, "ship")
        XCTAssertFalse(widget.dismissed)

        let historyEntries = try XCTUnwrap(projectMobileConversationWindowEntries([
            "id": "message-1",
            "role": "assistant",
            "text": "",
            "createdAtMs": 1234,
            "cards": [card],
        ]))
        XCTAssertEqual(historyEntries.count, 1)
        XCTAssertEqual(historyEntries[0].id, "message-1-card-0")
        XCTAssertEqual(
            mobileTranscriptWidgetProjection(historyEntries[0])?.respondedValue,
            "ship"
        )

        XCTAssertNil(projectMobileCanonicalHostTranscriptCard(
            event: [
                "entryId": "bad-card",
                "card": [
                    "kind": "widget",
                    "widget": [
                        "prompt": "Bad",
                        "options": [],
                    ],
                ],
            ],
            operationId: nil
        ))
    }

    func testCanonicalHostTranscriptCardsPreserveValidatedPayloadAndFailClosed() throws {
        let email = try XCTUnwrap(projectMobileCanonicalHostTranscriptCard(
            event: [
                "entryId": "draft-1",
                "card": [
                    "kind": "emailDraft",
                    "draft": [
                        "kind": "email",
                        "id": "email-1",
                        "to": ["you@example.com"],
                        "subject": "Release",
                        "body": "Ready",
                        "status": "editable",
                    ],
                ],
            ],
            operationId: "op-1"
        ))
        XCTAssertEqual(email.kind, .action)
        XCTAssertEqual(email.actionTitle, "Release")
        XCTAssertEqual(email.canonicalTranscriptCard?.kind, "emailDraft")
        XCTAssertTrue(email.canonicalTranscriptCard?.json.contains(#""id":"email-1""#) == true)
        guard case let .email(emailDraft)? = mobileTranscriptDraftProjection(
            email.canonicalTranscriptCard
        ) else {
            return XCTFail("expected email draft projection")
        }
        XCTAssertEqual(emailDraft.id, "email-1")
        XCTAssertEqual(emailDraft.to, ["you@example.com"])
        XCTAssertEqual(mobileEmailRecipients("a@example.com, b@example.org"), [
            "a@example.com", "b@example.org",
        ])
        XCTAssertNil(mobileEmailRecipients("not-an-email"))

        let event = try XCTUnwrap(projectMobileCanonicalHostTranscriptCard(
            event: [
                "entryId": "event-1",
                "card": [
                    "kind": "event",
                    "event": [
                        "source": "github",
                        "event": "pull_request",
                        "title": "PR opened",
                        "summary": "Review requested",
                        "fields": [["label": "repo", "value": "fabushi"]],
                        "occurredAtMs": 1234,
                    ],
                ],
            ],
            operationId: nil
        ))
        XCTAssertEqual(event.kind, .notice)
        XCTAssertEqual(event.text, "PR opened — Review requested")

        let secretMessage = try XCTUnwrap(projectMobileCanonicalHostTranscriptCard(
            event: [
                "entryId": "secret-entry",
                "card": [
                    "kind": "secretRequest",
                    "requestId": "secret-1",
                    "label": "Deploy token",
                    "description": "Required to deploy",
                    "provided": false,
                ],
            ],
            operationId: nil
        ))
        let secret = try XCTUnwrap(mobileSecretRequestProjection(secretMessage.canonicalTranscriptCard))
        XCTAssertEqual(secret.requestId, "secret-1")
        XCTAssertEqual(secret.label, "Deploy token")
        XCTAssertEqual(secret.description, "Required to deploy")
        XCTAssertFalse(secret.provided)

        XCTAssertNil(projectMobileCanonicalHostTranscriptCard(
            event: ["card": [
                "kind": "emailDraft",
                "draft": [
                    "kind": "email",
                    "id": "broken",
                    "to": "not-an-array",
                    "subject": "Broken",
                    "body": "",
                    "status": "editable",
                ],
            ]],
            operationId: nil
        ))
        XCTAssertNil(projectMobileCanonicalHostTranscriptCard(
            event: ["card": ["kind": "pdf", "name": "bad.pdf", "pageCount": -1]],
            operationId: nil
        ))
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

    func testLinkMetadataProjectionAcceptsSafeOptionalFieldsAndRejectsWrongTypes() throws {
        let metadata = try XCTUnwrap(MarketplaceModel.projectLinkMetadata(
            url: " https://example.com/path ",
            value: [
                "title": "Example",
                "description": "Description",
                "hostname": "example.com",
                "imageUrl": "https://example.com/og.png",
                "imageDataUrl": NSNull(),
                "faviconDataUrl": NSNull(),
            ]
        ))
        XCTAssertEqual(metadata.url, "https://example.com/path")
        XCTAssertEqual(metadata.displayTitle, "Example")
        XCTAssertEqual(metadata.description, "Description")
        XCTAssertEqual(metadata.hostname, "example.com")
        XCTAssertEqual(metadata.imageURL, "https://example.com/og.png")
        XCTAssertNil(metadata.imageDataURL)
        XCTAssertNil(metadata.faviconDataURL)

        let hostnameFallback = try XCTUnwrap(MarketplaceModel.projectLinkMetadata(
            url: "https://example.com",
            value: ["hostname": "example.com"]
        ))
        XCTAssertEqual(hostnameFallback.displayTitle, "example.com")

        XCTAssertNil(MarketplaceModel.projectLinkMetadata(
            url: "file:///tmp/a",
            value: ["title": "bad"]
        ))
        XCTAssertNil(MarketplaceModel.projectLinkMetadata(
            url: "https://example.com",
            value: ["title": 7]
        ))
        XCTAssertNil(MarketplaceModel.projectLinkMetadata(
            url: "https://example.com",
            value: "not-object"
        ))
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
        XCTAssertEqual(mobileAttachmentMediaPresentation(media.url), .image)

        let audio = try XCTUnwrap(projectMobileAttachmentCard([
            "kind": "send-message",
            "id": "audio-1",
            "message": [
                "type": "attachment",
                "url": "https://example.com/voice.m4a",
                "alt": "Voice note",
            ],
        ]))
        XCTAssertEqual(audio.kind, .media)
        XCTAssertEqual(mobileAttachmentMediaPresentation(audio.url), .audio)
        XCTAssertEqual(
            mobileAttachmentMediaPresentation("https://example.com/demo.mp4"),
            .video
        )

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

    func testUnknownCanonicalTranscriptCardGetsStableFabushiFallback() throws {
        let fallback = try XCTUnwrap(projectMobileTranscriptCardWithFallback(
            event: [
                "entryId": "unknown-card-1",
                "card": ["kind": "future-card"],
            ],
            operationId: "op-fallback"
        ))
        XCTAssertEqual(fallback.id, "transcript-card-fallback:unknown-card-1")
        XCTAssertEqual(fallback.canonicalMessageId, "unknown-card-1")
        XCTAssertEqual(fallback.role, .assistant)
        XCTAssertEqual(fallback.kind, .notice)
        XCTAssertEqual(
            fallback.text,
            "This message can’t be shown in this version of Fabushi"
        )
        XCTAssertEqual(fallback.operationId, "op-fallback")

        XCTAssertNil(projectMobileTranscriptCardWithFallback(
            event: ["card": ["kind": "future-card"]],
            operationId: nil
        ))
        XCTAssertNil(projectMobileTranscriptCardWithFallback(
            event: ["entryId": "unknown-card-2"],
            operationId: nil
        ))
    }

    func testNativeTranscriptCardsRejectMalformedOrUnknownRecoveredCards() {
        XCTAssertNil(projectMobileTranscriptCard(event: ["card": ["kind": "notice"]], operationId: nil))
        XCTAssertNil(projectMobileTranscriptCard(event: ["card": ["kind": "permission-request", "permission": [:]]], operationId: nil))
        XCTAssertNil(projectMobileTranscriptCard(event: ["card": ["kind": "timeline-event", "event": ["type": "automation-changed", "automationName": "Missing id"]]], operationId: nil))
        XCTAssertNil(projectMobileTranscriptCard(event: ["card": ["kind": "unknown"]], operationId: nil))
    }

    func testTranscriptReactionProjectionFiltersMalformedRowsAndPreservesSelf() {
        let projected = projectMobileTranscriptReactions([
            ["emoji": "👍", "by": "me"],
            ["emoji": "❤️", "by": "agent-2"],
            ["emoji": " ", "by": "me"],
            ["emoji": "😂", "by": ""],
            ["emoji": 7, "by": "me"],
        ])
        XCTAssertEqual(projected, [
            .init(emoji: "👍", by: "me"),
            .init(emoji: "❤️", by: "agent-2"),
        ])
        XCTAssertEqual(
            Set(projected.filter { $0.by == "me" }.map(\.emoji)),
            Set(["👍"])
        )
        XCTAssertTrue(projectMobileTranscriptReactions(nil).isEmpty)
        XCTAssertTrue(projectMobileTranscriptReactions("bad").isEmpty)
    }

    func testReactionPillsPreserveFirstSeenEmojiOrderAndDeduplicateReactors() {
        let pills = projectMobileReactionPills([
            .init(emoji: "❤️", by: "agent-2"),
            .init(emoji: "👍", by: "me"),
            .init(emoji: "❤️", by: "agent-3"),
            .init(emoji: "❤️", by: "agent-2"),
        ])
        XCTAssertEqual(pills.map(\.emoji), ["❤️", "👍"])
        XCTAssertEqual(pills[0].count, 2)
        XCTAssertEqual(pills[0].reactors, ["agent-2", "agent-3"])
        XCTAssertFalse(pills[0].chosenByMe)
        XCTAssertTrue(pills[1].chosenByMe)
    }

    func testReactionInputUsesDesktopUtf16BoundAndTransportIsAgentScoped() {
        XCTAssertEqual(normalizeMobileReactionInput(" 👍 "), "👍")
        XCTAssertNotNil(normalizeMobileReactionInput(String(repeating: "😀", count: 8)))
        XCTAssertNil(normalizeMobileReactionInput(String(repeating: "😀", count: 9)))
        XCTAssertNil(normalizeMobileReactionInput("   "))

        var messages = [
            MobileChatMessage(
                id: "local-user-1",
                role: .user,
                text: "thanks",
                canonicalMessageId: "canonical-user-1"
            )
        ]
        let event: [String: Any] = [
            "type": "host.transport",
            "channel": "transcript.reaction",
            "payload": [
                "agentId": "agent-1",
                "entryId": "canonical-user-1",
                "reactions": [
                    ["emoji": "👍", "by": "assistant"],
                    ["emoji": "❤️", "by": "me"],
                ],
                "myReactions": ["❤️"],
            ],
        ]
        XCTAssertFalse(
            applyMobileTranscriptReactionEvent(
                event,
                agentId: "other-agent",
                messages: &messages
            )
        )
        XCTAssertTrue(
            applyMobileTranscriptReactionEvent(
                event,
                agentId: "agent-1",
                messages: &messages
            )
        )
        XCTAssertEqual(messages[0].reactions.count, 2)
        XCTAssertEqual(messages[0].myReactions, Set(["❤️"]))
        XCTAssertFalse(
            applyMobileTranscriptReactionEvent(
                event,
                agentId: "agent-1",
                messages: &messages
            ),
            "unchanged authoritative reaction fingerprints must be a no-op"
        )
    }

    func testReactionRequestFenceRejectsAccountAgentAndGenerationReplacement() {
        let fence = MobileReactionRequestFence(
            accountKey: "account-a",
            agentId: "agent-1",
            generation: 7
        )
        XCTAssertTrue(fence.accepts(accountKey: "account-a", agentId: "agent-1", generation: 7))
        XCTAssertFalse(fence.accepts(accountKey: "account-b", agentId: "agent-1", generation: 7))
        XCTAssertFalse(fence.accepts(accountKey: "account-a", agentId: "agent-2", generation: 7))
        XCTAssertFalse(fence.accepts(accountKey: "account-a", agentId: "agent-1", generation: 8))
    }


    func testConversationWindowProjectionUsesCanonicalMessageIdentity() throws {
        let message = try XCTUnwrap(projectMobileConversationWindowMessage([
            "id": "message-42",
            "role": "assistant",
            "text": "Persisted answer",
            "createdAtMs": 1_700_000_000_000,
            "reactions": [
                ["emoji": "👍", "by": "me"],
            ],
        ]))
        XCTAssertEqual(message.id, "history:message-42")
        XCTAssertEqual(message.canonicalMessageId, "message-42")
        XCTAssertEqual(message.role, .assistant)
        XCTAssertEqual(message.text, "Persisted answer")
        XCTAssertEqual(message.reactions, [.init(emoji: "👍", by: "me")])
        XCTAssertEqual(message.createdAt.timeIntervalSince1970, 1_700_000_000, accuracy: 0.001)

        XCTAssertNil(projectMobileConversationWindowMessage([
            "id": "message-bad-role",
            "role": "system",
            "text": "nope",
            "createdAtMs": 1,
        ]))
        XCTAssertNil(projectMobileConversationWindowMessage([
            "id": "message-bad-time",
            "role": "user",
            "text": "nope",
            "createdAtMs": "now",
        ]))
    }

    func testLateConversationBaselinePreservesLiveAndOptimisticEntries() throws {
        let oldBaseline = MobileChatMessage(
            id: "history:old-1",
            role: .assistant,
            text: "old",
            canonicalMessageId: "old-1",
            createdAt: Date(timeIntervalSince1970: 1)
        )
        let staleCached = MobileChatMessage(
            id: "history:stale-cache",
            role: .assistant,
            text: "stale",
            canonicalMessageId: "stale-cache",
            createdAt: Date(timeIntervalSince1970: 2)
        )
        let liveAfterRequest = MobileChatMessage(
            id: "assistant:op-live",
            role: .assistant,
            text: "live",
            operationId: "op-live",
            canonicalMessageId: "live-2",
            createdAt: Date(timeIntervalSince1970: 4)
        )
        let optimistic = MobileChatMessage(
            id: "ios-mobile-bot-chat-request-7",
            role: .user,
            text: "pending",
            canonicalMessageId: "ios-mobile-bot-chat-request-7",
            createdAt: Date(timeIntervalSince1970: 5)
        )
        let authoritativeOld = MobileChatMessage(
            id: "history:old-1",
            role: .assistant,
            text: "authoritative old",
            canonicalMessageId: "old-1",
            createdAt: Date(timeIntervalSince1970: 1)
        )
        let authoritativeNew = MobileChatMessage(
            id: "history:baseline-2",
            role: .user,
            text: "baseline",
            canonicalMessageId: "baseline-2",
            createdAt: Date(timeIntervalSince1970: 3)
        )

        let reconciled = reconcileMobileConversationBaseline(
            baseline: [authoritativeOld, authoritativeNew],
            current: [oldBaseline, staleCached, liveAfterRequest, optimistic],
            identitiesAtRequestStart: Set(["old-1", "stale-cache"])
        )

        XCTAssertEqual(
            reconciled.map { $0.canonicalMessageId ?? $0.id },
            ["old-1", "baseline-2", "live-2", "ios-mobile-bot-chat-request-7"]
        )
        XCTAssertEqual(reconciled[0].text, "authoritative old")
        XCTAssertFalse(reconciled.contains(where: { $0.canonicalMessageId == "stale-cache" }))
        XCTAssertTrue(reconciled.contains(where: { $0.canonicalMessageId == "live-2" }))
        XCTAssertTrue(reconciled.contains(where: { $0.id == "ios-mobile-bot-chat-request-7" }))
    }

    func testConversationBaselineDoesNotDuplicateCanonicalLiveMessage() {
        let live = MobileChatMessage(
            id: "assistant:op-1",
            role: .assistant,
            text: "live version",
            operationId: "op-1",
            canonicalMessageId: "canonical-1",
            createdAt: Date(timeIntervalSince1970: 2)
        )
        let baseline = MobileChatMessage(
            id: "history:canonical-1",
            role: .assistant,
            text: "persisted version",
            canonicalMessageId: "canonical-1",
            createdAt: Date(timeIntervalSince1970: 1)
        )
        let reconciled = reconcileMobileConversationBaseline(
            baseline: [baseline],
            current: [live],
            identitiesAtRequestStart: []
        )
        XCTAssertEqual(reconciled.count, 1)
        XCTAssertEqual(reconciled[0].canonicalMessageId, "canonical-1")
        XCTAssertEqual(reconciled[0].text, "persisted version")
    }

    func testOptimisticUserEchoSettlesOnlyExactDurableMessageIdentity() {
        var messages = [
            MobileChatMessage(
                id: "ios-mobile-bot-chat-request-7",
                role: .user,
                text: "pending",
                canonicalMessageId: "ios-mobile-bot-chat-request-7",
                optimisticDeliveryPhase: .acceptedAwaitingEcho,
                optimisticDeliveryError: "old error"
            )
        ]

        XCTAssertFalse(applyMobileOptimisticUserEcho([
            "type": "chat.message",
            "role": "user",
            "messageId": "different-request",
            "text": "pending",
        ], messages: &messages))
        XCTAssertEqual(messages[0].optimisticDeliveryPhase, .acceptedAwaitingEcho)

        XCTAssertTrue(applyMobileOptimisticUserEcho([
            "type": "chat.message",
            "role": "user",
            "messageId": "ios-mobile-bot-chat-request-7",
            "text": "pending",
        ], messages: &messages))
        XCTAssertNil(messages[0].optimisticDeliveryPhase)
        XCTAssertNil(messages[0].optimisticDeliveryError)
    }

    func testNativeReactionPickerCatalogSearchCategoryAndLimit() {
        XCTAssertGreaterThan(mobileReactionCatalog.count, 96)

        let limited = mobileReactionPickerResults(
            query: "",
            category: .all
        )
        XCTAssertEqual(limited.count, 96)

        let cats = mobileReactionPickerResults(
            query: "cat",
            category: .nature
        )
        XCTAssertTrue(cats.contains(where: { $0.emoji == "🐱" }))
        XCTAssertTrue(cats.allSatisfy { $0.category == .nature })

        let heart = mobileReactionPickerResults(
            query: "heart",
            category: .symbols
        )
        XCTAssertFalse(heart.isEmpty)
        XCTAssertTrue(heart.allSatisfy { $0.category == .symbols })

        XCTAssertTrue(
            mobileReactionPickerResults(
                query: "nonexistent-reaction-query",
                category: .all
            ).isEmpty
        )
        XCTAssertTrue(
            mobileReactionPickerResults(
                query: "",
                category: .all,
                limit: 0
            ).isEmpty
        )
    }

    func testNativeReactionPickerAccessibilityReflectsCurrentUserState() {
        let item = MobileReactionCatalogItem(
            emoji: "👍",
            name: "thumbs up approve",
            category: .people
        )
        XCTAssertEqual(
            mobileReactionPickerAccessibilityLabel(
                item,
                reactedByCurrentUser: false
            ),
            "thumbs up approve, 👍, not reacted"
        )
        XCTAssertEqual(
            mobileReactionPickerAccessibilityLabel(
                item,
                reactedByCurrentUser: true
            ),
            "thumbs up approve, 👍, reacted by you"
        )
    }

    func testNativeReactionPickerKeyboardGridNavigationIsBounded() {
        XCTAssertEqual(
            mobileReactionPickerNextIndex(
                current: 7,
                count: 20,
                columns: 6,
                move: .left
            ),
            6
        )
        XCTAssertEqual(
            mobileReactionPickerNextIndex(
                current: 7,
                count: 20,
                columns: 6,
                move: .right
            ),
            8
        )
        XCTAssertEqual(
            mobileReactionPickerNextIndex(
                current: 7,
                count: 20,
                columns: 6,
                move: .up
            ),
            1
        )
        XCTAssertEqual(
            mobileReactionPickerNextIndex(
                current: 17,
                count: 20,
                columns: 6,
                move: .down
            ),
            19
        )
        XCTAssertEqual(
            mobileReactionPickerNextIndex(
                current: 9,
                count: 20,
                columns: 6,
                move: .first
            ),
            0
        )
        XCTAssertEqual(
            mobileReactionPickerNextIndex(
                current: 9,
                count: 20,
                columns: 6,
                move: .last
            ),
            19
        )
        XCTAssertNil(
            mobileReactionPickerNextIndex(
                current: 0,
                count: 0,
                columns: 6,
                move: .right
            )
        )
    }

    func testNativeReactionPickerFailsClosedUntilCanonicalMessageIsSettled() {
        var message = MobileChatMessage(
            id: "local-reaction",
            role: .assistant,
            text: "react",
            canonicalMessageId: "canonical-reaction"
        )
        XCTAssertTrue(isMobileReactionActionable(message))

        message.streaming = true
        XCTAssertFalse(isMobileReactionActionable(message))

        message.streaming = false
        message.optimisticDeliveryPhase = .pending
        XCTAssertFalse(isMobileReactionActionable(message))

        message.optimisticDeliveryPhase = .acceptedAwaitingEcho
        XCTAssertFalse(isMobileReactionActionable(message))

        message.optimisticDeliveryPhase = nil
        message.canonicalMessageId = nil
        XCTAssertFalse(isMobileReactionActionable(message))

        let notice = MobileChatMessage(
            id: "notice-reaction",
            role: .assistant,
            text: "notice",
            kind: .notice,
            canonicalMessageId: "notice-reaction"
        )
        XCTAssertFalse(isMobileReactionActionable(notice))
    }

    func testNativeTranscriptAdjacencyProjectsSixFieldGroupingContract() {
        let entries = [
            MobileChatMessage(
                id: "u1",
                role: .user,
                text: "one",
                canonicalMessageId: "u1"
            ),
            MobileChatMessage(
                id: "u2",
                role: .user,
                text: "two",
                canonicalMessageId: "u2"
            ),
            MobileChatMessage(
                id: "a1",
                role: .assistant,
                text: "answer",
                canonicalMessageId: "a1"
            ),
        ]

        let adjacency = projectMobileTranscriptAdjacency(entries)
        XCTAssertEqual(adjacency.count, 3)

        XCTAssertFalse(adjacency[0].isContinuedFromPrev)
        XCTAssertTrue(adjacency[0].isContinuedToNext)
        XCTAssertFalse(adjacency[0].isGroupStart)
        XCTAssertTrue(adjacency[0].isRunStart)
        XCTAssertFalse(adjacency[0].isGroupEnd)

        XCTAssertTrue(adjacency[1].isContinuedFromPrev)
        XCTAssertFalse(adjacency[1].isContinuedToNext)
        XCTAssertFalse(adjacency[1].isRunStart)
        XCTAssertTrue(adjacency[1].isGroupEnd)

        XCTAssertFalse(adjacency[2].isContinuedFromPrev)
        XCTAssertTrue(
            adjacency[2].isContinuedToNext,
            "Assistant indicator seam remains open until a thread chip or reaction closes it."
        )
        XCTAssertTrue(adjacency[2].isGroupStart)
        XCTAssertTrue(adjacency[2].isRunStart)
        XCTAssertFalse(adjacency[2].isGroupEnd)
    }

    func testNativeTranscriptAdjacencyThreadChipAndSpecialRowsBreakSeams() {
        var first = MobileChatMessage(
            id: "a1",
            role: .assistant,
            text: "one",
            canonicalMessageId: "a1"
        )
        let second = MobileChatMessage(
            id: "a2",
            role: .assistant,
            text: "two",
            canonicalMessageId: "a2"
        )
        let notice = MobileChatMessage(
            id: "notice",
            role: .assistant,
            text: "notice",
            kind: .notice
        )

        var adjacency = projectMobileTranscriptAdjacency(
            [first, second],
            threadChipEntryIDs: ["a1"]
        )
        XCTAssertTrue(adjacency[0].isFollowedByThreadChip)
        XCTAssertFalse(adjacency[0].isContinuedToNext)
        XCTAssertFalse(adjacency[1].isContinuedFromPrev)

        first.reactions = [.init(emoji: "👍", by: "me")]
        adjacency = projectMobileTranscriptAdjacency([first])
        XCTAssertFalse(
            adjacency[0].isContinuedToNext,
            "A reaction closes the assistant indicator seam."
        )

        adjacency = projectMobileTranscriptAdjacency([first, notice, second])
        XCTAssertEqual(adjacency[1], .empty)
        XCTAssertFalse(adjacency[2].isContinuedFromPrev)
        XCTAssertTrue(adjacency[2].isRunStart)
    }

    func testNativeTranscriptAdjacencyExcludesNonBubbleMessageVariants() {
        let emoji = MobileChatMessage(
            id: "emoji",
            role: .user,
            text: "👍",
            canonicalMessageId: "emoji"
        )
        let attachment = MobileChatMessage(
            id: "attachment",
            role: .user,
            text: "",
            canonicalMessageId: "attachment",
            attachmentURL: "file:///tmp/a.txt"
        )
        let sendCard = MobileChatMessage(
            id: "send",
            role: .assistant,
            text: "https://example.com",
            canonicalMessageId: "send",
            sendMessageTextProjection: .init(
                id: "send",
                content: "https://example.com",
                images: [],
                streaming: false,
                presentation: .urlCard("https://example.com")
            )
        )
        let imageMarkdown = MobileChatMessage(
            id: "image-markdown",
            role: .assistant,
            text: "![alt](https://example.com/a.png)",
            canonicalMessageId: "image-markdown"
        )

        let adjacency = projectMobileTranscriptAdjacency([
            emoji,
            attachment,
            sendCard,
            imageMarkdown,
        ])
        XCTAssertEqual(adjacency.count, 4)
        XCTAssertTrue(adjacency.allSatisfy { !$0.isContinuedFromPrev })
        XCTAssertTrue(
            adjacency.allSatisfy { !$0.isFollowedByThreadChip }
        )
    }

    func testNativeEditorSuggestionProjectsCanonicalRosterAndEveryone() {
        let visible = MobileBotSummary(
            id: "agent-1",
            name: "Research Bot",
            description: "Research",
            title: "Researcher"
        )
        let group = MobileBotSummary(
            id: "agent-2",
            name: "Review Team",
            description: "Review",
            isGroup: true,
            memberIds: ["a", "b"]
        )
        let hidden = MobileBotSummary(
            id: "agent-hidden",
            name: "Hidden",
            description: "Hidden",
            hidden: true
        )

        let rows = projectMobileEditorMentionSuggestions([visible, group, hidden])
        XCTAssertEqual(rows.map(\.id), ["__everyone__", "agent-1", "agent-2"])
        XCTAssertEqual(rows[0].insertion, "@everyone")
        XCTAssertEqual(rows[1].subtitle, "Researcher")
        XCTAssertEqual(rows[2].subtitle, "2 agents")
        XCTAssertFalse(rows.contains(where: { $0.id == "agent-hidden" }))
    }

    func testNativeEditorSuggestionProjectsTriggeredAndReferenceWorkflows() {
        let rows = projectMobileEditorWorkflowSuggestions([
            [
                "id": "daily",
                "name": "Daily Review",
                "description": "Review changes",
                "trigger": [
                    "schedule": "@daily",
                    "isEnabled": true,
                ],
            ],
            [
                "id": "release",
                "name": "Release Train",
                "description": "Prepare release",
            ],
            [
                "id": "bad-trigger",
                "name": "Bad",
                "trigger": ["schedule": ""],
            ],
        ])

        XCTAssertEqual(rows.map(\.id), ["daily", "release"])
        XCTAssertEqual(rows[0].triggerSchedule, "@daily")
        XCTAssertEqual(rows[0].triggerEnabled, true)
        XCTAssertNil(rows[1].triggerSchedule)

        let mentionContext = try! XCTUnwrap(mobileEditorSuggestionContext("@rev"))
        let mentionRows = mobileEditorSuggestionRows(
            context: mentionContext,
            assistants: [],
            workflows: rows
        )
        XCTAssertEqual(mentionRows.map(\.id), ["daily"])

        let slashContext = try! XCTUnwrap(mobileEditorSuggestionContext("/rel"))
        let slashRows = mobileEditorSuggestionRows(
            context: slashContext,
            assistants: [],
            workflows: rows
        )
        XCTAssertEqual(slashRows.map(\.id), ["release"])
    }

    func testNativeEditorSuggestionContextInsertionAndEmojiReuse() throws {
        let mention = try XCTUnwrap(
            mobileEditorSuggestionContext("Ask @Res")
        )
        XCTAssertEqual(mention.trigger, "@")
        XCTAssertEqual(mention.query, "Res")

        let assistant = MobileEditorSuggestionItem(
            id: "agent-1",
            category: .assistants,
            label: "Research Bot",
            insertion: "@Research Bot"
        )
        XCTAssertEqual(
            applyMobileEditorSuggestion(
                draft: "Ask @Res",
                context: mention,
                item: assistant
            ),
            "Ask @Research Bot "
        )

        let emojiContext = try XCTUnwrap(
            mobileEditorSuggestionContext("Looks :hea")
        )
        XCTAssertEqual(emojiContext.trigger, ":")
        let emojiRows = mobileEditorSuggestionRows(
            context: emojiContext,
            assistants: [],
            workflows: []
        )
        XCTAssertTrue(emojiRows.contains(where: {
            $0.category == .emoji && $0.subtitle?.contains("heart") == true
        }))
        XCTAssertLessThanOrEqual(emojiRows.count, 96)
    }

    func testNativeEditorSuggestionKeyboardSelectionWrapsAndBounds() {
        XCTAssertEqual(
            mobileEditorSuggestionNextIndex(
                current: 0,
                count: 3,
                move: .previous
            ),
            2
        )
        XCTAssertEqual(
            mobileEditorSuggestionNextIndex(
                current: 2,
                count: 3,
                move: .next
            ),
            0
        )
        XCTAssertEqual(
            mobileEditorSuggestionNextIndex(
                current: 1,
                count: 3,
                move: .first
            ),
            0
        )
        XCTAssertEqual(
            mobileEditorSuggestionNextIndex(
                current: 1,
                count: 3,
                move: .last
            ),
            2
        )
        XCTAssertNil(
            mobileEditorSuggestionNextIndex(
                current: nil,
                count: 0,
                move: .next
            )
        )
    }

    func testNativeEditorSuggestionRecencyOnlyBreaksEqualSearchScores() throws {
        let context = try XCTUnwrap(mobileEditorSuggestionContext("@"))
        let alpha = MobileEditorSuggestionItem(
            id: "alpha",
            category: .assistants,
            label: "Alpha",
            insertion: "@Alpha"
        )
        let beta = MobileEditorSuggestionItem(
            id: "beta",
            category: .assistants,
            label: "Beta",
            insertion: "@Beta"
        )
        let rows = mobileEditorSuggestionRows(
            context: context,
            assistants: [alpha, beta],
            workflows: [],
            recentKeys: ["assistants:beta", "assistants:alpha"]
        )
        XCTAssertEqual(rows.map(\.id), ["beta", "alpha"])
    }

    func testTranscriptLoadRetrySurfaceUsesCanonicalCopy() {
        XCTAssertEqual(MobileTranscriptLoadErrorCopy.title, "Couldn't load conversation")
        XCTAssertEqual(
            MobileTranscriptLoadErrorCopy.detail,
            "Couldn't load this conversation. Check your connection and try again."
        )
        XCTAssertEqual(MobileTranscriptLoadErrorCopy.retry, "Retry")
    }

}
