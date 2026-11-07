//
//  TelegramChatListTests.swift
//  FabushiTests
//
//  Unit tests covering tab bar metrics, badge counts, folder filtering,
//  swipe action mutations, PTS continuity logic, and client nonce generation.
//

#if canImport(XCTest)
import XCTest
#if canImport(Fabushi)
@testable import Fabushi
#endif

final class TelegramChatListTests: XCTestCase, @unchecked Sendable {

    // MARK: - 1. Tab Bar Metrics & Enum Verification
    func testTelegramTabBarMetricsAndTabs() {
        XCTAssertEqual(TelegramTabBarMetrics.barHeight, 49.0, "Canonical Telegram tab bar height must be 49.0pt")
        XCTAssertEqual(TelegramTabBarMetrics.iconSize, 24.0)
        XCTAssertEqual(TelegramTabBarMetrics.titleFontSize, 10.0)
        XCTAssertEqual(TelegramTabBarMetrics.topSpacing, 4.0)
        XCTAssertEqual(TelegramTabBarMetrics.bottomSpacing, 2.0)

        // 4 tabs canonical ordering
        XCTAssertEqual(TelegramTab.allCases.count, 4)
        XCTAssertEqual(TelegramTab.chats.rawValue, 0)
        XCTAssertEqual(TelegramTab.calls.rawValue, 1)
        XCTAssertEqual(TelegramTab.contacts.rawValue, 2)
        XCTAssertEqual(TelegramTab.settings.rawValue, 3)

        XCTAssertEqual(TelegramTab.chats.title, "Chats")
        XCTAssertEqual(TelegramTab.calls.title, "Calls")
        XCTAssertEqual(TelegramTab.contacts.title, "Contacts")
        XCTAssertEqual(TelegramTab.settings.title, "Settings")

        XCTAssertEqual(TelegramTab.chats.accessibilityIdentifier, "tab-bar-item-chats")
        XCTAssertEqual(TelegramTab.calls.accessibilityIdentifier, "tab-bar-item-calls")
        XCTAssertEqual(TelegramTab.contacts.accessibilityIdentifier, "tab-bar-item-contacts")
        XCTAssertEqual(TelegramTab.settings.accessibilityIdentifier, "tab-bar-item-settings")
    }

    // MARK: - 2. Dynamic Capsule Badges
    func testTabBarBadgeCountsAndSemantics() {
        let zeroBadge = TabBarBadge(count: 0)
        XCTAssertFalse(zeroBadge.isVisible)
        XCTAssertEqual(zeroBadge.text, "0")

        let unmutedBadge = TabBarBadge(count: 5, isMuted: false)
        XCTAssertTrue(unmutedBadge.isVisible)
        XCTAssertEqual(unmutedBadge.text, "5")
        XCTAssertFalse(unmutedBadge.isMuted)

        let mutedBadge = TabBarBadge(count: 42, isMuted: true)
        XCTAssertTrue(mutedBadge.isVisible)
        XCTAssertEqual(mutedBadge.text, "42")
        XCTAssertTrue(mutedBadge.isMuted)

        let overflowBadge = TabBarBadge(count: 1000)
        XCTAssertTrue(overflowBadge.isVisible)
        XCTAssertEqual(overflowBadge.text, "999+")
    }

    // MARK: - 3. Tab Coordinator Reselection Actions
    @MainActor
    func testTabCoordinatorReselectionBehavior() {
        let coordinator = TelegramTabNavigationCoordinator()
        XCTAssertEqual(coordinator.selectedTab, .chats)

        // When scrolled down, first tap triggers scrollToTop
        coordinator.setIsAtTop(false, for: .chats)
        coordinator.handleTabTap(.chats)
        XCTAssertEqual(coordinator.reselectTrigger[.chats]?.action, .scrollToTop)

        // When already at top, tap triggers scrollToFirstUnread
        coordinator.setIsAtTop(true, for: .chats)
        coordinator.handleTabTap(.chats)
        XCTAssertEqual(coordinator.reselectTrigger[.chats]?.action, .scrollToFirstUnread)

        // Selecting different tab switches active tab
        coordinator.handleTabTap(.calls)
        XCTAssertEqual(coordinator.selectedTab, .calls)
    }

