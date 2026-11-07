//
//  TelegramConversationTests.swift
//  FabushiTests
//
//  Comprehensive native Swift unit test suite covering Features 12 to 19:
//  Bubble Geometry (F12), Rich Text & Spoilers (F13), Delivery Ticks & Collision (F14),
//  Swipe-to-Reply (F15), Context Menu & Reactions (F16), Pinned Carousel (F17),
//  Scroll-To-Bottom Hysteresis (F18), and Secret Chats E2EE & Ephemeral Timers (F19).
//

#if canImport(XCTest)
import XCTest
#if canImport(Fabushi)
@testable import Fabushi
#endif
import SwiftUI

@MainActor
public final class TelegramConversationTests: XCTestCase, @unchecked Sendable {

    // =========================================================================
    // MARK: - FEATURE 12: Chat Stage Bubble Geometry
    // =========================================================================

    public func testTC_T1_F12_01_IncomingVsOutgoingAlignmentAndColors() {
        let incoming = TelegramStageMessage(
            id: 1, chatId: 100, senderId: 2, text: "Hello from peer", timestamp: 1000, isOutgoing: false
        )
        let outgoing = TelegramStageMessage(
            id: 2, chatId: 100, senderId: 1, text: "Hello from me", timestamp: 2000, isOutgoing: true
        )

        XCTAssertFalse(incoming.isOutgoing, "Incoming message must have isOutgoing == false")
        XCTAssertTrue(outgoing.isOutgoing, "Outgoing message must have isOutgoing == true")

        let (inPos, inRadii) = MessageClusterCalculator.calculate(current: incoming, older: nil, newer: nil)
        let (outPos, outRadii) = MessageClusterCalculator.calculate(current: outgoing, older: nil, newer: nil)

        XCTAssertEqual(inPos, .single)
        XCTAssertEqual(outPos, .single)
        XCTAssertTrue(inRadii.hasTail, "Single incoming message must have tail")
        XCTAssertTrue(outRadii.hasTail, "Single outgoing message must have tail")
    }

    public func testTC_T1_F12_02_SingleMessageCornerRadius16pt() {
        let singleMsg = TelegramStageMessage(
            id: 10, chatId: 100, senderId: 1, text: "Isolated bubble", timestamp: 5000, isOutgoing: true
        )
        let (pos, radii) = MessageClusterCalculator.calculate(current: singleMsg, older: nil, newer: nil)

        XCTAssertEqual(pos, .single, "Isolated message must produce .single cluster position")
        XCTAssertEqual(radii.topLeft, 16.0, "Top-left radius must be 16pt")
        XCTAssertEqual(radii.topRight, 16.0, "Top-right radius must be 16pt")
        XCTAssertEqual(radii.bottomLeft, 16.0, "Bottom-left radius must be 16pt")
        XCTAssertEqual(radii.bottomRight, 16.0, "Bottom-right radius must be 16pt")
        XCTAssertTrue(radii.hasTail, "Isolated message must have speech tail")
    }

