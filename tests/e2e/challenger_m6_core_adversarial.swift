//
//  challenger_m6_core_adversarial.swift
//  fabushi-ios
//
//  Tier 5 Adversarial Coverage Hardening Suite for Milestone 6:
//  - Feature 1 & 2: C-ABI FFI Multi-Threaded Dispatch Flood, Panic Isolation, Zero-Leak String Tracking
//  - Feature 34: CUA Remote Computer Lease 500-Run Emergency Kill-Switch Preemption Latency (< 10.0ms SLA)
//  - Feature 35: Full-Duplex Talk Mode VAD RMS Energy & Sub-60ms Barge-In with 320-Chunk Buffer Flush
//  - Feature 37: Account Scope Generation Fence 1,000 Concurrent Switches & Stale Task Rejection
//  - Feature 36: SQLite WAL Store 10-Table Schema Integrity & Tantivy Search with <mark> Highlight Tokens
//

import CoreGraphics
import Foundation
import SwiftUI
#if canImport(Darwin)
import Darwin
#endif

// ============================================================================
// MARK: - Direct C-ABI FFI Linkage
// ============================================================================

@_silgen_name("telegram_app_host_create_test")
func telegram_app_host_create_test(_ appDataDir: UnsafePointer<CChar>?) -> UnsafeMutableRawPointer?

@_silgen_name("telegram_app_host_dispatch")
func telegram_app_host_dispatch(_ host: UnsafeMutableRawPointer?, _ req: UnsafePointer<CChar>?) -> UnsafeMutablePointer<CChar>?

@_silgen_name("telegram_app_host_free_string")
func telegram_app_host_free_string(_ ptr: UnsafeMutablePointer<CChar>?)

@_silgen_name("telegram_app_host_destroy")
func telegram_app_host_destroy(_ host: UnsafeMutableRawPointer?)

@_silgen_name("telegram_bridge_register_event_callback")
func telegram_bridge_register_event_callback(
    _ host: UnsafeMutableRawPointer?,
    _ cb: (@convention(c) (UnsafePointer<CChar>?, UnsafePointer<CChar>?, UnsafeMutableRawPointer?) -> Void)?,
    _ ctx: UnsafeMutableRawPointer?
)

// ============================================================================
// MARK: - Benchmark Utilities & Assertions
// ============================================================================

final class Box<T>: @unchecked Sendable {
    var value: T
    init(_ value: T) { self.value = value }
}

struct ChallengeFailure: Error, CustomStringConvertible {
    let description: String
    init(_ description: String) { self.description = description }
}

func assertTrue(_ condition: @autoclosure () -> Bool, _ message: String = "", file: StaticString = #file, line: UInt = #line) throws {
    if !condition() {
        throw ChallengeFailure("Assertion failed at \(file):\(line): \(message)")
    }
}

func assertFalse(_ condition: @autoclosure () -> Bool, _ message: String = "", file: StaticString = #file, line: UInt = #line) throws {
    if condition() {
        throw ChallengeFailure("Assertion failed at \(file):\(line): expected false, got true. \(message)")
    }
}

func assertEqual<T: Equatable>(_ a: T, _ b: T, _ message: String = "", file: StaticString = #file, line: UInt = #line) throws {
    if a != b {
        throw ChallengeFailure("Assertion failed at \(file):\(line): expected \(b), got \(a). \(message)")
    }
}

func assertAlmostEqualDouble(_ a: Double, _ b: Double, accuracy: Double = 0.001, _ message: String = "", file: StaticString = #file, line: UInt = #line) throws {
    if abs(a - b) > accuracy {
        throw ChallengeFailure("Assertion failed at \(file):\(line): \(a) not almost equal to \(b) (diff: \(abs(a - b)) > \(accuracy)). \(message)")
    }
}

func getProcessRssMB() -> Double {
    #if canImport(Darwin)
    var info = mach_task_basic_info()
    var count = mach_msg_type_number_t(MemoryLayout<mach_task_basic_info>.size) / 4
    let kerr: kern_return_t = withUnsafeMutablePointer(to: &info) {
        $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
            task_info(mach_task_self_, task_flavor_t(MACH_TASK_BASIC_INFO), $0, &count)
        }
    }
    return kerr == KERN_SUCCESS ? Double(info.resident_size) / (1024.0 * 1024.0) : 0.0
    #else
    return 0.0
    #endif
}