    // MARK: - 4. Folder Filtering & Search in ChatListViewModel
    @MainActor
    func testChatListViewModelFolderFiltering() async {
        let viewModel = ChatListViewModel()

        let chat1 = TelegramChatSummary(id: 1, title: "Alice", kind: "direct", unreadCount: 2, lastMessage: "Hello", timestamp: 100)
        let chat2 = TelegramChatSummary(id: 2, title: "Telegram News", kind: "channel", unreadCount: 0, lastMessage: "Update v11", timestamp: 200)
        let chat3 = TelegramChatSummary(id: 3, title: "Developers", kind: "group", unreadCount: 5, lastMessage: "Code review", timestamp: 300)
        let chat4 = TelegramChatSummary(id: 4, title: "Saved Messages", kind: "saved", unreadCount: 0, lastMessage: "Note 1", timestamp: 400)

        viewModel.setChatsDirectly([chat1, chat2, chat3, chat4])

        // All folder
        viewModel.selectedFolder = .all
        XCTAssertEqual(viewModel.filteredChats.count, 4)

        // Unread folder
        viewModel.selectedFolder = .unread
        let unread = viewModel.filteredChats
        XCTAssertEqual(unread.count, 2)
        XCTAssertTrue(unread.contains(where: { $0.id == 1 }))
        XCTAssertTrue(unread.contains(where: { $0.id == 3 }))

        // Groups folder
        viewModel.selectedFolder = .groups
        let groups = viewModel.filteredChats
        XCTAssertEqual(groups.count, 1)
        XCTAssertEqual(groups.first?.id, 3)

        // Channels folder
        viewModel.selectedFolder = .channels
        let channels = viewModel.filteredChats
        XCTAssertEqual(channels.count, 1)
        XCTAssertEqual(channels.first?.id, 2)

        // Search text filtering
        viewModel.selectedFolder = .all
        viewModel.searchText = "alice"
        XCTAssertEqual(viewModel.filteredChats.count, 1)
        XCTAssertEqual(viewModel.filteredChats.first?.title, "Alice")

        viewModel.searchText = "review"
        XCTAssertEqual(viewModel.filteredChats.count, 1)
        XCTAssertEqual(viewModel.filteredChats.first?.title, "Developers")

        viewModel.searchText = "nonexistent"
        XCTAssertEqual(viewModel.filteredChats.count, 0)
    }

    // MARK: - 5. Swipe Actions & Mutations in ChatListViewModel
    @MainActor
    func testChatListViewModelSwipeActionMutations() async {
        let viewModel = ChatListViewModel()

        let chat1 = TelegramChatSummary(id: 1, title: "Alice", unreadCount: 3, isPinned: false, isMuted: false)
        let chat2 = TelegramChatSummary(id: 2, title: "Bob", unreadCount: 0, isPinned: false, isMuted: false)
        viewModel.setChatsDirectly([chat1, chat2])

        // Pin mutation: should toggle isPinned and move to index 0
        viewModel.togglePin(chatId: 2)
        XCTAssertTrue(viewModel.chats[0].isPinned)
        XCTAssertEqual(viewModel.chats[0].id, 2)

        // Unpin mutation: should toggle isPinned and move after pinned
        viewModel.togglePin(chatId: 2)
        XCTAssertFalse(viewModel.chats.first(where: { $0.id == 2 })?.isPinned ?? true)

        // Mute mutation: should toggle isMuted
        viewModel.toggleMute(chatId: 1)
        XCTAssertTrue(viewModel.chats.first(where: { $0.id == 1 })?.isMuted ?? false)

        // Mark as read mutation: should clear unreadCount
        viewModel.markAsRead(chatId: 1)
        XCTAssertEqual(viewModel.chats.first(where: { $0.id == 1 })?.unreadCount, 0)

        // Delete chat mutation: should remove from list
        viewModel.deleteChat(chatId: 1)
        XCTAssertEqual(viewModel.chats.count, 1)
        XCTAssertFalse(viewModel.chats.contains(where: { $0.id == 1 }))
    }

