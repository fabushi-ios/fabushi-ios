//
//  adversarial_settings_agent_runner.swift
//  FabushiTests
//
//  Standalone Empirical Adversarial Stress Runner for Milestone 5:
//  Features 27 to 37:
//  - Feature 27: Parallax Collapsing Peer Profile (Elastic Rubberband, Docking, Mute Presets, Block)
//  - Feature 28: Group & Channel Management (Owner Immunity Guard, Banned Cascade Purge, Expired Links, Slow Mode)
//  - Feature 29: Inset Grouped Settings Suite (Current Session Guard, 10-Attempt Lockout, Theme Switch, Cache Purge)
//  - Feature 30: WebRTC Call Stage (3-Ring Ripple Radii Formula, Mute/Speaker Toggles, Packet Loss Recovery)
//  - Feature 31: ACP Turn Cards (Token Streaming, Malformed JSON Fallback, Stop Signal)
//  - Feature 32: 7-State Task FSM & 5-Field Cron (Terminal Protection, 7-Field Rejection, Humanizer)
//  - Feature 33: MCP Approval Gate (10:00 Timer, Obfuscated rm -rf Defense, Timeout Auto-Reject, Wall-Clock Expiry)
//  - Feature 34: Sub-10ms CUA Computer Lease (Touch Clamping, Host Pixel Mapping, Sub-10ms Preemption)
//  - Feature 35: Full-Duplex Talk Mode (RMS Formula, Sub-60ms Barge-in, 320-Chunk Buffer Flush, Dynamic Orb)
//  - Feature 36: SQLite WAL & Tantivy Bridge (10-Table Integrity, Missing Table Error, <mark> Highlight Snippets)
//  - Feature 37: Account Scope Generation Fence (1,000 Switches, Monotonic Counter, Stale Callback Drop, Key Scrub)
//

import CoreGraphics
import Foundation
import SwiftUI
#if canImport(UIKit)
import UIKit
#endif
#if canImport(Darwin)
import Darwin
#endif

@main
struct AdversarialSettingsAgentRunner {

    @MainActor private static var passCount: Int = 0
    @MainActor private static var totalCount: Int = 0

    @MainActor
    private static func runTest(_ name: String, block: () async throws -> Void) async {
        totalCount += 1
        print("[\(totalCount)] Running \(name)...")
        do {
            try await block()
            passCount += 1
            print("  ✓ \(name) PASSED")
        } catch {
            print("  ✗ \(name) FAILED: \(error)")
            exit(1)
        }
    }

    private static func assertTrue(_ condition: @autoclosure () -> Bool, _ message: String = "", file: StaticString = #file, line: UInt = #line) throws {
        if !condition() {
            throw TestFailure("Assertion failed at \(file):\(line): \(message)")
        }
    }

    private static func assertFalse(_ condition: @autoclosure () -> Bool, _ message: String = "", file: StaticString = #file, line: UInt = #line) throws {
        if condition() {
            throw TestFailure("Assertion failed at \(file):\(line): expected false, got true. \(message)")
        }
    }

    private static func assertEqual<T: Equatable>(_ a: T, _ b: T, _ message: String = "", file: StaticString = #file, line: UInt = #line) throws {
        if a != b {
            throw TestFailure("Assertion failed at \(file):\(line): expected \(b), got \(a). \(message)")
        }
    }

    private static func assertAlmostEqual(_ a: CGFloat, _ b: CGFloat, accuracy: CGFloat = 0.001, _ message: String = "", file: StaticString = #file, line: UInt = #line) throws {
        if abs(a - b) > accuracy {
            throw TestFailure("Assertion failed at \(file):\(line): \(a) not almost equal to \(b) (diff: \(abs(a - b)) > \(accuracy)). \(message)")
        }
    }

    struct TestFailure: Error, CustomStringConvertible {
        let description: String
        init(_ description: String) { self.description = description }
    }

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
        print(" ADVERSARIAL EMPIRICAL STRESS RUNNER: Milestone 5 Profile, Settings & Capabilities")
        print(" Features 27 to 37: Profile, Group Admin, Settings, Calls, ACP, Tasks, MCP, CUA, Talk, Search, Fence")
        print(" Environment: Swift 6 Strict Concurrency (@MainActor, actor, Sendable)")
        print("================================================================================")
        let initialRss = getRssMB()
        print("Initial Process RSS: \(String(format: "%.2f", initialRss)) MB\n")

