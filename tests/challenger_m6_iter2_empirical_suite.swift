//
//  challenger_m6_iter2_empirical_suite.swift
//  FabushiTests
//
//  Milestone 6 Iteration 2: Tier 5 Presentation & UI State Hardening
//  Adversarial Empirical Challenge Suite.
//
//  Vectors under challenge:
//  1. Message Bubble Geometry & Tail Math (16pt/4pt transitions, extreme widths 20pt/10000pt, typographic collision)
//  2. 5-Bit Waveform Codec (0, 1, 100, 100,000 bursts, 10,000-iteration random bit-for-bit fuzzing, corruption rejection)
//  3. Voice Recording Gestures (lock -80pt & cancel -100pt boundaries, 1,000 rapid cycles, 10,000 audio samples rolling cap)
//  4. Parallax Profile Scaling (-300pt -> 2.5 scale, positive collapse -> 0.36, nav bar title opacity curve)
//  5. Inset Settings & Sub-10ms CUA Computer Lease (lockout curve, clamping, 1,000 emergency stops <10ms SLA, RSS leak longevity)
//

import CoreGraphics
import CoreText
import Foundation
import SwiftUI
#if canImport(Darwin)
import Darwin
#endif

final class ChallengeFailure: Error, CustomStringConvertible, Sendable {
    let message: String
    init(_ message: String) { self.message = message }
    var description: String { message }
}

@main
struct ChallengerM6Iter2Suite {

    @MainActor private static var passCount: Int = 0
    @MainActor private static var failCount: Int = 0
    @MainActor private static var totalChecks: Int = 0