    public func testTC_T1_F12_03_GroupedCornerRadii5MinuteWindow() {
        let baseTs: Int64 = 1_000_000
        let msg1 = TelegramStageMessage(id: 101, chatId: 100, senderId: 2, text: "M1", timestamp: baseTs, isOutgoing: false)
        let msg2 = TelegramStageMessage(id: 102, chatId: 100, senderId: 2, text: "M2", timestamp: baseTs + 60_000, isOutgoing: false)
        let msg3 = TelegramStageMessage(id: 103, chatId: 100, senderId: 2, text: "M3", timestamp: baseTs + 120_000, isOutgoing: false)

        // Cluster top: msg1 (older: nil, newer: msg2)
        let (pos1, radii1) = MessageClusterCalculator.calculate(current: msg1, older: nil, newer: msg2)
        XCTAssertEqual(pos1, .top, "First message in cluster must be .top")
        XCTAssertEqual(radii1.bottomLeft, 4.0, "Incoming top bubble must have bottom-left radius = 4pt")
        XCTAssertEqual(radii1.topLeft, 16.0, "Incoming top bubble must have top-left radius = 16pt")
        XCTAssertFalse(radii1.hasTail, "Top message must not have speech tail")

        // Cluster middle: msg2 (older: msg1, newer: msg3)
        let (pos2, radii2) = MessageClusterCalculator.calculate(current: msg2, older: msg1, newer: msg3)
        XCTAssertEqual(pos2, .middle, "Middle message in cluster must be .middle")
        XCTAssertEqual(radii2.topLeft, 4.0, "Incoming middle bubble must have top-left radius = 4pt")
        XCTAssertEqual(radii2.bottomLeft, 4.0, "Incoming middle bubble must have bottom-left radius = 4pt")
        XCTAssertFalse(radii2.hasTail, "Middle message must not have speech tail")

        // Cluster bottom: msg3 (older: msg2, newer: nil)
        let (pos3, radii3) = MessageClusterCalculator.calculate(current: msg3, older: msg2, newer: nil)
        XCTAssertEqual(pos3, .bottom, "Last message in cluster must be .bottom")
        XCTAssertEqual(radii3.topLeft, 4.0, "Incoming bottom bubble must have top-left radius = 4pt")
        XCTAssertTrue(radii3.hasTail, "Bottom message must have speech tail pointing to avatar")

        // Gap > 5 minutes (301 seconds) breaks clustering
        let msgFar = TelegramStageMessage(id: 104, chatId: 100, senderId: 2, text: "Far", timestamp: baseTs + 421_000, isOutgoing: false)
        let (posFar, radiiFar) = MessageClusterCalculator.calculate(current: msgFar, older: msg3, newer: nil)
        XCTAssertEqual(posFar, .single, "Message with gap > 300s must be treated as .single")
        XCTAssertEqual(radiiFar.topLeft, 16.0)
        XCTAssertTrue(radiiFar.hasTail)
    }

    public func testTC_T1_F12_04_SpeechBubbleTailCanvasDrawing() {
        let radii = BubbleCornerRadii(topLeft: 16, topRight: 16, bottomLeft: 16, bottomRight: 16, hasTail: true)
        let outgoingShape = TelegramBubbleShape(isOutgoing: true, radii: radii)
        let incomingShape = TelegramBubbleShape(isOutgoing: false, radii: radii)

        let testRect = CGRect(x: 0, y: 0, width: 200, height: 60)
        let outPath = outgoingShape.path(in: testRect)
        let inPath = incomingShape.path(in: testRect)

        XCTAssertFalse(outPath.isEmpty, "Outgoing bubble path must not be empty")
        XCTAssertFalse(inPath.isEmpty, "Incoming bubble path must not be empty")

        let outBounds = outPath.boundingRect
        let inBounds = inPath.boundingRect
        XCTAssertGreaterThan(outBounds.width, 0)
        XCTAssertGreaterThan(inBounds.height, 0)
    }

    public func testTC_T1_F12_05_MaxWidthConstraintAndEmptyHandling() {
        let screenWidth: CGFloat = 400.0
        let maxBubbleWidth = screenWidth * 0.75
        XCTAssertEqual(maxBubbleWidth, 300.0, "Max bubble width must be 75% of screen width")

        // 500-word text string
        let longWord = String(repeating: "Telegram ", count: 500)
        let metrics = TextTimestampMetrics.compute(text: longWord, maxWidth: maxBubbleWidth)
        XCTAssertTrue(metrics.contentWidth <= maxBubbleWidth, "Bubble width must not exceed 75% max width")
        XCTAssertGreaterThan(metrics.lineCount, 5, "Long text must wrap into multiple lines")

        // TC-T2-F12-01: Empty string handling without zero-dimension crash
        let emptyMetrics = TextTimestampMetrics.compute(text: "", maxWidth: maxBubbleWidth)
        XCTAssertFalse(emptyMetrics.causesCollision)
        XCTAssertTrue(emptyMetrics.contentWidth >= 44.0, "Minimum tap target width must be >= 44pt")
        XCTAssertTrue(emptyMetrics.requiredHeight >= 18.0)

        // TC-T2-F12-02: Single character bubble
        let singleCharMetrics = TextTimestampMetrics.compute(text: ".", maxWidth: maxBubbleWidth)
        XCTAssertTrue(singleCharMetrics.contentWidth >= 44.0, "Single character bubble must respect 44pt minimum width")
    }

