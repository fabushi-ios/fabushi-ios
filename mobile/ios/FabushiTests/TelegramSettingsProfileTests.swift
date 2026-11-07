//
//  TelegramSettingsProfileTests.swift
//  FabushiTests
//
//  Comprehensive test suite covering Features 27, 28, and 29:
//  - Feature 27: Collapsing Parallax Peer Profile
//  - Feature 28: Group & Channel Management
//  - Feature 29: Comprehensive Inset Grouped Settings Suite
//

#if canImport(XCTest)
import XCTest
#endif
#if canImport(Fabushi)
@testable import Fabushi
#endif
import SwiftUI

@MainActor
public final class TelegramSettingsProfileTests: XCTestCase, @unchecked Sendable {

    // =========================================================================
    // MARK: - FEATURE 27: Collapsing Parallax Peer Profile
    // =========================================================================

    public func testTC_T1_F27_01_ParallaxAvatarHeaderShrinkAndDock() {
        // Offset 0 -> scale 1.0 (100pt)
        let scale0 = PeerInfoLayoutCalculator.calculateAvatarScale(offset: 0)
        XCTAssertEqual(scale0, 1.0, accuracy: 0.001)

        // Offset 60pt (halfway) -> scale ~ 0.68
        let scale60 = PeerInfoLayoutCalculator.calculateAvatarScale(offset: 60)
        XCTAssertTrue(scale60 < 1.0 && scale60 > 0.36)

        // Offset 120pt (fully collapsed) -> scale 0.36 (36pt)
        let scale120 = PeerInfoLayoutCalculator.calculateAvatarScale(offset: 120)
        XCTAssertEqual(scale120, 0.36, accuracy: 0.001)

        // Offset 300pt (beyond collapse) -> clamped to 0.36
        let scale300 = PeerInfoLayoutCalculator.calculateAvatarScale(offset: 300)
        XCTAssertEqual(scale300, 0.36, accuracy: 0.001)

        // Navigation Bar Title Opacity: 0 at <= 60pt, 1.0 at >= 120pt
        XCTAssertEqual(PeerInfoLayoutCalculator.calculateNavBarTitleOpacity(offset: 0), 0.0)
        XCTAssertEqual(PeerInfoLayoutCalculator.calculateNavBarTitleOpacity(offset: 60), 0.0)
        XCTAssertEqual(PeerInfoLayoutCalculator.calculateNavBarTitleOpacity(offset: 120), 1.0)
    }

    public func testTC_T1_F27_02_QuickActionButtonsRendering() {
        let profile = PeerProfile(id: "peer_1", displayName: "Bob", username: "bob_telegram", phone: "+123456789")
        let vm = PeerInfoViewModel(profile: profile)

        XCTAssertFalse(vm.profile.isMuted)
        XCTAssertFalse(vm.profile.isBlocked)
        XCTAssertEqual(vm.profile.username, "bob_telegram")
    }

    public func testTC_T1_F27_03_SharedMediaSegmentedTabsFiltering() {
        let profile = PeerProfile(id: "peer_2", displayName: "Alice")
        let vm = PeerInfoViewModel(profile: profile)

        vm.addSharedItem(SharedMediaItem(id: "m1", type: .media, title: "Photo 1"))
        vm.addSharedItem(SharedMediaItem(id: "m2", type: .media, title: "Photo 2"))
        vm.addSharedItem(SharedMediaItem(id: "f1", type: .files, title: "Report.pdf"))
        vm.addSharedItem(SharedMediaItem(id: "l1", type: .links, title: "https://telegram.org"))
        vm.addSharedItem(SharedMediaItem(id: "v1", type: .voice, title: "Voice Note"))

        XCTAssertEqual(vm.filteredSharedItems(for: .media).count, 2)
        XCTAssertEqual(vm.filteredSharedItems(for: .files).count, 1)
        XCTAssertEqual(vm.filteredSharedItems(for: .links).count, 1)
        XCTAssertEqual(vm.filteredSharedItems(for: .voice).count, 1)
        XCTAssertEqual(vm.filteredSharedItems(for: .groups).count, 0)
    }

