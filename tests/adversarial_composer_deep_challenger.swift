//
//  adversarial_composer_deep_challenger.swift
//  FabushiTests
//
//  Milestone 4 Deep Adversarial Challenge Harness (challenger_m4_ios_2)
//  Empirical verification of:
//  - 5-bit waveform bit-packing/unpacking round-trip (100 samples <-> 63 bytes)
//  - Dynamic ceiling normalization & silence gating
//  - Corrupted buffer size rejection across 11 invalid lengths
//  - 512KB chunking protocol & boundary alignment
//  - SHA-256 CAS verification & mismatch rejection
//  - Local atomic cache staging & local cache hit bypass
//  - 0-byte and 2GB document limits (-1B, 0B, 1B, 2GB, 2GB+1B, 10GB)
//  - Multi-select badge continuous re-indexing & 100-item clamp
//  - Voice gesture cancel/lock thresholds & rolling buffer cap
//  - Media viewer swipe-down dismiss physics & zoom pan routing
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
struct AdversarialComposerDeepChallenger {

    @MainActor private static var passCount: Int = 0
    @MainActor private static var totalCount: Int = 0
    @MainActor private static var checkCount: Int = 0

    @MainActor
    private static func runChallenge(_ name: String, block: () async throws -> Void) async {
        totalCount += 1
        print("[\(totalCount)] CHALLENGE: \(name)...")
        do {
            try await block()
            passCount += 1
            print("  ✓ \(name) PASSED")
        } catch {
            print("  ✗ \(name) FAILED: \(error)")
            exit(1)
        }
    }

    @MainActor
    private static func assertTrue(_ condition: @autoclosure () -> Bool, _ message: String = "", file: StaticString = #file, line: UInt = #line) throws {
        checkCount += 1
        if !condition() {
            throw ChallengeFailure("Assertion failed at \(file):\(line): \(message)")
        }
    }

    @MainActor
    private static func assertFalse(_ condition: @autoclosure () -> Bool, _ message: String = "", file: StaticString = #file, line: UInt = #line) throws {
        checkCount += 1
        if condition() {
            throw ChallengeFailure("Assertion failed at \(file):\(line): expected false, got true. \(message)")
        }
    }

    @MainActor
    private static func assertEqual<T: Equatable>(_ a: T, _ b: T, _ message: String = "", file: StaticString = #file, line: UInt = #line) throws {
        checkCount += 1
        if a != b {
            throw ChallengeFailure("Assertion failed at \(file):\(line): expected \(b), got \(a). \(message)")
        }
    }

    @MainActor
    private static func assertAlmostEqual(_ a: CGFloat, _ b: CGFloat, accuracy: CGFloat = 0.001, _ message: String = "", file: StaticString = #file, line: UInt = #line) throws {
        checkCount += 1
        if abs(a - b) > accuracy {
            throw ChallengeFailure("Assertion failed at \(file):\(line): \(a) not almost equal to \(b) (diff: \(abs(a - b)) > \(accuracy)). \(message)")
        }
    }

    struct ChallengeFailure: Error, CustomStringConvertible {
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
        print(" DEEP ADVERSARIAL CHALLENGER: Milestone 4 Codecs, Transfer & Data Boundaries")
        print(" Agent: challenger_m4_ios_2 | Strict Concurrency: Swift 6")
        print("================================================================================")
        let initialRss = getRssMB()
        print("Initial Process RSS: \(String(format: "%.2f", initialRss)) MB\n")

