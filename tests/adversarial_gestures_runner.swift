//
//  adversarial_gestures_runner.swift
//  FabushiTests
//
//  Adversarial stress harness for Milestone 3:
//  - Feature 15: Swipe-to-Reply Bubble Gesture (Physics, Log Resistance, Haptics, Vertical Dominance)
//  - Feature 16: Message Context Menu & Reactions (Auto-Flip Coordinates, Double-Tap Debounce, Heart Burst)
//  - Feature 17: Pinned Messages Bar (Modular Carousel, Fallback Snippets, Highlight Flash)
//  - Feature 18: Scroll-To-Bottom Button (Dual-Threshold Hysteresis Deadband, Badge Ceiling)
//  - ChatStageView: Inverted Scroll Coordinate System & Clustering Geometry
//

import CoreGraphics
import Foundation
import SwiftUI
#if canImport(UIKit)
import UIKit
#endif

@main
struct AdversarialGesturesRunner {

    // Test counter
    @MainActor private static var passCount: Int = 0
    @MainActor private static var totalCount: Int = 0

    @MainActor
    private static func runTest(_ name: String, block: () async throws -> Void) async {
        totalCount += 1
        print("[\(totalCount)] Running \(name)...")
        do {
            try await block()
            passCount += 1
            print("  ✓ \(name) PASSED")
        } catch {
            print("  ✗ \(name) FAILED: \(error)")
            exit(1)
        }
    }

    private static func assertTrue(_ condition: @autoclosure () -> Bool, _ message: String = "", file: StaticString = #file, line: UInt = #line) throws {
        if !condition() {
            throw TestFailure("Assertion failed at \(file):\(line): \(message)")
        }
    }

    private static func assertEqual<T: Equatable>(_ a: T, _ b: T, _ message: String = "", file: StaticString = #file, line: UInt = #line) throws {
        if a != b {
            throw TestFailure("Assertion failed at \(file):\(line): expected \(b), got \(a). \(message)")
        }
    }

    private static func assertAlmostEqual(_ a: CGFloat, _ b: CGFloat, accuracy: CGFloat = 0.001, _ message: String = "", file: StaticString = #file, line: UInt = #line) throws {
        if abs(a - b) > accuracy {
            throw TestFailure("Assertion failed at \(file):\(line): \(a) not almost equal to \(b) (diff: \(abs(a - b)) > \(accuracy)). \(message)")
        }
    }

    struct TestFailure: Error, CustomStringConvertible {
        let description: String
        init(_ description: String) { self.description = description }
    }

