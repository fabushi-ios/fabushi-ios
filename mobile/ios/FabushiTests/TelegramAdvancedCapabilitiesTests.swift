//
//  TelegramAdvancedCapabilitiesTests.swift
//  FabushiTests
//
//  Comprehensive test suite covering Features 34, 35, 36, and 37:
//  - Feature 34: Sub-10ms CUA Computer Lease
//  - Feature 35: Full-Duplex Talk Mode Overlay
//  - Feature 36: SQLite WAL Store & Tantivy Search Bridge
//  - Feature 37: Account Scope Generation Fence
//

#if canImport(XCTest)
import XCTest
#endif
#if canImport(Fabushi)
@testable import Fabushi
#endif
import SwiftUI

@MainActor
public final class TelegramAdvancedCapabilitiesTests: XCTestCase, @unchecked Sendable {

    // =========================================================================
    // MARK: - FEATURE 34: Sub-10ms CUA Computer Lease
    // =========================================================================

    public func testCuaEmergencyKillSwitchPreemptionLatency() {
        let vm = CuaScreenOverlayViewModel()
        vm.assignLease(generation: 1)

        // Measure emergency takeover latency (must be strictly < 10ms)
        let latencyMs = vm.triggerEmergencyStop()

        XCTAssertLessThan(latencyMs, 10.0, "Emergency preemption must complete in < 10ms (actual: \(latencyMs)ms)")
        XCTAssertEqual(vm.leaseState, .preempted(byUser: true))
        XCTAssertEqual(vm.currentBrokerGen, 2)
        XCTAssertTrue(vm.clickIndicators.isEmpty)
    }

    public func testCuaCoordinateClamping() {
        // Out-of-bounds negative coordinates
        let pNegative = CGPoint(x: -15.0, y: -50.0)
        let clampedNeg = CuaCoordinateTransformer.clampNormalized(point: pNegative)
        XCTAssertEqual(clampedNeg.x, 0.0)
        XCTAssertEqual(clampedNeg.y, 0.0)

        // Out-of-bounds overflow coordinates
        let pOverflow = CGPoint(x: 25.0, y: 1500.0)
        let clampedOver = CuaCoordinateTransformer.clampNormalized(point: pOverflow)
        XCTAssertEqual(clampedOver.x, 1.0)
        XCTAssertEqual(clampedOver.y, 1.0)

        // Normal interior coordinate
        let pNormal = CGPoint(x: 0.35, y: 0.72)
        let clampedNorm = CuaCoordinateTransformer.clampNormalized(point: pNormal)
        XCTAssertEqual(clampedNorm.x, 0.35, accuracy: 0.001)
        XCTAssertEqual(clampedNorm.y, 0.72, accuracy: 0.001)
    }

    public func testCuaHostPixelMapping() {
        let center = CGPoint(x: 0.5, y: 0.5)
        let mapped = CuaCoordinateTransformer.mapToHostPixels(normalizedPoint: center, hostWidth: 1920, hostHeight: 1080)
        XCTAssertEqual(mapped.x, 960)
        XCTAssertEqual(mapped.y, 540)
    }

    // =========================================================================
    // MARK: - FEATURE 35: Full-Duplex Talk Mode Overlay
    // =========================================================================

    public func testVadBargeInSpeechDetection() {
        let vm = TalkModeViewModel()
        vm.setAgentSpeaking(withBufferedChunks: 320)

        // Synthetic 20ms frame of silence (RMS ~ 10.0)
        let silentSamples = [Int16](repeating: 10, count: 320)
        let triggeredSilent = vm.handlePcmFrame(samples: silentSamples)
        XCTAssertFalse(triggeredSilent)
        XCTAssertEqual(vm.sessionState, .speaking)

        // Synthetic 20ms frame of speech (amplitude 800 -> RMS ~ 800 > threshold 300)
        let speechSamples = [Int16](repeating: 800, count: 320)

        // Frame 1: consecutive count = 1
        XCTAssertFalse(vm.handlePcmFrame(samples: speechSamples))
        XCTAssertEqual(vm.consecutiveSpeechFrames, 1)

        // Frame 2: consecutive count = 2
        XCTAssertFalse(vm.handlePcmFrame(samples: speechSamples))
        XCTAssertEqual(vm.consecutiveSpeechFrames, 2)

        // Frame 3: consecutive count = 3 -> Sub-60ms barge-in triggers!
        let bargeInTriggered = vm.handlePcmFrame(samples: speechSamples)
        XCTAssertTrue(bargeInTriggered, "Barge-in interrupt must trigger on 3rd consecutive frame (<60ms)")
        XCTAssertEqual(vm.sessionState, .interrupted)
    }

    public func testTalkMode320ChunkBufferFlush() {
        let vm = TalkModeViewModel()
        vm.setAgentSpeaking(withBufferedChunks: 320)
        XCTAssertEqual(vm.queuedAudioBufferChunks, 320)

        let flushed = vm.flushAudioBuffer()
        XCTAssertEqual(flushed, 320, "Must flush all 320 queued chunks immediately")
        XCTAssertEqual(vm.queuedAudioBufferChunks, 0)
    }

    public func testVadOrbScaleMath() {
        let baseR: CGFloat = 80.0

        // Zero RMS
        let r0 = VadAudioCalculator.calculateOrbScale(rms: 0, baseRadius: baseR)
        XCTAssertEqual(r0, baseR, accuracy: 0.001)

        // 250 RMS (half ceiling)
        let r250 = VadAudioCalculator.calculateOrbScale(rms: 250, baseRadius: baseR)
        XCTAssertTrue(r250 > baseR && r250 < baseR * 1.35)

        // 500+ RMS (ceiling saturated)
        let rMax = VadAudioCalculator.calculateOrbScale(rms: 1000, baseRadius: baseR)
        XCTAssertEqual(rMax, baseR * 1.35, accuracy: 0.001)
    }