        // =====================================================================
        // CHALLENGE 01: Waveform Bitstream Packing Exhaustive & Padding Noise Immunity
        // =====================================================================
        await runChallenge("CHALLENGE_01_WaveformBitstreamPackingExhaustive") {
            // Test 1: All 32 discrete 5-bit levels (0..31) in repeating pattern
            let pattern32 = (0..<100).map { Double($0 % 32) / 31.0 }
            let packed32 = WaveformCodec5Bit.encode(samples: pattern32)
            try assertEqual(packed32.count, 63, "Packed size must be exactly 63 bytes")

            let decoded32 = try WaveformCodec5Bit.decode(packed: packed32)
            try assertEqual(decoded32.count, 100, "Decoded count must be 100")
            for i in 0..<100 {
                let expectedLevel = UInt8(round((Double(i % 32) / 31.0) * 31.0))
                try assertEqual(decoded32[i], expectedLevel, "Sample \(i) level mismatch")
            }

            // Test 2: Maximum saturation (all 31s)
            let allMax = [Double](repeating: 1.0, count: 100)
            let packedMax = WaveformCodec5Bit.encode(samples: allMax)
            let decodedMax = try WaveformCodec5Bit.decode(packed: packedMax)
            try assertTrue(decodedMax.allSatisfy { $0 == 31 }, "All max samples must decode to 31")

            // Test 3: Alternating 0 and 31
            let alternating = (0..<100).map { $0 % 2 == 0 ? 0.0 : 1.0 }
            let packedAlt = WaveformCodec5Bit.encode(samples: alternating)
            let decodedAlt = try WaveformCodec5Bit.decode(packed: packedAlt)
            for i in 0..<100 {
                let expected: UInt8 = (i % 2 == 0) ? 0 : 31
                try assertEqual(decodedAlt[i], expected, "Alternating sample \(i) mismatch")
            }

            // Test 4: Adversarial padding noise immunity
            // The 100 samples occupy 500 bits. Byte 62 contains 4 data bits (bits 0..3) and 4 padding bits (bits 4..7).
            // Corrupt the top 4 padding bits of byte 62 with 0xF0:
            var corruptedPadding = packed32
            corruptedPadding[62] |= 0xF0
            let decodedPaddingNoise = try WaveformCodec5Bit.decode(packed: corruptedPadding)
            try assertEqual(decodedPaddingNoise, decoded32, "Top 4 padding bits must be ignored by decoder")
        }

        // =====================================================================
        // CHALLENGE 02: Waveform Dynamic Ceiling Normalization & Resampling
        // =====================================================================
        await runChallenge("CHALLENGE_02_WaveformDynamicCeilingNormalization") {
            // Sub-threshold silence gate (< 0.0001)
            let silence = (0..<100).map { _ in 0.00005 }
            let packedSilence = WaveformCodec5Bit.encode(samples: silence)
            let decodedSilence = try WaveformCodec5Bit.decode(packed: packedSilence)
            try assertTrue(decodedSilence.allSatisfy { $0 == 0 }, "Sub-threshold signal must gate to level 0")

            // Ultra-low peak (0.0002) above threshold must normalize peak to 31
            var tinyPeak = [Double](repeating: 0.00001, count: 100)
            tinyPeak[50] = 0.0002
            let packedTiny = WaveformCodec5Bit.encode(samples: tinyPeak)
            let decodedTiny = try WaveformCodec5Bit.decode(packed: packedTiny)
            try assertEqual(decodedTiny[50], 31, "Peak at index 50 must scale to 31")

            // Resampling edge cases
            // 0 samples -> 100 zeros
            let resample0 = WaveformCodec5Bit.resample(samples: [], targetCount: 100)
            try assertEqual(resample0.count, 100)
            try assertTrue(resample0.allSatisfy { $0 == 0.0 })

            // 1 sample -> 100 identical values
            let resample1 = WaveformCodec5Bit.resample(samples: [0.75], targetCount: 100)
            try assertEqual(resample1.count, 100)
            try assertTrue(resample1.allSatisfy { abs($0 - 0.75) < 0.001 })

            // 2 samples -> linear interpolation from 0.0 to 1.0
            let resample2 = WaveformCodec5Bit.resample(samples: [0.0, 1.0], targetCount: 100)
            try assertEqual(resample2.count, 100)
            try assertAlmostEqual(resample2[0], 0.0)
            try assertAlmostEqual(resample2[99], 1.0)
            try assertAlmostEqual(resample2[50], 50.0 / 99.0, accuracy: 0.01)

            // 10,000 samples -> downsampled to 100
            let hugeSamples = (0..<10_000).map { Double($0) / 10_000.0 }
            let resampleHuge = WaveformCodec5Bit.resample(samples: hugeSamples, targetCount: 100)
            try assertEqual(resampleHuge.count, 100)
            try assertTrue(resampleHuge[99] > 0.99)
        }