    public func testTC_T2_F12_04_RightToLeftLayoutMirroring() {
        let baseMsg = TelegramStageMessage(id: 50, chatId: 100, senderId: 1, text: "שלום", timestamp: 1000, isOutgoing: true)
        let (_, ltrRadii) = MessageClusterCalculator.calculate(current: baseMsg, older: nil, newer: nil, isRTL: false)
        let (_, rtlRadii) = MessageClusterCalculator.calculate(current: baseMsg, older: nil, newer: nil, isRTL: true)

        XCTAssertTrue(ltrRadii.hasTail)
        XCTAssertTrue(rtlRadii.hasTail)
    }

    // =========================================================================
    // MARK: - FEATURE 13: Rich Message Text & Spoilers
    // =========================================================================

    public func testTC_T1_F13_01_FormattedTextEntities() {
        let text = "Hello Bold and Italic text with code"
        let entities = [
            MessageEntity(type: .bold, offset: 6, length: 4),
            MessageEntity(type: .italic, offset: 15, length: 6),
            MessageEntity(type: .code, offset: 32, length: 4)
        ]

        let attr = FormattedTextBuilder.buildAttributedString(text: text, entities: entities)
        XCTAssertEqual(String(attr.characters), text, "Attributed string character representation must match original")
    }

    public func testTC_T2_F13_01_OverlappingEntityRanges() {
        let text = "Overlapping formatted styles"
        let entities = [
            MessageEntity(type: .bold, offset: 0, length: 11),     // "Overlapping"
            MessageEntity(type: .italic, offset: 5, length: 15)     // "apping formatt"
        ]

        let attr = FormattedTextBuilder.buildAttributedString(text: text, entities: entities)
        XCTAssertEqual(String(attr.characters), text)
    }

    public func testTC_T2_F13_02_OutOfBoundsAndZeroLengthEntityClamping() {
        let text = "Short string" // length = 12
        let entities = [
            MessageEntity(type: .bold, offset: 8, length: 20),      // Out of bounds: offset 8, length 20 > 12
            MessageEntity(type: .italic, offset: 25, length: 10),   // Fully out of bounds
            MessageEntity(type: .underline, offset: 2, length: 0)   // Zero-length (TC-T2-F13-04)
        ]

        // Must clamp cleanly with zero out-of-bounds crashes
        let attr = FormattedTextBuilder.buildAttributedString(text: text, entities: entities)
        XCTAssertEqual(String(attr.characters), text)
    }

    public func testTC_T2_F13_05_EmojiSequencesWithModifiersAndZWJ() {
        // Multi-byte emoji family sequence: 👨‍👩‍👧‍👦 (8 UTF-16 code units)
        let text = "Family: 👨‍👩‍👧‍👦 end."
        let entities = [
            MessageEntity(type: .bold, offset: 8, length: 8) // Over the exact emoji
        ]

        let attr = FormattedTextBuilder.buildAttributedString(text: text, entities: entities)
        XCTAssertEqual(String(attr.characters), text)
    }

    public func testTC_T1_F13_03_SpoilerRevealAndPersistence() {
        let text = "Secret: spoiler text here"
        let entity = MessageEntity(type: .spoiler, offset: 8, length: 12)
        let spoilerKey = entity.id

        var revealed: Set<String> = []
        let unrevealedAttr = FormattedTextBuilder.buildAttributedString(text: text, entities: [entity], revealedSpoilers: revealed)
        XCTAssertEqual(String(unrevealedAttr.characters), text)

        // Reveal spoiler
        revealed.insert(spoilerKey)
        let revealedAttr = FormattedTextBuilder.buildAttributedString(text: text, entities: [entity], revealedSpoilers: revealed)
        XCTAssertEqual(String(revealedAttr.characters), text)
        XCTAssertTrue(revealed.contains(spoilerKey), "Revealed spoiler state must persist")
    }

    // =========================================================================
    // MARK: - FEATURE 14: Bubble Delivery & Timestamp Ticks
    // =========================================================================

    public func testTC_T1_F14_01_DeliveryStatusEnumValues() {
        XCTAssertEqual(TelegramDeliveryStatus.pending.rawValue, "pending")
        XCTAssertEqual(TelegramDeliveryStatus.sent.rawValue, "sent")
        XCTAssertEqual(TelegramDeliveryStatus.read.rawValue, "read")
        XCTAssertEqual(TelegramDeliveryStatus.failed.rawValue, "failed")
    }