    // =========================================================================
    // MARK: - FEATURE 19 & C-ABI: Real Rust LocalStore Persistence & Fail-Closed Dispatch
    // =========================================================================

    public func testRustLocalStorePersistenceRoundTripViaCAbi() throws {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("tg_store_roundtrip_\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: dir) }

        let host = try XCTUnwrap(dir.path.withCString { telegram_app_host_create_test($0) })
        defer {
            telegram_bridge_register_event_callback(host, nil, nil)
            telegram_app_host_destroy(host)
        }

        let sendReq = #"{"command":"telegram.sendMessage","params":{"chat_id":4242,"sender_id":7,"client_nonce":9001,"text":"Real Rust SQLite WAL store test"}}"#
        let sendPtr = try XCTUnwrap(sendReq.withCString { telegram_app_host_dispatch(host, $0) })
        let sendJson = String(cString: sendPtr)
        telegram_app_host_free_string(sendPtr)
        XCTAssertTrue(sendJson.contains(#""ok":true"#), "Expected sendMessage ok:true, got \(sendJson)")

        let getReq = #"{"command":"telegram.getMessages","params":{"chat_id":4242}}"#
        let getPtr = try XCTUnwrap(getReq.withCString { telegram_app_host_dispatch(host, $0) })
        let getJson = String(cString: getPtr)
        telegram_app_host_free_string(getPtr)
        XCTAssertTrue(getJson.contains(#""ok":true"#), "Expected getMessages ok:true, got \(getJson)")
        XCTAssertTrue(getJson.contains("Real Rust SQLite WAL store test"))
    }

    public func testDispatchRejectsUnknownCommandFailClosed() throws {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("tg_unknown_cmd_\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: dir) }

        let host = try XCTUnwrap(dir.path.withCString { telegram_app_host_create_test($0) })
        defer {
            telegram_bridge_register_event_callback(host, nil, nil)
            telegram_app_host_destroy(host)
        }

        let badReq = #"{"command":"telegram.nonExistentCommand","params":{}}"#
        let ptr = try XCTUnwrap(badReq.withCString { telegram_app_host_dispatch(host, $0) })
        let raw = String(cString: ptr)
        telegram_app_host_free_string(ptr)

        let data = try XCTUnwrap(raw.data(using: .utf8))
        let parsed = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        XCTAssertEqual(parsed["ok"] as? Bool, false)
        let err = try XCTUnwrap(parsed["error"] as? [String: Any])
        XCTAssertEqual(err["code"] as? String, "unknown_command")
    }

    public func testNullCallbackUnregistrationIsSafeAcrossDestroy() throws {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("tg_null_cb_\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: dir) }

        let host = try XCTUnwrap(dir.path.withCString { telegram_app_host_create_test($0) })
        telegram_bridge_register_event_callback(host, nil, nil)

        let pingReq = #"{"command":"telegram.ping","params":{"nonce":99}}"#
        let ptr = try XCTUnwrap(pingReq.withCString { telegram_app_host_dispatch(host, $0) })
        let raw = String(cString: ptr)
        telegram_app_host_free_string(ptr)
        XCTAssertTrue(raw.contains(#""ok":true"#))

        telegram_app_host_destroy(host)
    }

    // =========================================================================
    // MARK: - FEATURE 37: Account Scope Generation Fence
    // =========================================================================

    public func testAccountScopeMonotonicIncrementAndStaleTaskDrop() async throws {
        let manager = AccountScopeManager(initialGeneration: 1)
        let initialGen = await manager.getCurrentGeneration()
        XCTAssertEqual(initialGen, 1)

        // 1,000 rapid account switches
        for i in 1...1000 {
            let newGen = try await manager.switchAccount(to: "acc_\(i)")
            XCTAssertEqual(newGen, UInt64(i + 1))
        }

        let finalGen = await manager.getCurrentGeneration()
        XCTAssertEqual(finalGen, 1001)

        // Gen 1 task callback screening
        let isGen1Valid = await manager.validateGeneration(1)
        XCTAssertFalse(isGen1Valid, "Gen 1 callback must be rejected by active gen 1001 fence")

        let isGen1001Valid = await manager.validateGeneration(1001)
        XCTAssertTrue(isGen1001Valid)
    }

    public func testAccountLogoutScrubEpoch() async {
        let manager = AccountScopeManager(initialGeneration: 5, activeAccountId: "user_a")
        let hasKeyBefore = await manager.hasActiveKey(for: "user_a")
        XCTAssertTrue(hasKeyBefore)

        let newEpoch = await manager.logout()
        XCTAssertEqual(newEpoch, 6)
        let activeAcc = await manager.getActiveAccountId()
        XCTAssertNil(activeAcc)
        let hasKeyAfter = await manager.hasActiveKey(for: "user_a")
        XCTAssertFalse(hasKeyAfter, "Crypto keys must be purged from RAM on logout")
    }

    public func testFencedExecutionRejectionOnGenerationMismatch() async throws {
        let manager = AccountScopeManager(initialGeneration: 10)

        do {
            _ = try await manager.executeFenced { taskGen in
                // Interleaving account switch advances generation mid-task
                _ = try await manager.switchAccount(to: "acc_other")
                return "completed result"
            }
            XCTFail("executeFenced should have thrown staleGeneration")
        } catch let err as FenceRejectionError {
            if case .staleGeneration(let taskGen, let curGen) = err {
                XCTAssertEqual(taskGen, 10)
                XCTAssertEqual(curGen, 11)
            } else {
                XCTFail("Wrong rejection error: \(err)")
            }
        } catch {
            XCTFail("Unexpected error: \(error)")
        }
    }
}
