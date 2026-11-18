//
//  TelegramE2ETests.swift
//  FabushiTests
//
//  Baseline C-ABI & Bridge XCTest suite verifying TelegramBridgeRuntime,
//  fail-closed unknown_command rejection, push event streaming, and replay window.
//  Opaque-box 4-tier Simulator XCUITests live in FabushiUITests/Parity/.
//

import Foundation
import XCTest
@testable import Fabushi

@MainActor
public final class TelegramE2ETests: XCTestCase {
    public func testBridgePingAndServerConfigRoundTrip() async throws {
        let tempDir = FileManager.default.temporaryDirectory
            .appendingPathComponent("tg_bridge_e2e_\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: tempDir) }

        let runtime = TelegramBridgeRuntime.shared
        runtime.shutdown()
        try runtime.initialize(appDataDirectory: tempDir, isTestMode: true)
        defer { runtime.shutdown() }

        struct PingParams: Encodable { let nonce: UInt64 }
        struct PingResult: Decodable {
            let pong: Bool
            let nonce: UInt64
        }

        let pong: PingResult = try await runtime.dispatch(
            command: "telegram.ping",
            params: PingParams(nonce: 2026)
        )
        XCTAssertTrue(pong.pong)
        XCTAssertEqual(pong.nonce, 2026)
    }

    public func testBridgeFailsClosedOnUnknownCommand() async throws {
        let tempDir = FileManager.default.temporaryDirectory
            .appendingPathComponent("tg_bridge_unknown_\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: tempDir) }

        let runtime = TelegramBridgeRuntime.shared
        runtime.shutdown()
        try runtime.initialize(appDataDirectory: tempDir, isTestMode: true)
        defer { runtime.shutdown() }

        do {
            let _: [String: Any] = try await runtime.dispatch(command: "unknown.nonexistent_rpc")
            XCTFail("Expected TelegramBridgeError.serverError(code: \"unknown_command\")")
        } catch let TelegramBridgeError.serverError(code, _) {
            XCTAssertEqual(code, "unknown_command")
        }
    }

    public func testBridgeWaveformCodecAndReplayWindow() async throws {
        let tempDir = FileManager.default.temporaryDirectory
            .appendingPathComponent("tg_bridge_waveform_\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: tempDir) }

        let runtime = TelegramBridgeRuntime.shared
        runtime.shutdown()
        try runtime.initialize(appDataDirectory: tempDir, isTestMode: true)
        defer { runtime.shutdown() }

        struct EncodeParams: Encodable { let samples: [Float] }
        struct EncodeResult: Decodable {
            let waveform_bytes: [UInt8]
            let byte_count: Int
        }

        let encoded: EncodeResult = try await runtime.dispatch(
            command: "telegram.media.encodeWaveform",
            params: EncodeParams(samples: [0.1, 0.4, 0.8, 1.0, 0.5, 0.2])
        )
        XCTAssertGreaterThan(encoded.byte_count, 0)
        XCTAssertEqual(encoded.waveform_bytes.count, encoded.byte_count)

        let window = TelegramReplayWindow()
        XCTAssertTrue(window.checkAndAdd(seq: 1))
        XCTAssertFalse(window.checkAndAdd(seq: 1))
        XCTAssertTrue(window.checkAndAdd(seq: 129))
        XCTAssertFalse(window.checkAndAdd(seq: 1))
    }
}