        // =====================================================================
        // FEATURE 27: Collapsing Parallax Peer Profile
        // =====================================================================
        await runTest("F27_ADV01_AvatarScaleRubberBandAndDockingClamps") {
            // Offset 0: exactly 1.0
            try assertAlmostEqual(PeerInfoLayoutCalculator.calculateAvatarScale(offset: 0), 1.0)

            // Offset 60: collapsed halfway
            let scale60 = PeerInfoLayoutCalculator.calculateAvatarScale(offset: 60)
            try assertTrue(scale60 < 1.0 && scale60 > 0.36, "Scale at 60 must be in (0.36, 1.0)")

            // Offset 120: exactly 0.36
            try assertAlmostEqual(PeerInfoLayoutCalculator.calculateAvatarScale(offset: 120), 0.36)

            // Offset 500: clamped to 0.36
            try assertAlmostEqual(PeerInfoLayoutCalculator.calculateAvatarScale(offset: 500), 0.36)

            // Elastic rubberband overscroll: -200 -> 2.0
            try assertAlmostEqual(PeerInfoLayoutCalculator.calculateAvatarScale(offset: -200), 2.0)
        }

        await runTest("F27_ADV02_SharedMediaFilteringAndMutePresets") {
            let vm = PeerInfoViewModel(profile: PeerProfile(id: "p1", displayName: "Tester"))
            vm.addSharedItem(SharedMediaItem(id: "1", type: .media, title: "Photo"))
            vm.addSharedItem(SharedMediaItem(id: "2", type: .files, title: "Doc"))
            vm.addSharedItem(SharedMediaItem(id: "3", type: .links, title: "Link"))

            try assertEqual(vm.filteredSharedItems(for: .media).count, 1)
            try assertEqual(vm.filteredSharedItems(for: .files).count, 1)
            try assertEqual(vm.filteredSharedItems(for: .links).count, 1)
            try assertEqual(vm.filteredSharedItems(for: .voice).count, 0)

            try await vm.setMuted(duration: .oneHour)
            try assertTrue(vm.profile.isMuted)
            try await vm.setMuted(duration: .unmute)
            try assertFalse(vm.profile.isMuted)

            try await vm.toggleBlockUser()
            try assertTrue(vm.profile.isBlocked)
        }

        // =====================================================================
        // FEATURE 28: Group & Channel Management
        // =====================================================================
        await runTest("F28_ADV01_OwnerImmunityGuardAndExpiredInviteRejection") {
            let owner = GroupMemberItem(id: "owner_1", name: "SuperAdmin", role: .owner)
            let admin = GroupMemberItem(id: "admin_1", name: "SubAdmin", role: .admin)
            let vm = GroupChannelAdminViewModel(members: [owner, admin])

            // Demote admin -> succeeds
            try vm.demoteMember(id: "admin_1")
            try assertEqual(vm.members.first(where: { $0.id == "admin_1" })?.role, .member)

            // Demote owner -> throws cannotDemoteOwner
            var ownerDemoted = false
            do {
                try vm.demoteMember(id: "owner_1")
                ownerDemoted = true
            } catch let err as GroupAdminError {
                if case .cannotDemoteOwner = err {
                    // Expected
                } else {
                    throw TestFailure("Wrong error: \(err)")
                }
            }
            try assertFalse(ownerDemoted, "Demoting owner must be blocked by immunity guard")

            // Expired link rejection
            let expiredLink = InviteLinkInfo(id: "l1", url: URL(string: "https://t.me/exp")!, isExpired: true)
            var joinSucceeded = false
            do {
                try vm.joinViaInviteLink(expiredLink)
                joinSucceeded = true
            } catch let err as GroupAdminError {
                try assertEqual(err, .inviteLinkExpired)
            }
            try assertFalse(joinSucceeded, "Joining via expired link must be rejected")
        }

        await runTest("F28_ADV02_BannedUserCascadeDeletionAndSlowMode") {
            let troll = GroupMemberItem(id: "troll_1", name: "BadActor", role: .member)
            let vm = GroupChannelAdminViewModel(members: [troll])

            try await vm.banMember(id: "troll_1", deleteAllMessages: true)
            try assertEqual(vm.members.first?.role, .banned)
            try assertTrue(vm.purgedUserSenderIds.contains("troll_1"))

            vm.slowMode = .m1 // 60s
            try assertEqual(vm.slowMode.rawValue, 60)
            try assertEqual(vm.slowMode.title, "1m")
        }