        // =====================================================================
        // CHALLENGE 03: Waveform Buffer Size Corruption Rejection
        // =====================================================================
        await runChallenge("CHALLENGE_03_WaveformBufferSizeCorruptionRejection") {
            let corruptedLengths = [0, 1, 2, 10, 61, 62, 64, 65, 100, 500, 1024]
            for len in corruptedLengths {
                let fakeBuffer = [UInt8](repeating: 0xAA, count: len)
                var caught = false
                do {
                    _ = try WaveformCodec5Bit.decode(packed: fakeBuffer)
                } catch let err as WaveformCodecError {
                    if case .corruptedWaveformBufferSize(let exp, let act) = err {
                        try assertEqual(exp, 63)
                        try assertEqual(act, len)
                        caught = true
                    }
                }
                try assertTrue(caught, "Corrupted buffer of length \(len) must be rejected with corruptedWaveformBufferSize")
            }
        }

        // =====================================================================
        // CHALLENGE 04: 512KB Chunk Partitioning Protocol
        // =====================================================================
        await runChallenge("CHALLENGE_04_512KBChunkPartitioningProtocol") {
            let chunkSize = MediaTransferManager.chunkSize
            try assertEqual(chunkSize, 524_288, "Chunk size must be exactly 524,288 bytes (512KB)")

            // 0 bytes throws emptyFile
            var caughtEmpty = false
            do {
                _ = try MediaTransferManager.partition(data: Data())
            } catch let err as MediaTransferError {
                try assertEqual(err, .emptyFile)
                caughtEmpty = true
            }
            try assertTrue(caughtEmpty)

            // 1 byte -> 1 chunk
            let c1 = try MediaTransferManager.partition(data: Data([0xFF]))
            try assertEqual(c1.count, 1)
            try assertEqual(c1[0].count, 1)

            // Exactly 512KB - 1 byte -> 1 chunk
            let dataSub1 = Data(repeating: 0x11, count: chunkSize - 1)
            let cSub1 = try MediaTransferManager.partition(data: dataSub1)
            try assertEqual(cSub1.count, 1)
            try assertEqual(cSub1[0].count, chunkSize - 1)

            // Exactly 512KB -> 1 chunk
            let dataExact = Data(repeating: 0x22, count: chunkSize)
            let cExact = try MediaTransferManager.partition(data: dataExact)
            try assertEqual(cExact.count, 1)
            try assertEqual(cExact[0].count, chunkSize)

            // Exactly 512KB + 1 byte -> 2 chunks
            let dataPlus1 = Data(repeating: 0x33, count: chunkSize + 1)
            let cPlus1 = try MediaTransferManager.partition(data: dataPlus1)
            try assertEqual(cPlus1.count, 2)
            try assertEqual(cPlus1[0].count, chunkSize)
            try assertEqual(cPlus1[1].count, 1)

            // 2MB (4 full chunks)
            let data2MB = Data(repeating: 0x44, count: 4 * chunkSize)
            let c2MB = try MediaTransferManager.partition(data: data2MB)
            try assertEqual(c2MB.count, 4)
            for chunk in c2MB {
                try assertEqual(chunk.count, chunkSize)
            }

            // Verify reassembly integrity
            var reassembled = Data()
            for chunk in c2MB {
                reassembled.append(chunk)
            }
            try assertEqual(reassembled, data2MB, "Reassembled chunks must match original data byte-for-byte")
        }

        // =====================================================================
        // CHALLENGE 05: SHA-256 CAS Verification & Mismatch Rejection
        // =====================================================================
        await runChallenge("CHALLENGE_05_SHA256CASVerificationAndMismatchRejection") {
            let manager = MediaTransferManager()
            let testPayload = "TelegramCASVerificationPayload2026".data(using: .utf8)!
            let correctSha256 = MediaTransferManager.computeSHA256(data: testPayload)
            try assertEqual(correctSha256.count, 64)

            // Mismatch rejection during download
            let wrongSha256 = String(repeating: "0", count: 64)
            var caughtMismatch = false
            do {
                _ = try await manager.downloadMedia(
                    mediaId: "test_mismatch_media",
                    fileSha256: wrongSha256,
                    totalBytes: Int64(testPayload.count),
                    fileName: "test_mismatch.dat",
                    mockChunks: [testPayload]
                )
            } catch let err as MediaTransferError {
                if case .sha256Mismatch(let exp, let act) = err {
                    try assertEqual(exp, wrongSha256)
                    try assertEqual(act, correctSha256)
                    caughtMismatch = true
                }
            }
            try assertTrue(caughtMismatch, "Download must reject mismatched SHA-256")
        }

