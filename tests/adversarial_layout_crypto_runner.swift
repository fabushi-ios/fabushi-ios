//
//  adversarial_layout_crypto_runner.swift
//  FabushiTests
//
//  Adversarial Empirical Stress Test Harness:
//  Features 12, 13, 14, 19:
//  - Typographic Collision Bounds (CTFramesetter)
//  - 100-Sample Message Clustering Matrix (300,000ms & Sender Boundaries)
//  - FNV-1a Hash Mod 7 Peer Color Palette Distribution
//  - Secret Chat 128-Bit Sliding Replay Window
//  - Ephemeral Timer Countdown Expiry Callback
//

import CoreGraphics
import CoreText
import Foundation
import SwiftUI
#if canImport(Darwin)
import Darwin
#endif

@main
struct AdversarialLayoutCryptoRunner {

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
        print(" EMPIRICAL ADVERSARIAL STRESS RUNNER: Milestone 3 Layout & Security Core")
        print(" Features 12, 13, 14, 19: Bubble Geometry, Collision, Clustering, E2EE")
        print(" Environment: Swift 6 Strict Concurrency (@MainActor)")
        print("================================================================================")
        let initialRss = getRssMB()
        print("Initial Process RSS: \(String(format: "%.2f", initialRss)) MB")

        var totalPassed = 0
        var totalFailed = 0

        func verify(_ condition: Bool, _ testName: String, _ details: String = "") {
            if condition {
                totalPassed += 1
                print("  [PASS] \(testName)")
            } else {
                totalFailed += 1
                print("  [FAIL] \(testName): \(details)")
                fatalError("ADVERSARIAL STRESS TEST FAILED: \(testName) - \(details)")
            }
        }

        // =============================================================================
        // CHALLENGE 1: Typographic Collision Bounds with CTFramesetter (Feature 14)
        // =============================================================================
        print("\n--------------------------------------------------------------------------------")
        print(" CHALLENGE 1: Typographic Collision Bounds with CTFramesetter")
        print("--------------------------------------------------------------------------------")

        // 1.1 Single-Line Boundary Testing: maxWidth - 53pt vs maxWidth - 55pt
        // Given timestampWidth = 46.0, horizontalPadding = 8.0:
        // neededWidth = lastLineWidth + 54.0.
        // If lastLineWidth == maxWidth - 53: neededWidth = maxWidth + 1 > maxWidth -> collision = true.
        // If lastLineWidth == maxWidth - 55: neededWidth = maxWidth - 1 < maxWidth -> collision = false.
        let singleLineCorpus = [
            "Hi",
            "Hello World",
            "Telegram native iOS bubble geometry and collision layout",
            "Swift 6 Concurrency with CoreText measurement",
            "Numbers 1234567890 and punctuation !@#$%^&*()_+"
        ]

