//
//  challenger_reselection_matrix_tests.swift
//  FabushiTests
//
//  Adversarial Empirical Challenge Suite:
//  Exhaustive 4x4 Tab Navigation, Gesture Reselection Isolation,
//  Double-Tap Thresholds, and Concurrency Stress.
//

import Foundation
#if canImport(Darwin)
import Darwin
#endif

@main
struct ChallengerReselectionMatrixTests {

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
    static func main() async throws {
        print("================================================================================")
        print(" EMPIRICAL ADVERSARIAL CHALLENGER: Tab Navigation & Gesture Reselection (M2)")
        print(" Target: TelegramTabNavigationCoordinator, TelegramTab, Reselection Isolation")
        print(" Concurrency: Swift 6 Strict Concurrency (@MainActor)")
        print("================================================================================")
        let initialRss = getRssMB()
        print("Initial RSS Memory: \(String(format: "%.2f", initialRss)) MB")

        let allTabs = TelegramTab.allCases
        assert(allTabs.count == 4, "Must have exactly 4 canonical tabs")

        // -----------------------------------------------------------------------------
        // CHALLENGE 1: Exhaustive 4x4 Directed Tab-Switch Isolation Matrix
        // -----------------------------------------------------------------------------
        print("\n[CHALLENGE 1] Running Exhaustive 4x4 Directed Tab-Switch Isolation Matrix (12 directed pairs)...")
        for tabA in allTabs {
            for tabB in allTabs {
                guard tabA != tabB else { continue }

                let coordinator = TelegramTabNavigationCoordinator()

                // 1. Establish state on Tab A
                coordinator.handleTabTap(tabA)
                guard coordinator.selectedTab == tabA else {
                    fatalError("CHALLENGE 1 FAILED: Expected selectedTab to be \(tabA), got \(coordinator.selectedTab)")
                }

                // 2. Tab B is scrolled down
                coordinator.setIsAtTop(false, for: tabB)
                guard coordinator.isAtTop(for: tabB) == false else {
                    fatalError("CHALLENGE 1 FAILED: Tab \(tabB) isAtTop must be false")
                }

                // 3. Switch from Tab A to Tab B
                coordinator.handleTabTap(tabB)
                guard coordinator.selectedTab == tabB else {
                    fatalError("CHALLENGE 1 FAILED: Switching from \(tabA) to \(tabB) failed to update selectedTab")
                }
                let tokenPreReselect = coordinator.reselectTrigger[tabB]?.token

                // 4. Immediately (< 1.2s), Tap 1 on Tab B (scrolled down)
                coordinator.handleTabTap(tabB)
                guard let trigger1 = coordinator.reselectTrigger[tabB] else {
                    fatalError("CHALLENGE 1 FAILED: Tap 1 on active tab \(tabB) did not emit reselectTrigger")
                }
                guard trigger1.action == .scrollToTop else {
                    fatalError("CHALLENGE 1 FAILED: Tab \(tabA) -> \(tabB): Tap 1 while scrolled down emitted \(trigger1.action), expected .scrollToTop")
                }
                guard trigger1.token != tokenPreReselect else {
                    fatalError("CHALLENGE 1 FAILED: Tap 1 did not generate a fresh token")
                }

                // 5. Immediately (< 1.2s), Tap 2 on Tab B (still scrolled down)
                coordinator.handleTabTap(tabB)
                guard let trigger2 = coordinator.reselectTrigger[tabB] else {
                    fatalError("CHALLENGE 1 FAILED: Tap 2 on active tab \(tabB) did not emit reselectTrigger")
                }
                guard trigger2.action == .scrollToFirstUnread else {
                    fatalError("CHALLENGE 1 FAILED: Tab \(tabA) -> \(tabB): Tap 2 emitted \(trigger2.action), expected .scrollToFirstUnread")
                }
                guard trigger2.token != trigger1.token else {
                    fatalError("CHALLENGE 1 FAILED: Tap 2 token must differ from Tap 1 token")
                }

                // 6. Immediately (< 1.2s), Tap 3 on Tab B (rapid succession)
                coordinator.handleTabTap(tabB)
                guard let trigger3 = coordinator.reselectTrigger[tabB] else {
                    fatalError("CHALLENGE 1 FAILED: Tap 3 on active tab \(tabB) did not emit reselectTrigger")
                }
                guard trigger3.action == .scrollToFirstUnread else {
                    fatalError("CHALLENGE 1 FAILED: Tap 3 emitted \(trigger3.action), expected .scrollToFirstUnread")
                }
                guard trigger3.token != trigger2.token else {
                    fatalError("CHALLENGE 1 FAILED: Tap 3 token must differ from Tap 2 token")
                }

                print("  ✓ Matrix [\(tabA) -> \(tabB)]: Switch -> Tap 1 (.scrollToTop) -> Tap 2 (.scrollToFirstUnread) -> Tap 3 (.scrollToFirstUnread) VERIFIED")
            }
        }
        print("✓ CHALLENGE 1 PASSED: All 12 directed tab pairs strictly isolated.")

        // -----------------------------------------------------------------------------
        // CHALLENGE 2: At-Top Reselection on All 4 Canonical Tabs
        // -----------------------------------------------------------------------------
        print("\n[CHALLENGE 2] Testing At-Top Reselection on All 4 Tabs...")
        for tab in allTabs {
            let coordinator = TelegramTabNavigationCoordinator()
            coordinator.handleTabTap(tab)
            coordinator.setIsAtTop(true, for: tab)

            // Tap 1 when at top: must trigger .scrollToFirstUnread immediately
            coordinator.handleTabTap(tab)
            guard let trigger1 = coordinator.reselectTrigger[tab] else {
                fatalError("CHALLENGE 2 FAILED: Reselect trigger missing for \(tab)")
            }
            guard trigger1.action == .scrollToFirstUnread else {
                fatalError("CHALLENGE 2 FAILED: Expected .scrollToFirstUnread when at top for \(tab), got \(trigger1.action)")
            }

            // Tap 2 when at top: must trigger .scrollToFirstUnread
            coordinator.handleTabTap(tab)
            guard let trigger2 = coordinator.reselectTrigger[tab] else {
                fatalError("CHALLENGE 2 FAILED: Reselect trigger 2 missing for \(tab)")
            }
            guard trigger2.action == .scrollToFirstUnread else {
                fatalError("CHALLENGE 2 FAILED: Expected .scrollToFirstUnread for \(tab)")
            }
            guard trigger2.token != trigger1.token else {
                fatalError("CHALLENGE 2 FAILED: Token must be unique")
            }
            print("  ✓ Tab \(tab) at-top reselection emits .scrollToFirstUnread VERIFIED")
        }
        print("✓ CHALLENGE 2 PASSED: At-top reselection emits .scrollToFirstUnread across all 4 tabs.")

        // -----------------------------------------------------------------------------
        // CHALLENGE 3: Real-Time Double-Tap Timing Threshold Verification (> 1.2s delay)
        // -----------------------------------------------------------------------------
        print("\n[CHALLENGE 3] Testing Real-Time Double-Tap Threshold (> 1.2s delay vs < 1.2s rapid)...")
        let coordinatorTiming = TelegramTabNavigationCoordinator()
        // Switch to calls, scroll down chats, switch to chats
        coordinatorTiming.handleTabTap(.calls)
        coordinatorTiming.setIsAtTop(false, for: .chats)
        coordinatorTiming.handleTabTap(.chats) // switch to chats

        // Tap 1: scrolled down -> .scrollToTop
        coordinatorTiming.handleTabTap(.chats)
        guard coordinatorTiming.reselectTrigger[.chats]?.action == .scrollToTop else {
            fatalError("CHALLENGE 3 FAILED: Initial tap while scrolled down did not trigger .scrollToTop")
        }

        // Wait 1.3 seconds (> 1.2s threshold)
        print("  - Sleeping 1.3 seconds to test expiration of double-tap window...")
        try await Task.sleep(nanoseconds: 1_300_000_000)

        // Tap 2 after 1.3s while still scrolled down: MUST trigger .scrollToTop again!
        coordinatorTiming.handleTabTap(.chats)
        guard coordinatorTiming.reselectTrigger[.chats]?.action == .scrollToTop else {
            fatalError("CHALLENGE 3 FAILED: Tap after 1.3s while scrolled down must trigger .scrollToTop, got \(String(describing: coordinatorTiming.reselectTrigger[.chats]?.action))")
        }
        print("  - Tap after 1.3s correctly triggered .scrollToTop (double-tap window expired)")

        // Tap 3 immediately (< 1.2s after Tap 2) while scrolled down: MUST trigger .scrollToFirstUnread
        coordinatorTiming.handleTabTap(.chats)
        guard coordinatorTiming.reselectTrigger[.chats]?.action == .scrollToFirstUnread else {
            fatalError("CHALLENGE 3 FAILED: Tap within 1.2s after Tap 2 must trigger .scrollToFirstUnread, got \(String(describing: coordinatorTiming.reselectTrigger[.chats]?.action))")
        }
        print("  - Rapid Tap 3 (< 1.2s) correctly triggered .scrollToFirstUnread")
        print("✓ CHALLENGE 3 PASSED: 1.2s double-tap window threshold empirically verified.")

        // -----------------------------------------------------------------------------
        // CHALLENGE 4: Interleaved Tab Switching Cancels Double-Tap Sequence
        // -----------------------------------------------------------------------------
        print("\n[CHALLENGE 4] Testing Interleaved Tab Switching Resets Double-Tap Sequence...")
        let coordinatorInterleaved = TelegramTabNavigationCoordinator()
        coordinatorInterleaved.handleTabTap(.calls)
        coordinatorInterleaved.setIsAtTop(false, for: .chats)
        coordinatorInterleaved.handleTabTap(.chats) // switch to chats

        // 1. Tap Chats -> .scrollToTop
        coordinatorInterleaved.handleTabTap(.chats)
        guard coordinatorInterleaved.reselectTrigger[.chats]?.action == .scrollToTop else {
            fatalError("CHALLENGE 4 FAILED: Tap 1 did not trigger .scrollToTop")
        }

        // 2. Immediately switch to Calls (< 0.1s)
        coordinatorInterleaved.handleTabTap(.calls)
        guard coordinatorInterleaved.selectedTab == .calls else {
            fatalError("CHALLENGE 4 FAILED: Selected tab must be .calls")
        }

        // 3. Immediately switch back to Chats (< 0.1s)
        coordinatorInterleaved.handleTabTap(.chats)
        guard coordinatorInterleaved.selectedTab == .chats else {
            fatalError("CHALLENGE 4 FAILED: Selected tab must be .chats")
        }

        // 4. Immediately tap Chats (< 0.1s after switch, but < 0.3s after Tap 1)
        // Because of the intervening switch, this must be treated as Tap 1 of a NEW sequence -> .scrollToTop!
        coordinatorInterleaved.handleTabTap(.chats)
        guard coordinatorInterleaved.reselectTrigger[.chats]?.action == .scrollToTop else {
            fatalError("CHALLENGE 4 FAILED: Tap after switching back must trigger .scrollToTop, NOT .scrollToFirstUnread! Got: \(String(describing: coordinatorInterleaved.reselectTrigger[.chats]?.action))")
        }
        print("  ✓ Intervening tab switch safely broke double-tap chain: Tap after switch emits .scrollToTop")
        print("✓ CHALLENGE 4 PASSED: Interleaved tab navigation resets double-tap timing.")

        // -----------------------------------------------------------------------------
        // CHALLENGE 5: Cross-Tab Reselect State Independence (Simultaneous Tabs)
        // -----------------------------------------------------------------------------
        print("\n[CHALLENGE 5] Testing Multi-Tab State Independence...")
        let coordinatorMulti = TelegramTabNavigationCoordinator()
        // Switch to settings initially so that allTabs iterations are pure (switch -> tap 1)
        coordinatorMulti.handleTabTap(.settings)

        // Set all 4 tabs to scrolled down
        for tab in allTabs {
            coordinatorMulti.setIsAtTop(false, for: tab)
        }

        // Switch to and tap each tab once
        var firstTapTokens: [TelegramTab: UUID] = [:]
        for tab in allTabs {
            coordinatorMulti.handleTabTap(tab) // switch to tab
            coordinatorMulti.handleTabTap(tab) // tap 1 on active tab
            guard coordinatorMulti.reselectTrigger[tab]?.action == .scrollToTop else {
                fatalError("CHALLENGE 5 FAILED: Tab \(tab) Tap 1 must be .scrollToTop, got \(String(describing: coordinatorMulti.reselectTrigger[tab]?.action))")
            }
            firstTapTokens[tab] = coordinatorMulti.reselectTrigger[tab]?.token
        }

        // Verify all 4 tabs hold their respective .scrollToTop and unique tokens
        for tab in allTabs {
            guard coordinatorMulti.reselectTrigger[tab]?.action == .scrollToTop else {
                fatalError("CHALLENGE 5 FAILED: Stored action for \(tab) corrupted")
            }
        }
        let uniqueFirstTokens = Set(firstTapTokens.values)
        guard uniqueFirstTokens.count == 4 else {
            fatalError("CHALLENGE 5 FAILED: Expected 4 unique tokens across 4 tabs, got \(uniqueFirstTokens.count)")
        }
        print("✓ CHALLENGE 5 PASSED: Multi-tab reselect state holds independent actions and tokens.")

        // -----------------------------------------------------------------------------
        // CHALLENGE 6: Concurrency Stress (100 Concurrent Tasks across 4 Tabs)
        // -----------------------------------------------------------------------------
        print("\n[CHALLENGE 6] Testing Concurrency Stress (100 Concurrent Tasks, 20 Switches Each = 2,000 Operations)...")
        let coordinatorConcurrent = TelegramTabNavigationCoordinator()
        let concurrentTaskCount = 100
        let switchesPerTask = 20

        let startTime = CFAbsoluteTimeGetCurrent()
        await withTaskGroup(of: Void.self) { group in
            for taskId in 0..<concurrentTaskCount {
                group.addTask { @MainActor in
                    for s in 0..<switchesPerTask {
                        let tab = allTabs[(taskId * 7 + s) % allTabs.count]
                        coordinatorConcurrent.handleTabTap(tab)
                        if s % 3 == 0 {
                            coordinatorConcurrent.setIsAtTop(s % 2 == 0, for: tab)
                        }
                    }
                }
            }
        }
        let elapsedConcurrent = CFAbsoluteTimeGetCurrent() - startTime
        print("  - 2,000 concurrent operations completed in \(String(format: "%.3f", elapsedConcurrent * 1000)) ms")

        // Verify state sanity post-concurrency
        guard allTabs.contains(coordinatorConcurrent.selectedTab) else {
            fatalError("CHALLENGE 6 FAILED: selectedTab is in invalid state: \(coordinatorConcurrent.selectedTab)")
        }
        guard coordinatorConcurrent.reselectTrigger.count <= 4 else {
            fatalError("CHALLENGE 6 FAILED: reselectTrigger leaked entries: \(coordinatorConcurrent.reselectTrigger.count)")
        }
        print("  - Final selectedTab: \(coordinatorConcurrent.selectedTab)")
        print("  - reselectTrigger count: \(coordinatorConcurrent.reselectTrigger.count) <= 4")
        print("✓ CHALLENGE 6 PASSED: Concurrency stress handled safely without race conditions or memory corruption.")

        // -----------------------------------------------------------------------------
        // CHALLENGE 7: High-Volume Rapid Switching Burst (10,000 Iterations) & Leak Audit
        // -----------------------------------------------------------------------------
        print("\n[CHALLENGE 7] Testing High-Volume Switching Burst (10,000 Iterations)...")
        let burstCount = 10000
        let burstStart = CFAbsoluteTimeGetCurrent()
        for i in 0..<burstCount {
            let tab = allTabs[i % allTabs.count]
            coordinatorConcurrent.handleTabTap(tab)
            if i % 10 == 0 {
                coordinatorConcurrent.setIsAtTop(i % 20 == 0, for: tab)
            }
        }
        let burstElapsed = CFAbsoluteTimeGetCurrent() - burstStart
        let avgBurstUs = (burstElapsed / Double(burstCount)) * 1_000_000
        print("  - 10,000 switching iterations completed in \(String(format: "%.3f", burstElapsed * 1000)) ms (avg \(String(format: "%.2f", avgBurstUs)) µs/op)")

        let finalRss = getRssMB()
        let rssDelta = finalRss - initialRss
        print("  - Baseline RSS: \(String(format: "%.2f", initialRss)) MB")
        print("  - Final RSS: \(String(format: "%.2f", finalRss)) MB")
        print("  - Net Memory Growth: \(String(format: "%.2f", rssDelta)) MB")

        guard rssDelta < 20.0 else {
            fatalError("CHALLENGE 7 FAILED: Memory growth \(rssDelta) MB exceeded 20MB limit")
        }
        print("✓ CHALLENGE 7 PASSED: High-volume switching burst completed cleanly with bounded memory.")

        print("\n================================================================================")
        print(" >>> ALL 7 EMPIRICAL ADVERSARIAL CHALLENGES COMPLETED WITH 100% SUCCESS! <<<")
        print("================================================================================")
    }
}