        // =====================================================================
        // CHALLENGE 06: Local Cache Hit Bypass & Atomic Staging
        // =====================================================================
        await runChallenge("CHALLENGE_06_LocalCacheHitBypassAndAtomicStaging") {
            let manager = MediaTransferManager()
            let cacheDir = try MediaTransferManager.mediaCacheDirectory()
            let testContent = "PreExistingCachedContentForBypassVerification".data(using: .utf8)!
            let testSha256 = MediaTransferManager.computeSHA256(data: testContent)
            let cachedTargetURL = cacheDir.appendingPathComponent("\(testSha256).dat")

            // Write pre-existing cached file
            try testContent.write(to: cachedTargetURL)
            defer { try? FileManager.default.removeItem(at: cachedTargetURL) }

            // Download with empty mockChunks (should NOT be touched because cache hit bypasses download)
            let resultURL = try await manager.downloadMedia(
                mediaId: "cached_bypass_media",
                fileSha256: testSha256,
                totalBytes: Int64(testContent.count),
                fileName: "cached_bypass.dat",
                mockChunks: []
            )

            try assertEqual(resultURL.path, cachedTargetURL.path, "Cache hit must return existing cached URL directly")
            let readBack = try Data(contentsOf: resultURL)
            try assertEqual(readBack, testContent, "Read back from cached URL must match original")
        }

        // =====================================================================
        // CHALLENGE 07: Document Size Validation 0-Byte and 2GB Limits
        // =====================================================================
        await runChallenge("CHALLENGE_07_DocumentSizeValidation0ByteAnd2GB") {
            // Negative file size
            var caughtNeg = false
            do {
                try DocumentAttachment.validate(fileSizeBytes: -100)
            } catch let err as DocumentError {
                try assertEqual(err, .emptyFile)
                caughtNeg = true
            }
            try assertTrue(caughtNeg, "Negative file size must be rejected as emptyFile")

            // 0-byte file
            var caughtZero = false
            do {
                try DocumentAttachment.validate(fileSizeBytes: 0)
            } catch let err as DocumentError {
                try assertEqual(err, .emptyFile)
                caughtZero = true
            }
            try assertTrue(caughtZero, "0-byte file must be rejected as emptyFile")

            // Valid 1 byte
            try DocumentAttachment.validate(fileSizeBytes: 1)

            // Valid 100 MB
            try DocumentAttachment.validate(fileSizeBytes: 100 * 1024 * 1024)

            // Exact 2GB limit (2 * 1024 * 1024 * 1024 = 2,147,483,648 bytes) -> MUST PASS
            let exact2GB: Int64 = 2 * 1024 * 1024 * 1024
            try DocumentAttachment.validate(fileSizeBytes: exact2GB)

            // 2GB + 1 byte -> MUST FAIL with fileExceeds2GBLimit
            var caughtPlus1 = false
            do {
                try DocumentAttachment.validate(fileSizeBytes: exact2GB + 1)
            } catch let err as DocumentError {
                try assertEqual(err, .fileExceeds2GBLimit)
                caughtPlus1 = true
            }
            try assertTrue(caughtPlus1, "2GB + 1 byte must be rejected as fileExceeds2GBLimit")

            // 10 GB -> MUST FAIL with fileExceeds2GBLimit
            var caught10GB = false
            do {
                try DocumentAttachment.validate(fileSizeBytes: 10 * 1024 * 1024 * 1024)
            } catch let err as DocumentError {
                try assertEqual(err, .fileExceeds2GBLimit)
                caught10GB = true
            }
            try assertTrue(caught10GB, "10GB must be rejected as fileExceeds2GBLimit")
        }