        // =====================================================================
        // FEATURE 29: Inset Grouped Settings Suite
        // =====================================================================
        await runTest("F29_ADV01_CurrentSessionTerminationGuardAndCachePurge") {
            let vm = TelegramSettingsViewModel()
            try assertEqual(vm.activeSessions.count, 3)

            // Terminate other sessions
            try await vm.terminateOtherSessions()
            try assertEqual(vm.activeSessions.count, 1)
            try assertTrue(vm.activeSessions.first?.isCurrent == true)

            // Terminating current session throws
            var currentTerminated = false
            do {
                try vm.terminateSession(id: "sess_current")
                currentTerminated = true
            } catch let err as SettingsError {
                try assertEqual(err, .cannotTerminateCurrentSession)
            }
            try assertFalse(currentTerminated)

            // Cache purge zeroes cacheBytes
            try assertTrue(vm.storageUsage.cacheBytes > 0)
            try await vm.clearCache()
            try assertEqual(vm.storageUsage.cacheBytes, 0)
        }

        await runTest("F29_ADV02_PasscodeExponentialLockoutTenAttempts") {
            let vm = TelegramSettingsViewModel()
            try assertEqual(vm.recordPasscodeFailure(currentFailures: 2), 0)
            try assertEqual(vm.recordPasscodeFailure(currentFailures: 5), 60)
            try assertEqual(vm.recordPasscodeFailure(currentFailures: 8), 300)
            try assertEqual(vm.recordPasscodeFailure(currentFailures: 10), 900)
            try assertEqual(vm.recordPasscodeFailure(currentFailures: 20), 900)
        }

        // =====================================================================
        // FEATURE 30: WebRTC Call Stage
        // =====================================================================
        await runTest("F30_ADV01_AudioPulseRingsAndPacketLossRecovery") {
            let radii0 = CallAudioPulseCalculator.calculateRingRadii(baseRadius: 60, energy: 0.0)
            try assertEqual(radii0[0], 60.0)

            let radii1 = CallAudioPulseCalculator.calculateRingRadii(baseRadius: 60, energy: 1.0)
            try assertAlmostEqual(radii1[0], 60 * 1.28)
            try assertAlmostEqual(radii1[1], 60 * 1.58)
            try assertAlmostEqual(radii1[2], 60 * 1.95)

            let vm = CallViewModel(peerName: "Alice")
            vm.transitionToConnected()
            vm.handlePacketLoss()
            try assertEqual(vm.state, .reconnecting)
            vm.handlePacketRecovery()
            if case .connected = vm.state {
                // Succeeded
            } else {
                throw TestFailure("Call did not recover to connected")
            }
        }

        // =====================================================================
        // FEATURE 31: ACP Turn Cards
        // =====================================================================
        await runTest("F31_ADV01_ThinkingTokenStreamingAndMalformedJsonFallback") {
            let vm = AgentCardViewModel(agentName: "Bot")
            vm.appendThinkingToken("TokenA ", elapsedMs: 50)
            vm.appendThinkingToken("TokenB", elapsedMs: 100)
            if case .thinking(let tokens, let ms) = vm.phase {
                try assertEqual(tokens, "TokenA TokenB")
                try assertEqual(ms, 100)
            } else {
                throw TestFailure("Expected thinking phase")
            }

            // Malformed JSON parsing
            let malformed = "{unquoted_key: 123"
            let res = ToolCallPayloadParser.parse(rawJson: malformed)
            if case .syntaxError(let raw, _) = res {
                try assertEqual(raw, malformed)
            } else {
                throw TestFailure("Expected syntax error result")
            }

            // Abort turn
            vm.abortTurn()
            try assertEqual(vm.phase, .aborted)
        }

