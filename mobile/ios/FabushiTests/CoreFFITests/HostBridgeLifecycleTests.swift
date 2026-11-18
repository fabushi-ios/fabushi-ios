#if canImport(XCTest)
import XCTest
@testable import Fabushi

final class HostBridgeLifecycleTests: XCTestCase {

    private func makeIsolatedDir(_ suffix: String) -> URL {
        FileManager.default.temporaryDirectory
            .appendingPathComponent("HostBridgeLifecycle_\(suffix)_\(UUID().uuidString)", isDirectory: true)
    }

    func testDefaultProductionServerURLAndEndpointNormalization() async throws {
        XCTAssertEqual(ServerEndpointConfig.defaultProductionURL, "http://34.46.77.133:8080")
        XCTAssertEqual(ServerEndpointConfig.normalize("34.46.77.133:8080/"), "http://34.46.77.133:8080")
        XCTAssertEqual(
            ServerEndpointConfig(serverURL: "http://34.46.77.133:8080").webSocketURL,
            "ws://34.46.77.133:8080/ws"
        )
        XCTAssertEqual(HostBridgeClient.protoMaxPacketSize, 16 * 1024 * 1024)
    }

    func testHostLifecyclePingServerConfigAndFailClosedUnknownCommand() async throws {
        let sandbox = makeIsolatedDir("lifecycle")
        defer { try? FileManager.default.removeItem(at: sandbox) }

        let client = HostBridgeClient(keychain: KeychainSessionStore(serviceNamespace: "test.lifecycle.\(UUID().uuidString)"))
        let initializedBefore = await client.isInitialized
        XCTAssertFalse(initializedBefore)

        try await client.initialize(appDataDirectory: sandbox, isTestMode: true)
        let initializedAfter = await client.isInitialized
        XCTAssertTrue(initializedAfter)

        // 1. Ping round-trip
        let pong = try await client.ping(nonce: 123_456)
        XCTAssertTrue(pong.pong)
        XCTAssertEqual(pong.nonce, 123_456)
        XCTAssertEqual(pong.version, "1.3.0")

        // 2. Server URL configuration & persistence
        let updated = try await client.setServerURL("http://34.46.77.133:8080", locale: "zh-CN")
        XCTAssertEqual(updated.serverUrl, "http://34.46.77.133:8080")
        XCTAssertEqual(updated.locale, "zh-CN")

        // 3. Fresh unauthenticated chats.list MUST return empty array (zero fake sample_chats!)
        struct EmptyChatItem: Decodable, Sendable { let chat_id: Int64? }
        let initialChats: [EmptyChatItem] = try await client.call("chats.list")
        XCTAssertTrue(initialChats.isEmpty, "Fresh LocalStore must return [] without fake sample_chats")

        // 4. Fail-closed unknown_command rejection (zero wildcard 'acknowledged'!)
        do {
            let _: HostPingResponse = try await client.call("nonexistent.facade.command")
            XCTFail("Unknown command must fail closed")
        } catch let error as HostBridgeError {
            XCTAssertTrue(error.isUnknownCommand, "Expected unknown_command, got \(error)")
        }

        // 5. Deliberate Rust panic caught across C-ABI without crashing host
        do {
            let _: HostPingResponse = try await client.call("debug.trigger_panic")
            XCTFail("Expected recovered Rust panic error")
        } catch let error as HostBridgeError {
            guard case .rustPanic(let msg) = error else {
                XCTFail("Expected .rustPanic, got \(error)")
                return
            }
            XCTAssertTrue(msg.contains("Intentional test panic"))
        }

        // Host remains responsive immediately after recovered panic
        let afterPanic = try await client.ping(nonce: 999)
        XCTAssertEqual(afterPanic.nonce, 999)

        // 6. Clean shutdown & re-initialization
        await client.shutdown()
        let closedState = await client.isInitialized
        XCTAssertFalse(closedState)

        try await client.initialize(appDataDirectory: sandbox, isTestMode: true)
        let persistedCfg = try await client.getServerConfig()
        XCTAssertEqual(persistedCfg.locale, "zh-CN")
        await client.shutdown()
    }

    func testSlidingReplayWindowDuplicateAndStaleRejection() throws {
        let window = try HostSlidingReplayWindow()
        XCTAssertFalse(window.checkAndAdd(sequence: 0))
        XCTAssertTrue(window.checkAndAdd(sequence: 1))
        XCTAssertFalse(window.checkAndAdd(sequence: 1))
        XCTAssertTrue(window.checkAndAdd(sequence: 200))
        // Sequence 50 is >128 behind 200 -> stale rejected
        XCTAssertFalse(window.checkAndAdd(sequence: 50))
        // Sequence 150 is within 128 window and unseen -> admitted once
        XCTAssertTrue(window.checkAndAdd(sequence: 150))
        XCTAssertFalse(window.checkAndAdd(sequence: 150))
    }
}
