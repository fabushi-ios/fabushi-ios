import Foundation
import XCTest
@testable import Fabushi

final class TelegramBridgeStressTests: XCTestCase {

    struct PingParams: Encodable {
        let nonce: Int
    }

    struct PingResponse: Decodable {
        let pong: Bool
        let nonce: Int
        let timestamp: Int64
    }

    @MainActor
    func testConcurrentAsyncDispatchBursts() async throws {
        let runtime = TelegramBridgeRuntime.shared
        let tempDir = FileManager.default.temporaryDirectory.appendingPathComponent("test_bridge_burst_\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tempDir) }

        try runtime.initialize(appDataDirectory: tempDir, isTestMode: true)
        defer { runtime.shutdown() }

        let burstCount = 100
        var successes = 0

        await withTaskGroup(of: Result<PingResponse, Error>.self) { group in
            for i in 0..<burstCount {
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
                if case .success(let resp) = result, resp.pong {
                    successes += 1
                }
            }
        }

        XCTAssertEqual(successes, burstCount, "Expected all \(burstCount) parallel dispatches to succeed without deadlock")
    }

    @MainActor
    func testHighFrequencyPushEventFlooding() async throws {
        let stream = TelegramEventStream()
        let totalEvents = 5000
        var receivedCount = 0

        let consumerTask = Task {
            for await _ in stream.stream {
                receivedCount += 1
                if receivedCount == totalEvents {
                    break
                }
            }
        }

        let workerCount = 4
        let eventsPerWorker = totalEvents / workerCount

        await withTaskGroup(of: Void.self) { group in
            for workerIdx in 0..<workerCount {
                group.addTask {
                    let start = workerIdx * eventsPerWorker
                    let end = start + eventsPerWorker
                    for i in start..<end {
                        let payload = "{\"id\":\(i),\"chat_id\":1001,\"sender_id\":2002,\"text\":\"Flooding #\(i)\",\"timestamp\":1700000000}"
                        stream.parseAndYield(type: "newMessage", json: payload)
                    }
                }
            }
        }

        _ = await consumerTask.result
        XCTAssertEqual(receivedCount, totalEvents, "Expected exactly \(totalEvents) events with zero drops")
    }

    @MainActor
    func testTaskCancellationResilience() async throws {
        let runtime = TelegramBridgeRuntime.shared
        let tempDir = FileManager.default.temporaryDirectory.appendingPathComponent("test_bridge_cancel_\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tempDir) }

        try runtime.initialize(appDataDirectory: tempDir, isTestMode: true)
        defer { runtime.shutdown() }

        let tasksCount = 50
        await withTaskGroup(of: Void.self) { group in
            for i in 0..<tasksCount {
                group.addTask {
                    let t = Task {
                        try await runtime.dispatch(
                            command: "telegram.ping",
                            params: PingParams(nonce: i)
                        ) as PingResponse
                    }
                    if i % 2 == 0 {
                        t.cancel()
                    }
                    _ = try? await t.value
                }
            }
        }

        // Post-cancellation verification: runtime remains operational
        var postSuccessCount = 0
        for i in 0..<20 {
            let resp: PingResponse = try await runtime.dispatch(
                command: "telegram.ping",
                params: PingParams(nonce: 1000 + i)
            )
            if resp.pong && resp.nonce == 1000 + i {
                postSuccessCount += 1
            }
        }
        XCTAssertEqual(postSuccessCount, 20, "All post-cancellation dispatches must succeed cleanly")
    }
}