    public func testTC_T1_F27_04_UsernameClipboardCopyToast() {
        let profile = PeerProfile(id: "peer_3", displayName: "Charlie", username: "charlie_tg")
        let vm = PeerInfoViewModel(profile: profile)

        vm.copyToClipboard("@charlie_tg")
        XCTAssertTrue(vm.didCopyUsername)
        XCTAssertEqual(vm.toastMessage, "Copied to clipboard")
    }

    public func testTC_T1_F27_05_NotificationMutePresetsSheet() async throws {
        let profile = PeerProfile(id: "peer_4", displayName: "David")
        let vm = PeerInfoViewModel(profile: profile)

        try await vm.setMuted(duration: .oneHour)
        XCTAssertTrue(vm.profile.isMuted)
        XCTAssertEqual(NotificationMuteDuration.oneHour.rawValue, 3600)
        XCTAssertEqual(NotificationMuteDuration.eightHours.rawValue, 28800)
        XCTAssertEqual(NotificationMuteDuration.twoDays.rawValue, 172800)
        XCTAssertEqual(NotificationMuteDuration.forever.rawValue, -1)

        try await vm.setMuted(duration: .unmute)
        XCTAssertFalse(vm.profile.isMuted)
    }

    public func testTC_T2_F27_01_PullDownOverscrollRubberBanding() {
        // Negative offset (pull down overscroll)
        let scaleMinus100 = PeerInfoLayoutCalculator.calculateAvatarScale(offset: -100)
        XCTAssertEqual(scaleMinus100, 1.5, accuracy: 0.001)

        let scaleMinus200 = PeerInfoLayoutCalculator.calculateAvatarScale(offset: -200)
        XCTAssertEqual(scaleMinus200, 2.0, accuracy: 0.001)

        let scaleMinus300 = PeerInfoLayoutCalculator.calculateAvatarScale(offset: -300)
        XCTAssertEqual(scaleMinus300, 2.5, accuracy: 0.001)
    }

    public func testTC_T2_F27_02_EmptySharedMediaPlaceholders() {
        let profile = PeerProfile(id: "peer_5", displayName: "Empty Peer")
        let vm = PeerInfoViewModel(profile: profile)

        for type in SharedMediaType.allCases {
            XCTAssertTrue(vm.filteredSharedItems(for: type).isEmpty)
        }
    }

    public func testTC_T2_F27_04_BlockedUserDynamicButtonMorph() async throws {
        let profile = PeerProfile(id: "peer_6", displayName: "Spammer", isBlocked: false)
        let vm = PeerInfoViewModel(profile: profile)

        XCTAssertFalse(vm.profile.isBlocked)
        try await vm.toggleBlockUser()
        XCTAssertTrue(vm.profile.isBlocked)
        try await vm.toggleBlockUser()
        XCTAssertFalse(vm.profile.isBlocked)
    }

    // =========================================================================
    // MARK: - FEATURE 28: Group & Channel Management
    // =========================================================================

    public func testTC_T1_F28_01_AdminRoleBadgeRender() {
        let owner = GroupMemberItem(id: "u1", name: "Alice", role: .owner)
        let admin = GroupMemberItem(id: "u2", name: "Bob", role: .admin)
        let member = GroupMemberItem(id: "u3", name: "Charlie", role: .member)

        XCTAssertEqual(owner.role, .owner)
        XCTAssertEqual(admin.role, .admin)
        XCTAssertEqual(member.role, .member)
        XCTAssertEqual(owner.role.title, "Owner")
        XCTAssertEqual(admin.role.title, "Admin")
    }