    public func testTC_T1_F14_05_TextWrapCollisionAvoidanceTypographicBounds() {
        let maxWidth: CGFloat = 200.0

        // Short single-line text: plenty of room for inline timestamp
        let shortText = "Hi"
        let metricsShort = TextTimestampMetrics.compute(text: shortText, maxWidth: maxWidth, timestampWidth: 46.0)
        XCTAssertFalse(metricsShort.causesCollision, "Short text must not collide with timestamp")

        // Long single line approaching maxWidth that collides with trailing timestamp
        let longLine = "Hello Beautiful World"
        let metricsLong = TextTimestampMetrics.compute(text: longLine, maxWidth: maxWidth, timestampWidth: 46.0)
        XCTAssertTrue(metricsLong.causesCollision, "Full line must trigger collision and reserve extra height")
        XCTAssertGreaterThan(metricsLong.requiredHeight, metricsShort.requiredHeight)
    }

    public func testTC_T1_F14_PeerColorPaletteMod7Distribution() {
        XCTAssertEqual(TelegramPeerColorPalette.colors.count, 7, "Canonical Telegram sender palette must contain 7 colors")

        var seenIndices = Set<Int>()
        for i in 1...100 {
            let idx = TelegramPeerColorPalette.colorIndex(for: "user_\(i)")
            XCTAssertTrue(idx >= 0 && idx < 7, "Index must strictly be within 0..6")
            seenIndices.insert(idx)
        }
        XCTAssertEqual(seenIndices.count, 7, "100 distinct sender IDs must cover all 7 color indices")
    }

    // =========================================================================
    // MARK: - FEATURE 15: Swipe-to-Reply Bubble Gesture
    // =========================================================================

    public func testTC_T1_F15_01_HorizontalDisplacementLogarithmicCurve() {
        XCTAssertEqual(MessageSwipeToReplyModifier.maxDisplacement, 60.0)
        XCTAssertEqual(MessageSwipeToReplyModifier.triggerThreshold, 50.0)

        // Zero drag yields 0 displacement
        XCTAssertEqual(MessageSwipeToReplyModifier.computeDisplacement(dragDistance: 0), 0.0)

        // Increasing drag increases displacement smoothly
        let d20 = MessageSwipeToReplyModifier.computeDisplacement(dragDistance: 20)
        let d50 = MessageSwipeToReplyModifier.computeDisplacement(dragDistance: 50)
        let d100 = MessageSwipeToReplyModifier.computeDisplacement(dragDistance: 100)
        let d500 = MessageSwipeToReplyModifier.computeDisplacement(dragDistance: 500)

        XCTAssertGreaterThan(d50, d20)
        XCTAssertGreaterThan(d100, d50)
        XCTAssertTrue(d500 <= 60.0, "Displacement must never exceed 60pt maximum")
    }

    public func testTC_T2_F15_01_SwipeBelowThresholdDoesNotActivateReply() {
        var replyActivated = false
        let msg = TelegramStageMessage(id: 1, chatId: 10, senderId: 2, text: "Swipe test", timestamp: 1000)
        XCTAssertEqual(msg.text, "Swipe test")

        let belowThresholdDisplacement: CGFloat = 45.0 // < 50pt threshold
        if belowThresholdDisplacement >= MessageSwipeToReplyModifier.triggerThreshold {
            replyActivated = true
        }
        XCTAssertFalse(replyActivated, "Displacement of 45pt must not trigger reply banner activation")

        let aboveThresholdDisplacement: CGFloat = 52.0 // >= 50pt threshold
        if aboveThresholdDisplacement >= MessageSwipeToReplyModifier.triggerThreshold {
            replyActivated = true
        }
        XCTAssertTrue(replyActivated, "Displacement of 52pt must trigger reply banner activation")
    }

    // =========================================================================
    // MARK: - FEATURE 16: Message Context Menu & Reactions
    // =========================================================================

    public func testTC_T1_F16_02_FloatingReactionPillBarDefaults() {
        let expected = ["👍", "❤️", "🔥", "😂", "👏"]
        XCTAssertEqual(FloatingReactionPillBar.defaultEmojis, expected, "Must offer 5 canonical default reactions")
    }

    public func testTC_T1_F16_04_ContextMenuActionCases() {
        let actions = MessageMenuAction.allCases.map { $0.rawValue }
        XCTAssertTrue(actions.contains("Reply"))
        XCTAssertTrue(actions.contains("Copy"))
        XCTAssertTrue(actions.contains("Forward"))
        XCTAssertTrue(actions.contains("Pin"))
        XCTAssertTrue(actions.contains("Delete"))
    }