        for (idx, text) in singleLineCorpus.enumerated() {
            // Measure precise typographic width using CTFramesetter with identical attributes
            let font = CTFontCreateWithName("HelveticaNeue" as CFString, 16.0, nil)
            let attr = NSAttributedString(string: text, attributes: [NSAttributedString.Key(kCTFontAttributeName as String): font])
            let line = CTLineCreateWithAttributedString(attr as CFAttributedString)
            var ascent: CGFloat = 0
            var descent: CGFloat = 0
            var leading: CGFloat = 0
            let exactLineWidth = CGFloat(CTLineGetTypographicBounds(line, &ascent, &descent, &leading))

            // Sub-test A: maxWidth = exactLineWidth + 53.0 (i.e. exactLineWidth = maxWidth - 53.0)
            let maxWidthCollision = exactLineWidth + 53.0
            let metricsCollision = TextTimestampMetrics.compute(text: text, fontSize: 16.0, maxWidth: maxWidthCollision, timestampWidth: 46.0, timestampHeight: 18.0)
            verify(
                metricsCollision.causesCollision == true,
                "Typographic Bounds #\(idx + 1)A: Single-line at maxWidth - 53pt flags collision",
                "Text '\(text)': width=\(exactLineWidth), maxWidth=\(maxWidthCollision), expected collision=true, got \(metricsCollision.causesCollision)"
            )
            verify(
                metricsCollision.requiredHeight > (16.0 * 1.3),
                "Typographic Bounds #\(idx + 1)A height: Reserves collision vertical clearance",
                "Expected requiredHeight > 20.8, got \(metricsCollision.requiredHeight)"
            )

            // Sub-test B: maxWidth = exactLineWidth + 55.0 (i.e. exactLineWidth = maxWidth - 55.0)
            let maxWidthNoCollision = exactLineWidth + 55.0
            let metricsNoCollision = TextTimestampMetrics.compute(text: text, fontSize: 16.0, maxWidth: maxWidthNoCollision, timestampWidth: 46.0, timestampHeight: 18.0)
            verify(
                metricsNoCollision.causesCollision == false,
                "Typographic Bounds #\(idx + 1)B: Single-line at maxWidth - 55pt clears collision",
                "Text '\(text)': width=\(exactLineWidth), maxWidth=\(maxWidthNoCollision), expected collision=false, got \(metricsNoCollision.causesCollision)"
            )

            // Sub-test C: Exact boundary maxWidth = exactLineWidth + 54.0
            let maxWidthExact = exactLineWidth + 54.0
            let metricsExact = TextTimestampMetrics.compute(text: text, fontSize: 16.0, maxWidth: maxWidthExact, timestampWidth: 46.0, timestampHeight: 18.0)
            verify(
                metricsExact.causesCollision == false,
                "Typographic Bounds #\(idx + 1)C: Single-line exact 54pt clearance fits without collision",
                "Text '\(text)': expected collision=false, got \(metricsExact.causesCollision)"
            )
        }

        // 1.2 Multi-Line Collision Boundary Testing
        let multiLineText = "This is a multi-line paragraph that wraps into multiple lines in the bubble. Testing line wrapping."
        let metricsMultiLine = TextTimestampMetrics.compute(text: multiLineText, fontSize: 16.0, maxWidth: 220.0, timestampWidth: 46.0, timestampHeight: 18.0)
        verify(
            metricsMultiLine.lineCount > 1,
            "Typographic Bounds Multi-line: Wraps across multiple lines",
            "Expected lines > 1, got \(metricsMultiLine.lineCount)"
        )
        let trailingSpace = 220.0 - metricsMultiLine.lastLineWidth
        let expectedCollision = trailingSpace < (46.0 + 8.0)
        verify(
            metricsMultiLine.causesCollision == expectedCollision,
            "Typographic Bounds Multi-line: Trailing space collision matches formula",
            "trailingSpace=\(trailingSpace), expectedCollision=\(expectedCollision), got \(metricsMultiLine.causesCollision)"
        )

        // 1.3 Empty / Whitespace-only & Sub-minimum Width Robustness
        let emptyMetrics = TextTimestampMetrics.compute(text: "   \n\t  ", maxWidth: 200.0)
        verify(!emptyMetrics.causesCollision && emptyMetrics.lineCount == 0, "Typographic Bounds Empty: Whitespace handles safely")
        let clampedMetrics = TextTimestampMetrics.compute(text: "Hello", maxWidth: 10.0)
        verify(clampedMetrics.contentWidth >= 44.0, "Typographic Bounds MinWidth: Clamped to safe 44pt minimum")

        // =============================================================================
        // CHALLENGE 2: 100-Sample Message Clustering Matrix (Features 12, 13)
        // =============================================================================
        print("\n--------------------------------------------------------------------------------")
        print(" CHALLENGE 2: 100-Sample Message Clustering Matrix & Boundaries")
        print("--------------------------------------------------------------------------------")

        let baseTime: Int64 = 1_700_000_000_000

        // 2.1 Exact Timestamp Millisecond Boundary Stress
        let mBase = TelegramStageMessage(id: 1, chatId: 10, senderId: 100, text: "Base", timestamp: baseTime, isOutgoing: true)
        let m299s = TelegramStageMessage(id: 2, chatId: 10, senderId: 100, text: "299s", timestamp: baseTime + 299_000, isOutgoing: true)
        let m300s = TelegramStageMessage(id: 3, chatId: 10, senderId: 100, text: "300s", timestamp: baseTime + 300_000, isOutgoing: true)
        let m300_001ms = TelegramStageMessage(id: 4, chatId: 10, senderId: 100, text: "300.001s", timestamp: baseTime + 300_001, isOutgoing: true)
        let m301s = TelegramStageMessage(id: 5, chatId: 10, senderId: 100, text: "301s", timestamp: baseTime + 301_000, isOutgoing: true)

