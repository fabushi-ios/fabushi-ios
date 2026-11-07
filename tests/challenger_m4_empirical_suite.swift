//
//  challenger_m4_empirical_suite.swift
//  FabushiTests
//
//  Empirical Adversarial Challenge Suite for Milestone 4 (Features 20 - 26).
//  Adversarial stress-testing of gesture physics, bit-packing math,
//  dynamic height clamping, dismissal thresholds, and concurrency safety.
//

import CoreGraphics
import Foundation
import SwiftUI
#if canImport(UIKit)
import UIKit
#endif
#if canImport(Darwin)
import Darwin
#endif

@main
struct ChallengerM4EmpiricalSuite {

    @MainActor private static var passedTests = 0
    @MainActor private static var totalTests = 0

    @MainActor
    private static func runAdversarialTest(_ name: String, execute: () async throws -> Void) async {
        totalTests += 1
        print("[\(totalTests)] STRESS CHALLENGE: \(name)...")
        do {
            try await execute()
            passedTests += 1
            print("  ==> [PASS] \(name)")
        } catch {
            print("  ==> [FAIL] \(name): \(error)")
            exit(1)
        }
    }

    private static func require(_ condition: Bool, _ reason: String = "", file: StaticString = #file, line: UInt = #line) throws {
        if !condition {
            throw ChallengeError("ASSERTION FAILURE at \(file):\(line): \(reason)")
        }
    }

    private static func requireEqual<T: Equatable>(_ a: T, _ b: T, _ reason: String = "", file: StaticString = #file, line: UInt = #line) throws {
        if a != b {
            throw ChallengeError("EQUALS FAILURE at \(file):\(line): expected \(b), got \(a). \(reason)")
        }
    }

    private static func requireClose(_ a: CGFloat, _ b: CGFloat, tol: CGFloat = 0.0001, _ reason: String = "", file: StaticString = #file, line: UInt = #line) throws {
        if abs(a - b) > tol {
            throw ChallengeError("ACCURACY FAILURE at \(file):\(line): \(a) vs \(b) diff: \(abs(a - b)) > \(tol). \(reason)")
        }
    }

    struct ChallengeError: Error, CustomStringConvertible {
        let description: String
        init(_ description: String) { self.description = description }
    }