        // =====================================================================
        // FEATURE 32: 7-State Global Task Center
        // =====================================================================
        await runTest("F32_ADV01_7StateFsmTerminalGuardAnd5FieldCron") {
            let task = AgentTaskModel(id: "t1", title: "ETL", state: .pending)
            let vm = TaskCenterViewModel(tasks: [task])

            try vm.transitionTask(id: "t1", to: .assigned)
            try vm.transitionTask(id: "t1", to: .running)
            try vm.transitionTask(id: "t1", to: .completed)

            // Illegal transition from terminal state completed
            var transitioned = false
            do {
                try vm.transitionTask(id: "t1", to: .running)
                transitioned = true
            } catch let err as TaskFsmError {
                if case .illegalStateTransition = err {
                    // Expected
                } else {
                    throw TestFailure("Wrong error: \(err)")
                }
            }
            try assertFalse(transitioned, "Transition from terminal state must be blocked")

            // 7-field cron rejection
            var accepted7 = false
            do {
                try CronRoutineParser.validate(expression: "* * * * * * *")
                accepted7 = true
            } catch let err as CronError {
                try assertEqual(err, .invalidFormat("Invalid 5-field cron format: expected 5 fields, got 7"))
            }
            try assertFalse(accepted7)

            // 5-field cron humanizer
            try assertEqual(CronRoutineParser.humanize(expression: "0 9 * * 1-5"), "Weekdays at 9:00 AM")
            try assertEqual(CronRoutineParser.humanize(expression: "0 0 * * *"), "Every day at midnight")
        }

        // =====================================================================
        // FEATURE 33: MCP Approval Gate
        // =====================================================================
        await runTest("F33_ADV01_CriticalCommandScreeningAndTimeoutRejection") {
            // Dangerous commands
            try assertEqual(ToolRiskEvaluator.evaluate(toolName: "bash", command: "rm -rf /"), .critical)
            try assertEqual(ToolRiskEvaluator.evaluate(toolName: "bash", command: "rm\\ -rf /"), .critical)
            try assertEqual(ToolRiskEvaluator.evaluate(toolName: "bash", command: "eval(base64)"), .critical)
            try assertEqual(ToolRiskEvaluator.evaluate(toolName: "bash", command: "DROP TABLE accounts;"), .critical)
            try assertEqual(ToolRiskEvaluator.evaluate(toolName: "query", command: "select *"), .readOnly)

            let req = McpApprovalRequest(id: "a1", toolName: "rm", arguments: "rm -rf /", ttlSeconds: 600)
            try assertEqual(req.riskLevel, .critical)

            let vm = McpApprovalViewModel(requests: [req])
            try assertTrue(vm.remainingSeconds >= 598 && vm.remainingSeconds <= 600, "Remaining seconds must start near 600s, got \(vm.remainingSeconds)")
            try assertTrue(vm.formattedTime == "10:00" || vm.formattedTime == "09:59", "Formatted time must be ~10:00, got \(vm.formattedTime)")

            // Simulate wall clock expiration on foreground
            vm.handleAppBackgrounded()
            vm.remainingSeconds = 0
            vm.handleAppForegrounded()
            try assertEqual(vm.lastDecision, .timeout)
        }

        // =====================================================================
        // FEATURE 34: Sub-10ms CUA Computer Lease
        // =====================================================================
        await runTest("F34_ADV01_Sub10msEmergencyKillSwitchAndClamping") {
            let vm = CuaScreenOverlayViewModel()
            vm.assignLease(generation: 1)

            // Measure preemption latency (< 10ms)
            let latencyMs = vm.triggerEmergencyStop()
            print("  [CUA Preemption Benchmark] \(String(format: "%.3f", latencyMs)) ms (SLA < 10.0 ms)")
            try assertTrue(latencyMs < 10.0, "Preemption must execute in < 10ms")
            try assertEqual(vm.leaseState, .preempted(byUser: true))
            try assertEqual(vm.currentBrokerGen, 2)

            // Clamping out of bounds
            let clamped = CuaCoordinateTransformer.clampNormalized(point: CGPoint(x: -50, y: 2000))
            try assertEqual(clamped.x, 0.0)
            try assertEqual(clamped.y, 1.0)

            let pixel = CuaCoordinateTransformer.mapToHostPixels(normalizedPoint: CGPoint(x: 0.5, y: 0.5), hostWidth: 1920, hostHeight: 1080)
            try assertEqual(pixel.x, 960)
            try assertEqual(pixel.y, 540)
        }

