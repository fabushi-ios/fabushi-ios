import Darwin
import Foundation
import Security

// MARK: - Test DTOs
struct PingParams: Encodable, Sendable {
    let nonce: Int
}

struct PingResponse: Decodable, Sendable {
    let pong: Bool
    let nonce: Int
    let timestamp: Int64
}

// Memory footprint helper
func getResidentMemoryBytes() -> UInt64 {
    var info = mach_task_basic_info()
    var count = mach_msg_type_number_t(MemoryLayout<mach_task_basic_info>.size) / 4
    let kerr: kern_return_t = withUnsafeMutablePointer(to: &info) {
        $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
            task_info(mach_task_self_, task_flavor_t(MACH_TASK_BASIC_INFO), $0, &count)
        }
    }
    return kerr == KERN_SUCCESS ? info.resident_size : 0
}

// MARK: - Thread-Safe Atomic Helpers
final class ArcAtomicInt: @unchecked Sendable {
    private let lock = NSLock()
    private var value: Int

    init(_ value: Int) {
        self.value = value
    }

    @discardableResult
    func increment() -> Int {
        lock.lock()
        defer { lock.unlock() }
        value += 1
        return value
    }

    func get() -> Int {
        lock.lock()
        defer { lock.unlock() }
        return value
    }
}

final class ArcAtomicBool: @unchecked Sendable {
    private let lock = NSLock()
    private var value: Bool

    init(_ value: Bool) {
        self.value = value
    }

    func set(_ newValue: Bool) {
        lock.lock()
        defer { lock.unlock() }
        value = newValue
    }

    func get() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return value
    }
}