    @MainActor
    static func main() async {
        print("================================================================================")
        print(" EMPIRICAL ADVERSARIAL CHALLENGER SUITE: Milestone 4 Fullscreen & Gestures")
        print(" Strict Stress-Testing of Boundary Conditions, Mathematical Bounds, Concurrency")
        print("================================================================================")

        // =====================================================================
        // CHALLENGE 1: Voice Recording Gesture Mechanics & Edge Thresholds
        // =====================================================================
        await runAdversarialTest("VoiceRecord_BoundaryThresholdSubPointPrecision") {
            let handler = VoiceRecordGestureHandler()
            handler.onLongPressTriggered()
            try require(handler.isHolding, "Must be holding")

            // Horizontal Slide-to-Cancel Boundary: -99.999pt must NOT cancel
            handler.onDragChanged(translation: CGSize(width: -99.999, height: 0))
            try require(handler.isHolding, "-99.999pt must not cancel")
            try requireEqual(handler.currentOffset.width, -99.999)

            // Exact -100.0pt MUST cancel
            handler.onDragChanged(translation: CGSize(width: -100.0, height: 0))
            try requireEqual(handler.state, .cancelled, "-100.0pt must trigger cancel state")

            // Vertical Slide-to-Lock Boundary: -79.999pt must NOT lock
            let handler2 = VoiceRecordGestureHandler()
            handler2.onLongPressTriggered()
            handler2.onDragChanged(translation: CGSize(width: 0, height: -79.999))
            try require(handler2.isHolding, "-79.999pt must not lock")

            // Exact -80.0pt MUST lock
            handler2.onDragChanged(translation: CGSize(width: 0, height: -80.0))
            try require(handler2.isLocked, "-80.0pt must trigger lock state")
        }

        await runAdversarialTest("VoiceRecord_SimultaneousCancelAndLockPrecedence") {
            // If user drags diagonally left and up: width <= -100 AND height <= -80
            let handler = VoiceRecordGestureHandler()
            handler.onLongPressTriggered()
            handler.onDragChanged(translation: CGSize(width: -120.0, height: -120.0))
            // Cancel check is evaluated first, aborting recording
            try requireEqual(handler.state, .cancelled, "Cancel must take precedence over lock during diagonal drag")
        }

        await runAdversarialTest("VoiceRecord_SubSecondDurationThresholds") {
            // Discard below 500ms: 499ms must discard and trigger hint
            let handlerShort = VoiceRecordGestureHandler()
            handlerShort.onLongPressTriggered()
            var sentShort = false
            handlerShort.onTouchEnded(duration: 0.499) { shouldSend in
                sentShort = shouldSend
            }
            try require(!sentShort, "499ms must not send")
            try require(handlerShort.showTapHint, "499ms must trigger showTapHint")

            // Exact 500ms must send
            let handlerExact = VoiceRecordGestureHandler()
            handlerExact.onLongPressTriggered()
            var sentExact = false
            handlerExact.onTouchEnded(duration: 0.500) { shouldSend in
                sentExact = shouldSend
            }
            try require(sentExact, "500ms must send successfully")
            try require(!handlerExact.showTapHint, "Valid duration must not trigger tap hint")
        }

        await runAdversarialTest("VoiceRecord_WaveformRollingBufferFIFOCap100") {
            let handler = VoiceRecordGestureHandler()
            // Feed 1,000 samples
            for i in 0..<1000 {
                handler.appendLiveSample(Double(i))
            }
            try requireEqual(handler.liveSamples.count, 100, "Buffer size must strictly equal 100")
            // The newest sample appended was 999.0, clamped to 1.0
            try requireEqual(handler.liveSamples.last!, 1.0)
            // Clamp negative and >1.0
            handler.appendLiveSample(-5.0)
            try requireEqual(handler.liveSamples.last!, 0.0)
            handler.appendLiveSample(99.0)
            try requireEqual(handler.liveSamples.last!, 1.0)
        }

        await runAdversarialTest("VoiceRecord_ResetPurgesStateAndTimer") {
            let handler = VoiceRecordGestureHandler()
            handler.onLongPressTriggered()
            handler.appendLiveSample(0.5)
            handler.onDragChanged(translation: CGSize(width: -40, height: -20))
            handler.reset()

            try requireEqual(handler.state, .idle)
            try requireEqual(handler.currentOffset, .zero)
            try require(handler.liveSamples.isEmpty)
            try require(handler.pressTimerTask == nil)
        }

        // =====================================================================
        // CHALLENGE 2: Fullscreen Media Viewer Swipe-Down Dismissal Physics
        // =====================================================================
        await runAdversarialTest("MediaViewer_SwipeDownExactMathematicalFormulas") {
            let items = [TelegramMediaItem(id: "media_test_1", messageId: 101, chatId: 202)]
            let vm = MediaViewerViewModel(items: items)

            // Test 1: Upward drag (negative y): progress must clamp to 0.0, scale 1.0, opacity 1.0
            vm.dismissOffsetY = -100.0
            try requireClose(vm.dismissProgress, 0.0, tol: 0.0001, "Negative y must yield 0 progress")
            try requireClose(vm.dismissScale, 1.0, tol: 0.0001, "Negative y must maintain 1.0 scale")
            try requireClose(vm.dismissOpacity, 1.0, tol: 0.0001, "Negative y must maintain 1.0 opacity")

            // Test 2: Quarter drag: y = 60pt (60 / 240 = 0.25)
            // scale = 1.0 - 0.3 * 0.25 = 0.925
            // opacity = 1.0 - 0.25 = 0.75
            vm.dismissOffsetY = 60.0
            try requireClose(vm.dismissProgress, 0.25, tol: 0.0001)
            try requireClose(vm.dismissScale, 0.925, tol: 0.0001)
            try requireClose(vm.dismissOpacity, 0.75, tol: 0.0001)

            // Test 3: Half drag: y = 120pt (120 / 240 = 0.50)
            // scale = 1.0 - 0.3 * 0.50 = 0.85
            // opacity = 1.0 - 0.50 = 0.50
            vm.dismissOffsetY = 120.0
            try requireClose(vm.dismissProgress, 0.50, tol: 0.0001)
            try requireClose(vm.dismissScale, 0.85, tol: 0.0001)
            try requireClose(vm.dismissOpacity, 0.50, tol: 0.0001)

            // Test 4: Full drag: y = 240pt (240 / 240 = 1.00)
            // scale = 1.0 - 0.3 * 1.00 = 0.70
            // opacity = 1.0 - 1.00 = 0.00
            vm.dismissOffsetY = 240.0
            try requireClose(vm.dismissProgress, 1.00, tol: 0.0001)
            try requireClose(vm.dismissScale, 0.70, tol: 0.0001)
            try requireClose(vm.dismissOpacity, 0.00, tol: 0.0001)

            // Test 5: Over-drag: y = 600pt (must clamp progress to 1.0, scale to 0.70, opacity to 0.0)
            vm.dismissOffsetY = 600.0
            try requireClose(vm.dismissProgress, 1.00, tol: 0.0001)
            try requireClose(vm.dismissScale, 0.70, tol: 0.0001)
            try requireClose(vm.dismissOpacity, 0.00, tol: 0.0001)
        }

        await runAdversarialTest("MediaViewer_DismissalThresholdBoundaryConditions") {
            let items = [TelegramMediaItem(id: "media_test_2", messageId: 102, chatId: 202)]
            let vm = MediaViewerViewModel(items: items)

            // Boundary 1: y = 159.999pt and vy = 799.999pt/s -> Must NOT dismiss (snapback)
            var dismissed = false
            vm.handleDragEnded(translation: CGSize(width: 0, height: 159.999), velocityY: 799.999) {
                dismissed = true
            }
            try require(!dismissed, "159.999pt & 799.999pt/s must snapback, not dismiss")

            // Boundary 2: Exact distance threshold y = 160.000pt with 0 velocity -> MUST dismiss
            dismissed = false
            vm.handleDragEnded(translation: CGSize(width: 0, height: 160.000), velocityY: 0.0) {
                dismissed = true
            }
            try require(dismissed, "160.0pt distance must trigger dismiss")

            // Boundary 3: Exact velocity threshold vy = 800.000pt/s with 0 distance -> MUST dismiss
            dismissed = false
            vm.handleDragEnded(translation: CGSize(width: 0, height: 0.0), velocityY: 800.000) {
                dismissed = true
            }
            try require(dismissed, "800.0pt/s velocity must trigger dismiss")
        }

        await runAdversarialTest("MediaViewer_PinchZoomAndDoubleTapBounds") {
            let items = [TelegramMediaItem(id: "media_test_3", messageId: 103, chatId: 202)]
            let vm = MediaViewerViewModel(items: items)

            // Double tap toggling: 1.0 -> 2.5 -> 1.0
            try requireEqual(vm.currentScale, 1.0)
            vm.handleDoubleTap()
            try requireEqual(vm.currentScale, 2.5)
            vm.handleDoubleTap()
            try requireEqual(vm.currentScale, 1.0)

            // Pinch zoom clamping
            vm.handlePinch(scale: 0.01) // Extreme down-scale
            try requireEqual(vm.currentScale, 1.0, "Scale must clamp to minimum 1.0x")

            vm.handlePinch(scale: 100.0) // Extreme up-scale
            try requireEqual(vm.currentScale, 5.0, "Scale must clamp to maximum 5.0x")
        }

        await runAdversarialTest("MediaViewer_ZoomPanRoutingPreventsAccidentalDismiss") {
            let items = [TelegramMediaItem(id: "media_test_4", messageId: 104, chatId: 202)]
            let vm = MediaViewerViewModel(items: items)
            vm.currentScale = 3.0 // In zoomed state

            // When zoomed, downward drag must be interpreted as pan, NOT dismiss
            vm.handleDragChanged(translation: CGSize(width: 30, height: 200))
            try requireEqual(vm.dismissOffsetY, 0.0, "Dismiss offset must remain 0 while zoomed")
            try requireEqual(vm.panOffset, CGSize(width: 30, height: 200), "Pan offset must accumulate translation")

            var dismissed = false
            vm.handleDragEnded(translation: CGSize(width: 30, height: 200), velocityY: 1500.0) {
                dismissed = true
            }
            try require(!dismissed, "Zoomed drag release must NEVER trigger dismissal")
        }

        // =====================================================================
        // CHALLENGE 3: 5-Bit Waveform Audio Bit-Packing Codec
        // =====================================================================
        await runAdversarialTest("WaveformCodec_BitExactPacking100SamplesTo63Bytes") {
            // Encode all zeros
            let zeros = [Double](repeating: 0.0, count: 100)
            let packedZeros = WaveformCodec5Bit.encode(samples: zeros)
            try requireEqual(packedZeros.count, 63, "Must be exactly 63 bytes")
            try require(packedZeros.allSatisfy { $0 == 0 }, "All-zero samples must produce all 0 bytes")
            let decodedZeros = try WaveformCodec5Bit.decode(packed: packedZeros)
            try requireEqual(decodedZeros.count, 100)
            try require(decodedZeros.allSatisfy { $0 == 0 })

            // Encode all maximums (1.0)
            let maxSamples = [Double](repeating: 1.0, count: 100)
            let packedMax = WaveformCodec5Bit.encode(samples: maxSamples)
            try requireEqual(packedMax.count, 63)
            let decodedMax = try WaveformCodec5Bit.decode(packed: packedMax)
            try require(decodedMax.allSatisfy { $0 == 31 }, "All-max samples must decode to 31")

            // Alternating levels 0 and 31
            let altSamples = (0..<100).map { $0 % 2 == 0 ? 0.0 : 1.0 }
            let packedAlt = WaveformCodec5Bit.encode(samples: altSamples)
            let decodedAlt = try WaveformCodec5Bit.decode(packed: packedAlt)
            for i in 0..<100 {
                let expected: UInt8 = (i % 2 == 0) ? 0 : 31
                try requireEqual(decodedAlt[i], expected, "Alternating sample \(i) mismatch")
            }
        }

        await runAdversarialTest("WaveformCodec_CorruptedBufferSizeValidation") {
            // Test 0 bytes, 62 bytes, 64 bytes
            for invalidSize in [0, 1, 62, 64, 100] {
                let buf = [UInt8](repeating: 0x55, count: invalidSize)
                var caught = false
                do {
                    _ = try WaveformCodec5Bit.decode(packed: buf)
                } catch WaveformCodecError.corruptedWaveformBufferSize(let exp, let act) {
                    try requireEqual(exp, 63)
                    try requireEqual(act, invalidSize)
                    caught = true
                }
                try require(caught, "Size \(invalidSize) must throw corruptedWaveformBufferSize")
            }
        }

        await runAdversarialTest("WaveformCodec_ArbitraryDurationResamplingSafety") {
            // 0 samples
            let res0 = WaveformCodec5Bit.resample(samples: [], targetCount: 100)
            try requireEqual(res0.count, 100)
            try require(res0.allSatisfy { $0 == 0.0 })

            // 1 sample
            let res1 = WaveformCodec5Bit.resample(samples: [0.85], targetCount: 100)
            try requireEqual(res1.count, 100)
            try requireClose(CGFloat(res1[0]), 0.85, tol: 0.001)

            // 50,000 samples (stress downsampling)
            let hugeSamples = [Double](repeating: 0.42, count: 50_000)
            let resHuge = WaveformCodec5Bit.resample(samples: hugeSamples, targetCount: 100)
            try requireEqual(resHuge.count, 100)
            try requireClose(CGFloat(resHuge[50]), 0.42, tol: 0.001)
        }

        // =====================================================================
        // CHALLENGE 4: Telegram Composer Bar Dynamic Height Clamping
        // =====================================================================
        await runAdversarialTest("Composer_DynamicHeightClampingBoundaryMatrix") {
            // Formula: H = min(120, max(36, lineCount * 20 + 16))
            // 0 or negative lineCount -> max(1, count) = 1 -> 36pt
            try requireEqual(TelegramComposerLayoutCalculator.clampedHeight(for: 0), 36.0)
            try requireEqual(TelegramComposerLayoutCalculator.clampedHeight(for: -5), 36.0)

            // 1 line: 1 * 20 + 16 = 36pt
            try requireEqual(TelegramComposerLayoutCalculator.clampedHeight(for: 1), 36.0)

            // 2 lines: 2 * 20 + 16 = 56pt
            try requireEqual(TelegramComposerLayoutCalculator.clampedHeight(for: 2), 56.0)

            // 3 lines: 3 * 20 + 16 = 76pt
            try requireEqual(TelegramComposerLayoutCalculator.clampedHeight(for: 3), 76.0)

            // 4 lines: 4 * 20 + 16 = 96pt
            try requireEqual(TelegramComposerLayoutCalculator.clampedHeight(for: 4), 96.0)

            // 5 lines: 5 * 20 + 16 = 116pt
            try requireEqual(TelegramComposerLayoutCalculator.clampedHeight(for: 5), 116.0)

            // 6 lines: 6 * 20 + 16 = 136 -> clamped to 120.0pt
            try requireEqual(TelegramComposerLayoutCalculator.clampedHeight(for: 6), 120.0)

            // 100 lines -> clamped to 120.0pt
            try requireEqual(TelegramComposerLayoutCalculator.clampedHeight(for: 100), 120.0)

            // Direct content height clamping
            try requireEqual(TelegramComposerLayoutCalculator.clampedHeight(contentHeight: -20.0), 36.0)
            try requireEqual(TelegramComposerLayoutCalculator.clampedHeight(contentHeight: 0.0), 36.0)
            try requireEqual(TelegramComposerLayoutCalculator.clampedHeight(contentHeight: 88.5), 88.5)
            try requireEqual(TelegramComposerLayoutCalculator.clampedHeight(contentHeight: 120.0), 120.0)
            try requireEqual(TelegramComposerLayoutCalculator.clampedHeight(contentHeight: 9999.0), 120.0)
        }

        await runAdversarialTest("Composer_SendVsMicMorphingAndCharacterLimiting") {
            // Whitespace variations must show Mic button
            try require(!TelegramComposerBar.shouldShowSendButton(text: ""))
            try require(!TelegramComposerBar.shouldShowSendButton(text: "   "))
            try require(!TelegramComposerBar.shouldShowSendButton(text: "\n\t  \r\n"))

            // Non-empty text must show Send button
            try require(TelegramComposerBar.shouldShowSendButton(text: "."))
            try require(TelegramComposerBar.shouldShowSendButton(text: "  Hello  "))

            // Character limiting: 4,096 chars
            let text5000 = String(repeating: "A", count: 5000)
            let sanitized = TelegramComposerTextLimiter.sanitize(text5000, limit: 4096)
            try requireEqual(sanitized.count, 4096)
            try require(TelegramComposerTextLimiter.isExceedingLimit(text5000))
            try require(!TelegramComposerTextLimiter.isExceedingLimit(sanitized))
        }

        // =====================================================================
        // CHALLENGE 5: Attachment Drawer Badge Re-indexing & Document Limits
        // =====================================================================
        await runAdversarialTest("Attachment_SelectionOrderReindexingAndCaps") {
            let vm = AttachmentDrawerViewModel()
            let itemA = MediaPreviewItem(id: "itemA")
            let itemB = MediaPreviewItem(id: "itemB")
            let itemC = MediaPreviewItem(id: "itemC")

            vm.toggleSelection(itemA)
            vm.toggleSelection(itemB)
            vm.toggleSelection(itemC)

            try requireEqual(vm.badgeIndex(for: itemA), 1)
            try requireEqual(vm.badgeIndex(for: itemB), 2)
            try requireEqual(vm.badgeIndex(for: itemC), 3)

            // Remove itemA (the first one) -> itemB must re-index to 1, itemC must re-index to 2
            vm.toggleSelection(itemA)
            try requireEqual(vm.badgeIndex(for: itemB), 1, "Item B must re-index to 1")
            try requireEqual(vm.badgeIndex(for: itemC), 2, "Item C must re-index to 2")
            try requireEqual(vm.badgeIndex(for: itemA), nil, "Deselected item has no badge")
        }

        await runAdversarialTest("Attachment_DocumentSizeValidation0ByteAnd2GB") {
            // 0 byte rejected
            var caught0 = false
            do {
                try DocumentAttachment.validate(fileSizeBytes: 0)
            } catch DocumentError.emptyFile {
                caught0 = true
            }
            try require(caught0, "0-byte file must throw .emptyFile")

            // Exactly 2GB allowed
            let twoGB: Int64 = 2 * 1024 * 1024 * 1024
            try DocumentAttachment.validate(fileSizeBytes: twoGB)

            // 2GB + 1 byte rejected
            var caughtOversized = false
            do {
                try DocumentAttachment.validate(fileSizeBytes: twoGB + 1)
            } catch DocumentError.fileExceeds2GBLimit {
                caughtOversized = true
            }
            try require(caughtOversized, "2GB+1 must throw .fileExceeds2GBLimit")
        }

        // =====================================================================
        // CHALLENGE 6: Chunked Media Streaming & CAS SHA-256
        // =====================================================================
        await runAdversarialTest("MediaTransfer_512KBExactPartitioning") {
            // Exact 1MB -> exactly 2 chunks of 512KB
            let oneMB = Data(repeating: 0xAA, count: 1024 * 1024)
            let chunks = try MediaTransferManager.partition(data: oneMB, chunkSize: 512 * 1024)
            try requireEqual(chunks.count, 2)
            try requireEqual(chunks[0].count, 512 * 1024)
            try requireEqual(chunks[1].count, 512 * 1024)

            // 512KB + 1 byte -> exactly 2 chunks
            let partialData = Data(repeating: 0xBB, count: 512 * 1024 + 1)
            let partialChunks = try MediaTransferManager.partition(data: partialData, chunkSize: 512 * 1024)
            try requireEqual(partialChunks.count, 2)
            try requireEqual(partialChunks[0].count, 512 * 1024)
            try requireEqual(partialChunks[1].count, 1)
        }

        // =====================================================================
        // Summary
        // =====================================================================
        print("================================================================================")
        print(" >>> ALL \(totalTests)/\(totalTests) EMPIRICAL ADVERSARIAL CHALLENGES PASSED! <<<")
        print("================================================================================")
    }
}