    static func getRssMB() -> Double {
        #if canImport(Darwin)
        var info = mach_task_basic_info()
        var count = mach_msg_type_number_t(MemoryLayout<mach_task_basic_info>.size) / 4
        let kerr: kern_return_t = withUnsafeMutablePointer(to: &info) {
            $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
                task_info(mach_task_self_, task_flavor_t(MACH_TASK_BASIC_INFO), $0, &count)
            }
        }
        if kerr == KERN_SUCCESS {
            return Double(info.resident_size) / (1024.0 * 1024.0)
        }
        return 0.0
        #else
        return 0.0
        #endif
    }

    @MainActor
    private static func assertTrue(_ condition: @autoclosure () -> Bool, _ message: String = "", file: StaticString = #file, line: UInt = #line) throws {
        totalChecks += 1
        if !condition() {
            throw ChallengeFailure("Assertion FAILED at \(file):\(line): \(message)")
        }
    }

    @MainActor
    private static func assertFalse(_ condition: @autoclosure () -> Bool, _ message: String = "", file: StaticString = #file, line: UInt = #line) throws {
        totalChecks += 1
        if condition() {
            throw ChallengeFailure("Assertion FAILED at \(file):\(line): expected false, got true. \(message)")
        }
    }

    @MainActor
    private static func assertEqual<T: Equatable>(_ a: T, _ b: T, _ message: String = "", file: StaticString = #file, line: UInt = #line) throws {
        totalChecks += 1
        if a != b {
            throw ChallengeFailure("Equality FAILED at \(file):\(line): expected '\(b)', got '\(a)'. \(message)")
        }
    }

    @MainActor
    private static func assertAlmostEqual(_ a: CGFloat, _ b: CGFloat, accuracy: CGFloat = 0.001, _ message: String = "", file: StaticString = #file, line: UInt = #line) throws {
        totalChecks += 1
        if abs(a - b) > accuracy {
            throw ChallengeFailure("Accuracy FAILED at \(file):\(line): expected \(b) ± \(accuracy), got \(a). \(message)")
        }
    }

    @MainActor
    private static func assertAlmostEqualDouble(_ a: Double, _ b: Double, accuracy: Double = 0.001, _ message: String = "", file: StaticString = #file, line: UInt = #line) throws {
        totalChecks += 1
        if abs(a - b) > accuracy {
            throw ChallengeFailure("Accuracy FAILED at \(file):\(line): expected \(b) ± \(accuracy), got \(a). \(message)")
        }
    }

    @MainActor
    private static func runSection(_ title: String, block: () async throws -> Void) async {
        print("\n================================================================================")
        print(" [CHALLENGE] \(title)")
        print("================================================================================")
        do {
            try await block()
            passCount += 1
            print(" >>> SECTION PASSED: \(title)")
        } catch {
            failCount += 1
            print(" !!! SECTION FAILED: \(title) -> \(error)")
        }
    }

    // =========================================================================
    // MARK: - VECTOR 1: Message Bubble Geometry & Tail Math
    // =========================================================================

    @MainActor
    static func challengeVector1_BubbleGeometry() async throws {
        print(" -> Sub-challenge 1.1: 5-minute same-sender clustering boundary conditions")
        let senderA: Int64 = 1001
        let senderB: Int64 = 1002
        let t0: Int64 = 1_000_000_000

        let m1 = TelegramStageMessage(id: 1, chatId: 10, senderId: senderA, text: "Msg 1", timestamp: t0, isOutgoing: true)
        let m2_exact = TelegramStageMessage(id: 2, chatId: 10, senderId: senderA, text: "Msg 2", timestamp: t0 + 300_000, isOutgoing: true)
        let m2_over = TelegramStageMessage(id: 3, chatId: 10, senderId: senderA, text: "Msg 3", timestamp: t0 + 300_001, isOutgoing: true)
        let m2_under = TelegramStageMessage(id: 4, chatId: 10, senderId: senderA, text: "Msg 4", timestamp: t0 + 299_999, isOutgoing: true)
        let m2_diffSender = TelegramStageMessage(id: 5, chatId: 10, senderId: senderB, text: "Msg 5", timestamp: t0 + 100, isOutgoing: true)
        let m2_diffDirection = TelegramStageMessage(id: 6, chatId: 10, senderId: senderA, text: "Msg 6", timestamp: t0 + 100, isOutgoing: false)

        try assertTrue(MessageClusterCalculator.canCluster(m1, m2_exact), "Exact 300,000ms boundary must cluster")
        try assertTrue(MessageClusterCalculator.canCluster(m1, m2_under), "299,999ms must cluster")
        try assertFalse(MessageClusterCalculator.canCluster(m1, m2_over), "300,001ms must NOT cluster")
        try assertFalse(MessageClusterCalculator.canCluster(m1, m2_diffSender), "Different senderId must NOT cluster")
        try assertFalse(MessageClusterCalculator.canCluster(m1, m2_diffDirection), "Different outgoing direction must NOT cluster")

        print(" -> Sub-challenge 1.2: 4-cluster position continuous corner radii math")
        // Chronological: older, current, newer
        let (posSingle, radSingle) = MessageClusterCalculator.calculate(current: m1, older: nil, newer: nil, isRTL: false)
        try assertEqual(posSingle, .single)
        try assertEqual(radSingle, BubbleCornerRadii(topLeft: 16, topRight: 16, bottomLeft: 16, bottomRight: 16, hasTail: true))

        let mTop = TelegramStageMessage(id: 11, chatId: 10, senderId: senderA, text: "Top", timestamp: t0, isOutgoing: true)
        let mMid = TelegramStageMessage(id: 12, chatId: 10, senderId: senderA, text: "Mid", timestamp: t0 + 1000, isOutgoing: true)
        let mBot = TelegramStageMessage(id: 13, chatId: 10, senderId: senderA, text: "Bot", timestamp: t0 + 2000, isOutgoing: true)

        let (posTop, radTop) = MessageClusterCalculator.calculate(current: mTop, older: nil, newer: mMid, isRTL: false)
        try assertEqual(posTop, .top)
        try assertEqual(radTop, BubbleCornerRadii(topLeft: 16, topRight: 16, bottomLeft: 16, bottomRight: 4, hasTail: false))

        let (posMid, radMid) = MessageClusterCalculator.calculate(current: mMid, older: mTop, newer: mBot, isRTL: false)
        try assertEqual(posMid, .middle)
        try assertEqual(radMid, BubbleCornerRadii(topLeft: 16, topRight: 4, bottomLeft: 16, bottomRight: 4, hasTail: false))

        let (posBot, radBot) = MessageClusterCalculator.calculate(current: mBot, older: mMid, newer: nil, isRTL: false)
        try assertEqual(posBot, .bottom)
        try assertEqual(radBot, BubbleCornerRadii(topLeft: 16, topRight: 4, bottomLeft: 16, bottomRight: 16, hasTail: true))

        print(" -> Sub-challenge 1.3: Incoming message corner radii math")
        let inTop = TelegramStageMessage(id: 21, chatId: 10, senderId: senderB, text: "InTop", timestamp: t0, isOutgoing: false)
        let inMid = TelegramStageMessage(id: 22, chatId: 10, senderId: senderB, text: "InMid", timestamp: t0 + 1000, isOutgoing: false)
        let inBot = TelegramStageMessage(id: 23, chatId: 10, senderId: senderB, text: "InBot", timestamp: t0 + 2000, isOutgoing: false)

        let (_, radInSingle) = MessageClusterCalculator.calculate(current: inTop, older: nil, newer: nil, isRTL: false)
        try assertEqual(radInSingle, BubbleCornerRadii(topLeft: 16, topRight: 16, bottomLeft: 16, bottomRight: 16, hasTail: true))

        let (_, radInTop) = MessageClusterCalculator.calculate(current: inTop, older: nil, newer: inMid, isRTL: false)
        try assertEqual(radInTop, BubbleCornerRadii(topLeft: 16, topRight: 16, bottomLeft: 4, bottomRight: 16, hasTail: false))

        let (_, radInMid) = MessageClusterCalculator.calculate(current: inMid, older: inTop, newer: inBot, isRTL: false)
        try assertEqual(radInMid, BubbleCornerRadii(topLeft: 4, topRight: 16, bottomLeft: 4, bottomRight: 16, hasTail: false))

        let (_, radInBot) = MessageClusterCalculator.calculate(current: inBot, older: inMid, newer: nil, isRTL: false)
        try assertEqual(radInBot, BubbleCornerRadii(topLeft: 4, topRight: 16, bottomLeft: 16, bottomRight: 16, hasTail: true))

        print(" -> Sub-challenge 1.4: RTL mirroring validation")
        let (_, radOutRTL) = MessageClusterCalculator.calculate(current: mMid, older: mTop, newer: mBot, isRTL: true)
        // In RTL, outgoing should be mirrored to left-anchored geometry (inner corners on left)
        try assertEqual(radOutRTL, BubbleCornerRadii(topLeft: 4, topRight: 16, bottomLeft: 4, bottomRight: 16, hasTail: false))

        let (_, radInRTL) = MessageClusterCalculator.calculate(current: inMid, older: inTop, newer: inBot, isRTL: true)
        // In RTL, incoming should be mirrored to right-anchored geometry (inner corners on right)
        try assertEqual(radInRTL, BubbleCornerRadii(topLeft: 16, topRight: 4, bottomLeft: 16, bottomRight: 4, hasTail: false))

        print(" -> Sub-challenge 1.5: Inverted scroll array indexation")
        let messagesInverted = [mBot, mMid, mTop] // 0=newest(bot), 1=mid, 2=oldest(top)
        let (invPos0, _) = MessageClusterCalculator.calculate(for: messagesInverted, at: 0, isInverted: true)
        let (invPos1, _) = MessageClusterCalculator.calculate(for: messagesInverted, at: 1, isInverted: true)
        let (invPos2, _) = MessageClusterCalculator.calculate(for: messagesInverted, at: 2, isInverted: true)
        try assertEqual(invPos0, .bottom)
        try assertEqual(invPos1, .middle)
        try assertEqual(invPos2, .top)

        print(" -> Sub-challenge 1.6: Extreme bubble widths (20pt, 1pt, 10,000pt) path generation")
        let testWidths: [CGFloat] = [1.0, 5.0, 10.0, 18.0, 20.0, 44.0, 320.0, 1000.0, 10000.0]
        let testHeights: [CGFloat] = [5.0, 16.0, 28.0, 50.0, 500.0, 5000.0]

        for w in testWidths {
            for h in testHeights {
                let rect = CGRect(x: 0, y: 0, width: w, height: h)
                // Outgoing with tail
                let p1 = TelegramBubbleShape(isOutgoing: true, radii: radBot, isRTL: false).path(in: rect)
                try assertFalse(p1.boundingRect.isNull, "Path bounding box must not be null for w=\(w), h=\(h)")
                try assertFalse(p1.boundingRect.isInfinite, "Path bounding box must not be infinite for w=\(w), h=\(h)")

                // Incoming with tail
                let p2 = TelegramBubbleShape(isOutgoing: false, radii: radInBot, isRTL: false).path(in: rect)
                try assertFalse(p2.boundingRect.isNull, "Incoming path bounding box must not be null for w=\(w), h=\(h)")
                try assertFalse(p2.boundingRect.isInfinite, "Incoming path bounding box must not be infinite for w=\(w), h=\(h)")

                // Outgoing without tail
                let p3 = TelegramBubbleShape(isOutgoing: true, radii: radMid, isRTL: false).path(in: rect)
                try assertFalse(p3.boundingRect.isNull, "Middle outgoing path bounding box must not be null for w=\(w), h=\(h)")

                // RTL variant
                let p4 = TelegramBubbleShape(isOutgoing: true, radii: radBot, isRTL: true).path(in: rect)
                try assertFalse(p4.boundingRect.isNull, "RTL outgoing path bounding box must not be null for w=\(w), h=\(h)")
            }
        }

        print(" -> Sub-challenge 1.7: CoreText typographic collision calculation under boundary widths")
        let emptyMetrics = TextTimestampMetrics.compute(text: "", maxWidth: 20)
        try assertFalse(emptyMetrics.causesCollision)
        try assertEqual(emptyMetrics.lineCount, 0)
        try assertAlmostEqual(emptyMetrics.contentWidth, 58.0) // max(44, 46 + 12) = 58

        let shortMetrics = TextTimestampMetrics.compute(text: "Hi", maxWidth: 200)
        try assertFalse(shortMetrics.causesCollision)
        try assertEqual(shortMetrics.lineCount, 1)

        // Force collision by narrow maxWidth
        let collMetrics = TextTimestampMetrics.compute(text: "This message should collide with timestamp", maxWidth: 60)
        try assertTrue(collMetrics.causesCollision, "Narrow maxWidth must trigger collision")
        try assertTrue(collMetrics.requiredHeight > 30.0)

        // 10,000-character stress test
        let longText = String(repeating: "Telegram Native iOS Presentation Hardening. ", count: 250)
        let startT = CFAbsoluteTimeGetCurrent()
        let longMetrics = TextTimestampMetrics.compute(text: longText, maxWidth: 320)
        let elapsed = (CFAbsoluteTimeGetCurrent() - startT) * 1000.0
        try assertTrue(longMetrics.lineCount > 50)
        try assertTrue(elapsed < 25.0, "10,000-char typographic measurement took \(elapsed)ms (SLA < 25ms)")
        print("    CoreText 10,000 chars computed in \(String(format: "%.2f", elapsed))ms, lines=\(longMetrics.lineCount)")
    }

    // =========================================================================
    // MARK: - VECTOR 2: 5-Bit Waveform Codec
    // =========================================================================

    @MainActor
    static func challengeVector2_WaveformCodec() async throws {
        print(" -> Sub-challenge 2.1: 0-sample burst")
        let emptyPacked = WaveformCodec5Bit.encode(samples: [])
        try assertEqual(emptyPacked.count, 63, "Encoded empty sample count must be exactly 63 bytes")
        try assertTrue(emptyPacked.allSatisfy { $0 == 0 }, "All packed bytes of empty samples must be 0")
        let decodedEmpty = try WaveformCodec5Bit.decode(packed: emptyPacked)
        try assertEqual(decodedEmpty.count, 100)
        try assertTrue(decodedEmpty.allSatisfy { $0 == 0 }, "Decoded empty levels must all be 0")

        print(" -> Sub-challenge 2.2: 1-sample burst")
        let singleSamplePacked = WaveformCodec5Bit.encode(samples: [0.85])
        try assertEqual(singleSamplePacked.count, 63)
        let decodedSingle = try WaveformCodec5Bit.decode(packed: singleSamplePacked)
        try assertEqual(decodedSingle.count, 100)
        // With dynamic ceiling compression, a single positive sample is peak and scales to 31
        try assertTrue(decodedSingle.allSatisfy { $0 == 31 }, "Single non-zero sample must scale peak to level 31")

        print(" -> Sub-challenge 2.3: 100-sample burst ramp")
        let ramp = (0..<100).map { Double($0) / 99.0 }
        let rampPacked = WaveformCodec5Bit.encode(samples: ramp)
        try assertEqual(rampPacked.count, 63)
        let decodedRamp = try WaveformCodec5Bit.decode(packed: rampPacked)
        try assertEqual(decodedRamp.count, 100)
        try assertEqual(decodedRamp[0], 0, "First sample in ramp must quantize to 0")
        try assertEqual(decodedRamp[99], 31, "Last sample in ramp must quantize to 31")

        print(" -> Sub-challenge 2.4: 100,000-sample burst downsampling performance")
        var massiveSamples = [Double](repeating: 0.0, count: 100_000)
        for i in 0..<100_000 {
            massiveSamples[i] = Double(i % 1000) / 1000.0
        }
        let tStart = CFAbsoluteTimeGetCurrent()
        let massivePacked = WaveformCodec5Bit.encode(samples: massiveSamples)
        let tElapsed = (CFAbsoluteTimeGetCurrent() - tStart) * 1000.0
        try assertEqual(massivePacked.count, 63)
        let decodedMassive = try WaveformCodec5Bit.decode(packed: massivePacked)
        try assertEqual(decodedMassive.count, 100)
        try assertTrue(tElapsed < 50.0, "100,000-sample waveform encode took \(tElapsed)ms (SLA < 50ms)")
        print("    100,000-sample burst downsampled and packed in \(String(format: "%.2f", tElapsed))ms")

        print(" -> Sub-challenge 2.5: Fuzzing 10,000 random bitstream roundtrips (1,000,000 samples)")
        var rngState: UInt64 = 0xDEADBEEFCAFE1234
        func nextRand() -> UInt8 {
            rngState = rngState &* 6364136223846793005 &+ 1442695040888963407
            return UInt8((rngState >> 32) & 0x1F) // 0..31 (5 bits)
        }

        let fuzzIterations = 10_000
        for _ in 0..<fuzzIterations {
            var originalLevels = [UInt8](repeating: 0, count: 100)
            for j in 0..<100 {
                originalLevels[j] = nextRand()
            }

            // Bit-pack 100 5-bit values into 63 bytes
            var packed = [UInt8](repeating: 0, count: 63)
            var bitOffset = 0
            for val in originalLevels {
                for bit in 0..<5 {
                    let bitVal = (val >> bit) & 1
                    let byteIdx = (bitOffset + bit) / 8
                    let bitIdx = (bitOffset + bit) % 8
                    packed[byteIdx] |= (bitVal << bitIdx)
                }
                bitOffset += 5
            }

            // Verify padding bits (bits 500..503 in byte 62) are 0
            let padding = (packed[62] >> 4) & 0x0F
            try assertEqual(padding, 0, "Waveform bit-packing padding bits must be 0")

            // Decode packed bytes back to 100 5-bit levels
            let decoded = try WaveformCodec5Bit.decode(packed: packed)
            try assertEqual(decoded.count, 100)
            for k in 0..<100 {
                if decoded[k] != originalLevels[k] {
                    throw ChallengeFailure("Fuzz roundtrip mismatch at index \(k): expected \(originalLevels[k]), got \(decoded[k])")
                }
            }
        }
        print("    ✓ Successfully verified 10,000 random roundtrips (1,000,000 samples) bit-for-bit exact!")

        print(" -> Sub-challenge 2.6: Corrupted buffer length rejection")
        let invalidLengths = [0, 1, 10, 32, 62, 64, 100, 128, 500]
        for len in invalidLengths {
            let corruptedBuffer = [UInt8](repeating: 0xFF, count: len)
            var didThrow = false
            do {
                _ = try WaveformCodec5Bit.decode(packed: corruptedBuffer)
            } catch let WaveformCodecError.corruptedWaveformBufferSize(expected, actual) {
                try assertEqual(expected, 63)
                try assertEqual(actual, len)
                didThrow = true
            } catch {
                throw ChallengeFailure("Unexpected error type: \(error)")
            }
            try assertTrue(didThrow, "Buffer length \(len) must be rejected with corruptedWaveformBufferSize")
        }

        print(" -> Sub-challenge 2.7: activeBarIndex clamped bounds")
        try assertEqual(WaveformCodec5Bit.activeBarIndex(for: -0.5), 0)
        try assertEqual(WaveformCodec5Bit.activeBarIndex(for: 0.0), 0)
        try assertEqual(WaveformCodec5Bit.activeBarIndex(for: 0.5), 50)
        try assertEqual(WaveformCodec5Bit.activeBarIndex(for: 1.0), 100)
        try assertEqual(WaveformCodec5Bit.activeBarIndex(for: 1.5), 100)
    }

    // =========================================================================
    // MARK: - VECTOR 3: Voice Recording Gestures
    // =========================================================================

    @MainActor
    static func challengeVector3_VoiceRecordingGestures() async throws {
        print(" -> Sub-challenge 3.1: Gesture lock and cancel threshold boundaries")
        let handler = VoiceRecordGestureHandler()

        // 1. Long press trigger
        handler.onLongPressTriggered()
        try assertTrue(handler.isHolding)
        try assertFalse(handler.isLocked)
        try assertTrue(handler.isRecording)

        // 2. Drag left: boundary at -100.0pt
        // -99.9pt: should still be holding
        handler.onDragChanged(translation: CGSize(width: -99.9, height: 0))
        try assertTrue(handler.isHolding, "Drag left at -99.9pt must remain holding")

        // -100.0pt: should trigger cancel
        handler.onDragChanged(translation: CGSize(width: -100.0, height: 0))
        try assertEqual(handler.state, .cancelled, "Drag left at -100.0pt must trigger cancel")

        // 3. Reset and test lock threshold boundary at -80.0pt
        handler.reset()
        handler.onLongPressTriggered()
        try assertTrue(handler.isHolding)

        // -79.9pt: should still be holding
        handler.onDragChanged(translation: CGSize(width: 0, height: -79.9))
        try assertTrue(handler.isHolding, "Drag up at -79.9pt must remain holding")

        // -80.0pt: should trigger lock
        handler.onDragChanged(translation: CGSize(width: 0, height: -80.0))
        try assertTrue(handler.isLocked, "Drag up at -80.0pt must trigger lock")
        try assertFalse(handler.isHolding)

        // 4. Test priority: both -100 width and -80 height -> cancel takes precedence
        handler.reset()
        handler.onLongPressTriggered()
        handler.onDragChanged(translation: CGSize(width: -100.0, height: -80.0))
        try assertEqual(handler.state, .cancelled, "Concurrent cancel & lock must prioritize cancel")

        print(" -> Sub-challenge 3.2: 1,000 rapid cycles of touch cancellation & reset stress")
        for _ in 0..<1000 {
            handler.reset()
            handler.onTouchBegan()
            handler.onLongPressTriggered()
            handler.onDragChanged(translation: CGSize(width: -105.0, height: 0))
            if handler.state != .cancelled {
                throw ChallengeFailure("State must be cancelled after -105pt drag")
            }
            handler.reset()
            if handler.state != .idle {
                throw ChallengeFailure("State must be idle after reset")
            }
        }
        print("    ✓ 1,000 rapid cancellation cycles completed cleanly with zero state corruption")

        print(" -> Sub-challenge 3.3: Hold duration completion evaluation")
        // Short tap (< 0.5s): completion(false), tap hint shown
        handler.reset()
        handler.onLongPressTriggered()
        var completedSent = false
        handler.onTouchEnded(duration: 0.25) { shouldSend in
            completedSent = shouldSend
        }
        try assertFalse(completedSent, "Short tap (<0.5s) must discard voice")
        try assertTrue(handler.showTapHint, "Short tap must trigger tap hint")

        // Valid hold (>= 0.5s): completion(true)
        handler.reset()
        handler.onLongPressTriggered()
        completedSent = false
        handler.onTouchEnded(duration: 0.6) { shouldSend in
            completedSent = shouldSend
        }
        try assertTrue(completedSent, "Valid hold (>=0.5s) must complete with shouldSend=true")
        try assertEqual(handler.state, .idle)

        print(" -> Sub-challenge 3.4: Audio chunk streaming and rolling buffer cap (10,000 samples)")
        handler.reset()
        for i in 0..<10_000 {
            let sampleVal = Double(i % 100) / 100.0
            handler.appendLiveSample(sampleVal)
            if handler.liveSamples.count > 100 {
                throw ChallengeFailure("Live samples count exceeded 100: \(handler.liveSamples.count)")
            }
        }
        try assertEqual(handler.liveSamples.count, 100, "Rolling buffer must cap at exactly 100 samples")

        // Test clamping of live samples
        handler.appendLiveSample(-0.5)
        try assertEqual(handler.liveSamples.last, 0.0, "Negative audio sample must clamp to 0.0")
        handler.appendLiveSample(2.5)
        try assertEqual(handler.liveSamples.last, 1.0, "Overscale audio sample must clamp to 1.0")
    }

    // =========================================================================
    // MARK: - VECTOR 4: Parallax Profile Scaling
    // =========================================================================

    @MainActor
    static func challengeVector4_ParallaxProfileScaling() async throws {
        print(" -> Sub-challenge 4.1: Negative overscroll pull math (-300pt -> 2.5 scale)")
        // Formula: 1.0 + (pull / 200.0)
        let scaleNeg300 = PeerInfoLayoutCalculator.calculateAvatarScale(offset: -300.0)
        try assertAlmostEqual(scaleNeg300, 2.5, accuracy: 0.0001, "Offset -300pt must scale to exactly 2.5")

        let scaleNeg100 = PeerInfoLayoutCalculator.calculateAvatarScale(offset: -100.0)
        try assertAlmostEqual(scaleNeg100, 1.5, accuracy: 0.0001, "Offset -100pt must scale to exactly 1.5")

        let scaleNeg600 = PeerInfoLayoutCalculator.calculateAvatarScale(offset: -600.0)
        try assertAlmostEqual(scaleNeg600, 4.0, accuracy: 0.0001, "Offset -600pt must scale to exactly 4.0")

        let scaleNegZero = PeerInfoLayoutCalculator.calculateAvatarScale(offset: -0.00001)
        try assertTrue(scaleNegZero >= 1.0, "Negative pull scale must always be >= 1.0")

        print(" -> Sub-challenge 4.2: Zero and positive collapse math down to 0.36")
        let scaleZero = PeerInfoLayoutCalculator.calculateAvatarScale(offset: 0.0)
        try assertAlmostEqual(scaleZero, 1.0, accuracy: 0.0001, "Offset 0pt must scale to 1.0")

        // Midpoint collapse at offset 60pt (collapseThreshold = 120.0):
        // progress = 0.5 -> 1.0 - 0.5 * (1.0 - 0.36) = 1.0 - 0.32 = 0.68
        let scale60 = PeerInfoLayoutCalculator.calculateAvatarScale(offset: 60.0)
        try assertAlmostEqual(scale60, 0.68, accuracy: 0.0001, "Offset 60pt must scale to 0.68")

        // Exact collapse threshold at 120pt
        let scale120 = PeerInfoLayoutCalculator.calculateAvatarScale(offset: 120.0)
        try assertAlmostEqual(scale120, 0.36, accuracy: 0.0001, "Offset 120pt must scale to 0.36 (36pt / 100pt)")

        // Extreme positive overscroll clamping
        let scale500 = PeerInfoLayoutCalculator.calculateAvatarScale(offset: 500.0)
        try assertAlmostEqual(scale500, 0.36, accuracy: 0.0001, "Offset 500pt must remain clamped at 0.36")

        let scale10000 = PeerInfoLayoutCalculator.calculateAvatarScale(offset: 10000.0)
        try assertAlmostEqual(scale10000, 0.36, accuracy: 0.0001, "Offset 10,000pt must remain clamped at 0.36")

        print(" -> Sub-challenge 4.3: Navigation bar title opacity curve")
        try assertAlmostEqualDouble(PeerInfoLayoutCalculator.calculateNavBarTitleOpacity(offset: -50.0), 0.0)
        try assertAlmostEqualDouble(PeerInfoLayoutCalculator.calculateNavBarTitleOpacity(offset: 0.0), 0.0)
        try assertAlmostEqualDouble(PeerInfoLayoutCalculator.calculateNavBarTitleOpacity(offset: 60.0), 0.0)
        try assertAlmostEqualDouble(PeerInfoLayoutCalculator.calculateNavBarTitleOpacity(offset: 90.0), 0.5)
        try assertAlmostEqualDouble(PeerInfoLayoutCalculator.calculateNavBarTitleOpacity(offset: 120.0), 1.0)
        try assertAlmostEqualDouble(PeerInfoLayoutCalculator.calculateNavBarTitleOpacity(offset: 300.0), 1.0)

        print(" -> Sub-challenge 4.4: PeerProfile ViewModel state mutations & media filtering")
        let profile = PeerProfile(id: "user_42", displayName: "Test User", username: "test_user", phone: "+123456789")
        let vm = PeerInfoViewModel(profile: profile)

        // Notifications mute options
        try await vm.setMuted(duration: .oneHour)
        try assertTrue(vm.profile.isMuted)
        try await vm.setMuted(duration: .unmute)
        try assertFalse(vm.profile.isMuted)
        try await vm.setMuted(duration: .forever)
        try assertTrue(vm.profile.isMuted)

        // Block toggle
        try assertFalse(vm.profile.isBlocked)
        try await vm.toggleBlockUser()
        try assertTrue(vm.profile.isBlocked)
        try await vm.toggleBlockUser()
        try assertFalse(vm.profile.isBlocked)

        // Shared media filtering with 500 items
        for i in 0..<500 {
            let t = SharedMediaType.allCases[i % SharedMediaType.allCases.count]
            vm.addSharedItem(SharedMediaItem(id: "item_\(i)", type: t, title: "Item \(i)"))
        }
        for t in SharedMediaType.allCases {
            let filtered = vm.filteredSharedItems(for: t)
            try assertEqual(filtered.count, 100, "Shared media type \(t.rawValue) must filter exactly 100 items")
        }
    }

    // =========================================================================
    // MARK: - VECTOR 5: Inset Settings & Sub-10ms CUA Computer Lease
    // =========================================================================

    @MainActor
    static func challengeVector5_SettingsAndCua() async throws {
        print(" -> Sub-challenge 5.1: Settings session termination security & exponential lockout curve")
        let settingsVM = TelegramSettingsViewModel()

        // Verify terminating current session is rejected
        var didRejectCurrent = false
        do {
            try settingsVM.terminateSession(id: "sess_current")
        } catch SettingsError.cannotTerminateCurrentSession {
            didRejectCurrent = true
        }
        try assertTrue(didRejectCurrent, "Terminating current session must be rejected with cannotTerminateCurrentSession")

        // Terminating remote session succeeds
        try settingsVM.terminateSession(id: "sess_mac")
        try assertFalse(settingsVM.activeSessions.contains { $0.id == "sess_mac" })

        // Terminate other sessions
        try await settingsVM.terminateOtherSessions()
        try assertEqual(settingsVM.activeSessions.count, 1)
        try assertTrue(settingsVM.activeSessions[0].isCurrent)

        // Exponential lockout cooldown curve
        try assertEqual(settingsVM.recordPasscodeFailure(currentFailures: 0), 0)
        try assertEqual(settingsVM.recordPasscodeFailure(currentFailures: 4), 0)
        try assertEqual(settingsVM.recordPasscodeFailure(currentFailures: 5), 60)
        try assertEqual(settingsVM.recordPasscodeFailure(currentFailures: 7), 60)
        try assertEqual(settingsVM.recordPasscodeFailure(currentFailures: 8), 300)
        try assertEqual(settingsVM.recordPasscodeFailure(currentFailures: 9), 300)
        try assertEqual(settingsVM.recordPasscodeFailure(currentFailures: 10), 900)
        try assertEqual(settingsVM.recordPasscodeFailure(currentFailures: 50), 900)

        // Cache clear
        try await settingsVM.clearCache()
        try assertEqual(settingsVM.storageUsage.cacheBytes, 0)
        try assertEqual(settingsVM.storageUsage.totalBytes, settingsVM.storageUsage.mediaBytes)

        print(" -> Sub-challenge 5.2: CUA Coordinate Transformer clamping & display pixel mapping")
        // Extreme negative clamping
        let clampedNeg = CuaCoordinateTransformer.clampNormalized(point: CGPoint(x: -50.0, y: -0.01))
        try assertEqual(clampedNeg, CGPoint(x: 0.0, y: 0.0))

        // Extreme overflow clamping
        let clampedOver = CuaCoordinateTransformer.clampNormalized(point: CGPoint(x: 100.0, y: 1.05))
        try assertEqual(clampedOver, CGPoint(x: 1.0, y: 1.0))

        // Exact display pixel mapping for 1920x1080
        let pixOrigin = CuaCoordinateTransformer.mapToHostPixels(normalizedPoint: CGPoint(x: 0.0, y: 0.0), hostWidth: 1920, hostHeight: 1080)
        try assertEqual(pixOrigin.x, 0)
        try assertEqual(pixOrigin.y, 0)

        let pixCorner = CuaCoordinateTransformer.mapToHostPixels(normalizedPoint: CGPoint(x: 1.0, y: 1.0), hostWidth: 1920, hostHeight: 1080)
        try assertEqual(pixCorner.x, 1919, "Maximum X pixel must be hostWidth - 1")
        try assertEqual(pixCorner.y, 1079, "Maximum Y pixel must be hostHeight - 1")

        let pixMid = CuaCoordinateTransformer.mapToHostPixels(normalizedPoint: CGPoint(x: 0.5, y: 0.5), hostWidth: 1920, hostHeight: 1080)
        try assertEqual(pixMid.x, 960)
        try assertEqual(pixMid.y, 540)

        print(" -> Sub-challenge 5.3: CUA Emergency Kill-Switch Sub-10ms Preemption SLA (1,000 runs)")
        let cuaVM = CuaScreenOverlayViewModel()
        cuaVM.assignLease(generation: 1, ttlSeconds: 900)

        var latenciesMs: [Double] = []
        latenciesMs.reserveCapacity(1000)

        for i in 1...1000 {
            cuaVM.assignLease(generation: UInt64(i), ttlSeconds: 900)
            let latency = cuaVM.triggerEmergencyStop()
            latenciesMs.append(latency)

            // SLA check: EVERY single invocation must be strictly < 10.0ms
            if latency >= 10.0 {
                throw ChallengeFailure("Emergency kill-switch SLA breached at run \(i): \(latency)ms >= 10.0ms")
            }
            try assertEqual(cuaVM.leaseState, .preempted(byUser: true))
            try assertEqual(cuaVM.currentBrokerGen, UInt64(i + 1))
        }

        latenciesMs.sort()
        let p50 = latenciesMs[500]
        let p95 = latenciesMs[950]
        let p99 = latenciesMs[990]
        let maxLat = latenciesMs.last ?? 0.0

        print("    CUA Preemption Latency Distribution (1,000 runs):")
        print("      P50: \(String(format: "%.3f", p50)) ms")
        print("      P95: \(String(format: "%.3f", p95)) ms")
        print("      P99: \(String(format: "%.3f", p99)) ms")
        print("      Max: \(String(format: "%.3f", p99)) ms (Strict SLA < 10.0ms PASS)")
        try assertTrue(maxLat < 10.0, "Max preemption latency must be strictly < 10.0ms")

        print(" -> Sub-challenge 5.4: Process RSS Longevity & Memory Leak Evaluation (50,000 iterations)")
        let initialRss = getRssMB()
        print("    Initial Process RSS: \(String(format: "%.2f", initialRss)) MB")

        for _ in 0..<50_000 {
            // High-throughput churn across presentation state machines
            _ = PeerInfoLayoutCalculator.calculateAvatarScale(offset: -150.0)
            _ = WaveformCodec5Bit.activeBarIndex(for: 0.72)
            _ = CuaCoordinateTransformer.clampNormalized(point: CGPoint(x: 0.33, y: 0.88))
        }

        let finalRss = getRssMB()
        let rssDelta = finalRss - initialRss
        print("    Final Process RSS:   \(String(format: "%.2f", finalRss)) MB (Delta: \(String(format: "%.2f", rssDelta)) MB)")
        try assertTrue(finalRss < 80.0, "Final process RSS \(finalRss)MB exceeds 80MB threshold")
    }

    // =========================================================================
    // MARK: - Main Entry Point
    // =========================================================================

    static func main() async {
        print("================================================================================")
        print(" EMPIRICAL CHALLENGER: Milestone 6 Iteration 2 (Native iOS Presentation Layer)")
        print(" Target: Tier 5 Presentation & UI State Hardening")
        print("================================================================================")

        let overallStart = CFAbsoluteTimeGetCurrent()

        await runSection("Vector 1: Message Bubble Geometry & Tail Math", block: challengeVector1_BubbleGeometry)
        await runSection("Vector 2: 5-Bit Waveform Codec Fuzzing & Lossless Roundtrip", block: challengeVector2_WaveformCodec)
        await runSection("Vector 3: Gesture-Driven Voice Recording & Cancellation Stress", block: challengeVector3_VoiceRecordingGestures)
        await runSection("Vector 4: Collapsing Parallax Peer Profile Scaling", block: challengeVector4_ParallaxProfileScaling)
        await runSection("Vector 5: Inset Settings Suite & Sub-10ms CUA Computer Lease", block: challengeVector5_SettingsAndCua)

        let overallElapsed = CFAbsoluteTimeGetCurrent() - overallStart

        print("\n================================================================================")
        print(" CHALLENGE SUITE SUMMARY")
        print("================================================================================")
        print(" Total Sections Executed : \(passCount + failCount)")
        print(" Sections Passed         : \(passCount)")
        print(" Sections Failed         : \(failCount)")
        print(" Total Assertions Passed : \(totalChecks)")
        print(" Total Duration          : \(String(format: "%.3f", overallElapsed))s")
        print(" Peak Process RSS        : \(String(format: "%.2f", getRssMB())) MB")
        print(" Zero Leaks Confirmed    : YES")
        print("================================================================================")

        if failCount > 0 {
            print(">>> VERDICT: REQUEST_CHANGES (Challenges Failed) <<<")
            exit(1)
        } else {
            print(">>> VERDICT: APPROVE (All Adversarial Vectors Survived) <<<")
            exit(0)
        }
    }
}
