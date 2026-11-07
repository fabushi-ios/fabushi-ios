//
// tests/adversarial_ffi/test_replay_window_swift.swift
// Native Swift Concurrency & Lifecycle Challenger for TelegramReplayWindow
//

import Foundation

@main
struct SwiftReplayWindowChallenger {
    static func main() async throws {
        print("===========================================================================")
        print(" NATIVE SWIFT CHALLENGER: TelegramReplayWindow Concurrency & Lifecycle")
        print("===========================================================================")

        // 1. Basic lifecycle & boundaries
        let window = TelegramReplayWindow()

        // Seq 0 must be rejected
        guard !window.checkAndAdd(seq: 0) else {
            fatalError("FAIL: Seq 0 was accepted")
        }
        print("  [PASS] Swift: Seq 0 rejected")

        // Seq 1..128
        for s: UInt64 in 1...128 {
            guard window.checkAndAdd(seq: s) else {
                fatalError("FAIL: Seq \(s) rejected")
            }
        }
        print("  [PASS] Swift: Seq 1..128 admitted")

        // Duplicates 1..128
        for s: UInt64 in 1...128 {
            guard !window.checkAndAdd(seq: s) else {
                fatalError("FAIL: Duplicate seq \(s) accepted")
            }
        }
        print("  [PASS] Swift: Duplicates 1..128 rejected")

        // Slide window by adding 129
        guard window.checkAndAdd(seq: 129) else {
            fatalError("FAIL: Seq 129 rejected")
        }
        // Seq 1 is now stale (> 128)
        guard !window.checkAndAdd(seq: 1) else {
            fatalError("FAIL: Stale seq 1 accepted")
        }
        print("  [PASS] Swift: Window slide and stale seq 1 rejected")

        // 2. Extreme UInt64.max boundaries
        let maxWindow = TelegramReplayWindow()
        let maxSeq = UInt64.max
        guard maxWindow.checkAndAdd(seq: maxSeq - 127) else {
            fatalError("FAIL: UInt64.max - 127 rejected")
        }
        guard maxWindow.checkAndAdd(seq: maxSeq) else {
            fatalError("FAIL: UInt64.max rejected")
        }
        guard !maxWindow.checkAndAdd(seq: maxSeq) else {
            fatalError("FAIL: UInt64.max duplicate accepted")
        }
        guard !maxWindow.checkAndAdd(seq: maxSeq - 128) else {
            fatalError("FAIL: UInt64.max - 128 stale accepted")
        }
        print("  [PASS] Swift: UInt64.max extreme boundary checks passed")

        // 3. Swift Concurrency TaskGroup Churn (10,000 instances created and deallocated via ARC)
        print("  [Execute] Allocating 10,000 TelegramReplayWindow instances across Swift TaskGroup...")
        let t0 = CFAbsoluteTimeGetCurrent()
        await withTaskGroup(of: Void.self) { group in
            for _ in 0..<8 {
                group.addTask {
                    for _ in 0..<1250 {
                        let w = TelegramReplayWindow()
                        _ = w.checkAndAdd(seq: 42)
                        _ = w.checkAndAdd(seq: 43)
                        // w deallocates here via ARC deinit -> telegram_replay_window_destroy
                    }
                }
            }
        }
        let elapsed = CFAbsoluteTimeGetCurrent() - t0
        print("  [PASS] Swift: 10,000 instances allocated and destroyed across 8 tasks in \(String(format: "%.3f", elapsed))s")

        print("===========================================================================")
        print(" >>> ALL SWIFT REPLAY WINDOW CHALLENGER TESTS PASSED <<<")
        print("===========================================================================")
    }
}