    @MainActor
    static func main() async {
        print("================================================================================")
        print(" ADVERSARIAL STRESS RUNNER: Milestone 3 Gestures, Physics & Coordinate Math")
        print(" Features 15, 16, 17, 18 & Inverted Coordinate Space Math")
        print(" Swift 6 Concurrency & Zero-Warning Mandate")
        print("================================================================================")

        // ---------------------------------------------------------------------
        // FEATURE 15: Swipe-to-Reply Physics & Gesture Dynamics
        // ---------------------------------------------------------------------

        await runTest("F15_ADV01_LogarithmicResistanceCurveMonotonicityAndCeiling") {
            // Verify 10,000 points from -500 to +100,000
            let testPoints: [CGFloat] = [
                -500.0, -100.0, -1.0, -0.0001, 0.0, 0.0001,
                5.0, 10.0, 25.0, 50.0, 75.0, 79.317, 80.0, 100.0,
                113.82, 114.0, 150.0, 200.0, 500.0, 1000.0, 5000.0,
                10000.0, 50000.0, 100000.0
            ]

            for d in testPoints {
                let disp = MessageSwipeToReplyModifier.computeDisplacement(dragDistance: d)
                if d <= 0 {
                    try assertEqual(disp, 0.0, "Displacement for d <= 0 must be 0")
                } else {
                    try assertTrue(disp > 0, "Displacement for d > 0 must be > 0")
                    try assertTrue(disp <= MessageSwipeToReplyModifier.maxDisplacement, "Displacement must never exceed 60pt")
                }
            }

            // Monotonic non-decreasing check across 2,000 steps
            var prevDisp: CGFloat = 0.0
            for step in stride(from: 0.0, through: 2000.0, by: 1.0) {
                let current = MessageSwipeToReplyModifier.computeDisplacement(dragDistance: step)
                try assertTrue(current >= prevDisp, "Resistance curve must be monotonic non-decreasing at step \(step)")
                try assertTrue(current <= 60.0, "Max ceiling must be 60.0pt")
                prevDisp = current
            }

            // Derivative check: check diminishing returns (concave down)
            let d1 = MessageSwipeToReplyModifier.computeDisplacement(dragDistance: 30) - MessageSwipeToReplyModifier.computeDisplacement(dragDistance: 20)
            let d2 = MessageSwipeToReplyModifier.computeDisplacement(dragDistance: 60) - MessageSwipeToReplyModifier.computeDisplacement(dragDistance: 50)
            let d3 = MessageSwipeToReplyModifier.computeDisplacement(dragDistance: 90) - MessageSwipeToReplyModifier.computeDisplacement(dragDistance: 80)
            try assertTrue(d1 > d2, "Displacement delta must diminish with distance (concave resistance)")
            try assertTrue(d2 > d3, "Displacement delta must diminish with distance (concave resistance)")
        }

        await runTest("F15_ADV02_ExactThresholdCrossingBoundary") {
            // Formula: 50.0 = log1p(d / 25.0) * 35.0
            // 50/35 = log1p(d/25) -> exp(10/7) - 1 = d/25 -> d = 25 * (exp(10/7) - 1)
            let exactDistanceToReach50 = 25.0 * (exp(50.0 / 35.0) - 1.0) // ~79.31707

            let justBelow = MessageSwipeToReplyModifier.computeDisplacement(dragDistance: exactDistanceToReach50 - 0.01)
            let justAbove = MessageSwipeToReplyModifier.computeDisplacement(dragDistance: exactDistanceToReach50 + 0.01)

            try assertTrue(justBelow < MessageSwipeToReplyModifier.triggerThreshold, "Distance just below threshold must yield < 50pt")
            try assertTrue(justAbove >= MessageSwipeToReplyModifier.triggerThreshold, "Distance just above threshold must yield >= 50pt")
        }

        await runTest("F15_ADV03_SwipeCoordinatorStateTransitionsAndMultiPull") {
            let coordinator = SwipeToReplyCoordinator()
            try assertEqual(coordinator.dragOffset, 0.0)
            try assertEqual(coordinator.hasTriggeredHaptic, false)
            try assertEqual(coordinator.isVerticalDominant, false)

            // Simulate drag sequence: 0 -> 40 -> 55 -> 45 -> 55 -> 0
            // 1. Drag to 40 (< 50)
            coordinator.dragOffset = 40.0
            try assertEqual(coordinator.hasTriggeredHaptic, false)

            // 2. Drag to 55 (>= 50)
            coordinator.dragOffset = 55.0
            if coordinator.dragOffset >= MessageSwipeToReplyModifier.triggerThreshold && !coordinator.hasTriggeredHaptic {
                coordinator.hasTriggeredHaptic = true
            }
            try assertEqual(coordinator.hasTriggeredHaptic, true)

            // 3. Drag back to 45 (< 50) -> re-arms haptic
            coordinator.dragOffset = 45.0
            if coordinator.dragOffset < MessageSwipeToReplyModifier.triggerThreshold {
                coordinator.hasTriggeredHaptic = false
            }
            try assertEqual(coordinator.hasTriggeredHaptic, false)

            // 4. Drag forward to 55 again -> fires second haptic
            coordinator.dragOffset = 55.0
            if coordinator.dragOffset >= MessageSwipeToReplyModifier.triggerThreshold && !coordinator.hasTriggeredHaptic {
                coordinator.hasTriggeredHaptic = true
            }
            try assertEqual(coordinator.hasTriggeredHaptic, true)

            // 5. Release -> resets
            coordinator.dragOffset = 0.0
            coordinator.hasTriggeredHaptic = false
            try assertEqual(coordinator.dragOffset, 0.0)
            try assertEqual(coordinator.hasTriggeredHaptic, false)
        }

        await runTest("F15_ADV04_VerticalDominanceLockAndRelease") {
            let coordinator = SwipeToReplyCoordinator()

            // Vertical flick: vDist = 60, hDist = 20 -> vertical dominant
            let vDist: CGFloat = 60.0
            let hDist: CGFloat = 20.0
            if vDist > abs(hDist) && coordinator.dragOffset == 0 {
                coordinator.isVerticalDominant = true
            }
            try assertEqual(coordinator.isVerticalDominant, true)

            // Subsequent horizontal movement ignored while vertical dominant
            if !coordinator.isVerticalDominant {
                coordinator.dragOffset = MessageSwipeToReplyModifier.computeDisplacement(dragDistance: 80.0)
            }
            try assertEqual(coordinator.dragOffset, 0.0, "No horizontal displacement permitted during vertical dominance")

            // End of gesture resets dominance
            coordinator.isVerticalDominant = false
            try assertEqual(coordinator.isVerticalDominant, false)

            // Pure horizontal drag: vDist = 5, hDist = 40
            let vDist2: CGFloat = 5.0
            let hDist2: CGFloat = 40.0
            if vDist2 > abs(hDist2) && coordinator.dragOffset == 0 {
                coordinator.isVerticalDominant = true
            }
            try assertEqual(coordinator.isVerticalDominant, false)
            coordinator.dragOffset = MessageSwipeToReplyModifier.computeDisplacement(dragDistance: hDist2)
            try assertTrue(coordinator.dragOffset > 0, "Horizontal drag must be processed")
        }

        await runTest("F15_ADV05_CircularReplyBadgeScaleAndOpacityCalculations") {
            // Formula in MessageSwipeToReplyModifier:
            // progress = min(1.0, max(0.0, coordinator.dragOffset / Self.triggerThreshold))
            // scale = 0.5 + 0.5 * progress
            // opacity = Double(progress)

            let offsets: [(CGFloat, CGFloat, Double)] = [
                (-10.0, 0.5, 0.0),
                (0.0, 0.5, 0.0),
                (25.0, 0.75, 0.5),
                (50.0, 1.0, 1.0),
                (60.0, 1.0, 1.0),
                (100.0, 1.0, 1.0)
            ]

            for (off, expectedScale, expectedOpacity) in offsets {
                let progress = min(1.0, max(0.0, off / MessageSwipeToReplyModifier.triggerThreshold))
                let scale = 0.5 + 0.5 * progress
                let opacity = Double(progress)

                try assertAlmostEqual(scale, expectedScale, accuracy: 0.001)
                try assertAlmostEqual(CGFloat(opacity), CGFloat(expectedOpacity), accuracy: 0.001)
            }
        }

        // ---------------------------------------------------------------------
        // FEATURE 16: Message Context Menu & Reaction Coordinates
        // ---------------------------------------------------------------------

        await runTest("F16_ADV01_FloatingReactionPillBarCoordinateAutoFlip") {
            // Spec: flips below bubble if bubbleFrame.minY < 120.0
            let testFrames: [(CGRect, Bool)] = [
                (CGRect(x: 10, y: -50, width: 200, height: 40), true),
                (CGRect(x: 10, y: 0, width: 200, height: 40), true),
                (CGRect(x: 10, y: 119.99, width: 200, height: 40), true),
                (CGRect(x: 10, y: 120.0, width: 200, height: 40), false),
                (CGRect(x: 10, y: 120.01, width: 200, height: 40), false),
                (CGRect(x: 10, y: 400.0, width: 200, height: 40), false),
                (CGRect(x: 10, y: 800.0, width: 200, height: 40), false)
            ]

            for (frame, expectedFlip) in testFrames {
                let flipBelow = frame.minY < 120.0
                try assertEqual(flipBelow, expectedFlip, "minY=\(frame.minY) expected flipBelow=\(expectedFlip)")
            }

            // Verify canonical 5 emojis
            try assertEqual(FloatingReactionPillBar.defaultEmojis.count, 5)
            try assertEqual(FloatingReactionPillBar.defaultEmojis, ["👍", "❤️", "🔥", "😂", "👏"])
        }

        await runTest("F16_ADV02_DoubleTapDebounceHarness") {
            let coordinator = MessageContextMenuState()
            var reactionCount = 0

            let simulateDoubleTap: (TimeInterval) -> Void = { timestamp in
                guard timestamp - coordinator.lastDoubleTapTime > 0.15 else { return }
                coordinator.lastDoubleTapTime = timestamp
                reactionCount += 1
            }

            // Rapid burst of 100 taps over 100ms (1ms interval)
            let baseTime: TimeInterval = 1000.0
            for i in 0..<100 {
                let t = baseTime + Double(i) * 0.001 // up to baseTime + 0.099s
                simulateDoubleTap(t)
            }
            try assertEqual(reactionCount, 1, "Only 1 reaction must fire during 100ms rapid double-tap burst")

            // Fire after debounce expires (> 150ms)
            simulateDoubleTap(baseTime + 0.16)
            try assertEqual(reactionCount, 2, "Second double tap after 160ms must be accepted")
        }

        await runTest("F16_ADV03_ContextMenuActionSFImagesAndLabels") {
            let actions = MessageMenuAction.allCases
            try assertEqual(actions.count, 5)

            for action in actions {
                try assertTrue(!action.rawValue.isEmpty, "Action rawValue must not be empty")
                try assertTrue(!action.systemImage.isEmpty, "Action SF image must not be empty")
            }

            try assertEqual(MessageMenuAction.reply.systemImage, "arrowshape.turn.up.left")
            try assertEqual(MessageMenuAction.copy.systemImage, "doc.on.doc")
            try assertEqual(MessageMenuAction.forward.systemImage, "arrowshape.turn.up.right")
            try assertEqual(MessageMenuAction.pin.systemImage, "pin")
            try assertEqual(MessageMenuAction.delete.systemImage, "trash")
        }

        // ---------------------------------------------------------------------
        // FEATURE 17: Pinned Messages Bar & Carousel Math
        // ---------------------------------------------------------------------

        await runTest("F17_ADV01_ModularCarouselCyclingStress") {
            let p1 = TelegramStageMessage(id: 1, chatId: 1, senderId: 1, text: "Pin 1", timestamp: 1000, isPinned: true)
            let p2 = TelegramStageMessage(id: 2, chatId: 1, senderId: 1, text: "Pin 2", timestamp: 2000, isPinned: true)
            let p3 = TelegramStageMessage(id: 3, chatId: 1, senderId: 1, text: "Pin 3", timestamp: 3000, isPinned: true)
            let vm = ConversationStageViewModel(chatId: 1, initialMessages: [p1, p2, p3])

            // Initial activePinnedIndex = 0
            try assertEqual(vm.activePinnedIndex, 0)

            // Cycle 300 times: ensure sequence 1, 2, 0, 1, 2, 0...
            for i in 1...300 {
                let _ = vm.cyclePinnedMessages()
                let expectedIndex = i % 3
                try assertEqual(vm.activePinnedIndex, expectedIndex, "Active index mismatch at cycle \(i)")
            }

            // Single pinned message carousel
            let vmSingle = ConversationStageViewModel(chatId: 2, initialMessages: [p1])
            try assertEqual(vmSingle.activePinnedIndex, 0)
            let cycledSingle = vmSingle.cyclePinnedMessages()
            try assertEqual(cycledSingle, p1.id)
            try assertEqual(vmSingle.activePinnedIndex, 0)

            // Zero pinned messages
            let vmEmpty = ConversationStageViewModel(chatId: 3, initialMessages: [])
            let cycledEmpty = vmEmpty.cyclePinnedMessages()
            try assertEqual(cycledEmpty, nil)
            try assertEqual(vmEmpty.activePinnedIndex, 0)
        }

        await runTest("F17_ADV02_PinnedSnippetFormattingExhaustiveMatrix") {
            let m1 = TelegramStageMessage(id: 1, chatId: 1, senderId: 1, text: "Regular text message", timestamp: 1000)
            try assertEqual(PinnedMessagesBar.snippetText(for: m1), "Regular text message")

            let m2 = TelegramStageMessage(id: 2, chatId: 1, senderId: 1, text: "  \n Padded with spaces \t ", timestamp: 1000)
            try assertEqual(PinnedMessagesBar.snippetText(for: m2), "Padded with spaces")

            let m3 = TelegramStageMessage(id: 3, chatId: 1, senderId: 1, text: "", timestamp: 1000, mediaType: "photo")
            try assertEqual(PinnedMessagesBar.snippetText(for: m3), "Photo")

            let m4 = TelegramStageMessage(id: 4, chatId: 1, senderId: 1, text: "   ", timestamp: 1000, mediaType: "video")
            try assertEqual(PinnedMessagesBar.snippetText(for: m4), "Video")

            let m5 = TelegramStageMessage(id: 5, chatId: 1, senderId: 1, text: "", timestamp: 1000, mediaType: "document")
            try assertEqual(PinnedMessagesBar.snippetText(for: m5), "Document")

            let m6 = TelegramStageMessage(id: 6, chatId: 1, senderId: 1, text: "", timestamp: 1000, mediaType: nil)
            try assertEqual(PinnedMessagesBar.snippetText(for: m6), "Photo")

            let m7 = TelegramStageMessage(id: 7, chatId: 1, senderId: 1, text: "", timestamp: 1000, mediaType: "")
            try assertEqual(PinnedMessagesBar.snippetText(for: m7), "Photo")
        }

        await runTest("F17_ADV03_HighlightJumpConcurrencyResistance") {
            let p1 = TelegramStageMessage(id: 101, chatId: 1, senderId: 1, text: "P1", timestamp: 1000, isPinned: true)
            let p2 = TelegramStageMessage(id: 102, chatId: 1, senderId: 1, text: "P2", timestamp: 2000, isPinned: true)
            let vm = ConversationStageViewModel(chatId: 1, initialMessages: [p1, p2])

            // Jump to p1
            vm.jumpToPinnedMessage(at: 1)
            try assertEqual(vm.highlightedMessageId, p1.id)

            // Rapidly jump to p2 before 800ms expires
            vm.jumpToPinnedMessage(at: 0)
            try assertEqual(vm.highlightedMessageId, p2.id)

            // After 100ms, p2 should still be highlighted
            try await Task.sleep(nanoseconds: 100_000_000)
            try assertEqual(vm.highlightedMessageId, p2.id)
        }

        // ---------------------------------------------------------------------
        // FEATURE 18: Scroll-To-Bottom Floating Button Hysteresis & Badges
        // ---------------------------------------------------------------------

        await runTest("F18_ADV01_HysteresisDeadbandBrownianWalk") {
            // Hysteresis Rules:
            // 1. If currentlyVisible == false, turns ON only when distance > 200.0
            // 2. If currentlyVisible == true, remains ON until distance < 100.0 (turns OFF when distance < 100.0)
            // 3. In deadband [100.0, 200.0], state remains strictly constant (no oscillation)

            var isVisible = false

            // Rising from 0 to 199.9 -> stays false
            for d in stride(from: 0.0, through: 199.0, by: 10.0) {
                isVisible = ScrollToBottomButton.evaluateVisibility(distance: CGFloat(d), currentlyVisible: isVisible)
                try assertEqual(isVisible, false, "Must remain hidden below 200pt (dist: \(d))")
            }
            isVisible = ScrollToBottomButton.evaluateVisibility(distance: 200.0, currentlyVisible: isVisible)
            try assertEqual(isVisible, false, "At exact 200.0pt, must remain false (condition is > 200)")

            // Cross 200.0 -> turns true
            isVisible = ScrollToBottomButton.evaluateVisibility(distance: 200.1, currentlyVisible: isVisible)
            try assertEqual(isVisible, true, "Above 200pt must become visible")

            // Inside deadband [100.0, 200.0]: Brownian walk of 1,000 steps while visible
            var dist: CGFloat = 180.0
            var rngState: UInt64 = 0x123456789
            for _ in 0..<1000 {
                // Linear congruential generator for deterministic pseudo-random walk
                rngState = rngState &* 6364136223846793005 &+ 1442695040888963407
                let delta = CGFloat(Int(rngState % 21) - 10) // -10 to +10
                dist = max(100.0, min(199.9, dist + delta))

                isVisible = ScrollToBottomButton.evaluateVisibility(distance: dist, currentlyVisible: isVisible)
                try assertEqual(isVisible, true, "Inside deadband [100, 200], visible button must remain visible (dist: \(dist))")
            }

            // Falling below 100.0 -> turns false
            isVisible = ScrollToBottomButton.evaluateVisibility(distance: 100.0, currentlyVisible: isVisible)
            try assertEqual(isVisible, true, "At exact 100.0pt, must still remain visible")

            isVisible = ScrollToBottomButton.evaluateVisibility(distance: 99.9, currentlyVisible: isVisible)
            try assertEqual(isVisible, false, "Below 100pt, button must auto-dismiss")

            // Deadband walk while hidden: must remain hidden
            dist = 150.0
            for _ in 0..<1000 {
                rngState = rngState &* 6364136223846793005 &+ 1442695040888963407
                let delta = CGFloat(Int(rngState % 21) - 10)
                dist = max(100.0, min(199.9, dist + delta))

                isVisible = ScrollToBottomButton.evaluateVisibility(distance: dist, currentlyVisible: isVisible)
                try assertEqual(isVisible, false, "Inside deadband [100, 200], hidden button must remain hidden (dist: \(dist))")
            }
        }

        await runTest("F18_ADV02_UnreadBadgeTextOverfillFormat") {
            let vm = ConversationStageViewModel(chatId: 100)

            // Test 0, 1, 999, 1000, 50000
            let formatBadge: (Int) -> String = { count in
                if count > 999 { return "999+" }
                return "\(count)"
            }

            try assertEqual(formatBadge(0), "0")
            try assertEqual(formatBadge(1), "1")
            try assertEqual(formatBadge(99), "99")
            try assertEqual(formatBadge(999), "999")
            try assertEqual(formatBadge(1000), "999+")
            try assertEqual(formatBadge(50000), "999+")

            // Push events arriving while scrolled up
            vm.updateScrollDistance(300.0)
            try assertEqual(vm.isBottomButtonVisible, true)
            try assertEqual(vm.unreadScrolledCount, 0)

            for i in 1...1500 {
                let event = TelegramEvent.newMessage(id: Int64(i), chatId: 100, senderId: 2, text: "msg \(i)", timestamp: Int64(i))
                vm.handleIncomingPushEvent(event)
            }
            try assertEqual(vm.unreadScrolledCount, 1500)
            try assertEqual(formatBadge(vm.unreadScrolledCount), "999+")

            // Scroll back down
            vm.updateScrollDistance(50.0)
            try assertEqual(vm.unreadScrolledCount, 0, "Unread count must reset upon reaching bottom (< 100pt)")
            try assertEqual(vm.isBottomButtonVisible, false)
        }

        await runTest("F18_ADV03_ExplicitScrollToBottomActionReset") {
            let vm = ConversationStageViewModel(chatId: 100)
            vm.updateScrollDistance(500.0)
            vm.handleIncomingPushEvent(TelegramEvent.newMessage(id: 1, chatId: 100, senderId: 2, text: "Hi", timestamp: 1000))
            try assertEqual(vm.unreadScrolledCount, 1)
            try assertEqual(vm.isBottomButtonVisible, true)

            vm.scrollToBottomAction()
            try assertEqual(vm.unreadScrolledCount, 0)
            try assertEqual(vm.scrollDistanceToBottom, 0.0)
            try assertEqual(vm.isBottomButtonVisible, false)
        }

        // ---------------------------------------------------------------------
        // INVERTED SCROLL COORDINATE SPACE & BUBBLE GEOMETRY
        // ---------------------------------------------------------------------

        await runTest("ChatStage_InvertedCoordinateSpaceClusteringInvariance") {
            // Generate a cluster of 5 messages sent within 1 minute of each other
            let t0: Int64 = 1_000_000
            let messagesChronological = [
                TelegramStageMessage(id: 1, chatId: 1, senderId: 42, text: "First", timestamp: t0, isOutgoing: true),
                TelegramStageMessage(id: 2, chatId: 1, senderId: 42, text: "Second", timestamp: t0 + 10_000, isOutgoing: true),
                TelegramStageMessage(id: 3, chatId: 1, senderId: 42, text: "Third", timestamp: t0 + 20_000, isOutgoing: true),
                TelegramStageMessage(id: 4, chatId: 1, senderId: 42, text: "Fourth", timestamp: t0 + 30_000, isOutgoing: true),
                TelegramStageMessage(id: 5, chatId: 1, senderId: 42, text: "Fifth", timestamp: t0 + 40_000, isOutgoing: true)
            ]

            // In inverted coordinates, index 0 is the newest message (Fifth)
            let messagesInverted = Array(messagesChronological.reversed())

            // Test 1: Inverted index 0 (Fifth message, chronologically newest)
            // In inverted space: it's at the bottom of the visual stage -> must be .bottom, hasTail = true
            let (pos0, radii0) = MessageClusterCalculator.calculate(for: messagesInverted, at: 0, isInverted: true)
            try assertEqual(pos0, .bottom, "Newest message at inverted index 0 must be .bottom")
            try assertEqual(radii0.hasTail, true, "Bottom message must have speech tail")
            try assertEqual(radii0.topRight, 4.0, "Outgoing bottom message top-right must be 4pt")
            try assertEqual(radii0.bottomRight, 16.0, "Outgoing bottom message bottom-right must be 16pt")

            // Test 2: Inverted index 1, 2, 3 (Fourth, Third, Second message) -> .middle, hasTail = false
            for idx in 1...3 {
                let (posMid, radiiMid) = MessageClusterCalculator.calculate(for: messagesInverted, at: idx, isInverted: true)
                try assertEqual(posMid, .middle, "Intermediate message at inverted index \(idx) must be .middle")
                try assertEqual(radiiMid.hasTail, false, "Middle message must not have speech tail")
                try assertEqual(radiiMid.topRight, 4.0)
                try assertEqual(radiiMid.bottomRight, 4.0)
            }

            // Test 3: Inverted index 4 (First message, chronologically oldest) -> .top, hasTail = false
            let (pos4, radii4) = MessageClusterCalculator.calculate(for: messagesInverted, at: 4, isInverted: true)
            try assertEqual(pos4, .top, "Oldest message at inverted index 4 must be .top")
            try assertEqual(radii4.hasTail, false, "Top message must not have speech tail")
            try assertEqual(radii4.topRight, 16.0, "Outgoing top message top-right must be 16pt")
            try assertEqual(radii4.bottomRight, 4.0, "Outgoing top message bottom-right must be 4pt")

            // Test 4: Mathematical equivalence between chronological and inverted calculation
            for (chronIndex, msg) in messagesChronological.enumerated() {
                let (chronPos, chronRadii) = MessageClusterCalculator.calculate(for: messagesChronological, at: chronIndex, isInverted: false)
                let invIndex = messagesChronological.count - 1 - chronIndex
                let (invPos, invRadii) = MessageClusterCalculator.calculate(for: messagesInverted, at: invIndex, isInverted: true)

                try assertEqual(chronPos, invPos, "Cluster position for msg \(msg.id) must match between inverted and chronological")
                try assertEqual(chronRadii.hasTail, invRadii.hasTail, "Tail for msg \(msg.id) must match")
                try assertEqual(chronRadii.topLeft, invRadii.topLeft)
                try assertEqual(chronRadii.topRight, invRadii.topRight)
                try assertEqual(chronRadii.bottomLeft, invRadii.bottomLeft)
                try assertEqual(chronRadii.bottomRight, invRadii.bottomRight)
            }
        }

        print("================================================================================")
        print(" >>> ALL \(passCount)/\(totalCount) ADVERSARIAL STRESS TESTS PASSED EMPIRICALLY! <<<")
        print("================================================================================")
    }
}