    // MARK: - 6. PTS Sequence Difference Sync & Gap Detection
    func testPtsSyncCoordinatorGapDetectionAndCatchup() async {
        let coordinator = PtsSyncCoordinator()
        let chatId: Int64 = 42

        // Initialize PTS to 100
        await coordinator.setInitialPts(chatId: chatId, pts: 100)
        var currentPts = await coordinator.currentPts(for: chatId)
        XCTAssertEqual(currentPts, 100)

        // 1. Contiguous update: pts=101, ptsCount=1 (prevPts = 100 == currentPts 100)
        let res1 = await coordinator.evaluateIncomingUpdate(chatId: chatId, pts: 101, ptsCount: 1)
        XCTAssertEqual(res1, .applyImmediately)
        currentPts = await coordinator.currentPts(for: chatId)
        XCTAssertEqual(currentPts, 101)

        // 2. Stale or duplicate update: pts=101, ptsCount=1 <= currentPts 101
        let res2 = await coordinator.evaluateIncomingUpdate(chatId: chatId, pts: 101, ptsCount: 1)
        XCTAssertEqual(res2, .duplicateIgnored)

        // 3. Ephemeral update: ptsCount=0, pts=101 <= currentPts
        let res3 = await coordinator.evaluateIncomingUpdate(chatId: chatId, pts: 101, ptsCount: 0)
        XCTAssertEqual(res3, .applyImmediately)

        // 3b. Ephemeral update with pts > currentPts: ptsCount=0, pts=120 > currentPts (must not trigger difference sync)
        let res3b = await coordinator.evaluateIncomingUpdate(chatId: chatId, pts: 120, ptsCount: 0)
        XCTAssertEqual(res3b, .applyImmediately)
        let state3b = await coordinator.currentState(for: chatId)
        XCTAssertEqual(state3b, .idle)
        let ptsAfter3b = await coordinator.currentPts(for: chatId)
        XCTAssertEqual(ptsAfter3b, 101)

        // 4. Gap detected: update jumps to pts=105, ptsCount=1 (prevPts = 104 > current 101)
        let res4 = await coordinator.evaluateIncomingUpdate(chatId: chatId, pts: 105, ptsCount: 1, eventJson: "{\"id\": 105}")
        XCTAssertEqual(res4, .needsDifferenceSync(missingRange: 102...104))
        var state = await coordinator.currentState(for: chatId)
        XCTAssertEqual(state, .syncing(fromPts: 101))

        // 5. Another update arrives while syncing: pts=106, ptsCount=1 (prevPts = 105)
        let res5 = await coordinator.evaluateIncomingUpdate(chatId: chatId, pts: 106, ptsCount: 1, eventJson: "{\"id\": 106}")
        XCTAssertEqual(res5, .needsDifferenceSync(missingRange: 102...105))

        // 6. Diff response catches up with journal: newPts = 104, hasMore = false
        let action = await coordinator.applyDifferenceResponse(
            chatId: chatId,
            newPts: 104,
            hasMore: false,
            snapshotRequired: false
        )

        // Buffered updates 105 and 106 should be drained and replayed sequentially
        if case .completedCatchup(let replayed) = action {
            XCTAssertEqual(replayed.count, 2)
            XCTAssertEqual(replayed[0].pts, 105)
            XCTAssertEqual(replayed[1].pts, 106)
        } else {
            XCTFail("Expected .completedCatchup action, got \(action)")
        }

        // Final state should return to idle with currentPts = 106
        state = await coordinator.currentState(for: chatId)
        XCTAssertEqual(state, .idle)
        currentPts = await coordinator.currentPts(for: chatId)
        XCTAssertEqual(currentPts, 106)

        // 7. Test snapshotRequired recovery
        let snapshotAction = await coordinator.applyDifferenceResponse(
            chatId: chatId,
            newPts: 50,
            hasMore: false,
            snapshotRequired: true
        )
        XCTAssertEqual(snapshotAction, .performSnapshotReload)
        state = await coordinator.currentState(for: chatId)
        XCTAssertEqual(state, .snapshotRequired)
    }

    // MARK: - 7. 64-Bit Client Nonce Generation for Deduplication
    func test64BitClientNonceGeneration() {
        var nonces: Set<UInt64> = []
        let sampleCount = 1000

        for _ in 0..<sampleCount {
            let nonce = NonceGenerator.nextNonce()
            XCTAssertNotEqual(nonce, 0, "Nonce must be a non-zero 64-bit integer")
            nonces.insert(nonce)
        }

        XCTAssertEqual(nonces.count, sampleCount, "All \(sampleCount) generated 64-bit client nonces must be unique")
    }

    // MARK: - 8. Search Scope Filters
    func testSearchScopeFilters() {
        XCTAssertEqual(SearchScopeFilter.allCases.count, 6)
        XCTAssertEqual(SearchScopeFilter.chats.rawValue, "Chats")
        XCTAssertEqual(SearchScopeFilter.media.rawValue, "Media")
        XCTAssertEqual(SearchScopeFilter.links.rawValue, "Links")
        XCTAssertEqual(SearchScopeFilter.files.rawValue, "Files")
        XCTAssertEqual(SearchScopeFilter.music.rawValue, "Music")
        XCTAssertEqual(SearchScopeFilter.voice.rawValue, "Voice")

        XCTAssertFalse(SearchScopeFilter.chats.icon.isEmpty)
        XCTAssertFalse(SearchScopeFilter.media.icon.isEmpty)
        XCTAssertFalse(SearchScopeFilter.links.icon.isEmpty)
        XCTAssertFalse(SearchScopeFilter.files.icon.isEmpty)
        XCTAssertFalse(SearchScopeFilter.music.icon.isEmpty)
        XCTAssertFalse(SearchScopeFilter.voice.icon.isEmpty)
    }

    // MARK: - 9. Tab Coordinator Tab Switch Reselection Isolation
    @MainActor
    func testTabCoordinatorTabSwitchReselectionIsolation() {
        let coordinator = TelegramTabNavigationCoordinator()

        // Switch to Calls
        coordinator.handleTabTap(.calls)
        XCTAssertEqual(coordinator.selectedTab, .calls)

        // Chats is scrolled down
        coordinator.setIsAtTop(false, for: .chats)

        // Switch to Chats
        coordinator.handleTabTap(.chats)
        XCTAssertEqual(coordinator.selectedTab, .chats)

        // Immediately tap Chats while scrolled down (< 1.2s after switch)
        coordinator.handleTabTap(.chats)
        XCTAssertEqual(
            coordinator.reselectTrigger[.chats]?.action,
            .scrollToTop,
            "First tap on active tab after switch must trigger scrollToTop, NOT scrollToFirstUnread"
        )

        // Tap Chats again rapidly (< 1.2s)
        coordinator.handleTabTap(.chats)
        XCTAssertEqual(
            coordinator.reselectTrigger[.chats]?.action,
            .scrollToFirstUnread,
            "Rapid second tap on active tab must trigger scrollToFirstUnread"
        )
    }
}
#endif
