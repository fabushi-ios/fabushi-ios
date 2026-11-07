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
    // MARK: - FEATURE 36: SQLite WAL Store & Tantivy Search Bridge
    // =========================================================================

    public func testSqliteWal10TableSchemaVerification() async throws {
        let bridge = SearchIndexingBridge()
        let isHealthy = try await bridge.verifySchemaIntegrity()
        XCTAssertTrue(isHealthy)

        // Verify all 10 required tables are present
        XCTAssertEqual(SearchIndexingBridge.requiredWalTables.count, 10)
        let expectedTables: Set<String> = [
            "messages", "chats", "peers", "drafts", "attachments",
            "pts_sync", "tasks", "sessions", "settings", "secret_chats"
        ]
        XCTAssertEqual(SearchIndexingBridge.requiredWalTables, expectedTables)
    }

    public func testSqliteMissingTableError() async {
        let bridge = SearchIndexingBridge()
        await bridge.dropTableForTesting("secret_chats")

        do {
            _ = try await bridge.verifySchemaIntegrity()
            XCTFail("Expected schemaCorrupted error")
        } catch let err as SearchIndexError {
            if case .schemaCorrupted(let missing) = err {
                XCTAssertEqual(missing, ["secret_chats"])
            } else {
                XCTFail("Wrong error type: \(err)")
            }
        } catch {
            XCTFail("Unexpected error: \(error)")
        }
    }

    public func testTantivySearchSnippetHighlights() async throws {
        let bridge = SearchIndexingBridge()

        let doc1 = SearchDocument(id: 101, chatId: 1, text: "The quarterly financial report is ready for review.")
        let doc2 = SearchDocument(id: 102, chatId: 1, text: "Please review the presentation slides.")

        try await bridge.indexMessage(doc: doc1)
        try await bridge.indexMessage(doc: doc2)

        let query = SearchQueryParams(query: "financial")
        let results = try await bridge.search(query: query)

        XCTAssertEqual(results.count, 1)
        XCTAssertEqual(results.first?.id, 101)
        XCTAssertTrue(results.first?.snippetWithHighlights.contains("<mark>financial</mark>") == true, "Must format match with <mark> tags")
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