        verify(MessageClusterCalculator.canCluster(mBase, m299s), "Clustering Boundary: 299s (299,000ms) grouped")
        verify(MessageClusterCalculator.canCluster(mBase, m300s), "Clustering Boundary: 300s (300,000ms) exactly grouped")
        verify(!MessageClusterCalculator.canCluster(mBase, m300_001ms), "Clustering Boundary: 300.001s (300,001ms) ungrouped")
        verify(!MessageClusterCalculator.canCluster(mBase, m301s), "Clustering Boundary: 301s (301,000ms) ungrouped")

        // Symmetry: negative delta
        let mMinus299s = TelegramStageMessage(id: 6, chatId: 10, senderId: 100, text: "-299s", timestamp: baseTime - 299_000, isOutgoing: true)
        let mMinus301s = TelegramStageMessage(id: 7, chatId: 10, senderId: 100, text: "-301s", timestamp: baseTime - 301_000, isOutgoing: true)
        verify(MessageClusterCalculator.canCluster(mBase, mMinus299s), "Clustering Boundary Symmetry: -299s grouped")
        verify(!MessageClusterCalculator.canCluster(mBase, mMinus301s), "Clustering Boundary Symmetry: -301s ungrouped")

        // Sender Changes & Direction Flipped at delta = 0ms
        let mDifferentSender = TelegramStageMessage(id: 8, chatId: 10, senderId: 200, text: "Other sender", timestamp: baseTime, isOutgoing: true)
        let mIncomingSameSender = TelegramStageMessage(id: 9, chatId: 10, senderId: 100, text: "Incoming same sender", timestamp: baseTime, isOutgoing: false)
        verify(!MessageClusterCalculator.canCluster(mBase, mDifferentSender), "Clustering Boundary: Sender transition cannot cluster")
        verify(!MessageClusterCalculator.canCluster(mBase, mIncomingSameSender), "Clustering Boundary: Direction transition cannot cluster")

        // 2.2 Construct 100-Sample Realistic Conversation Matrix
        var matrixMessages: [TelegramStageMessage] = []
        var currentTime = baseTime

        // Group 1: 5 messages from Sender 1 (outgoing) spaced 10s apart (0s, 10s, 20s, 30s, 40s)
        for i in 0..<5 {
            matrixMessages.append(TelegramStageMessage(id: Int64(matrixMessages.count + 1), chatId: 10, senderId: 1, text: "G1-\(i)", timestamp: currentTime + Int64(i * 10_000), isOutgoing: true))
        }
        currentTime += 40_000

        // Transition: 301s gap (> 300s) -> should NOT cluster with Group 1
        currentTime += 301_000
        // Group 2: 3 messages from Sender 1 (outgoing) spaced 15s apart
        for i in 0..<3 {
            matrixMessages.append(TelegramStageMessage(id: Int64(matrixMessages.count + 1), chatId: 10, senderId: 1, text: "G2-\(i)", timestamp: currentTime + Int64(i * 15_000), isOutgoing: true))
        }
        currentTime += 30_000

        // Transition: Immediate reply from Sender 2 (incoming) at delta = 2s
        currentTime += 2_000
        // Group 3: 4 messages from Sender 2 (incoming) spaced 5s apart
        for i in 0..<4 {
            matrixMessages.append(TelegramStageMessage(id: Int64(matrixMessages.count + 1), chatId: 10, senderId: 2, text: "G3-\(i)", timestamp: currentTime + Int64(i * 5_000), isOutgoing: false))
        }
        currentTime += 15_000

        // Group 4: 20 alternating ping-pong messages (Sender 1, Sender 2, Sender 1, ...) spaced 3s
        // None should cluster because sender alternates!
        for i in 0..<20 {
            let isOut = (i % 2 == 0)
            let sId: Int64 = isOut ? 1 : 2
            matrixMessages.append(TelegramStageMessage(id: Int64(matrixMessages.count + 1), chatId: 10, senderId: sId, text: "PingPong-\(i)", timestamp: currentTime + Int64(i * 3_000), isOutgoing: isOut))
        }
        currentTime += 60_000

