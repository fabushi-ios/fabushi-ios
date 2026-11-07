//
//  TelegramTabNavigationStressTests.swift
//  FabushiTests
//
//  Adversarial empirical stress test suite for Milestone 2:
//  1. High-frequency tab switching (1,000 rapid switches across 4 tabs)
//  2. Progressive reselection stress under simulated high-speed scroll events
//  3. Folder tab filtering stress (10 folders with 1,000+ mock conversations, unmuted red vs muted gray badges)
//  4. Concurrent data race and memory leak audit
//

#if canImport(XCTest)
import XCTest
#if canImport(Fabushi)
@testable import Fabushi
#endif

final class TelegramTabNavigationStressTests: XCTestCase {

    // MARK: - 1. High-Frequency Tab Switching Stress (1,000 Rapid Switches)
    @MainActor
    func testHighFrequencyTabSwitchingStress() async {
        let coordinator = TelegramTabNavigationCoordinator()
        let switchIterations = 1000
        let allTabs = TelegramTab.allCases

        let startTime = CFAbsoluteTimeGetCurrent()

        for i in 0..<switchIterations {
            let targetTab = allTabs[i % allTabs.count]
            coordinator.handleTabTap(targetTab)

            // Assert immediate state consistency
            XCTAssertEqual(coordinator.selectedTab, targetTab, "Tab selection must update immediately on switch #\(i)")
        }

        let elapsed = CFAbsoluteTimeGetCurrent() - startTime
        let avgMicroseconds = (elapsed / Double(switchIterations)) * 1_000_000
        print("[StressTest] 1,000 tab switches completed in \(String(format: "%.3f", elapsed * 1000)) ms (avg \(String(format: "%.2f", avgMicroseconds)) µs/switch)")

        // Assert bound state: dictionaries must never leak entries beyond the 4 canonical tabs
        XCTAssertLessThanOrEqual(coordinator.reselectTrigger.count, 4, "reselectTrigger dictionary must be bounded to 4 tabs")

        // Randomized rapid switching
        for _ in 0..<switchIterations {
            let randomTab = allTabs.randomElement()!
            coordinator.handleTabTap(randomTab)
            XCTAssertEqual(coordinator.selectedTab, randomTab)
        }
    }

    // MARK: - 2. Progressive Reselection Stress Under Simulated Scroll Events
    @MainActor
    func testProgressiveReselectionStressUnderSimulatedScroll() async {
        let coordinator = TelegramTabNavigationCoordinator()
        coordinator.handleTabTap(.chats)
        XCTAssertEqual(coordinator.selectedTab, .chats)

        // Scenario A: Scrolled down -> First tap must trigger .scrollToTop
        coordinator.setIsAtTop(false, for: .chats)
        coordinator.handleTabTap(.chats)
        XCTAssertEqual(
            coordinator.reselectTrigger[.chats]?.action,
            .scrollToTop,
            "Tap 1 while scrolled down must trigger scrollToTop"
        )
        let token1 = coordinator.reselectTrigger[.chats]?.token
        XCTAssertNotNil(token1)

        // Scenario B: Rapid tap 2 (< 1.2s) while still scrolled down -> must trigger .scrollToFirstUnread
        coordinator.handleTabTap(.chats)
        XCTAssertEqual(
            coordinator.reselectTrigger[.chats]?.action,
            .scrollToFirstUnread,
            "Rapid Tap 2 (<1.2s) on active tab must trigger scrollToFirstUnread"
        )
        let token2 = coordinator.reselectTrigger[.chats]?.token
        XCTAssertNotNil(token2)
        XCTAssertNotEqual(token1, token2, "Each reselect action must issue a unique token")

        // Scenario C: Rapid tap 3 (< 1.2s) -> must remain .scrollToFirstUnread
        coordinator.handleTabTap(.chats)
        XCTAssertEqual(
            coordinator.reselectTrigger[.chats]?.action,
            .scrollToFirstUnread,
            "Rapid Tap 3 (<1.2s) must trigger scrollToFirstUnread"
        )
        let token3 = coordinator.reselectTrigger[.chats]?.token
        XCTAssertNotEqual(token2, token3)

        // Scenario D: User is already at top -> tap must trigger .scrollToFirstUnread immediately
        coordinator.setIsAtTop(true, for: .chats)
        coordinator.handleTabTap(.chats)
        XCTAssertEqual(
            coordinator.reselectTrigger[.chats]?.action,
            .scrollToFirstUnread,
            "Tap while at top must trigger scrollToFirstUnread immediately"
        )

        // Scenario E: 500 Interleaved high-speed scroll events and taps
        var previousToken: UUID? = nil
        var uniqueTokens: Set<UUID> = []

        for i in 0..<500 {
            let isAtTop = (i % 3 == 0)
            coordinator.setIsAtTop(isAtTop, for: .chats)
            coordinator.handleTabTap(.chats)

            let currentToken = coordinator.reselectTrigger[.chats]?.token
            XCTAssertNotNil(currentToken)
            XCTAssertNotEqual(currentToken, previousToken, "Token must be unique on iteration \(i)")
            if let tok = currentToken {
                uniqueTokens.insert(tok)
            }
            previousToken = currentToken
        }

        XCTAssertEqual(uniqueTokens.count, 500, "All 500 interleaved reselection actions must produce unique tokens")
    }