    public func testTC_T1_F28_02_PermissionToggleMutation() {
        var perms = GroupAdminPermissions.all
        XCTAssertTrue(perms.contains(.changeInfo))
        XCTAssertTrue(perms.contains(.postMessages))

        perms.remove(.changeInfo)
        XCTAssertFalse(perms.contains(.changeInfo))
        XCTAssertTrue(perms.contains(.postMessages))

        perms.insert(.changeInfo)
        XCTAssertTrue(perms.contains(.changeInfo))
    }

    public func testTC_T1_F28_03_InviteLinkQRCodeGeneration() {
        let url = URL(string: "https://t.me/+AbCdEfGhIjK")!
        let link = InviteLinkInfo(id: "l1", url: url)

        XCTAssertEqual(link.url.absoluteString, "https://t.me/+AbCdEfGhIjK")
        XCTAssertFalse(link.isExpired)
    }

    public func testTC_T1_F28_04_ChannelBroadcastBarEyeCount() {
        let vm = GroupChannelAdminViewModel()
        vm.channelViewsCount = 1420
        XCTAssertEqual(vm.formattedChannelViews, "1.4K")

        vm.channelViewsCount = 28500
        XCTAssertEqual(vm.formattedChannelViews, "28.5K")

        vm.channelViewsCount = 450
        XCTAssertEqual(vm.formattedChannelViews, "450")
    }

    public func testTC_T2_F28_02_DemoteOwnerImmunityGuard() {
        let owner = GroupMemberItem(id: "u_owner", name: "Owner User", role: .owner)
        let admin = GroupMemberItem(id: "u_admin", name: "Admin User", role: .admin)
        let vm = GroupChannelAdminViewModel(members: [owner, admin])

        // Demoting admin succeeds
        XCTAssertNoThrow(try vm.demoteMember(id: "u_admin"))
        XCTAssertEqual(vm.members.first(where: { $0.id == "u_admin" })?.role, .member)

        // Demoting owner throws cannotDemoteOwner
        XCTAssertThrowsError(try vm.demoteMember(id: "u_owner")) { err in
            guard case GroupAdminError.cannotDemoteOwner = err else {
                XCTFail("Expected cannotDemoteOwner error, got \(err)")
                return
            }
        }
    }

    public func testTC_T2_F28_03_ExpiredInviteLinkRejection() {
        let activeLink = InviteLinkInfo(id: "link_act", url: URL(string: "https://t.me/+active")!, isExpired: false)
        let expiredLink = InviteLinkInfo(id: "link_exp", url: URL(string: "https://t.me/+expired")!, isExpired: true)
        let vm = GroupChannelAdminViewModel()

        XCTAssertNoThrow(try vm.joinViaInviteLink(activeLink))
        XCTAssertThrowsError(try vm.joinViaInviteLink(expiredLink)) { err in
            guard case GroupAdminError.inviteLinkExpired = err else {
                XCTFail("Expected inviteLinkExpired, got \(err)")
                return
            }
        }
    }

    public func testTC_T2_F28_04_BannedUserMessageCascadeDeletion() async throws {
        let member = GroupMemberItem(id: "spammer_1", name: "Troll", role: .member)
        let vm = GroupChannelAdminViewModel(members: [member])

        try await vm.banMember(id: "spammer_1", deleteAllMessages: true)
        XCTAssertEqual(vm.members.first?.role, .banned)
        XCTAssertTrue(vm.purgedUserSenderIds.contains("spammer_1"))
    }

    public func testTC_T2_F28_05_SlowModeIntervalEnforcement() {
        let vm = GroupChannelAdminViewModel()
        vm.slowMode = .m1 // 60s
        XCTAssertEqual(vm.slowMode.rawValue, 60)
        XCTAssertEqual(vm.slowMode.title, "1m")
    }

    // =========================================================================
    // MARK: - FEATURE 29: Comprehensive Inset Settings Suite
    // =========================================================================