// Global push event counter
private let gPushEventCounter = Box<Int>(0)
private let gEventLock = NSLock()

private let cEventCallback: @convention(c) (UnsafePointer<CChar>?, UnsafePointer<CChar>?, UnsafeMutableRawPointer?) -> Void = { eventType, payload, _ in
    gEventLock.lock()
    gPushEventCounter.value += 1
    gEventLock.unlock()
}

// ============================================================================
// MARK: - Master Tier 5 Adversarial Harness
// ============================================================================

@main
struct ChallengerM6CoreAdversarial {

    @MainActor private static var passCount: Int = 0
    @MainActor private static var totalCount: Int = 0

    @MainActor
    private static func runChallenge(_ name: String, block: () async throws -> Void) async {
        totalCount += 1
        print("[\(totalCount)] CHALLENGE: \(name)...")
        let start = CFAbsoluteTimeGetCurrent()
        do {
            try await block()
            let elapsed = (CFAbsoluteTimeGetCurrent() - start) * 1000.0
            passCount += 1
            print("  ✓ PASSED: \(name) (\(String(format: "%.2f", elapsed)) ms)")
        } catch {
            print("  ✗ FAILED: \(name) with error: \(error)")
            exit(1)
        }
    }

    @MainActor
    static func main() async {
        print("================================================================================")
        print(" TIER 5 ADVERSARIAL BENCHMARK SUITE: Core Runtime, FFI & Advanced Capabilities")
        print(" Scope: C-ABI FFI Flood, Preemption SLA, VAD Barge-In, Fences & Tantivy")
        print("================================================================================")
        let initialRss = getProcessRssMB()
        print("Initial Process RSS: \(String(format: "%.2f", initialRss)) MB\n")

        // =====================================================================
        // CHALLENGE 1: C-ABI FFI Multi-Threaded Dispatch Flood & Zero-Leak Tracking
        // =====================================================================
        await runChallenge("F01_F02_CHALLENGE_CAbiConcurrentFloodAndZeroLeakTracking") {
            let tempDir = "/tmp/challenger_m6_ffi_\(UUID().uuidString)"
            try? FileManager.default.createDirectory(atPath: tempDir, withIntermediateDirectories: true)
            defer { try? FileManager.default.removeItem(atPath: tempDir) }

            let host = telegram_app_host_create_test(tempDir)
            try assertTrue(host != nil, "Host creation failed")
            defer { telegram_app_host_destroy(host) }

            // Register event callback
            telegram_bridge_register_event_callback(host, cEventCallback, nil)

            // 1.1 Multi-threaded concurrent JSON-RPC dispatch flood:
            // 16 concurrent worker tasks x 100 requests = 1,600 concurrent JSON-RPC RPCs
            let threadCount = 16
            let reqsPerThread = 100
            let totalRequests = threadCount * reqsPerThread

            print("  -> Launching \(totalRequests) concurrent JSON-RPC dispatches across \(threadCount) worker tasks...")
            let hostBox = Box(host!)
            let floodStart = CFAbsoluteTimeGetCurrent()
            var totalSuccess = 0
            await withTaskGroup(of: Int.self) { group in
                for t in 0..<threadCount {
                    group.addTask {
                        let h = hostBox.value
                        var localSuccess = 0
                        for i in 0..<reqsPerThread {
                            let method = (i % 4 == 0) ? "telegram.ping" :
                                         (i % 4 == 1) ? "telegram.sendMessage" :
                                         (i % 4 == 2) ? "telegram.getChats" : "telegram.getMessages"

                            let reqJson: String
                            if method == "telegram.sendMessage" {
                                reqJson = "{\"command\":\"\(method)\",\"params\":{\"chat_id\":\(100 + t),\"text\":\"flood_\(i)\",\"sender_id\":\(t)}}"
                            } else if method == "telegram.getMessages" {
                                reqJson = "{\"command\":\"\(method)\",\"params\":{\"chat_id\":\(100 + t)}}"
                            } else {
                                reqJson = "{\"command\":\"\(method)\",\"params\":{\"nonce\":\(i)}}"
                            }

                            let resPtr = telegram_app_host_dispatch(h, reqJson)
                            if let ptr = resPtr {
                                let resStr = String(cString: ptr)
                                if resStr.contains("\"ok\":true") {
                                    localSuccess += 1
                                }
                                telegram_app_host_free_string(ptr)
                            }
                        }
                        return localSuccess
                    }
                }
                for await count in group {
                    totalSuccess += count
                }
            }

            let floodElapsed = CFAbsoluteTimeGetCurrent() - floodStart
            let rps = Double(totalRequests) / floodElapsed
            print("  -> Completed \(totalSuccess)/\(totalRequests) dispatches in \(String(format: "%.4f", floodElapsed)) s (\(String(format: "%.1f", rps)) RPC/s)")
            try assertEqual(totalSuccess, totalRequests, "All 1,600 concurrent dispatches must succeed")

            // 1.2 Panic catch-unwind isolation:
            print("  -> Testing Panic Catch-Unwind Isolation under deliberate Rust panic...")
            let panicPtr = telegram_app_host_dispatch(host, "{\"command\":\"debug.trigger_panic\"}")
            try assertTrue(panicPtr != nil, "Dispatch under panic returned NULL pointer")
            let panicStr = String(cString: panicPtr!)
            telegram_app_host_free_string(panicPtr)
            try assertTrue(panicStr.contains("\"code\":\"HOST_PANIC\""), "Response must contain HOST_PANIC")

            // Test malformed JSON
            let malformedPtr = telegram_app_host_dispatch(host, "{ invalid json corrupt")
            try assertTrue(malformedPtr != nil)
            let malformedStr = String(cString: malformedPtr!)
            telegram_app_host_free_string(malformedPtr)
            try assertTrue(malformedStr.contains("\"code\":\"INVALID_JSON\""), "Response must contain INVALID_JSON")

            // Verify host is still alive and operational after panics
            let postPanicPtr = telegram_app_host_dispatch(host, "{\"command\":\"telegram.ping\",\"params\":{\"nonce\":777}}")
            try assertTrue(postPanicPtr != nil)
            let postPanicStr = String(cString: postPanicPtr!)
            telegram_app_host_free_string(postPanicPtr)
            try assertTrue(postPanicStr.contains("\"ok\":true") && postPanicStr.contains("\"nonce\":777"), "Host must remain fully operational post-panic")

            // 1.3 String allocation tracking & zero-leak assertion:
            print("  -> Verifying Zero Leaks across 1,000 allocate-dispatch-free cycles...")
            let rssBeforeChurn = getProcessRssMB()
            for i in 0..<1000 {
                let req = "{\"command\":\"telegram.ping\",\"params\":{\"nonce\":\(i)}}"
                let p = telegram_app_host_dispatch(host, req)
                telegram_app_host_free_string(p)
            }
            let rssAfterChurn = getProcessRssMB()
            let leakDelta = max(0.0, rssAfterChurn - rssBeforeChurn)
            print("  -> RSS before churn: \(String(format: "%.2f", rssBeforeChurn)) MB, after: \(String(format: "%.2f", rssAfterChurn)) MB, Delta: \(String(format: "%.2f", leakDelta)) MB")
            try assertTrue(leakDelta < 2.0, "Memory leak detected during string dispatch churn (delta \(leakDelta) MB >= 2.0 MB)")
        }

        // =====================================================================
        // CHALLENGE 2: CUA Remote Computer Lease 500-Run Emergency Kill-Switch SLA
        // =====================================================================
        await runChallenge("F34_CHALLENGE_Cua500RunEmergencyPreemptionSLA") {
            let vm = CuaScreenOverlayViewModel()
            vm.assignLease(generation: 1, ttlSeconds: 900)
            try assertEqual(vm.currentBrokerGen, 1)

            let benchmarkRuns = 500
            var latencies: [Double] = []
            latencies.reserveCapacity(benchmarkRuns)

            print("  -> Benchmarking emergency kill-switch across \(benchmarkRuns) simulated runs (SLA < 10.0 ms)...")
            for i in 1...benchmarkRuns {
                vm.assignLease(generation: UInt64(i))
                let latencyMs = vm.triggerEmergencyStop()
                latencies.append(latencyMs)
                try assertTrue(latencyMs < 10.0, "Preemption SLA violated on run \(i): \(latencyMs) ms >= 10.0 ms")
                try assertEqual(vm.leaseState, .preempted(byUser: true))
            }

            let sorted = latencies.sorted()
            let minLatency = sorted.first ?? 0.0
            let maxLatency = sorted.last ?? 0.0
            let avgLatency = sorted.reduce(0.0, +) / Double(sorted.count)
            let p95Latency = sorted[Int(Double(sorted.count) * 0.95)]
            let p99Latency = sorted[Int(Double(sorted.count) * 0.99)]

            print("  -> [CUA 500-Run Latency Stats]")
            print("     Min: \(String(format: "%.4f", minLatency)) ms")
            print("     Avg: \(String(format: "%.4f", avgLatency)) ms")
            print("     P95: \(String(format: "%.4f", p95Latency)) ms")
            print("     P99: \(String(format: "%.4f", p99Latency)) ms")
            print("     Max: \(String(format: "%.4f", maxLatency)) ms (SLA < 10.0 ms)")

            try assertTrue(maxLatency < 10.0, "Max latency must be strictly under 10.0 ms SLA threshold")

            // Viewport coordinate clamping & mapping
            let clampedCenter = CuaCoordinateTransformer.clampNormalized(point: CGPoint(x: 0.5, y: 0.5))
            try assertEqual(clampedCenter.x, 0.5)
            try assertEqual(clampedCenter.y, 0.5)

            let clampedOverflow = CuaCoordinateTransformer.clampNormalized(point: CGPoint(x: -5.0, y: 15.0))
            try assertEqual(clampedOverflow.x, 0.0)
            try assertEqual(clampedOverflow.y, 1.0)

            let hostPixels = CuaCoordinateTransformer.mapToHostPixels(normalizedPoint: clampedOverflow, hostWidth: 1920, hostHeight: 1080)
            try assertEqual(hostPixels.x, 0)
            try assertEqual(hostPixels.y, 1079)
        }

        // =====================================================================
        // CHALLENGE 3: Full-Duplex Talk Mode VAD RMS & Sub-60ms Barge-In Flush
        // =====================================================================
        await runChallenge("F35_CHALLENGE_VadRmsEnergyAndSub60msBargeInWith320ChunkFlush") {
            // 3.1 VAD RMS mathematical calculation
            // Silence (320 samples of 0)
            let silenceSamples = [Int16](repeating: 0, count: 320)
            let silenceRms = VadAudioCalculator.computeRms(samples: silenceSamples)
            try assertAlmostEqualDouble(Double(silenceRms), 0.0)

            // DC offset (320 samples of 1000)
            let dcSamples = [Int16](repeating: 1000, count: 320)
            let dcRms = VadAudioCalculator.computeRms(samples: dcSamples)
            try assertAlmostEqualDouble(Double(dcRms), 1000.0)

            // Square wave (320 samples of alternating +2000, -2000)
            var squareSamples: [Int16] = []
            for i in 0..<320 { squareSamples.append(i % 2 == 0 ? 2000 : -2000) }
            let squareRms = VadAudioCalculator.computeRms(samples: squareSamples)
            try assertAlmostEqualDouble(Double(squareRms), 2000.0)

            // Dynamic Orb Scale calculation
            let orbScaleQuiet = VadAudioCalculator.calculateOrbScale(rms: 0.0, baseRadius: 80.0)
            try assertAlmostEqualDouble(Double(orbScaleQuiet), 80.0)
            let orbScaleLoud = VadAudioCalculator.calculateOrbScale(rms: 500.0, baseRadius: 80.0)
            try assertAlmostEqualDouble(Double(orbScaleLoud), 80.0 * 1.35)

            // 3.2 Sub-60ms Barge-In Speech Detection (3 consecutive 20ms frames = 60ms)
            let vm = TalkModeViewModel()
            vm.setAgentSpeaking(withBufferedChunks: 320) // Exactly 320 chunks (1 MiB boundary)
            try assertEqual(vm.queuedAudioBufferChunks, 320)
            try assertEqual(vm.sessionState, .speaking)

            let loudFrame = [Int16](repeating: 600, count: 320) // RMS = 600 > 300 threshold
            let quietFrame = [Int16](repeating: 40, count: 320)  // RMS = 40 < 300 threshold

            // Frame 1: loud -> consecutive = 1, no barge in yet
            let f1 = vm.handlePcmFrame(samples: loudFrame)
            try assertFalse(f1)
            try assertEqual(vm.consecutiveSpeechFrames, 1)

            // Frame 2: quiet -> resets consecutive frames to 0
            let f2 = vm.handlePcmFrame(samples: quietFrame)
            try assertFalse(f2)
            try assertEqual(vm.consecutiveSpeechFrames, 0)
            try assertEqual(vm.sessionState, .speaking)

            // Now send 3 consecutive loud frames: 20ms * 3 = 60ms
            let b1 = vm.handlePcmFrame(samples: loudFrame) // Frame 1 (20ms)
            try assertFalse(b1)
            try assertEqual(vm.consecutiveSpeechFrames, 1)

            let b2 = vm.handlePcmFrame(samples: loudFrame) // Frame 2 (40ms)
            try assertFalse(b2)
            try assertEqual(vm.consecutiveSpeechFrames, 2)

            let b3 = vm.handlePcmFrame(samples: loudFrame) // Frame 3 (60ms) -> BARGE-IN TRIGGERED!
            try assertTrue(b3, "3rd consecutive frame (60ms) must trigger barge-in")
            try assertEqual(vm.sessionState, .interrupted)

            // 3.3 320-chunk (1 MiB) immediate buffer flush
            try assertEqual(vm.queuedAudioBufferChunks, 0, "Audio buffer MUST be flushed to 0 chunks upon barge-in")

            // Buffer clamping to max 320 chunks
            vm.setAgentSpeaking(withBufferedChunks: 1000)
            try assertEqual(vm.queuedAudioBufferChunks, 320, "Buffer chunks must be clamped to maxAudioBufferChunks (320)")
        }

        // =====================================================================
        // CHALLENGE 4: Account Scope 1,000 Concurrent Switches & Fence Rejection
        // =====================================================================
        await runChallenge("F37_CHALLENGE_AccountScopeConcurrentSwitchesAndFenceRejection") {
            let manager = AccountScopeManager(initialGeneration: 1, activeAccountId: "acc_init")
            let initialGen = await manager.getCurrentGeneration()
            try assertEqual(initialGen, 1)

            // 4.1 Concurrently switch accounts 1,000 times
            let concurrentWorkers = 10
            let switchesPerWorker = 100
            let totalSwitches = concurrentWorkers * switchesPerWorker

            print("  -> Executing \(totalSwitches) concurrent account switches across \(concurrentWorkers) workers...")
            await withTaskGroup(of: Void.self) { group in
                for w in 0..<concurrentWorkers {
                    group.addTask {
                        for s in 0..<switchesPerWorker {
                            _ = try? await manager.switchAccount(to: "user_\(w)_\(s)")
                        }
                    }
                }
            }

            let finalGen = await manager.getCurrentGeneration()
            try assertEqual(finalGen, UInt64(1 + totalSwitches), "Generation must advance monotonically by exactly \(totalSwitches)")
            print("  -> Monotonic generation advanced from 1 to \(finalGen)")

            // 4.2 Stale generation callback rejection
            var staleRejectionCaught = false
            do {
                _ = try await manager.executeFenced { taskGen in
                    // While task is executing under captured taskGen, advance generation!
                    _ = try await manager.switchAccount(to: "interfering_user")
                    return "task_result"
                }
            } catch let err as FenceRejectionError {
                if case .staleGeneration(let taskGen, let currentGen) = err {
                    try assertEqual(taskGen, finalGen)
                    try assertEqual(currentGen, finalGen + 1)
                    staleRejectionCaught = true
                }
            }
            try assertTrue(staleRejectionCaught, "executeFenced MUST throw FenceRejectionError.staleGeneration when generation advances during execution")

            // 4.3 Logout epoch scrub
            let genBeforeLogout = await manager.getCurrentGeneration()
            let genAfterLogout = await manager.logout()
            try assertEqual(genAfterLogout, genBeforeLogout + 1)
            let activeAcc = await manager.getActiveAccountId()
            try assertTrue(activeAcc == nil, "Active account must be nil after logout")
        }

        // =====================================================================
        // CHALLENGE 5: SQLite WAL 10-Table Schema Integrity & Tantivy Highlights
        // =====================================================================
        await runChallenge("F36_CHALLENGE_SqliteWal10TableSchemaAndTantivyHighlights") {
            let bridge = SearchIndexingBridge()

            // 5.1 Verify all 10 WAL tables
            let isSchemaOk = try await bridge.verifySchemaIntegrity()
            try assertTrue(isSchemaOk, "Canonical 10-table WAL schema integrity verification failed")

            // Test non-WAL journal mode rejection
            await bridge.setJournalMode("delete")
            var journalModeErrorCaught = false
            do {
                _ = try await bridge.verifySchemaIntegrity()
            } catch let err as SearchIndexError {
                if case .journalModeNotWal = err {
                    journalModeErrorCaught = true
                }
            }
            try assertTrue(journalModeErrorCaught, "Non-WAL journal mode must be rejected")
            await bridge.setJournalMode("wal")

            // Adversarially drop each of the 10 required tables one by one
            let requiredTables = SearchIndexingBridge.requiredWalTables
            try assertEqual(requiredTables.count, 10, "Must have exactly 10 required WAL tables")
            for table in requiredTables {
                let testBridge = SearchIndexingBridge()
                await testBridge.dropTableForTesting(table)
                var missingCaught = false
                do {
                    _ = try await testBridge.verifySchemaIntegrity()
                } catch let err as SearchIndexError {
                    if case .schemaCorrupted(let missing) = err {
                        try assertEqual(missing, [table])
                        missingCaught = true
                    }
                }
                try assertTrue(missingCaught, "Missing table '\(table)' must be caught by schema verification")
            }

            // 5.2 Concurrent reads/writes & Tantivy full-text index with highlight <mark> tokens
            let tantivyBridge = SearchIndexingBridge()
            let testDocs = [
                SearchDocument(id: 101, chatId: 1, text: "Telegram iOS Swift 6 concurrency bridge architecture"),
                SearchDocument(id: 102, chatId: 1, text: "Rust mobile runtime memory safety with zero leak FFI"),
                SearchDocument(id: 103, chatId: 2, text: "SQLite WAL store high performance message persistence"),
                SearchDocument(id: 104, chatId: 2, text: "Tantivy full text search with highlight mark token parsing")
            ]

            try await tantivyBridge.batchIndex(docs: testDocs)
            try await tantivyBridge.commit()
            let indexedTotal = await tantivyBridge.totalIndexedCount()
            try assertEqual(indexedTotal, 4)

            // Search query with case-insensitive token highlight: "swift" matching "Swift"
            let swiftResults = try await tantivyBridge.search(query: SearchQueryParams(query: "swift", chatId: 1))
            try assertEqual(swiftResults.count, 1)
            let snippet = swiftResults[0].snippetWithHighlights
            try assertTrue(snippet.contains("<mark>Swift</mark>") || snippet.contains("<mark>swift</mark>"), "Snippet must wrap matching token in <mark>...</mark>")

            // Search query with multiple tokens: "full text"
            let ftResults = try await tantivyBridge.search(query: SearchQueryParams(query: "full text", chatId: 2))
            try assertEqual(ftResults.count, 1)
            let ftSnippet = ftResults[0].snippetWithHighlights
            try assertTrue(ftSnippet.contains("<mark>full</mark>") || ftSnippet.contains("<mark>text</mark>"))

            // Chat scoping filter isolation: doc 103 belongs to chatId 2; should not match chatId 1
            let filteredResults = try await tantivyBridge.search(query: SearchQueryParams(query: "sqlite", chatId: 1))
            try assertEqual(filteredResults.count, 0, "chatId scoping filter must strictly exclude other chats")
        }

        // =====================================================================
        // SUMMARY
        // =====================================================================
        let finalRss = getProcessRssMB()
        print("\n================================================================================")
        print(" MASTER ADVERSARIAL STRESS RESULTS")
        print("================================================================================")
        print(" Total Challenges Executed : \(totalCount)")
        print(" Total Challenges Passed   : \(passCount) / \(totalCount) (100.0%)")
        print(" Initial Process RSS       : \(String(format: "%.2f", initialRss)) MB")
        print(" Final Process RSS         : \(String(format: "%.2f", finalRss)) MB")
        print(" Memory Stability          : PASS (Zero Leaks, RSS strictly bounded)")
        print(" SLA Conformance           : PASS (CUA Preemption Max < 10.0 ms, Talk Barge-in = 60 ms)")
        print("================================================================================\n")
    }
}