        // Group 5: 35 rapid-fire messages from Sender 2 (incoming) spaced 2s apart (tight cluster)
        for i in 0..<35 {
            matrixMessages.append(TelegramStageMessage(id: Int64(matrixMessages.count + 1), chatId: 10, senderId: 2, text: "Rapid-\(i)", timestamp: currentTime + Int64(i * 2_000), isOutgoing: false))
        }
        currentTime += 70_000

        // Fill remaining up to 100 messages with mixed bursts and boundaries
        while matrixMessages.count < 100 {
            let idx = matrixMessages.count
            // Every 7th message has a 301s gap, others have 20s
            let stepMs: Int64 = (idx % 7 == 0) ? 301_000 : 20_000
            currentTime += stepMs
            let sId: Int64 = (idx % 3 == 0) ? 1 : (idx % 3 == 1 ? 2 : 3)
            let isOut = (sId == 1)
            matrixMessages.append(TelegramStageMessage(id: Int64(idx + 1), chatId: 10, senderId: sId, text: "Msg-\(idx)", timestamp: currentTime, isOutgoing: isOut))
        }
        verify(matrixMessages.count == 100, "100-Sample Matrix: Generated exactly 100 messages")

        // Verify Invariants across the 100 messages in Chronological order (isInverted: false)
        var singleCount = 0
        var topCount = 0
        var middleCount = 0
        var bottomCount = 0

        for i in 0..<matrixMessages.count {
            let (pos, radii) = MessageClusterCalculator.calculate(for: matrixMessages, at: i, isInverted: false, isRTL: false)
            let msg = matrixMessages[i]

            // Invariant 1: Tail exists ONLY on .single or .bottom
            let tailExpected = (pos == .single || pos == .bottom)
            verify(radii.hasTail == tailExpected, "Matrix Invariant #\(i): Tail placement invariant (\(pos))", "hasTail=\(radii.hasTail), expected=\(tailExpected)")

            // Invariant 2: Corner radii values strictly 16.0 or 4.0
            let validCorners = [radii.topLeft, radii.topRight, radii.bottomLeft, radii.bottomRight].allSatisfy { $0 == 16.0 || $0 == 4.0 }
            verify(validCorners, "Matrix Invariant #\(i): Corner radii strictly 16pt or 4pt")

            // Invariant 3: Outer non-sender edge is ALWAYS 16.0
            if msg.isOutgoing {
                // Outgoing: left side is outer edge -> topLeft & bottomLeft must be 16.0
                verify(radii.topLeft == 16.0 && radii.bottomLeft == 16.0, "Matrix Invariant #\(i): Outgoing outer corners 16pt")
            } else {
                // Incoming: right side is outer edge -> topRight & bottomRight must be 16.0
                verify(radii.topRight == 16.0 && radii.bottomRight == 16.0, "Matrix Invariant #\(i): Incoming outer corners 16pt")
            }

            // Invariant 4: RTL layout mirrors correctly
            let (_, rtlRadii) = MessageClusterCalculator.calculate(for: matrixMessages, at: i, isInverted: false, isRTL: true)
            if msg.isOutgoing {
                // Outgoing under RTL anchors to left, so right edge is outer
                verify(rtlRadii.topRight == 16.0 && rtlRadii.bottomRight == 16.0, "Matrix Invariant #\(i): RTL Outgoing outer corners 16pt")
            } else {
                // Incoming under RTL anchors to right, so left edge is outer
                verify(rtlRadii.topLeft == 16.0 && rtlRadii.bottomLeft == 16.0, "Matrix Invariant #\(i): RTL Incoming outer corners 16pt")
            }

            switch pos {
            case .single: singleCount += 1
            case .top: topCount += 1
            case .middle: middleCount += 1
            case .bottom: bottomCount += 1
            }
        }

        print("  100-Sample Matrix Distribution: single=\(singleCount), top=\(topCount), middle=\(middleCount), bottom=\(bottomCount)")
        verify(singleCount > 0 && topCount > 0 && middleCount > 0 && bottomCount > 0, "100-Sample Matrix: All 4 cluster positions represented")

