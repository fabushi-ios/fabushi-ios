import XCTest
@testable import Fabushi

final class SharedMessagingCoreParityTests: XCTestCase {
    private func chatMessage(
        id: String,
        conversationId: String = "conversation-1",
        text: String,
        time: String
    ) -> ChatMessage {
        ChatMessage(
            id: id,
            conversationId: conversationId,
            text: text,
            contentType: "text",
            mediaFileName: nil,
            mediaBlobId: nil,
            mediaMimeType: nil,
            mediaSizeBytes: 0,
            contactName: nil,
            latitude: nil,
            longitude: nil,
            pollQuestion: nil,
            pollOptions: [],
            pollMultipleAnswers: false,
            isOutgoing: false,
            time: time,
            replyToMessageId: nil,
            forwardOrigin: nil,
            reactions: [],
            deliveryState: "delivered",
            isEdited: false,
            isPinned: false
        )
    }

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

    func testLateSyncBaselinePreservesOnlyIncrementalMessagesObservedBeforeBaseline() {
        let staleCached = chatMessage(id: "stale", text: "stale cached row", time: "09:00")
        let baseline = chatMessage(id: "baseline", text: "snapshot row", time: "10:00")
        let live = chatMessage(id: "live", text: "live event", time: "10:01")
        let authoritativeLive = chatMessage(id: "live", text: "snapshot wins on same id", time: "10:02")

        let preserved = reconcileMessagingSyncBaseline(
            current: ["conversation-1": [staleCached, live]],
            incoming: ["conversation-1": [baseline]],
            observedBeforeBaseline: ["conversation-1": ["live"]]
        )
        XCTAssertEqual(preserved["conversation-1"]?.map(\.id), ["baseline", "live"])
        XCTAssertEqual(preserved["conversation-1"]?.last?.text, "live event")
        XCTAssertFalse(preserved["conversation-1"]?.contains(where: { $0.id == "stale" }) ?? true)

        let deduped = reconcileMessagingSyncBaseline(
            current: ["conversation-1": [live]],
            incoming: ["conversation-1": [baseline, authoritativeLive]],
            observedBeforeBaseline: ["conversation-1": ["live"]]
        )
        XCTAssertEqual(deduped["conversation-1"]?.map(\.id), ["baseline", "live"])
        XCTAssertEqual(deduped["conversation-1"]?.last?.text, "snapshot wins on same id")
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
    func testHumanMediaProjectionUsesOnlyExplicitDurableGroupIdentity() {
        func mediaMessage(
            id: String,
            group: String?,
            index: Int?,
            count: Int?,
            outgoing: Bool
        ) -> ChatMessage {
            var message = ChatMessage(
                id: id,
                conversationId: "conversation-1",
                text: "media",
                contentType: "photo",
                mediaFileName: "\(id).jpg",
                mediaBlobId: "blob-\(id)",
                mediaMimeType: "image/jpeg",
                mediaSizeBytes: 10,
                contactName: nil,
                latitude: nil,
                longitude: nil,
                pollQuestion: nil,
                pollOptions: [],
                pollMultipleAnswers: false,
                isOutgoing: outgoing,
                time: "10:00",
                replyToMessageId: nil,
                forwardOrigin: nil,
                reactions: [],
                deliveryState: "delivered",
                isEdited: false,
                isPinned: false
            )
            message.mediaGroupId = group
            message.mediaGroupIndex = index
            message.mediaGroupCount = count
            message.mediaAttachments = [
                ChatMediaAttachment(
                    id: id,
                    messageId: id,
                    contentType: "photo",
                    fileName: "\(id).jpg",
                    blobId: "blob-\(id)",
                    mimeType: "image/jpeg",
                    sizeBytes: 10,
                    groupIndex: index
                )
            ]
            message.groupedMessageIds = [id]
            return message
        }

        let first = mediaMessage(id: "m1", group: "g", index: 0, count: 2, outgoing: true)
        let second = mediaMessage(id: "m2", group: "g", index: 1, count: 2, outgoing: true)
        let unrelated = mediaMessage(id: "m3", group: nil, index: nil, count: nil, outgoing: true)
        let otherSender = mediaMessage(id: "m4", group: "g", index: 0, count: 2, outgoing: false)

        let projected = projectHumanMediaGroups([second, unrelated, first, otherSender])
        XCTAssertEqual(projected.count, 3)
        let gallery = try! XCTUnwrap(projected.first(where: { $0.mediaGroupId == "g" && $0.isOutgoing }))
        XCTAssertEqual(gallery.mediaAttachments.map(\.messageId), ["m1", "m2"])
        XCTAssertEqual(gallery.groupedMessageIds, ["m1", "m2"])
        XCTAssertEqual(projected.filter { $0.id == "m3" }.count, 1)
        XCTAssertEqual(projected.filter { $0.id == "m4" }.count, 1)
    }

}