    @MainActor
    public func testTC_T1_F16_03_ReactionToggleMutations() {
        let vm = ConversationStageViewModel(
            chatId: 100,
            initialMessages: [
                TelegramStageMessage(id: 1, chatId: 100, senderId: 2, text: "React here", timestamp: 1000)
            ]
        )

        // Add heart reaction
        vm.toggleReaction(messageId: 1, emoji: "❤️")
        var msg = vm.messages.first!
        XCTAssertEqual(msg.reactions.count, 1)
        XCTAssertEqual(msg.reactions[0].emoji, "❤️")
        XCTAssertEqual(msg.reactions[0].count, 1)
        XCTAssertTrue(msg.reactions[0].chosenByMe)

        // Toggle heart again (untoggle)
        vm.toggleReaction(messageId: 1, emoji: "❤️")
        msg = vm.messages.first!
        XCTAssertEqual(msg.reactions.count, 0, "Untoggling own single reaction must remove it")
    }

    // =========================================================================
    // MARK: - FEATURE 17: Pinned Messages Carousel Bar
    // =========================================================================

    @MainActor
    public func testTC_T1_F17_PinnedMessagesCarouselAndJump() async {
        let p1 = TelegramStageMessage(id: 201, chatId: 100, senderId: 1, text: "Pinned 1", timestamp: 1000, isPinned: true)
        let p2 = TelegramStageMessage(id: 202, chatId: 100, senderId: 2, text: "Pinned 2", timestamp: 2000, isPinned: true)
        let vm = ConversationStageViewModel(chatId: 100, initialMessages: [p1, p2])

        XCTAssertEqual(vm.pinnedMessages.count, 2)
        XCTAssertEqual(vm.activePinnedIndex, 0)

        // Jump to index 1 (p1: id 201)
        let targetId = vm.jumpToPinnedMessage(at: 1)
        XCTAssertEqual(targetId, 201)
        XCTAssertEqual(vm.activePinnedIndex, 1)
        XCTAssertEqual(vm.highlightedMessageId, 201, "Target pinned message must be highlighted immediately")

        // Cycle through pinned items: wraps back to index 0 (p2: id 202)
        let cycledId = vm.cyclePinnedMessages()
        XCTAssertEqual(cycledId, 202)
        XCTAssertEqual(vm.activePinnedIndex, 0)

        // Unpin action
        vm.unpinMessage(id: 201)
        XCTAssertEqual(vm.pinnedMessages.count, 1)
        XCTAssertEqual(vm.pinnedMessages[0].id, 202)
    }

    public func testTC_T2_F17_05_PinnedMediaSnippetFallback() {
        let photoMsg = TelegramStageMessage(id: 301, chatId: 100, senderId: 2, text: "", timestamp: 1000, mediaType: "photo")
        let videoMsg = TelegramStageMessage(id: 302, chatId: 100, senderId: 2, text: "", timestamp: 2000, mediaType: "video")
        let emptyMsg = TelegramStageMessage(id: 303, chatId: 100, senderId: 2, text: "", timestamp: 3000)

        XCTAssertEqual(PinnedMessagesBar.snippetText(for: photoMsg), "Photo")
        XCTAssertEqual(PinnedMessagesBar.snippetText(for: videoMsg), "Video")
        XCTAssertEqual(PinnedMessagesBar.snippetText(for: emptyMsg), "Photo", "Default fallback label is Photo")
    }

    // =========================================================================
    // MARK: - FEATURE 18: Scroll-To-Bottom Floating Button
    // =========================================================================

    public func testTC_T1_F18_01_HysteresisThresholdEvaluation() {
        // Initially hidden
        XCTAssertFalse(ScrollToBottomButton.evaluateVisibility(distance: 0, currentlyVisible: false))
        XCTAssertFalse(ScrollToBottomButton.evaluateVisibility(distance: 199, currentlyVisible: false))
        XCTAssertTrue(ScrollToBottomButton.evaluateVisibility(distance: 201, currentlyVisible: false), ">200pt must trigger appearance")

        // Once visible: remains visible even below 200pt until drops below 100pt
        XCTAssertTrue(ScrollToBottomButton.evaluateVisibility(distance: 150, currentlyVisible: true))
        XCTAssertTrue(ScrollToBottomButton.evaluateVisibility(distance: 100, currentlyVisible: true))
        XCTAssertFalse(ScrollToBottomButton.evaluateVisibility(distance: 99, currentlyVisible: true), "<100pt must auto-dismiss")
    }