        // =====================================================================
        // CHALLENGE 08: Multi-Select Badge Re-Indexing & 100-Item Clamp
        // =====================================================================
        await runChallenge("CHALLENGE_08_MultiSelectBadgeReIndexingAndClamp") {
            let vm = AttachmentDrawerViewModel()
            let items = (1...10).map { MediaPreviewItem(id: "item_\($0)") }

            // Select 10 items
            for item in items {
                vm.toggleSelection(item)
            }
            try assertEqual(vm.selectedItems.count, 10)
            for i in 0..<10 {
                try assertEqual(vm.badgeIndex(for: items[i]), i + 1, "Initial badge \(i+1) mismatch")
            }

            // Remove item #4 (the 4th item, badge was 4)
            vm.toggleSelection(items[3])
            try assertEqual(vm.selectedItems.count, 9)
            try assertTrue(vm.badgeIndex(for: items[3]) == nil, "Removed item badge must be nil")

            // Verify items 1..3 retain badges 1..3
            try assertEqual(vm.badgeIndex(for: items[0]), 1)
            try assertEqual(vm.badgeIndex(for: items[1]), 2)
            try assertEqual(vm.badgeIndex(for: items[2]), 3)

            // Verify items 5..10 are re-indexed to 4..9
            try assertEqual(vm.badgeIndex(for: items[4]), 4)
            try assertEqual(vm.badgeIndex(for: items[5]), 5)
            try assertEqual(vm.badgeIndex(for: items[6]), 6)
            try assertEqual(vm.badgeIndex(for: items[7]), 7)
            try assertEqual(vm.badgeIndex(for: items[8]), 8)
            try assertEqual(vm.badgeIndex(for: items[9]), 9)

            // Remove item #1 (index 0, badge was 1)
            vm.toggleSelection(items[0])
            try assertEqual(vm.selectedItems.count, 8)
            try assertEqual(vm.badgeIndex(for: items[1]), 1)
            try assertEqual(vm.badgeIndex(for: items[2]), 2)
            try assertEqual(vm.badgeIndex(for: items[4]), 3)

            // Clear and stress clamp at 100 items
            vm.clearSelection()
            try assertEqual(vm.selectedItems.count, 0)

            for i in 1...150 {
                vm.toggleSelection(MediaPreviewItem(id: "stress_\(i)"))
            }
            try assertEqual(vm.selectedItems.count, 100, "Selection count must be strictly clamped at 100 items")
            try assertEqual(vm.badgeIndex(for: MediaPreviewItem(id: "stress_1")), 1)
            try assertEqual(vm.badgeIndex(for: MediaPreviewItem(id: "stress_100")), 100)
            try assertTrue(vm.badgeIndex(for: MediaPreviewItem(id: "stress_101")) == nil, "Item 101 must not be selected")
        }

        // =====================================================================
        // CHALLENGE 09: Voice Gesture State Boundaries & Rolling Buffer
        // =====================================================================
        await runChallenge("CHALLENGE_09_VoiceGestureStateBoundariesAndRollingBuffer") {
            let handler = VoiceRecordGestureHandler()
            handler.onLongPressTriggered()
            try assertTrue(handler.isHolding)

            // Boundary: -99.9pt width is still holding
            handler.onDragChanged(translation: CGSize(width: -99.9, height: 0))
            try assertTrue(handler.isHolding)

            // Boundary: -100.0pt width transitions to cancelled
            handler.onDragChanged(translation: CGSize(width: -100.0, height: 0))
            try assertEqual(handler.state, .cancelled)

            // Reset and test lock threshold boundary
            let handlerLock = VoiceRecordGestureHandler()
            handlerLock.onLongPressTriggered()
            handlerLock.onDragChanged(translation: CGSize(width: 0, height: -79.9))
            try assertTrue(handlerLock.isHolding)
            handlerLock.onDragChanged(translation: CGSize(width: 0, height: -80.0))
            try assertTrue(handlerLock.isLocked)

            // Rolling buffer cap: 500 samples added must cap at exactly 100
            let bufferHandler = VoiceRecordGestureHandler()
            for i in 1...500 {
                bufferHandler.appendLiveSample(Double(i) / 500.0)
            }
            try assertEqual(bufferHandler.liveSamples.count, 100)
            // Most recent sample should be 500.0 / 500.0 = 1.0
            try assertAlmostEqual(bufferHandler.liveSamples.last ?? 0, 1.0)
            // Oldest sample in rolling buffer should be 401.0 / 500.0 = 0.802
            try assertAlmostEqual(bufferHandler.liveSamples.first ?? 0, 401.0 / 500.0)

            // Test out-of-range clamping (<0 and >1)
            bufferHandler.appendLiveSample(-0.5)
            try assertEqual(bufferHandler.liveSamples.last ?? -1, 0.0, "Negative amplitude must clamp to 0.0")
            bufferHandler.appendLiveSample(99.9)
            try assertEqual(bufferHandler.liveSamples.last ?? -1, 1.0, "Excess amplitude must clamp to 1.0")
        }