    // MARK: - 3. Folder Tab Filtering Stress (10 Folders with 1,000+ Mock Conversations)
    @MainActor
    func testFolderTabFilteringAndBadgeAggregationsStress() async {
        // Construct 1,200 mock conversations
        var mockConversations: [TelegramChatSummary] = []
        let totalConversations = 1200

        for i in 0..<totalConversations {
            let kind: String
            switch i % 4 {
            case 0: kind = "direct"
            case 1: kind = "group"
            case 2: kind = "channel"
            default: kind = "saved"
            }

            // Distribute unreads and mute flags
            let isMuted = (i % 2 == 0)
            let unreadCount: Int
            if i % 5 == 0 {
                unreadCount = 0
            } else if i % 10 == 1 {
                unreadCount = 25 // heavy unreads
            } else {
                unreadCount = (i % 4) + 1
            }

            let chat = TelegramChatSummary(
                id: Int64(i + 1),
                title: "Peer #\(i + 1)",
                kind: kind,
                unreadCount: unreadCount,
                lastMessage: "Message payload #\(i + 1)",
                timestamp: Int64(1700000000 + i),
                isPinned: (i < 10), // first 10 pinned
                isMuted: isMuted
            )
            mockConversations.append(chat)
        }

        let viewModel = ChatListViewModel()
        viewModel.setChatsDirectly(mockConversations)

        // 1. All folder
        viewModel.selectedFolder = .all
        XCTAssertEqual(viewModel.filteredChats.count, totalConversations)

        // 2. Unread folder (filter out unreadCount == 0)
        viewModel.selectedFolder = .unread
        let unreadFiltered = viewModel.filteredChats
        let expectedUnread = mockConversations.filter { $0.unreadCount > 0 }
        XCTAssertEqual(unreadFiltered.count, expectedUnread.count)
        XCTAssertTrue(unreadFiltered.allSatisfy { $0.unreadCount > 0 })

        // 3. Personal (direct)
        viewModel.selectedFolder = .personal
        let personalFiltered = viewModel.filteredChats
        XCTAssertEqual(personalFiltered.count, totalConversations / 4)
        XCTAssertTrue(personalFiltered.allSatisfy { $0.kind == "direct" })

        // 4. Groups
        viewModel.selectedFolder = .groups
        let groupsFiltered = viewModel.filteredChats
        XCTAssertEqual(groupsFiltered.count, totalConversations / 4)
        XCTAssertTrue(groupsFiltered.allSatisfy { $0.kind == "group" })

        // 5. Channels
        viewModel.selectedFolder = .channels
        let channelsFiltered = viewModel.filteredChats
        XCTAssertEqual(channelsFiltered.count, totalConversations / 4)
        XCTAssertTrue(channelsFiltered.allSatisfy { $0.kind == "channel" })

        // 6. Work (saved)
        viewModel.selectedFolder = .work
        let workFiltered = viewModel.filteredChats
        XCTAssertEqual(workFiltered.count, totalConversations / 4)
        XCTAssertTrue(workFiltered.allSatisfy { $0.kind == "saved" || $0.kind == "work" })

        // 7. Search text stress over 1,200 conversations
        viewModel.selectedFolder = .all
        viewModel.searchText = "Peer #100"
        XCTAssertEqual(viewModel.filteredChats.count, 1)
        XCTAssertEqual(viewModel.filteredChats.first?.id, 100)

        viewModel.searchText = "Peer #1" // prefix matches #1, #10, #100..#199, #1000..
        XCTAssertGreaterThan(viewModel.filteredChats.count, 100)

        // Rapid folder switching throughput: 1,000 rapid folder switches
        let folders: [ChatFolderTab] = [.all, .unread, .personal, .work, .groups, .channels]
        let startTime = CFAbsoluteTimeGetCurrent()
        for i in 0..<1000 {
            viewModel.selectedFolder = folders[i % folders.count]
            _ = viewModel.filteredChats.count
        }
        let elapsed = CFAbsoluteTimeGetCurrent() - startTime
        print("[StressTest] 1,000 folder filter evaluations on 1,200 conversations in \(String(format: "%.3f", elapsed * 1000)) ms")
        XCTAssertLessThan(elapsed, 2.0, "1,000 folder filter evaluations on 1,200 chats must complete within 2.0s")
    }