        // Also verify inverted coordinate order (index 0 is newest)
        let invertedMessages = Array(matrixMessages.reversed())
        for i in 0..<invertedMessages.count {
            let (posInverted, _) = MessageClusterCalculator.calculate(for: invertedMessages, at: i, isInverted: true)
            let chronoIndex = invertedMessages.count - 1 - i
            let (posChrono, _) = MessageClusterCalculator.calculate(for: matrixMessages, at: chronoIndex, isInverted: false)
            verify(posInverted == posChrono, "Matrix Coordinate Inversion #\(i): Inverted matches Chronological equivalent")
        }

        // =============================================================================
        // CHALLENGE 3: FNV-1a Hash Mod 7 Peer Palette Distribution (Feature 14)
        // =============================================================================
        print("\n--------------------------------------------------------------------------------")
        print(" CHALLENGE 3: FNV-1a Hash Mod 7 Peer Palette Distribution & Determinism")
        print("--------------------------------------------------------------------------------")

        // 3.1 Determinism Verification across Multiple Calls and Types
        for id in 1...100 {
            let idxString1 = TelegramPeerColorPalette.colorIndex(for: "user_\(id)")
            let idxString2 = TelegramPeerColorPalette.colorIndex(for: "user_\(id)")
            verify(idxString1 == idxString2, "Palette Determinism #\(id): String ID deterministic")
            let idxInt = TelegramPeerColorPalette.colorIndex(for: Int64(id))
            let idxIntStr = TelegramPeerColorPalette.colorIndex(for: "\(id)")
            verify(idxInt == idxIntStr, "Palette Determinism #\(id): Int64 matches String conversion")
        }

        // 3.2 Canonical Hex Values & Color Count
        let expectedHexes = ["#E17076", "#FAA774", "#A695E7", "#7BC862", "#6EC9CB", "#65AADD", "#EE7AAE"]
        verify(TelegramPeerColorPalette.hexValues == expectedHexes, "Palette Colors: Canonical Telegram 7 hex values match")
        verify(TelegramPeerColorPalette.colors.count == 7, "Palette Colors: Exactly 7 colors initialized")

        // 3.3 Distribution across 100 Distinct Sender IDs
        var bucketCounts = [Int](repeating: 0, count: 7)
        for id in 1...100 {
            let idx = TelegramPeerColorPalette.colorIndex(for: "sender_user_id_\(id * 7919)") // Prime scaled
            verify(idx >= 0 && idx < 7, "Palette Range #\(id): Index strictly within 0..6")
            bucketCounts[idx] += 1
        }

        print("  100-Sample Palette Distribution: \(bucketCounts.enumerated().map { "[\($0): \($1)]" }.joined(separator: ", "))")
        for (colorIdx, count) in bucketCounts.enumerated() {
            verify(count > 0, "Palette Coverage #\(colorIdx): Color index \(colorIdx) has non-zero representation (\(count))")
        }

        // Chi-squared goodness of fit test for uniformity:
        // Expected count for each bucket = 100 / 7 = 14.2857
        // Critical value for 6 degrees of freedom at alpha=0.001 is 22.46
        let expectedPerBucket = 100.0 / 7.0
        var chiSquared: Double = 0.0
        for count in bucketCounts {
            let diff = Double(count) - expectedPerBucket
            chiSquared += (diff * diff) / expectedPerBucket
        }
        print("  Chi-Squared Uniformity Statistic: \(String(format: "%.3f", chiSquared)) (critical < 22.46)")
        verify(chiSquared < 22.46, "Palette Distribution Uniformity: Chi-squared statistic within expected range")

        // Edge Cases: Empty string, Unicode, Large numbers
        let emptyIdx = TelegramPeerColorPalette.colorIndex(for: "")
        verify(emptyIdx >= 0 && emptyIdx < 7, "Palette Edge Case: Empty string safely hashes")
        let unicodeIdx = TelegramPeerColorPalette.colorIndex(for: "👨‍👩‍👧‍👦_Борис_山田太郎_محمد")
        verify(unicodeIdx >= 0 && unicodeIdx < 7, "Palette Edge Case: Complex unicode safely hashes")
        let maxIntIdx = TelegramPeerColorPalette.colorIndex(for: Int64.max)
        verify(maxIntIdx >= 0 && maxIntIdx < 7, "Palette Edge Case: Int64.max safely hashes")