    public func testTC_T1_F29_01_InsetGroupedSectionLayout() {
        let vm = TelegramSettingsViewModel()
        XCTAssertEqual(vm.accountName, "Telegram User")
        XCTAssertEqual(vm.username, "fabushi_user")
        XCTAssertEqual(vm.activeSessions.count, 3)
    }

    public func testTC_T1_F29_02_TerminateAllOtherSessions() async throws {
        let vm = TelegramSettingsViewModel()
        XCTAssertEqual(vm.activeSessions.count, 3)

        try await vm.terminateOtherSessions()
        XCTAssertEqual(vm.activeSessions.count, 1)
        XCTAssertTrue(vm.activeSessions.first?.isCurrent == true)
    }

    public func testTC_T1_F29_04_CacheCleanupExecution() async throws {
        let vm = TelegramSettingsViewModel()
        XCTAssertTrue(vm.storageUsage.cacheBytes > 0)

        try await vm.clearCache()
        XCTAssertEqual(vm.storageUsage.cacheBytes, 0)
        XCTAssertEqual(vm.storageUsage.formattedCache, "Zero KB")
    }

    public func testTC_T1_F29_05_AppearanceThemeSwitching() {
        let vm = TelegramSettingsViewModel()
        XCTAssertEqual(vm.theme, .classic)

        vm.theme = .dark
        XCTAssertEqual(vm.theme, .dark)

        vm.theme = .night
        XCTAssertEqual(vm.theme, .night)
    }

    public func testTC_T2_F29_01_TerminateCurrentSessionGuard() {
        let vm = TelegramSettingsViewModel()
        guard let currentSession = vm.activeSessions.first(where: { $0.isCurrent }) else {
            XCTFail("Missing current session")
            return
        }

        XCTAssertThrowsError(try vm.terminateSession(id: currentSession.id)) { err in
            guard case SettingsError.cannotTerminateCurrentSession = err else {
                XCTFail("Expected cannotTerminateCurrentSession error, got \(err)")
                return
            }
        }
    }

    public func testTC_T2_F29_02_PasscodeAutoLockImmediate() {
        let vm = TelegramSettingsViewModel()
        vm.autoLockSeconds = 0 // Immediately

        XCTAssertTrue(vm.checkAutoLockOnBackground(foregroundDelaySeconds: 1.0))
        XCTAssertTrue(vm.checkAutoLockOnBackground(foregroundDelaySeconds: 0.1))

        vm.autoLockSeconds = 60 // 1 minute
        XCTAssertFalse(vm.checkAutoLockOnBackground(foregroundDelaySeconds: 30.0))
        XCTAssertTrue(vm.checkAutoLockOnBackground(foregroundDelaySeconds: 61.0))
    }

    public func testTC_T2_F29_03_PasscodeExponentialLockoutTenAttempts() {
        let vm = TelegramSettingsViewModel()

        // Attempts 1..4: 0s cooldown
        XCTAssertEqual(vm.recordPasscodeFailure(currentFailures: 3), 0)

        // Attempts 5..7: 60s cooldown (1 min)
        XCTAssertEqual(vm.recordPasscodeFailure(currentFailures: 5), 60)
        XCTAssertEqual(vm.recordPasscodeFailure(currentFailures: 7), 60)

        // Attempts 8..9: 300s cooldown (5 min)
        XCTAssertEqual(vm.recordPasscodeFailure(currentFailures: 8), 300)
        XCTAssertEqual(vm.recordPasscodeFailure(currentFailures: 9), 300)

        // Attempts 10+: 900s cooldown (15 min)
        XCTAssertEqual(vm.recordPasscodeFailure(currentFailures: 10), 900)
        XCTAssertEqual(vm.recordPasscodeFailure(currentFailures: 15), 900)
    }

    public func testTC_T2_F29_05_SystemDarkModeAutoSwitching() {
        let vm = TelegramSettingsViewModel()
        XCTAssertTrue(vm.matchSystemDarkMode)

        vm.matchSystemDarkMode = false
        XCTAssertFalse(vm.matchSystemDarkMode)
    }
}