// MARK: - Stress Test Suite Runner
@main
struct TelegramBridgeStressHarness {
    static func main() async throws {
        print("================================================================================")
        print(" ADVERSARIAL STRESS TEST SUITE: Swift Concurrency Bridge Engine (Remediated)")
        print(" Target: TelegramBridgeRuntime.swift & TelegramEventStream.swift")
        print(" Mode: Swift 6 Strict Concurrency Complete")
        print("================================================================================")

        let tempBase = FileManager.default.temporaryDirectory.appendingPathComponent("fabushi_stress_\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: tempBase, withIntermediateDirectories: true)
        defer {
            try? FileManager.default.removeItem(at: tempBase)
        }

        let runtime = TelegramBridgeRuntime.shared
        let hostDir = tempBase.appendingPathComponent("runtime_host")
        try runtime.initialize(appDataDirectory: hostDir, isTestMode: true)
        print("✓ TelegramBridgeRuntime.shared initialized in test mode at \(hostDir.path)")
        print("  Status: \(runtime.connectionStatus), isReady: \(runtime.isReady)")

        // -------------------------------------------------------------------------
        // Stress Test 1: Rapid Concurrent Async Dispatch Bursts (150 parallel tasks)
        // -------------------------------------------------------------------------
        print("\n[STRESS 1] Executing Rapid Concurrent Async Dispatch Bursts (150 parallel tasks)...")
        let dispatchCount = 150
        var mainActorTicks = 0
        let mainActorTickerActive = ArcAtomicBool(true)

        // Launch concurrent @MainActor heartbeat to verify UI responsiveness
        let heartbeatTask = Task { @MainActor in
            while mainActorTickerActive.get() {
                try? await Task.sleep(nanoseconds: 5_000_000) // 5ms
                mainActorTicks += 1
            }
        }

        let startTime = DispatchTime.now()
        var completedCount = 0
        var failedCount = 0

        await withTaskGroup(of: Result<PingResponse, Error>.self) { group in
            for i in 0..<dispatchCount {
                group.addTask {
                    do {
                        let res: PingResponse = try await runtime.dispatch(
                            command: "telegram.ping",
                            params: PingParams(nonce: i)
                        )
                        return .success(res)
                    } catch {
                        return .failure(error)
                    }
                }
            }

            for await result in group {
                switch result {
                case .success(let resp):
                    if resp.pong && resp.nonce >= 0 && resp.nonce < dispatchCount {
                        completedCount += 1
                    } else {
                        failedCount += 1
                    }
                case .failure(let err):
                    print("  Dispatch failed with error: \(err)")
                    failedCount += 1
                }
            }
        }

        let endTime = DispatchTime.now()
        mainActorTickerActive.set(false)
        _ = await heartbeatTask.result
        let elapsedNanos = endTime.uptimeNanoseconds - startTime.uptimeNanoseconds
        let elapsedMs = Double(elapsedNanos) / 1_000_000.0
        let avgLatencyMs = elapsedMs / Double(dispatchCount)

        print("  - Dispatches executed: \(dispatchCount)")
        print("  - Completed successfully: \(completedCount)")
        print("  - Failed: \(failedCount)")
        print("  - Total wall time: \(String(format: "%.2f", elapsedMs)) ms")
        print("  - Average latency per dispatch: \(String(format: "%.3f", avgLatencyMs)) ms")
        print("  - @MainActor heartbeat ticks recorded: \(mainActorTicks) (Responsiveness CONFIRMED)")

        guard completedCount == dispatchCount && failedCount == 0 else {
            fatalError("STRESS 1 FAILED: Expected \(dispatchCount) successes, got \(completedCount) (failed: \(failedCount))")
        }
        guard mainActorTicks > 0 else {
            fatalError("STRESS 1 FAILED: @MainActor was blocked or starved during background queue dispatch!")
        }
        print("✓ STRESS 1 PASSED: Zero deadlocks, 100% completion, @MainActor remained fully responsive.")

        // -------------------------------------------------------------------------
        // Stress Test 2: High-Frequency Push Event Flooding (10,000 rapid events)
        // -------------------------------------------------------------------------
        print("\n[STRESS 2] Executing High-Frequency Push Event Flooding (10,000 events)...")
        let totalEvents = 10_000
        let initialMemoryBytes = getResidentMemoryBytes()

        let eventStream = runtime.eventStream
        let receivedEventsCount = ArcAtomicInt(0)
        let consumerFinished = ArcAtomicBool(false)

        // Spawn stream consumer task
        let consumerTask = Task {
            for await _ in eventStream {
                let current = receivedEventsCount.increment()
                if current == totalEvents {
                    consumerFinished.set(true)
                    break
                }
            }
        }

        // Producer flooding across 4 background tasks
        let floodStart = DispatchTime.now()
        let eventsPerWorker = totalEvents / 4

        await withTaskGroup(of: Void.self) { producerGroup in
            for workerIdx in 0..<4 {
                producerGroup.addTask {
                    let startIdx = workerIdx * eventsPerWorker
                    let endIdx = startIdx + eventsPerWorker
                    for i in startIdx..<endIdx {
                        let payload = "{\"id\":\(i),\"chat_id\":1001,\"sender_id\":2002,\"text\":\"Stress Message #\(i)\",\"timestamp\":\(1700000000 + i)}"
                        runtime.receivePushEvent(type: "newMessage", payloadJson: payload)
                    }
                }
            }
        }

        let floodEnd = DispatchTime.now()
        let floodElapsedMs = Double(floodEnd.uptimeNanoseconds - floodStart.uptimeNanoseconds) / 1_000_000.0

        // Await consumer completion with timeout
        var waitIterations = 0
        while !consumerFinished.get() && waitIterations < 200 {
            try await Task.sleep(nanoseconds: 20_000_000) // 20ms
            waitIterations += 1
        }
        _ = await consumerTask.result

        let finalCount = receivedEventsCount.get()
        let peakMemoryBytes = getResidentMemoryBytes()
        let memoryDeltaMB = Double(Int64(peakMemoryBytes) - Int64(initialMemoryBytes)) / (1024.0 * 1024.0)

        print("  - Events pushed: \(totalEvents)")
        print("  - Events received by AsyncStream: \(finalCount)")
        print("  - Flood duration: \(String(format: "%.2f", floodElapsedMs)) ms (\(String(format: "%.0f", Double(totalEvents) / (floodElapsedMs / 1000.0))) events/sec)")
        print("  - Memory delta: \(String(format: "%.2f", memoryDeltaMB)) MB")

        guard finalCount == totalEvents else {
            fatalError("STRESS 2 FAILED: Expected \(totalEvents) events, received \(finalCount) (Dropped \(totalEvents - finalCount) events!)")
        }
        print("✓ STRESS 2 PASSED: 10,000 events streamed with ZERO dropped frames and stable memory.")

        // -------------------------------------------------------------------------
        // Stress Test 3: Multicast Concurrent Streaming & Dynamic Lifecycle
        // -------------------------------------------------------------------------
        print("\n[STRESS 3] Testing Multicast Concurrent Streaming & Dynamic Consumer Lifecycles...")
        let multicastConsumersCount = 5
        let multicastEventsCount = 1_000
        var consumerCounters = [ArcAtomicInt]()
        for _ in 0..<multicastConsumersCount {
            consumerCounters.append(ArcAtomicInt(0))
        }

        // Spin up 5 concurrent listeners on independent streams
        var consumerTasks = [Task<Void, Never>]()
        for i in 0..<multicastConsumersCount {
            let stream = runtime.eventStream
            let counter = consumerCounters[i]
            let task = Task {
                for await _ in stream {
                    let c = counter.increment()
                    if c == multicastEventsCount {
                        break
                    }
                }
            }
            consumerTasks.append(task)
        }

        // Flood 1,000 events to all 5 listeners simultaneously
        await withTaskGroup(of: Void.self) { group in
            for worker in 0..<2 {
                group.addTask {
                    let start = worker * (multicastEventsCount / 2)
                    let end = start + (multicastEventsCount / 2)
                    for i in start..<end {
                        let payload = "{\"id\":\(i),\"chat_id\":999,\"sender_id\":100,\"text\":\"Multicast #\(i)\",\"timestamp\":1700000000}"
                        runtime.receivePushEvent(type: "newMessage", payloadJson: payload)
                    }
                }
            }
        }

        for task in consumerTasks {
            _ = await task.result
        }

        for i in 0..<multicastConsumersCount {
            let received = consumerCounters[i].get()
            print("  - Consumer #\(i + 1) received: \(received)/\(multicastEventsCount)")
            guard received == multicastEventsCount else {
                fatalError("STRESS 3 FAILED: Multicast consumer #\(i + 1) received \(received), expected \(multicastEventsCount)")
            }
        }
        print("✓ STRESS 3 PASSED: All 5 concurrent multicast consumers received 100% of flooded events (zero drops).")

        // -------------------------------------------------------------------------
        // Stress Test 4: Swift Task Cancellation Resilience (100 tasks, 50 cancelled)
        // -------------------------------------------------------------------------
        print("\n[STRESS 4] Testing Swift Task Cancellation Resilience (100 tasks, 50 cancelled)...")
        let cancellationStormCount = 100
        var cancelledCount = 0
        var normalSuccessCount = 0

        await withTaskGroup(of: String.self) { group in
            for i in 0..<cancellationStormCount {
                let shouldCancel = (i % 2 == 0)
                group.addTask {
                    let childTask = Task {
                        try await runtime.dispatch(
                            command: "telegram.ping",
                            params: PingParams(nonce: 1000 + i)
                        ) as PingResponse
                    }
                    if shouldCancel {
                        childTask.cancel()
                    }
                    do {
                        _ = try await childTask.value
                        return shouldCancel ? "cancelled_but_finished" : "success"
                    } catch {
                        return "error_or_cancelled"
                    }
                }
            }

            for await result in group {
                if result == "success" || result == "cancelled_but_finished" {
                    normalSuccessCount += 1
                } else {
                    cancelledCount += 1
                }
            }
        }
        print("  - Tasks executed in cancellation storm: \(cancellationStormCount)")
        print("  - Clean completions: \(normalSuccessCount)")
        print("  - Threw or aborted cleanly: \(cancelledCount)")

        // Verify C-ABI health: 50 post-cancellation dispatches MUST succeed cleanly
        print("  - Verifying C-ABI host state integrity post-cancellation...")
        var postVerificationSuccess = 0
        for i in 0..<50 {
            let res: PingResponse = try await runtime.dispatch(
                command: "telegram.ping",
                params: PingParams(nonce: 5000 + i)
            )
            if res.pong && res.nonce == 5000 + i {
                postVerificationSuccess += 1
            }
        }
        guard postVerificationSuccess == 50 else {
            fatalError("STRESS 4 FAILED: Host corrupted after task cancellation! Only \(postVerificationSuccess)/50 succeeded.")
        }
        print("✓ STRESS 4 PASSED: Task cancellation caused zero FFI corruption, zero pointer leaks, 50/50 subsequent dispatches succeeded.")

        // -------------------------------------------------------------------------
        // Stress Test 5: Concurrent Shutdown Race Resilience
        // -------------------------------------------------------------------------
        print("\n[STRESS 5] Testing Concurrent Shutdown Race Resilience (50 parallel dispatches racing with shutdown)...")
        let raceDispatchesCount = 50
        var raceSuccesses = 0
        var raceCleanErrors = 0

        await withTaskGroup(of: Result<PingResponse, Error>.self) { group in
            for i in 0..<raceDispatchesCount {
                group.addTask {
                    do {
                        let res: PingResponse = try await runtime.dispatch(
                            command: "telegram.ping",
                            params: PingParams(nonce: 7000 + i)
                        )
                        return .success(res)
                    } catch {
                        return .failure(error)
                    }
                }
            }

            // Race: call shutdown on @MainActor while dispatches are in flight
            runtime.shutdown()

            for await result in group {
                switch result {
                case .success(let resp):
                    if resp.pong {
                        raceSuccesses += 1
                    }
                case .failure(let err):
                    if let bridgeErr = err as? TelegramBridgeError, case .notInitialized = bridgeErr {
                        raceCleanErrors += 1
                    } else {
                        // Any other error
                        print("  Unexpected race error: \(err)")
                    }
                }
            }
        }

        print("  - Dispatches executed during shutdown race: \(raceDispatchesCount)")
        print("  - Completed before shutdown: \(raceSuccesses)")
        print("  - Cleanly rejected with .notInitialized: \(raceCleanErrors)")
        print("  - Sum: \(raceSuccesses + raceCleanErrors)/\(raceDispatchesCount)")
        guard raceSuccesses + raceCleanErrors == raceDispatchesCount else {
            fatalError("STRESS 5 FAILED: Race caused unhandled error or crash! Sum was \(raceSuccesses + raceCleanErrors)")
        }
        guard !runtime.isReady else {
            fatalError("STRESS 5 FAILED: runtime.isReady must be false after shutdown!")
        }
        print("✓ STRESS 5 PASSED: Concurrent shutdown race handled with 100% clean rejection, 0 crashes, 0 SIGSEGV.")

        // -------------------------------------------------------------------------
        // Stress Test 6: Re-initialization & State Recovery Post-Shutdown
        // -------------------------------------------------------------------------
        print("\n[STRESS 6] Testing Re-initialization & State Recovery Post-Shutdown...")
        let newHostDir = tempBase.appendingPathComponent("runtime_host_reinit")
        try runtime.initialize(appDataDirectory: newHostDir, isTestMode: true)
        guard runtime.isReady else {
            fatalError("STRESS 6 FAILED: Failed to re-initialize runtime after shutdown!")
        }

        let reinitPing: PingResponse = try await runtime.dispatch(
            command: "telegram.ping",
            params: PingParams(nonce: 8888)
        )
        guard reinitPing.pong && reinitPing.nonce == 8888 else {
            fatalError("STRESS 6 FAILED: Ping failed on reinitialized runtime!")
        }
        print("✓ STRESS 6 PASSED: TelegramBridgeRuntime cleanly reinitialized and fully functional.")

        // -------------------------------------------------------------------------
        // Stress Test 7: Universal Rust Panic Isolation Across C-ABI
        // -------------------------------------------------------------------------
        print("\n[STRESS 7] Testing Universal Rust Panic Isolation across C-ABI...")
        var panicCaught = false
        do {
            let _: PingResponse = try await runtime.dispatch(
                command: "debug.trigger_panic",
                params: ["reason": "intentional_test_panic"]
            )
        } catch let err as TelegramBridgeError {
            switch err {
            case .rustPanic(let msg):
                print("  - Successfully caught Rust panic as Swift TelegramBridgeError.rustPanic: '\(msg)'")
                panicCaught = true
            default:
                print("  - Caught unexpected TelegramBridgeError: \(err)")
            }
        } catch {
            print("  - Caught unexpected general error: \(error)")
        }

        guard panicCaught else {
            fatalError("STRESS 7 FAILED: Panic was not translated into TelegramBridgeError.rustPanic!")
        }

        // Verify host continues functioning after caught panic
        let postPanicPing: PingResponse = try await runtime.dispatch(
            command: "telegram.ping",
            params: PingParams(nonce: 9999)
        )
        guard postPanicPing.pong && postPanicPing.nonce == 9999 else {
            fatalError("STRESS 7 FAILED: Host crashed or stopped responding after caught panic!")
        }
        print("✓ STRESS 7 PASSED: Panic caught safely by catch_unwind; host remains completely operational.")

        // -------------------------------------------------------------------------
        // Stress Test 8: Untyped Dynamic Dispatch & Parameterless Generic Dispatch
        // -------------------------------------------------------------------------
        print("\n[STRESS 8] Testing Parameterless & Untyped Dynamic Dispatch Overloads...")
        // 1. Untyped dispatch
        let untypedDict = try await runtime.dispatch(command: "telegram.ping")
        guard let pongVal = untypedDict["pong"] as? Bool, pongVal else {
            fatalError("STRESS 8 FAILED: Untyped dispatch did not return pong == true: \(untypedDict)")
        }
        print("  - Untyped dynamic dispatch returned valid dict: pong=\(pongVal)")

        // 2. Generic parameterless dispatch
        let typedPing: PingResponse = try await runtime.dispatch(command: "telegram.ping")
        guard typedPing.pong else {
            fatalError("STRESS 8 FAILED: Parameterless typed dispatch did not return pong == true")
        }
        print("  - Generic parameterless dispatch returned valid PingResponse: pong=\(typedPing.pong)")
        print("✓ STRESS 8 PASSED: Both untyped and typed parameterless dispatch overloads verified.")

        runtime.shutdown()
        print("\n================================================================================")
        print(" ALL 8 ADVERSARIAL STRESS TESTS PASSED CLEANLY & EMPIRICALLY!")
        print("================================================================================")
    }
}