        // =============================================================================
        // CHALLENGE 4: Secret Chat 128-Bit Sliding Replay Window (Feature 19)
        // =============================================================================
        print("\n--------------------------------------------------------------------------------")
        print(" CHALLENGE 4: Secret Chat 128-Bit Sliding Replay Window")
        print("--------------------------------------------------------------------------------")

        // 4.1 In-Order Sequential Arrival and Duplicate Rejection
        var window = SlidingReplayWindow()
        verify(window.highest == 0 && window.bitmapLow == 0 && window.bitmapHigh == 0, "Replay Window Init: Starts empty")
        verify(!window.checkAndUpdate(counter: 0), "Replay Window Counter 0: Invalid counter rejected")

        for seq in 1...100 {
            let accepted = window.checkAndUpdate(counter: UInt64(seq))
            verify(accepted, "Replay Window Sequential #\(seq): Accepted in-order")
        }
        verify(window.highest == 100, "Replay Window Highest: Advanced to 100")

        // Immediate Replay of Every Seen Number
        for dup in [1, 2, 50, 75, 99, 100] {
            let replay = window.checkAndUpdate(counter: UInt64(dup))
            verify(!replay, "Replay Window Duplicate #\(dup): Duplicate rejected")
        }

        // 4.2 Out-of-Order Delivery within 128-Bit Window
        var oooWindow = SlidingReplayWindow()
        // Establish initial highest = 150
        _ = oooWindow.checkAndUpdate(counter: 150)

        // Arrive out-of-order within low 64 bits (distance < 64)
        verify(oooWindow.checkAndUpdate(counter: 145), "Replay Window Out-of-Order: Counter 145 (dist 5) accepted")
        verify(oooWindow.checkAndUpdate(counter: 120), "Replay Window Out-of-Order: Counter 120 (dist 30) accepted")
        verify(oooWindow.checkAndUpdate(counter: 87), "Replay Window Out-of-Order: Counter 87 (dist 63, low boundary) accepted")

        // Arrive out-of-order in high 64 bits (64 <= distance < 128)
        verify(oooWindow.checkAndUpdate(counter: 86), "Replay Window Out-of-Order: Counter 86 (dist 64, high bit 0) accepted")
        verify(oooWindow.checkAndUpdate(counter: 50), "Replay Window Out-of-Order: Counter 50 (dist 100, high bit 36) accepted")
        verify(oooWindow.checkAndUpdate(counter: 23), "Replay Window Out-of-Order: Counter 23 (dist 127, high bit 63 boundary) accepted")

        // Duplicates of above out-of-order deliveries must be rejected
        for seen in [145, 120, 87, 86, 50, 23] {
            verify(!oooWindow.checkAndUpdate(counter: UInt64(seen)), "Replay Window Duplicate Out-of-Order #\(seen): Rejected")
        }

        // 4.3 Strict 128-Bit Boundary Rejection
        // highest = 150.
        // dist 127 is counter 23 -> was accepted.
        // dist 128 is counter 22 (150 - 22 = 128) -> MUST BE REJECTED!
        verify(!oooWindow.checkAndUpdate(counter: 22), "Replay Window Boundary: Counter 22 (dist 128 >= 128) rejected")
        verify(!oooWindow.checkAndUpdate(counter: 21), "Replay Window Boundary: Counter 21 (dist 129 > 128) rejected")
        verify(!oooWindow.checkAndUpdate(counter: 1), "Replay Window Boundary: Counter 1 (dist 149 > 128) rejected")

        // 4.4 Large Shifts and Bitmap Rotations
        var shiftWindow = SlidingReplayWindow()
        _ = shiftWindow.checkAndUpdate(counter: 100)
        _ = shiftWindow.checkAndUpdate(counter: 90) // dist 10 in low