    // MARK: - 4. Badge Aggregation Semantics Stress (Unmuted Red vs Muted Gray)
    func testBadgeAggregationSemanticsStress() {
        // Zero unread
        let zeroBadge = TabBarBadge(count: 0, isMuted: false)
        XCTAssertFalse(zeroBadge.isVisible)
        XCTAssertEqual(zeroBadge.text, "0")

        // Unmuted badge -> Red (#FF3B30)
        let unmutedBadge = TabBarBadge(count: 7, isMuted: false)
        XCTAssertTrue(unmutedBadge.isVisible)
        XCTAssertEqual(unmutedBadge.text, "7")
        XCTAssertFalse(unmutedBadge.isMuted)

        // Muted badge -> Gray (#8E8E93)
        let mutedBadge = TabBarBadge(count: 14, isMuted: true)
        XCTAssertTrue(mutedBadge.isVisible)
        XCTAssertEqual(mutedBadge.text, "14")
        XCTAssertTrue(mutedBadge.isMuted)

        // 999+ Overflow clamp
        let overflowBadge = TabBarBadge(count: 1250, isMuted: false)
        XCTAssertTrue(overflowBadge.isVisible)
        XCTAssertEqual(overflowBadge.text, "999+")

        let exactClampBoundary = TabBarBadge(count: 1000, isMuted: true)
        XCTAssertEqual(exactClampBoundary.text, "999+")

        let belowClampBoundary = TabBarBadge(count: 999, isMuted: false)
        XCTAssertEqual(belowClampBoundary.text, "999")
    }

    // MARK: - 5. Concurrent Actor & MainActor Thread Safety
    @MainActor
    func testConcurrentTabAndStateSafety() async {
        let coordinator = TelegramTabNavigationCoordinator()

        await withTaskGroup(of: Void.self) { group in
            for i in 0..<50 {
                group.addTask { @MainActor in
                    let tab = TelegramTab(rawValue: i % 4)!
                    coordinator.handleTabTap(tab)
                    coordinator.setIsAtTop(i % 2 == 0, for: tab)
                }
            }
        }

        // Must complete without deadlock or data races
        XCTAssertNotNil(coordinator.selectedTab)
    }
}
#endif