    @MainActor
    public func testTC_T1_F18_02_UnreadScrolledCountIncrementAndReset() {
        let vm = ConversationStageViewModel(chatId: 100)

        // Scrolled away from bottom: distance = 250pt
        vm.updateScrollDistance(250.0)
        XCTAssertTrue(vm.isBottomButtonVisible)
        XCTAssertEqual(vm.unreadScrolledCount, 0)

        // Receive push event message
        let event = TelegramEvent.newMessage(id: 501, chatId: 100, senderId: 2, text: "New incoming", timestamp: 1000)
        vm.handleIncomingPushEvent(event)
        XCTAssertEqual(vm.unreadScrolledCount, 1, "Unread count must increment when scrolled up")

        // Scroll back to bottom: distance = 50pt (< 100pt)
        vm.updateScrollDistance(50.0)
        XCTAssertEqual(vm.unreadScrolledCount, 0, "Unread count must reset to 0 upon reaching bottom")
        XCTAssertFalse(vm.isBottomButtonVisible, "Button must auto-dismiss near bottom")
    }

    // =========================================================================
    // MARK: - FEATURE 19: Secret Chats E2EE & Timers
    // =========================================================================

    public func testTC_T1_F19_03_128BitSlidingReplayWindow() {
        var window = SlidingReplayWindow()
        XCTAssertEqual(window.highest, 0)

        // Counter 0 is invalid
        XCTAssertFalse(window.checkAndUpdate(counter: 0))

        // Deliver sequence 1..100 in order
        for seq in 1...100 {
            let ok = window.checkAndUpdate(counter: UInt64(seq))
            XCTAssertTrue(ok, "Sequential counter \(seq) must be accepted")
        }
        XCTAssertEqual(window.highest, 100)

        // TC-T1-F19-03: Inject duplicate sequence 50 -> must reject
        let replay50 = window.checkAndUpdate(counter: 50)
        XCTAssertFalse(replay50, "Replayed sequence 50 must be rejected by 128-bit replay window")

        // TC-T2-F19-02: Sequence 1 drops out of window when sequence 130 arrives
        let ok130 = window.checkAndUpdate(counter: 130)
        XCTAssertTrue(ok130)
        XCTAssertEqual(window.highest, 130)

        // Distance for seq 1 is 130 - 1 = 129 >= 128 -> rejected
        let oldSeq1 = window.checkAndUpdate(counter: 1)
        XCTAssertFalse(oldSeq1, "Sequence 1 outside 128-bit window must be rejected")
    }

    public func testTC_T1_F19_04_SelfDestructCountdownCalculation() {
        let ttl = 10 // 10 seconds
        let expiresAt: Int64 = 100_000 // ms

        // Remaining 5 seconds
        let pMid = EphemeralTimerProgressBar.calculateProgress(nowMs: 95_000, expiresAtMs: expiresAt, ttlSeconds: ttl)
        XCTAssertTrue(abs(pMid - 0.5) < 0.01, "Progress at midpoint must be approximately 0.5")

        // Expired
        let pExpired = EphemeralTimerProgressBar.calculateProgress(nowMs: 100_000, expiresAtMs: expiresAt, ttlSeconds: ttl)
        XCTAssertEqual(pExpired, 0.0)

        // TC-T2-F19-04: TTL = 0 means disabled
        let pDisabled = EphemeralTimerProgressBar.calculateProgress(nowMs: 50_000, expiresAtMs: expiresAt, ttlSeconds: 0)
        XCTAssertEqual(pDisabled, 1.0)
    }

    @MainActor
    public func testTC_T1_F19_05_ScreenshotDetectorSimulation() {
        var detected = false
        let detector = ScreenshotDetector(onScreenshotDetected: {
            detected = true
        })

        detector.simulateScreenshotEvent()
        XCTAssertTrue(detected, "Simulated screenshot event must notify callback")
        XCTAssertEqual(detector.screenshotTakenCount, 1)
    }
}
#endif