        // =====================================================================
        // FEATURE 35: Full-Duplex Talk Mode
        // =====================================================================
        await runTest("F35_ADV01_VadRmsFormulaAndSub60msBargeIn") {
            let vm = TalkModeViewModel()
            vm.setAgentSpeaking(withBufferedChunks: 320)
            try assertEqual(vm.queuedAudioBufferChunks, 320)

            // Frame 1, 2, 3 of speech (3 * 20ms = 60ms)
            let speech = [Int16](repeating: 600, count: 320)
            try assertFalse(vm.handlePcmFrame(samples: speech))
            try assertFalse(vm.handlePcmFrame(samples: speech))
            let bargeIn = vm.handlePcmFrame(samples: speech)
            try assertTrue(bargeIn, "3rd consecutive speech frame must trigger barge-in (<60ms)")
            try assertEqual(vm.sessionState, .interrupted)
            try assertEqual(vm.queuedAudioBufferChunks, 0, "Buffer must flush all 320 chunks")

            // Orb scale formula
            let scale0 = VadAudioCalculator.calculateOrbScale(rms: 0, baseRadius: 80)
            try assertEqual(scale0, 80.0)
            let scaleMax = VadAudioCalculator.calculateOrbScale(rms: 1000, baseRadius: 80)
            try assertAlmostEqual(scaleMax, 80 * 1.35)
        }

        // =====================================================================
        // FEATURE 36: SQLite WAL & Tantivy Bridge
        // =====================================================================
        await runTest("F36_ADV01_10TableWalSchemaAndSnippetHighlighting") {
            let bridge = SearchIndexingBridge()
            let isSchemaHealthy = try await bridge.verifySchemaIntegrity()
            try assertTrue(isSchemaHealthy)

            // Dropping a table triggers schemaCorrupted
            await bridge.dropTableForTesting("secret_chats")
            var schemaBroken = false
            do {
                _ = try await bridge.verifySchemaIntegrity()
            } catch let err as SearchIndexError {
                if case .schemaCorrupted(let missing) = err {
                    try assertEqual(missing, ["secret_chats"])
                    schemaBroken = true
                }
            }
            try assertTrue(schemaBroken)

            // Tantivy full-text index & snippet
            let cleanBridge = SearchIndexingBridge()
            try await cleanBridge.indexMessage(doc: SearchDocument(id: 1, chatId: 10, text: "Q3 financial quarterly report"))
            let res = try await cleanBridge.search(query: SearchQueryParams(query: "financial"))
            try assertEqual(res.count, 1)
            try assertTrue(res.first?.snippetWithHighlights.contains("<mark>financial</mark>") == true)
        }

        // =====================================================================
        // FEATURE 37: Account Scope Generation Fence
        // =====================================================================
        await runTest("F37_ADV01_1000SwitchesMonotonicCounterAndLogoutScrub") {
            let manager = AccountScopeManager(initialGeneration: 1, activeAccountId: "user_init")
            let initialGen = await manager.getCurrentGeneration()
            try assertEqual(initialGen, 1)

            // 1,000 rapid switches
            for i in 1...1000 {
                let gen = try await manager.switchAccount(to: "acc_\(i)")
                try assertEqual(gen, UInt64(i + 1))
            }
            let currentGen = await manager.getCurrentGeneration()
            try assertEqual(currentGen, 1001)

            // Old generation 1 rejected
            let gen1Valid = await manager.validateGeneration(1)
            try assertFalse(gen1Valid)
            let gen1001Valid = await manager.validateGeneration(1001)
            try assertTrue(gen1001Valid)

            // Logout epoch scrub
            let logoutGen = await manager.logout()
            try assertEqual(logoutGen, 1002)
            let activeAcc = await manager.getActiveAccountId()
            try assertTrue(activeAcc == nil)
            let hasKey = await manager.hasActiveKey(for: "user_init")
            try assertFalse(hasKey)

            // executeFenced rejection on generation mismatch
            let fenceMgr = AccountScopeManager(initialGeneration: 5)
            var fenceCaught = false
            do {
                _ = try await fenceMgr.executeFenced { taskGen in
                    _ = try await fenceMgr.switchAccount(to: "acc_other")
                    return "result"
                }
            } catch let err as FenceRejectionError {
                if case .staleGeneration(let taskGen, let curGen) = err {
                    try assertEqual(taskGen, 5)
                    try assertEqual(curGen, 6)
                    fenceCaught = true
                }
            }
            try assertTrue(fenceCaught)
        }

        print("\n================================================================================")
        print(" ALL \(passCount) ADVERSARIAL STRESS CHALLENGES PASSED (100% SUCCESS RATE)")
        print(" Final Process RSS: \(String(format: "%.2f", getRssMB())) MB")
        print("================================================================================")
    }
}