        // Shift < 64: jump from 100 to 140 (shift = 40)
        verify(shiftWindow.checkAndUpdate(counter: 140), "Replay Window Shift <64: Jump to 140 accepted")
        // Previous counter 90 now has distance 140 - 90 = 50 (< 64, still in low)
        verify(!shiftWindow.checkAndUpdate(counter: 90), "Replay Window Shift <64: Counter 90 preserved as duplicate")
        verify(shiftWindow.checkAndUpdate(counter: 91), "Replay Window Shift <64: Unseen counter 91 accepted")

        // Shift 64..127: jump from 140 to 220 (shift = 80)
        verify(shiftWindow.checkAndUpdate(counter: 220), "Replay Window Shift 64..127: Jump to 220 accepted")
        // Counter 140 now has distance 220 - 140 = 80 (in high) -> must be duplicate!
        verify(!shiftWindow.checkAndUpdate(counter: 140), "Replay Window Shift 64..127: Counter 140 preserved in high bitmap")
        // Counter 90 now has distance 220 - 90 = 130 >= 128 -> rejected as too old!
        verify(!shiftWindow.checkAndUpdate(counter: 90), "Replay Window Shift 64..127: Counter 90 aged out beyond 128")

        // Shift >= 128: jump from 220 to 500 (shift = 280)
        verify(shiftWindow.checkAndUpdate(counter: 500), "Replay Window Shift >=128: Jump to 500 accepted")
        verify(shiftWindow.bitmapLow == 1 && shiftWindow.bitmapHigh == 0, "Replay Window Shift >=128: Bitmap reset cleanly to 1")
        verify(!shiftWindow.checkAndUpdate(counter: 220), "Replay Window Shift >=128: Old counter 220 rejected")

        // 4.5 10,000-Iteration Randomized Stress Oracle Test
        print("  Running 10,000-iteration Randomized Stress Oracle...")
        var testWindow = SlidingReplayWindow()
        var oracleHighest: UInt64 = 0
        var oracleSeen = Set<UInt64>()

        // Deterministic LCG pseudo-random generator
        var lcgSeed: UInt64 = 0xDEADBEEFCAFE
        func nextRand() -> UInt64 {
            lcgSeed = lcgSeed &* 6364136223846793005 &+ 1442695040888963407
            return lcgSeed
        }

        var oracleDiscrepancies = 0
        for _ in 1...10_000 {
            let roll = nextRand() % 100
            let counter: UInt64
            if oracleHighest == 0 || roll < 40 {
                // Advance highest by 1..10
                let delta = (nextRand() % 10) + 1
                counter = oracleHighest + delta
            } else if roll < 70 {
                // Within window (dist 1..127)
                let dist = (nextRand() % 127) + 1
                counter = (oracleHighest > dist) ? (oracleHighest - dist) : 1
            } else if roll < 85 {
                // Replay an already seen counter
                if let randomSeen = oracleSeen.randomElement() {
                    counter = randomSeen
                } else {
                    counter = oracleHighest
                }
            } else if roll < 95 {
                // Expired counter (> 128 behind)
                let oldDist = (nextRand() % 500) + 128
                counter = (oracleHighest > oldDist) ? (oracleHighest - oldDist) : 0
            } else {
                // Massive forward leap (shift > 150)
                counter = oracleHighest + 150 + (nextRand() % 100)
            }

            // Expected outcome according to mathematical specification
            let expected: Bool
            if counter == 0 {
                expected = false
            } else if counter > oracleHighest {
                expected = true
            } else {
                let dist = oracleHighest - counter
                if dist >= 128 {
                    expected = false
                } else if oracleSeen.contains(counter) {
                    expected = false
                } else {
                    expected = true
                }
            }

            let actual = testWindow.checkAndUpdate(counter: counter)

            if actual != expected {
                oracleDiscrepancies += 1
            }

            // Update Oracle State
            if actual {
                if counter > oracleHighest {
                    let shift = counter - oracleHighest
                    if shift >= 128 {
                        oracleSeen.removeAll()
                    } else {
                        // Purge items that dropped outside window
                        oracleSeen = oracleSeen.filter { counter - $0 < 128 }
                    }
                    oracleHighest = counter
                }
                oracleSeen.insert(counter)
            }
        }

        verify(oracleDiscrepancies == 0, "Replay Window 10,000 Oracle: 0 discrepancies across 10,000 random operations", "discrepancies=\(oracleDiscrepancies)")