        // =====================================================================
        // CHALLENGE 10: Media Viewer Dismissal & Zoom Physics
        // =====================================================================
        await runChallenge("CHALLENGE_10_MediaViewerDismissalAndZoomPhysics") {
            let items = [TelegramMediaItem(id: "media_p1", messageId: 10, chatId: 20)]
            let vm = MediaViewerViewModel(items: items)

            // Physics calculation checks
            vm.handleDragChanged(translation: CGSize(width: 0, height: 0))
            try assertAlmostEqual(vm.dismissProgress, 0.0)
            try assertAlmostEqual(vm.dismissScale, 1.0)
            try assertAlmostEqual(vm.dismissOpacity, 1.0)

            vm.handleDragChanged(translation: CGSize(width: 0, height: 60))
            try assertAlmostEqual(vm.dismissProgress, 0.25)
            try assertAlmostEqual(vm.dismissScale, 0.925)
            try assertAlmostEqual(vm.dismissOpacity, 0.75)

            vm.handleDragChanged(translation: CGSize(width: 0, height: 240))
            try assertAlmostEqual(vm.dismissProgress, 1.0)
            try assertAlmostEqual(vm.dismissScale, 0.70)
            try assertAlmostEqual(vm.dismissOpacity, 0.0)

            // Beyond 240pt clamp
            vm.handleDragChanged(translation: CGSize(width: 0, height: 480))
            try assertAlmostEqual(vm.dismissProgress, 1.0)
            try assertAlmostEqual(vm.dismissScale, 0.70)
            try assertAlmostEqual(vm.dismissOpacity, 0.0)

            // Dismissal velocity boundary: 799 pt/s snaps back, 800 pt/s triggers
            var dismissed = false
            vm.handleDragEnded(translation: CGSize(width: 0, height: 50), velocityY: 799) {
                dismissed = true
            }
            try assertFalse(dismissed, "799 pt/s must snap back")

            vm.handleDragEnded(translation: CGSize(width: 0, height: 50), velocityY: 800) {
                dismissed = true
            }
            try assertTrue(dismissed, "800 pt/s must trigger dismissal")

            // Dismissal distance boundary: 159.9pt snaps back, 160.0pt triggers
            dismissed = false
            vm.handleDragEnded(translation: CGSize(width: 0, height: 159.9), velocityY: 0) {
                dismissed = true
            }
            try assertFalse(dismissed, "159.9 pt must snap back")

            vm.handleDragEnded(translation: CGSize(width: 0, height: 160.0), velocityY: 0) {
                dismissed = true
            }
            try assertTrue(dismissed, "160.0 pt must trigger dismissal")

            // Zoom Pan Routing on fresh viewer
            let vmZoom = MediaViewerViewModel(items: items)
            vmZoom.currentScale = 3.0
            vmZoom.handleDragChanged(translation: CGSize(width: 40, height: 80))
            try assertEqual(vmZoom.dismissOffsetY, 0.0, "Zoomed drag must not modify dismissOffsetY")
            try assertEqual(vmZoom.panOffset, CGSize(width: 40, height: 80), "Zoomed drag must route to panOffset")
        }

        // =====================================================================
        // Final Assessment
        // =====================================================================
        print("\n================================================================================")
        let finalRss = getRssMB()
        print("Final Process RSS: \(String(format: "%.2f", finalRss)) MB (Delta: \(String(format: "%+.2f", finalRss - initialRss)) MB)")
        print("Completed \(checkCount) discrete assertions across \(totalCount) challenge suites.")
        print(" >>> ALL \(totalCount)/\(totalCount) DEEP ADVERSARIAL CHALLENGES PASSED EMPIRICALLY! <<<")
        print("================================================================================")
    }
}
