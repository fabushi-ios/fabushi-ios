//
//  challenger_m5_settings_profile_deep_suite.swift
//  FabushiTests
//
//  Empirical Adversarial Stress Suite for Milestone 5 (Features 27, 28, 29):
//  - Feature 27: Profile Parallax Header Math, Damping, Rubber-banding, Mute Presets & Block Toggle
//  - Feature 28: Group & Channel Permissions, Owner Immunity Guards, Expired Link Rejection, Cascade Purge & Slow Mode
//  - Feature 29: Inset Grouped Settings, Current Session Guard, Passcode Exponential Lockout & Concurrent Cache Purge
//

import CoreGraphics
import Foundation
import SwiftUI
#if canImport(Darwin)
import Darwin
#endif

@main
struct ChallengerM5SettingsProfileDeepSuite {

    @MainActor private static var passCount: Int = 0
    @MainActor private static var totalCount: Int = 0

    @MainActor
    private static func runChallenge(_ name: String, block: () async throws -> Void) async {
        totalCount += 1
        print("[\(totalCount)] CHALLENGE: \(name)...")
        do {
            try await block()
            passCount += 1
            print("  ✓ PASSED: \(name)")
        } catch {
            print("  ✗ FAILED: \(name) with error: \(error)")
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
        print(" CHALLENGER EMPIRICAL DEEP SUITE: Features 27, 28, 29 (M5)")
        print(" Rigorous Mathematical, Boundary & Concurrency Stress Verification")
        print("================================================================================")
        let initialRss = getRssMB()
        print("Initial Memory RSS: \(String(format: "%.2f", initialRss)) MB\n")

        // =====================================================================
        // FEATURE 27: Profile Parallax Header & Media Controls
        // =====================================================================

        await runChallenge("F27_CH01_ParallaxScaleContinuousMonotonicityAndClamping") {
            // Verify offset 0
            try assertAlmostEqual(PeerInfoLayoutCalculator.calculateAvatarScale(offset: 0), 1.0)

            // Test 1,000 steps from 0 to 120: strictly decreasing
            var prevScale: CGFloat = 1.0
            for i in 1...120 {
                let offset = CGFloat(i)
                let scale = PeerInfoLayoutCalculator.calculateAvatarScale(offset: offset)
                try assertTrue(scale < prevScale || abs(scale - prevScale) < 0.0001, "Scale at offset \(offset) must be non-increasing")
                try assertTrue(scale >= 0.36, "Scale at offset \(offset) must not drop below 0.36")
                prevScale = scale
            }
            try assertAlmostEqual(prevScale, 0.36, accuracy: 0.001)

            // Beyond 120: strictly clamped at 0.36
            for offset in [120.1, 150.0, 300.0, 1000.0, 10000.0, 50000.0] {
                let scale = PeerInfoLayoutCalculator.calculateAvatarScale(offset: offset)
                try assertAlmostEqual(scale, 0.36, accuracy: 0.0001, "Scale at \(offset) must clamp to 0.36")
            }

            // Pull-down overscroll rubber-band: offset < 0
            // Formula: 1.0 + (|offset| / 200.0)
            for offset in [-1.0, -10.0, -50.0, -100.0, -200.0, -500.0, -1000.0, -5000.0] {
                let scale = PeerInfoLayoutCalculator.calculateAvatarScale(offset: offset)
                let expected = 1.0 + (abs(offset) / 200.0)
                try assertAlmostEqual(scale, expected, accuracy: 0.0001, "Scale at \(offset) rubberband mismatch")
            }
        }

        await runChallenge("F27_CH02_NavBarTitleOpacityThresholdsAndMonotonicity") {
            // Under 60: opacity is 0.0
            for offset in [-500.0, -50.0, 0.0, 10.0, 30.0, 60.0] {
                let opacity = PeerInfoLayoutCalculator.calculateNavBarTitleOpacity(offset: offset)
                try assertAlmostEqual(CGFloat(opacity), 0.0, accuracy: 0.0001, "Opacity at \(offset) must be 0")
            }

            // Between 60 and 120: linear interpolation
            let op75 = PeerInfoLayoutCalculator.calculateNavBarTitleOpacity(offset: 75)
            try assertAlmostEqual(CGFloat(op75), 0.25, accuracy: 0.0001)
            let op90 = PeerInfoLayoutCalculator.calculateNavBarTitleOpacity(offset: 90)
            try assertAlmostEqual(CGFloat(op90), 0.50, accuracy: 0.0001)
            let op105 = PeerInfoLayoutCalculator.calculateNavBarTitleOpacity(offset: 105)
            try assertAlmostEqual(CGFloat(op105), 0.75, accuracy: 0.0001)

            // Over 120: opacity clamped at 1.0
            for offset in [120.0, 150.0, 500.0, 10000.0] {
                let opacity = PeerInfoLayoutCalculator.calculateNavBarTitleOpacity(offset: offset)
                try assertAlmostEqual(CGFloat(opacity), 1.0, accuracy: 0.0001, "Opacity at \(offset) must be 1.0")
            }
        }

        await runChallenge("F27_CH03_MutePresetsAndRapidBlockToggleStress") {
            let profile = PeerProfile(id: "peer_m5", displayName: "Tester M5")
            let vm = PeerInfoViewModel(profile: profile)

            // Test all mute durations
            try await vm.setMuted(duration: .oneHour)
            try assertTrue(vm.profile.isMuted)
            try await vm.setMuted(duration: .eightHours)
            try assertTrue(vm.profile.isMuted)
            try await vm.setMuted(duration: .twoDays)
            try assertTrue(vm.profile.isMuted)
            try await vm.setMuted(duration: .forever)
            try assertTrue(vm.profile.isMuted)
            try await vm.setMuted(duration: .unmute)
            try assertFalse(vm.profile.isMuted)

            // Rapid 2,000 block/unblock toggles
            for _ in 0..<2000 {
                try await vm.toggleBlockUser()
            }
            try assertFalse(vm.profile.isBlocked, "After even number of toggles, block state should be false")
            try await vm.toggleBlockUser()
            try assertTrue(vm.profile.isBlocked, "After odd toggle, block state should be true")
        }

        await runChallenge("F27_CH04_SharedMediaCrossCategoryIsolation") {
            let vm = PeerInfoViewModel(profile: PeerProfile(id: "p_media", displayName: "Media Tester"))

            for i in 1...20 {
                vm.addSharedItem(SharedMediaItem(id: "m_\(i)", type: .media, title: "Photo \(i)"))
                vm.addSharedItem(SharedMediaItem(id: "f_\(i)", type: .files, title: "Document \(i)"))
                vm.addSharedItem(SharedMediaItem(id: "l_\(i)", type: .links, title: "Link \(i)"))
                vm.addSharedItem(SharedMediaItem(id: "v_\(i)", type: .voice, title: "Voice \(i)"))
                vm.addSharedItem(SharedMediaItem(id: "g_\(i)", type: .groups, title: "Group \(i)"))
            }

            try assertEqual(vm.filteredSharedItems(for: .media).count, 20)
            try assertEqual(vm.filteredSharedItems(for: .files).count, 20)
            try assertEqual(vm.filteredSharedItems(for: .links).count, 20)
            try assertEqual(vm.filteredSharedItems(for: .voice).count, 20)
            try assertEqual(vm.filteredSharedItems(for: .groups).count, 20)
        }

        // =====================================================================
        // FEATURE 28: Group & Channel Management
        // =====================================================================

        await runChallenge("F28_CH01_OwnerImmunityDemoteAndBanGuards") {
            let owner = GroupMemberItem(id: "super_owner", name: "Channel Creator", role: .owner)
            let admin = GroupMemberItem(id: "lead_admin", name: "Admin 1", role: .admin)
            let member = GroupMemberItem(id: "regular_user", name: "Member 1", role: .member)
            let vm = GroupChannelAdminViewModel(members: [owner, admin, member])

            // 1. Demoting owner must throw cannotDemoteOwner
            var ownerDemoteFailed = false
            do {
                try vm.demoteMember(id: "super_owner")
            } catch let err as GroupAdminError {
                if case .cannotDemoteOwner = err {
                    ownerDemoteFailed = true
                }
            }
            try assertTrue(ownerDemoteFailed, "Demote owner must throw cannotDemoteOwner")
            try assertEqual(vm.members.first(where: { $0.id == "super_owner" })?.role, .owner)

            // 2. Banning owner must throw permissionDenied
            var ownerBanFailed = false
            do {
                try await vm.banMember(id: "super_owner", deleteAllMessages: true)
            } catch let err as GroupAdminError {
                if case .permissionDenied = err {
                    ownerBanFailed = true
                }
            }
            try assertTrue(ownerBanFailed, "Ban owner must throw permissionDenied")
            try assertEqual(vm.members.first(where: { $0.id == "super_owner" })?.role, .owner)
            try assertFalse(vm.purgedUserSenderIds.contains("super_owner"))

            // 3. Demoting non-existent member throws userNotFound
            var notFoundCaught = false
            do {
                try vm.demoteMember(id: "ghost_user")
            } catch let err as GroupAdminError {
                if case .userNotFound = err {
                    notFoundCaught = true
                }
            }
            try assertTrue(notFoundCaught, "Demoting ghost user must throw userNotFound")
        }

        await runChallenge("F28_CH02_AdminPromotionAndDemotionLifecycle") {
            let member = GroupMemberItem(id: "cand_1", name: "Candidate", role: .member)
            let vm = GroupChannelAdminViewModel(members: [member])

            // Promote member to admin
            try vm.promoteMember(id: "cand_1")
            try assertEqual(vm.members.first?.role, .admin)

            // Demote back to member
            try vm.demoteMember(id: "cand_1")
            try assertEqual(vm.members.first?.role, .member)

            // Promoting owner must not alter role
            let owner = GroupMemberItem(id: "owner_2", name: "Owner", role: .owner)
            let vm2 = GroupChannelAdminViewModel(members: [owner])
            try vm2.promoteMember(id: "owner_2")
            try assertEqual(vm2.members.first?.role, .owner, "Promote owner must leave role as .owner")
        }

        await runChallenge("F28_CH03_InviteLinkExpirationAndJoinValidation") {
            let vm = GroupChannelAdminViewModel()

            // Expired links rejected
            let expiredLink1 = InviteLinkInfo(id: "exp_1", url: URL(string: "https://t.me/+old1")!, isExpired: true)
            var expiredRejected = false
            do {
                try vm.joinViaInviteLink(expiredLink1)
            } catch let err as GroupAdminError {
                try assertEqual(err, .inviteLinkExpired)
                expiredRejected = true
            }
            try assertTrue(expiredRejected, "Expired invite link must be rejected")

            // Active links accepted
            let activeLink = InviteLinkInfo(id: "act_1", url: URL(string: "https://t.me/+active1")!, isExpired: false, usageCount: 42)
            try vm.joinViaInviteLink(activeLink)
        }

        await runChallenge("F28_CH04_CascadeMessagePurgeOnBanAndSlowModeIntervals") {
            let trollA = GroupMemberItem(id: "troll_a", name: "Troll A", role: .member)
            let trollB = GroupMemberItem(id: "troll_b", name: "Troll B", role: .member)
            let vm = GroupChannelAdminViewModel(members: [trollA, trollB])

            // Ban trollA WITH deleteAllMessages
            try await vm.banMember(id: "troll_a", deleteAllMessages: true)
            try assertEqual(vm.members.first(where: { $0.id == "troll_a" })?.role, .banned)
            try assertTrue(vm.purgedUserSenderIds.contains("troll_a"))

            // Ban trollB WITHOUT deleteAllMessages
            try await vm.banMember(id: "troll_b", deleteAllMessages: false)
            try assertEqual(vm.members.first(where: { $0.id == "troll_b" })?.role, .banned)
            try assertFalse(vm.purgedUserSenderIds.contains("troll_b"), "Purge set must not contain troll_b")

            // Slow mode interval values
            try assertEqual(SlowModeInterval.off.rawValue, 0)
            try assertEqual(SlowModeInterval.s10.rawValue, 10)
            try assertEqual(SlowModeInterval.s30.rawValue, 30)
            try assertEqual(SlowModeInterval.m1.rawValue, 60)
            try assertEqual(SlowModeInterval.m5.rawValue, 300)
            try assertEqual(SlowModeInterval.m15.rawValue, 900)
            try assertEqual(SlowModeInterval.h1.rawValue, 3600)
        }

        await runChallenge("F28_CH05_JoinRequestApprovalAndRejectionPipeline") {
            let req1 = JoinRequestItem(id: "r1", name: "Alice")
            let req2 = JoinRequestItem(id: "r2", name: "Bob")
            let vm = GroupChannelAdminViewModel(pendingRequests: [req1, req2])

            try assertEqual(vm.pendingRequests.count, 2)
            // Approve req1 -> becomes member
            try await vm.approveJoinRequest(id: "r1")
            try assertEqual(vm.pendingRequests.count, 1)
            try assertEqual(vm.members.first?.id, "r1")
            try assertEqual(vm.members.first?.role, .member)

            // Reject req2 -> discarded
            try await vm.rejectJoinRequest(id: "r2")
            try assertEqual(vm.pendingRequests.count, 0)
            try assertFalse(vm.members.contains(where: { $0.id == "r2" }))
        }

        // =====================================================================
        // FEATURE 29: Inset Grouped Settings Suite
        // =====================================================================

        await runChallenge("F29_CH01_CurrentSessionTerminationGuardStrictEnforcement") {
            let current = ActiveSessionItem(id: "sess_curr", deviceName: "iPhone 16", isCurrent: true)
            let other1 = ActiveSessionItem(id: "sess_mac", deviceName: "MacBook", isCurrent: false)
            let other2 = ActiveSessionItem(id: "sess_ipad", deviceName: "iPad", isCurrent: false)
            let vm = TelegramSettingsViewModel(activeSessions: [current, other1, other2])

            // 1. Cannot terminate current session directly
            var blocked = false
            do {
                try vm.terminateSession(id: "sess_curr")
            } catch let err as SettingsError {
                try assertEqual(err, .cannotTerminateCurrentSession)
                blocked = true
            }
            try assertTrue(blocked, "Terminating current session directly must throw SettingsError")
            try assertEqual(vm.activeSessions.count, 3)

            // 2. Can terminate other single session
            try vm.terminateSession(id: "sess_mac")
            try assertEqual(vm.activeSessions.count, 2)
            try assertFalse(vm.activeSessions.contains(where: { $0.id == "sess_mac" }))

            // 3. Terminate other sessions cleans all except current
            try await vm.terminateOtherSessions()
            try assertEqual(vm.activeSessions.count, 1)
            try assertEqual(vm.activeSessions.first?.id, "sess_curr")
            try assertTrue(vm.activeSessions.first?.isCurrent == true)

            // 4. Calling terminateOtherSessions again is idempotent
            try await vm.terminateOtherSessions()
            try assertEqual(vm.activeSessions.count, 1)
        }

        await runChallenge("F29_CH02_PasscodeExponentialLockoutExhaustiveSchedule") {
            let vm = TelegramSettingsViewModel()

            // 0..4 attempts -> 0s
            for attempts in 0...4 {
                try assertEqual(vm.recordPasscodeFailure(currentFailures: attempts), 0, "Failures \(attempts) must yield 0s cooldown")
            }

            // 5..7 attempts -> 60s
            for attempts in 5...7 {
                try assertEqual(vm.recordPasscodeFailure(currentFailures: attempts), 60, "Failures \(attempts) must yield 60s cooldown")
            }

            // 8..9 attempts -> 300s
            for attempts in 8...9 {
                try assertEqual(vm.recordPasscodeFailure(currentFailures: attempts), 300, "Failures \(attempts) must yield 300s cooldown")
            }

            // 10..100 attempts -> 900s
            for attempts in [10, 11, 15, 20, 50, 100, 1000] {
                try assertEqual(vm.recordPasscodeFailure(currentFailures: attempts), 900, "Failures \(attempts) must yield 900s cooldown")
            }

            // Negative failure defense
            try assertEqual(vm.recordPasscodeFailure(currentFailures: -1), 0)
        }

        await runChallenge("F29_CH03_AutoLockBoundaryDelays") {
            let vm = TelegramSettingsViewModel()

            // autoLockSeconds = 0 (Immediate)
            vm.autoLockSeconds = 0
            try assertTrue(vm.checkAutoLockOnBackground(foregroundDelaySeconds: 0.0))
            try assertTrue(vm.checkAutoLockOnBackground(foregroundDelaySeconds: 0.001))
            try assertTrue(vm.checkAutoLockOnBackground(foregroundDelaySeconds: 100.0))

            // autoLockSeconds = 60
            vm.autoLockSeconds = 60
            try assertFalse(vm.checkAutoLockOnBackground(foregroundDelaySeconds: 0.0))
            try assertFalse(vm.checkAutoLockOnBackground(foregroundDelaySeconds: 59.999))
            try assertTrue(vm.checkAutoLockOnBackground(foregroundDelaySeconds: 60.0))
            try assertTrue(vm.checkAutoLockOnBackground(foregroundDelaySeconds: 120.0))

            // autoLockSeconds = 300
            vm.autoLockSeconds = 300
            try assertFalse(vm.checkAutoLockOnBackground(foregroundDelaySeconds: 299.9))
            try assertTrue(vm.checkAutoLockOnBackground(foregroundDelaySeconds: 300.0))
        }

        await runChallenge("F29_CH04_ConcurrentCachePurgeStressAndStorageSanity") {
            let vm = TelegramSettingsViewModel()
            try assertEqual(vm.storageUsage.cacheBytes, 2_097_152_000)
            try assertEqual(vm.storageUsage.mediaBytes, 3_145_728_000)

            // Spawn 20 concurrent cache clear calls
            await withTaskGroup(of: Void.self) { group in
                for _ in 0..<20 {
                    group.addTask { @MainActor in
                        try? await vm.clearCache()
                    }
                }
            }

            try assertEqual(vm.storageUsage.cacheBytes, 0, "Cache bytes must be 0 after purge")
            try assertEqual(vm.storageUsage.totalBytes, vm.storageUsage.mediaBytes, "Total bytes must equal media bytes")
            try assertEqual(vm.storageUsage.formattedCache, "Zero KB")
        }

        await runChallenge("F29_CH05_DynamicThemeSwitchingAndSystemDarkModeToggle") {
            let vm = TelegramSettingsViewModel()
            try assertTrue(vm.matchSystemDarkMode)

            for theme in AppearanceTheme.allCases {
                vm.theme = theme
                try assertEqual(vm.theme, theme)
                try assertFalse(theme.icon.isEmpty)
            }

            vm.matchSystemDarkMode.toggle()
            try assertFalse(vm.matchSystemDarkMode)
            vm.matchSystemDarkMode.toggle()
            try assertTrue(vm.matchSystemDarkMode)
        }

        print("\n================================================================================")
        print(" ALL \(passCount) CHALLENGER EMPIRICAL DEEP SUITE TESTS PASSED (100% SUCCESS)")
        print(" Final Memory RSS: \(String(format: "%.2f", getRssMB())) MB")
        print("================================================================================")
    }
}