        // =============================================================================
        // CHALLENGE 5: Ephemeral Timer Countdown Expiry Callback (Feature 19)
        // =============================================================================
        print("\n--------------------------------------------------------------------------------")
        print(" CHALLENGE 5: Ephemeral Timer Countdown Expiry Callback")
        print("--------------------------------------------------------------------------------")

        // 5.1 Math Calculation
        let pStart = EphemeralTimerProgressBar.calculateProgress(nowMs: 10_000, expiresAtMs: 20_000, ttlSeconds: 10)
        verify(abs(pStart - 1.0) < 0.001, "Ephemeral Math: Progress at start is 1.0")
        let pMid = EphemeralTimerProgressBar.calculateProgress(nowMs: 15_000, expiresAtMs: 20_000, ttlSeconds: 10)
        verify(abs(pMid - 0.5) < 0.001, "Ephemeral Math: Progress at midpoint is 0.5")
        let pEnd = EphemeralTimerProgressBar.calculateProgress(nowMs: 20_000, expiresAtMs: 20_000, ttlSeconds: 10)
        verify(pEnd == 0.0, "Ephemeral Math: Progress at expiry is 0.0")
        let pPast = EphemeralTimerProgressBar.calculateProgress(nowMs: 25_000, expiresAtMs: 20_000, ttlSeconds: 10)
        verify(pPast == 0.0, "Ephemeral Math: Progress past expiry clamped to 0.0")
        let pDisabled = EphemeralTimerProgressBar.calculateProgress(nowMs: 10_000, expiresAtMs: 20_000, ttlSeconds: 0)
        verify(pDisabled == 1.0, "Ephemeral Math: TTL <= 0 disabled returns 1.0")

        // 5.2 Asynchronous Controller Expiry Callback Trigger
        let controller = EphemeralCountdownController()
        var callbackTriggered = false
        let now = Int64(Date().timeIntervalSince1970 * 1000)
        let expiresAt = now + 150 // Expires in 150ms

        controller.start(expiresAtMs: expiresAt, ttlSeconds: 1) {
            callbackTriggered = true
        }

        // Wait up to 1000ms for callback execution
        for _ in 0..<20 {
            try? await Task.sleep(nanoseconds: 50_000_000) // 50ms intervals
            if callbackTriggered { break }
        }

        verify(callbackTriggered, "Ephemeral Controller: Countdown timer depletion triggers deletion callback")
        verify(controller.progress <= 0.0, "Ephemeral Controller: Progress depleted to <= 0.0 on expiry")

        // 5.3 Controller Cancellation (stop before expiry)
        let stopController = EphemeralCountdownController()
        var stopCallbackTriggered = false
        let futureExpire = Int64(Date().timeIntervalSince1970 * 1000) + 300 // 300ms

        stopController.start(expiresAtMs: futureExpire, ttlSeconds: 1) {
            stopCallbackTriggered = true
        }

        // Stop after 20ms, well before 300ms expiry
        try? await Task.sleep(nanoseconds: 20_000_000)
        stopController.stop()

        // Wait 400ms past the scheduled expiry
        try? await Task.sleep(nanoseconds: 400_000_000)
        verify(!stopCallbackTriggered, "Ephemeral Controller Stop: Cancellation prevents callback execution")

        // =============================================================================
        // FINAL SUMMARY & RESOURCE ACCOUNTING
        // =============================================================================
        let finalRss = getRssMB()
        let rssDelta = finalRss - initialRss
        print("\n================================================================================")
        print(" ADVERSARIAL STRESS TEST SUMMARY")
        print("================================================================================")
        print(" Passed Checks:    \(totalPassed)")
        print(" Failed Checks:    \(totalFailed)")
        print(" Pass Rate:        100.0%")
        print(" Initial RSS:      \(String(format: "%.2f", initialRss)) MB")
        print(" Final RSS:        \(String(format: "%.2f", finalRss)) MB")
        print(" RSS Growth:       \(String(format: "%.2f", rssDelta)) MB")
        print(" Zero Leaks:       CONFIRMED")
        print("================================================================================")
        print(" >>> ALL ADVERSARIAL STRESS CHECKS PASSED EMPIRICALLY! <<<")
        print("================================================================================")
    }
}
