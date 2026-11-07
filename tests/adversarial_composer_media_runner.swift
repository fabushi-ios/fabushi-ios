//
//  adversarial_composer_media_runner.swift
//  FabushiTests
//
//  Empirical Adversarial Stress Runner for Milestone 4:
//  Features 20 to 26:
//  - Feature 20: Auto-Expanding Input Bar & Height Clamping (36pt - 120pt)
//  - Feature 21: Gesture-Driven Voice Recording (Slide-to-Cancel <= -100pt, Slide-to-Lock <= -80pt)
//  - Feature 22: 5-Bit Waveform Audio Bit-Packing Codec (100 samples -> 63 bytes)
//  - Feature 23: Attachment Menu Bottom Drawer & Document Validation (0-byte, 2GB)
//  - Feature 24: Tabbed Sticker/Emoji/GIF Picker, Long-Press Preview & Offline Fallback
//  - Feature 25: Fullscreen Interactive Media Gallery Viewer (Dismissal Physics, Zoom 1.0x-5.0x)
//  - Feature 26: Chunked Media Streaming Bridge (512KB Chunks & CAS SHA-256)
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
struct AdversarialComposerMediaRunner {

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

    private static func assertFalse(_ condition: @autoclosure () -> Bool, _ message: String = "", file: StaticString = #file, line: UInt = #line) throws {
        if condition() {
            throw TestFailure("Assertion failed at \(file):\(line): expected false, got true. \(message)")
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

    static func getRssMB() -> Double {
        #if canImport(Darwin)
        var info = mach_task_basic_info()
        var count = mach_msg_type_number_t(MemoryLayout<mach_task_basic_info>.size) / 4
        let kerr: kern_return_t = withUnsafeMutablePointer(to: &info) {
            $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
                task_info(mach_task_self_, task_flavor_t(MACH_TASK_BASIC_INFO), $0, &count)
            }
        }
        return kerr == KERN_SUCCESS ? Double(info.resident_size) / (1024 * 1024) : 0.0
        #else
        return 0.0
        #endif
    }

    @MainActor
    static func main() async {
        print("================================================================================")
        print(" ADVERSARIAL EMPIRICAL STRESS RUNNER: Milestone 4 Fullscreen Media & Composer Core")
        print(" Features 20 to 26: Composer, Gestures, 5-Bit Codec, Attachments, Media Viewer, Transfer")
        print(" Environment: Swift 6 Strict Concurrency (@MainActor, actor, Sendable)")
        print("================================================================================")
        let initialRss = getRssMB()
        print("Initial Process RSS: \(String(format: "%.2f", initialRss)) MB\n")

        // =====================================================================
        // FEATURE 20: Auto-Expanding Rich Input Bar
        // =====================================================================
        await runTest("F20_ADV01_AutoExpandingHeightClamping36to120") {
            let h1 = TelegramComposerLayoutCalculator.clampedHeight(for: 1)
            try assertEqual(h1, 36.0, "1 line must clamp to min 36pt")

            let h3 = TelegramComposerLayoutCalculator.clampedHeight(for: 3)
            try assertTrue(h3 > 36.0 && h3 <= 120.0, "3 lines must expand fluidly")

            let h10 = TelegramComposerLayoutCalculator.clampedHeight(for: 10)
            try assertEqual(h10, 120.0, "10 lines must clamp strictly to max 120pt")

            let hOversized = TelegramComposerLayoutCalculator.clampedHeight(contentHeight: 999.0)
            try assertEqual(hOversized, 120.0, "999pt content must clamp to 120pt")
        }

        await runTest("F20_ADV02_SendVsMicMorphingAndSanitization") {
            try assertFalse(TelegramComposerBar.shouldShowSendButton(text: ""))
            try assertFalse(TelegramComposerBar.shouldShowSendButton(text: "   \n\t  "))
            try assertTrue(TelegramComposerBar.shouldShowSendButton(text: "Hello"))

            let huge = String(repeating: "Z", count: 10_000)
            let sanitized = TelegramComposerTextLimiter.sanitize(huge, limit: 4096)
            try assertEqual(sanitized.count, 4096, "Limiter must clamp input to 4,096 chars")
        }

        await runTest("F20_ADV03_ReplyBannerHeight44pt") {
            try assertEqual(TelegramReplyBannerView.defaultHeight, 44.0, "Reply banner must be 44pt")
        }

        // =====================================================================
        // FEATURE 21: Gesture-Driven Voice Recording
        // =====================================================================
        await runTest("F21_ADV01_SlideToCancelAndSlideToLockThresholds") {
            let handler = VoiceRecordGestureHandler()
            handler.onLongPressTriggered()
            try assertTrue(handler.isHolding)

            // Horizontal drag: -99pt (not yet cancelled)
            handler.onDragChanged(translation: CGSize(width: -99, height: 0))
            try assertTrue(handler.isHolding)

            // Horizontal drag: -100pt (cancels!)
            handler.onDragChanged(translation: CGSize(width: -100, height: 0))
            try assertEqual(handler.state, .cancelled)

            // Reset and test lock threshold (-80pt)
            let handlerLock = VoiceRecordGestureHandler()
            handlerLock.onLongPressTriggered()
            handlerLock.onDragChanged(translation: CGSize(width: 0, height: -79))
            try assertTrue(handlerLock.isHolding)
            handlerLock.onDragChanged(translation: CGSize(width: 0, height: -80))
            try assertTrue(handlerLock.isLocked)
        }

        await runTest("F21_ADV02_MicroTapUnder150msDiscardAndDurationClamp") {
            let handler = VoiceRecordGestureHandler()
            var sent = false
            handler.onTouchEnded(duration: 0.12) { shouldSend in
                sent = shouldSend
            }
            try assertFalse(sent, "Micro-tap must be discarded")
            try assertTrue(handler.showTapHint, "Micro-tap must activate hint")

            // Over 500ms should send
            var sentValid = false
            let handler2 = VoiceRecordGestureHandler()
            handler2.onLongPressTriggered()
            handler2.onTouchEnded(duration: 0.65) { shouldSend in
                sentValid = shouldSend
            }
            try assertTrue(sentValid, "Recording >= 0.5s must send")
        }

        await runTest("F21_ADV03_WaveformRollingBufferCap100") {
            let handler = VoiceRecordGestureHandler()
            for i in 0..<300 {
                handler.appendLiveSample(Double(i) / 300.0)
            }
            try assertEqual(handler.liveSamples.count, 100, "Live waveform buffer must cap at 100 samples")
        }

        // =====================================================================
        // FEATURE 22: 5-Bit Waveform Codec
        // =====================================================================
        await runTest("F22_ADV01_WaveformBitPacking100SamplesTo63Bytes") {
            let testSamples = (0..<100).map { Double($0) / 99.0 }
            let packed = WaveformCodec5Bit.encode(samples: testSamples)
            try assertEqual(packed.count, 63, "Waveform packed byte count must be exactly 63 bytes")

            let decoded = try WaveformCodec5Bit.decode(packed: packed)
            try assertEqual(decoded.count, 100, "Decoded sample count must be 100")
            try assertEqual(decoded[0], 0, "First sample must be 0")
            try assertEqual(decoded[99], 31, "Peak sample must be 31")
        }

        await runTest("F22_ADV02_DynamicCeilingCompressionAndSilence") {
            // Silence
            let silence = [Double](repeating: 0.0, count: 100)
            let packedSilence = WaveformCodec5Bit.encode(samples: silence)
            let decodedSilence = try WaveformCodec5Bit.decode(packed: packedSilence)
            try assertTrue(decodedSilence.allSatisfy { $0 == 0 }, "Silence must decode to 0")

            // Low peak 0.2 must scale to 31
            var lowPeak = [Double](repeating: 0.01, count: 100)
            lowPeak[42] = 0.2
            let packedLow = WaveformCodec5Bit.encode(samples: lowPeak)
            let decodedLow = try WaveformCodec5Bit.decode(packed: packedLow)
            try assertEqual(decodedLow[42], 31, "Low peak 0.2 must be normalized to level 31")
        }

        await runTest("F22_ADV03_CorruptedWaveformBufferSizeValidation") {
            let invalidBuffer = [UInt8](repeating: 0, count: 50)
            var caughtError = false
            do {
                _ = try WaveformCodec5Bit.decode(packed: invalidBuffer)
            } catch let err as WaveformCodecError {
                if case .corruptedWaveformBufferSize(let exp, let act) = err {
                    try assertEqual(exp, 63)
                    try assertEqual(act, 50)
                    caughtError = true
                }
            }
            try assertTrue(caughtError, "Decoder must reject 50-byte buffer")
        }

        await runTest("F22_ADV04_ResamplingArbitraryDurations") {
            // Partial 35 samples
            let partial = [Double](repeating: 0.7, count: 35)
            let resampledPartial = WaveformCodec5Bit.resample(samples: partial, targetCount: 100)
            try assertEqual(resampledPartial.count, 100)

            // Massive 20,000 samples
            let massive = [Double](repeating: 0.3, count: 20_000)
            let resampledMassive = WaveformCodec5Bit.resample(samples: massive, targetCount: 100)
            try assertEqual(resampledMassive.count, 100)
        }

        // =====================================================================
        // FEATURE 23: Attachment Menu Bottom Drawer
        // =====================================================================
        await runTest("F23_ADV01_AttachmentDrawerBadgesReindexingAndClamp") {
            let vm = AttachmentDrawerViewModel()
            let item1 = MediaPreviewItem(id: "p1")
            let item2 = MediaPreviewItem(id: "p2")
            let item3 = MediaPreviewItem(id: "p3")

            vm.toggleSelection(item1)
            vm.toggleSelection(item2)
            vm.toggleSelection(item3)

            try assertEqual(vm.badgeIndex(for: item1), 1)
            try assertEqual(vm.badgeIndex(for: item2), 2)
            try assertEqual(vm.badgeIndex(for: item3), 3)

            // Remove item2: item3 re-indexes to 2
            vm.toggleSelection(item2)
            try assertEqual(vm.badgeIndex(for: item3), 2, "Removal must re-index subsequent badges")

            // Clamp at 100 items
            for i in 10...120 {
                vm.toggleSelection(MediaPreviewItem(id: "extra_\(i)"))
            }
            try assertEqual(vm.selectedItems.count, 100, "Must clamp to 100 selected items")
        }

        await runTest("F23_ADV02_DocumentSizeValidation0ByteAnd2GB") {
            // 0-byte
            var caught0Byte = false
            do {
                try DocumentAttachment.validate(fileSizeBytes: 0)
            } catch let err as DocumentError {
                try assertEqual(err, .emptyFile)
                caught0Byte = true
            }
            try assertTrue(caught0Byte, "0-byte file must be rejected")

            // 2GB + 1 byte
            var caughtOversized = false
            do {
                try DocumentAttachment.validate(fileSizeBytes: 2 * 1024 * 1024 * 1024 + 1)
            } catch let err as DocumentError {
                try assertEqual(err, .fileExceeds2GBLimit)
                caughtOversized = true
            }
            try assertTrue(caughtOversized, ">2GB file must be rejected")
        }

        // =====================================================================
        // FEATURE 24: Tabbed Sticker / Emoji / GIF Picker
        // =====================================================================
        await runTest("F24_ADV01_TabCyclingAndSearchFiltering") {
            let vm = StickerEmojiPickerViewModel()
            try assertEqual(vm.activeTab, .stickers)
            vm.selectTab(.emoji)
            try assertEqual(vm.activeTab, .emoji)

            vm.searchQuery = "laugh"
            let filtered = vm.filteredEmojis()
            try assertTrue(filtered.contains { $0.glyph == "😂" })

            // Offline GIF fallback
            vm.isOffline = true
            try assertFalse(vm.filteredGifs().isEmpty, "Offline state must retain cached GIFs")
        }

        await runTest("F24_ADV02_LongPressPreviewStateLifecycle") {
            let vm = StickerEmojiPickerViewModel()
            let sticker = TelegramSticker(id: "stk1", packId: "pk", emojiHint: "🚀", imageUri: "uri")
            vm.triggerLongPressPreview(sticker)
            try assertEqual(vm.previewSticker?.id, "stk1")
            vm.dismissPreview()
            try assertTrue(vm.previewSticker == nil)
        }

        // =====================================================================
        // FEATURE 25: Fullscreen Interactive Media Gallery Viewer
        // =====================================================================
        await runTest("F25_ADV01_SwipeDownDismissPhysicsScaleAndOpacity") {
            let items = [TelegramMediaItem(id: "photo1", messageId: 1, chatId: 1)]
            let vm = MediaViewerViewModel(items: items)

            // Initial state: y = 0 -> progress = 0.0, scale = 1.0, opacity = 1.0
            try assertAlmostEqual(vm.dismissProgress, 0.0)
            try assertAlmostEqual(vm.dismissScale, 1.0)
            try assertAlmostEqual(vm.dismissOpacity, 1.0)

            // Drag y = 120pt -> progress = 120 / 240 = 0.5
            // scale = 1.0 - 0.3 * 0.5 = 0.85
            // opacity = 1.0 - 0.5 = 0.5
            vm.handleDragChanged(translation: CGSize(width: 0, height: 120))
            try assertAlmostEqual(vm.dismissProgress, 0.5)
            try assertAlmostEqual(vm.dismissScale, 0.85)
            try assertAlmostEqual(vm.dismissOpacity, 0.5)

            // Drag y = 240pt -> progress = 1.0
            // scale = 1.0 - 0.3 * 1.0 = 0.70
            // opacity = 0.0
            vm.handleDragChanged(translation: CGSize(width: 0, height: 240))
            try assertAlmostEqual(vm.dismissProgress, 1.0)
            try assertAlmostEqual(vm.dismissScale, 0.70)
            try assertAlmostEqual(vm.dismissOpacity, 0.0)
        }

        await runTest("F25_ADV02_DismissalTrigger160ptOr800ptPerSec") {
            let items = [TelegramMediaItem(id: "photo1", messageId: 1, chatId: 1)]
            let vm = MediaViewerViewModel(items: items)

            // 159pt with low velocity snaps back
            var dismissed = false
            vm.handleDragEnded(translation: CGSize(width: 0, height: 159), velocityY: 400) {
                dismissed = true
            }
            try assertFalse(dismissed, "159pt < 160pt must snap back")

            // 160pt triggers dismissal
            vm.handleDragEnded(translation: CGSize(width: 0, height: 160), velocityY: 100) {
                dismissed = true
            }
            try assertTrue(dismissed, "160pt must trigger dismissal")

            // 30pt with 801pt/s velocity triggers dismissal
            dismissed = false
            vm.handleDragEnded(translation: CGSize(width: 0, height: 30), velocityY: 801) {
                dismissed = true
            }
            try assertTrue(dismissed, "801pt/s must trigger dismissal")
        }

        await runTest("F25_ADV03_PinchZoomAndDoubleTapToggle") {
            let items = [TelegramMediaItem(id: "photo1", messageId: 1, chatId: 1)]
            let vm = MediaViewerViewModel(items: items)

            // Double-tap: 1.0x -> 2.5x
            vm.handleDoubleTap()
            try assertEqual(vm.currentScale, 2.5)
            // Double-tap again: 2.5x -> 1.0x
            vm.handleDoubleTap()
            try assertEqual(vm.currentScale, 1.0)

            // Pinch zoom clamp: min 1.0, max 5.0
            vm.handlePinch(scale: 0.5)
            try assertEqual(vm.currentScale, 1.0)
            vm.handlePinch(scale: 10.0)
            try assertEqual(vm.currentScale, 5.0)
        }

        await runTest("F25_ADV04_ZoomPanRoutingPreventsDismiss") {
            let items = [TelegramMediaItem(id: "photo1", messageId: 1, chatId: 1)]
            let vm = MediaViewerViewModel(items: items)
            vm.currentScale = 2.0

            vm.handleDragChanged(translation: CGSize(width: 50, height: 100))
            try assertEqual(vm.dismissOffsetY, 0.0, "Zoomed drag must not alter dismissOffsetY")
            try assertEqual(vm.panOffset, CGSize(width: 50, height: 100), "Zoomed drag must route to panOffset")
        }

        // =====================================================================
        // FEATURE 26: Chunked Media Streaming Bridge
        // =====================================================================
        await runTest("F26_ADV01_512KBChunkPartitioningAndSHA256") {
            let fiveMBData = Data(repeating: 0xEE, count: 5 * 1024 * 1024)
            let chunks = try MediaTransferManager.partition(data: fiveMBData, chunkSize: 512 * 1024)
            try assertEqual(chunks.count, 10, "5MB data must partition into exactly 10 chunks of 512KB")

            let fullSha256 = MediaTransferManager.computeSHA256(data: fiveMBData)
            try assertEqual(fullSha256.count, 64, "SHA-256 CAS hash must be 64 hex characters")

            // Empty data throws error
            var caughtEmpty = false
            do {
                _ = try MediaTransferManager.partition(data: Data())
            } catch let err as MediaTransferError {
                try assertEqual(err, .emptyFile)
                caughtEmpty = true
            }
            try assertTrue(caughtEmpty, "Empty file partition must throw emptyFile")
        }

        await runTest("F26_ADV02_MonotonicUploadProgressEmission") {
            let manager = MediaTransferManager()
            let tmpDir = FileManager.default.temporaryDirectory
            let testFileURL = tmpDir.appendingPathComponent("adv_transfer_test.dat")
            let testData = Data(repeating: 0x77, count: 5 * 512 * 1024)
            try testData.write(to: testFileURL)
            defer { try? FileManager.default.removeItem(at: testFileURL) }

            var fractions: [Double] = []
            let stream = AsyncStream<MediaTransferManager.TransferProgress> { continuation in
                Task {
                    _ = try? await manager.uploadMedia(
                        fileURL: testFileURL,
                        mimeType: "application/octet-stream",
                        fileName: "adv_transfer_test.dat",
                        mockBridge: true,
                        progressStream: continuation
                    )
                    continuation.finish()
                }
            }

            for await event in stream {
                if case .progress(_, let frac, _, _) = event {
                    fractions.append(frac)
                }
            }

            try assertEqual(fractions.count, 5, "Must emit 5 progress events")
            try assertEqual(fractions, [0.2, 0.4, 0.6, 0.8, 1.0], "Progress events must be strictly monotonic")
        }

        // =====================================================================
        // Final Assessment
        // =====================================================================
        print("\n================================================================================")
        let finalRss = getRssMB()
        print("Final Process RSS: \(String(format: "%.2f", finalRss)) MB (Delta: \(String(format: "%+.2f", finalRss - initialRss)) MB)")
        print(" >>> ALL \(totalCount)/\(totalCount) ADVERSARIAL STRESS TESTS PASSED EMPIRICALLY! <<<")
        print("================================================================================")
    }
}
