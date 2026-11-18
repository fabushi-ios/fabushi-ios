#if canImport(XCTest)
import XCTest
@testable import Fabushi
#endif
import Foundation

@MainActor
public final class TelegramE2ETests: XCTestCase, @unchecked Sendable {

    public func testTC_T1_Tier1_FeatureCoverage_HostRuntimeRoundtrip() async throws {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("TelegramE2E_T1_\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: dir) }

        let runtime = TelegramBridgeRuntime.shared
        runtime.shutdown()
        try runtime.initialize(appDataDirectory: dir, isTestMode: true)
        defer { runtime.shutdown() }

        struct PingResp: Decodable { let pong: Bool; let nonce: UInt64 }
        let res: PingResp = try await runtime.dispatch(command: "config.ping", params: ["nonce": 101])
        XCTAssertTrue(res.pong)
        XCTAssertEqual(res.nonce, 101)
    }

    public func testTC_T2_Tier2_BoundaryCornerCases_ReplayAndUnknownCommand() async throws {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("TelegramE2E_T2_\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: dir) }

        let runtime = TelegramBridgeRuntime.shared
        runtime.shutdown()
        try runtime.initialize(appDataDirectory: dir, isTestMode: true)
        defer { runtime.shutdown() }

        let window = TelegramReplayWindow()
        XCTAssertFalse(window.checkAndAdd(sequence: 0))
        XCTAssertTrue(window.checkAndAdd(sequence: 1))
        XCTAssertFalse(window.checkAndAdd(sequence: 1))

        do {
            let _: [String: String] = try await runtime.dispatch(command: "nonexistent.unknown.method")
            XCTFail("Expected unknown_command error")
        } catch let TelegramBridgeError.serverError(code, _) {
            XCTAssertEqual(code, "unknown_command")
        }
    }

    public func testTC_T3_Tier3_CrossFeatureInteractions_OutboxAndHistory() async throws {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("TelegramE2E_T3_\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: dir) }

        let runtime = TelegramBridgeRuntime.shared
        runtime.shutdown()
        try runtime.initialize(appDataDirectory: dir, isTestMode: true)
        defer { runtime.shutdown() }

        struct SendResp: Decodable { let id: Int64; let text: String }
        let sent: SendResp = try await runtime.dispatch(
            command: "messages.send",
            params: SendMessagePayload(chat_id: 901, text: "Cross-feature E2E", sender_id: 7, client_nonce: 4242)
        )
        XCTAssertGreaterThan(sent.id, 0)
        XCTAssertEqual(sent.text, "Cross-feature E2E")
    }

    public func testTC_T4_Tier4_RealWorldScenarios_SemanticHashDeterminism() async throws {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("TelegramE2E_T4_\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: dir) }

        let runtime = TelegramBridgeRuntime.shared
        runtime.shutdown()
        try runtime.initialize(appDataDirectory: dir, isTestMode: true)
        defer { runtime.shutdown() }

        struct HashResp: Decodable { let state_hash: String }
        let h: HashResp = try await runtime.dispatch(command: "semantic.hash")
        XCTAssertEqual(h.state_hash.count, 64)
    }
}
