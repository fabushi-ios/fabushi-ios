import Foundation

// MARK: - Standalone PtsSyncCoordinator Test Harness
// Directly testing PtsSyncCoordinator logic from ChatListViewModel.swift

@main
struct PtsSyncEphemeralAdversarialTests {
    static func main() async {
        print("================================================================================")
        print(" EMPIRICAL ADVERSARIAL CHALLENGE: PTS Ephemeral Updates & Continuity")
        print(" Target: PtsSyncCoordinator.evaluateIncomingUpdate")
        print("================================================================================")

        let coordinator = PtsSyncCoordinator()
        let chatId: Int64 = 42

        // Step 1: Initialize PTS to 100
        print("\n[STEP 1] Initializing chat \(chatId) PTS to 100...")
        await coordinator.setInitialPts(chatId: chatId, pts: 100)
        let initialPts = await coordinator.currentPts(for: chatId)
        let initialState = await coordinator.currentState(for: chatId)
        assert(initialPts == 100, "Initial PTS must be 100, got \(initialPts)")
        assert(initialState == .idle, "Initial state must be .idle, got \(initialState)")
        print("✓ Verified: Initial PTS = 100, State = .idle")

        // Step 2: Challenge with Ephemeral update: ptsCount == 0, pts = 120 > currentPts (100)
        print("\n[STEP 2] Ephemeral challenge: ptsCount=0, pts=120 > currentPts (100)...")
        let resEphemeral = await coordinator.evaluateIncomingUpdate(
            chatId: chatId,
            pts: 120,
            ptsCount: 0,
            eventJson: "{\"action\":\"typing\",\"actor\":\"Alice\"}"
        )

        // Assertions per prompt:
        // 1. Returns .applyImmediately
        // 2. Does NOT set state to .syncing (state remains .idle)
        // 3. Does NOT return .needsDifferenceSync
        // 4. Does NOT advance currentPts (remains 100)
        guard resEphemeral == .applyImmediately else {
            fatalError("CHALLENGE FAILED: Ephemeral update returned \(resEphemeral), expected .applyImmediately")
        }
        print("✓ Verified: Returned .applyImmediately (NOT .needsDifferenceSync)")

        let stateAfterEphemeral = await coordinator.currentState(for: chatId)
        guard stateAfterEphemeral == .idle else {
            fatalError("CHALLENGE FAILED: Ephemeral update altered state to \(stateAfterEphemeral), expected .idle")
        }
        print("✓ Verified: State remains .idle (NOT .syncing)")

        let ptsAfterEphemeral = await coordinator.currentPts(for: chatId)
        guard ptsAfterEphemeral == 100 else {
            fatalError("CHALLENGE FAILED: Ephemeral update advanced currentPts to \(ptsAfterEphemeral), expected 100")
        }
        print("✓ Verified: Persistent PTS unchanged at 100")

        // Step 3: Challenge with subsequent contiguous update: pts = 101, ptsCount = 1
        print("\n[STEP 3] Subsequent contiguous update: pts=101, ptsCount=1 (prevPts = 100)...")
        let resContiguous = await coordinator.evaluateIncomingUpdate(
            chatId: chatId,
            pts: 101,
            ptsCount: 1,
            eventJson: "{\"id\":101,\"text\":\"Hello\"}"
        )

        guard resContiguous == .applyImmediately else {
            fatalError("CHALLENGE FAILED: Contiguous update returned \(resContiguous), expected .applyImmediately")
        }
        print("✓ Verified: Contiguous update returned .applyImmediately")

        let ptsAfterContiguous = await coordinator.currentPts(for: chatId)
        guard ptsAfterContiguous == 101 else {
            fatalError("CHALLENGE FAILED: Contiguous update did not advance PTS to 101, got \(ptsAfterContiguous)")
        }
        print("✓ Verified: Persistent PTS correctly advanced to 101")

        let stateAfterContiguous = await coordinator.currentState(for: chatId)
        guard stateAfterContiguous == .idle else {
            fatalError("CHALLENGE FAILED: State after contiguous is \(stateAfterContiguous), expected .idle")
        }
        print("✓ Verified: State remains .idle")

        // Step 4: Empirical proof that Ephemeral update was NOT buffered into eventBuffers
        print("\n[STEP 4] Empirical verification that Ephemeral update (120) was NOT buffered...")
        // Introduce a gap: pts = 105, ptsCount = 1 (prevPts = 104 > current 101)
        let resGap = await coordinator.evaluateIncomingUpdate(
            chatId: chatId,
            pts: 105,
            ptsCount: 1,
            eventJson: "{\"id\":105,\"text\":\"Gap message\"}"
        )
        guard resGap == .needsDifferenceSync(missingRange: 102...104) else {
            fatalError("CHALLENGE FAILED: Expected gap 102...104, got \(resGap)")
        }
        let syncingState = await coordinator.currentState(for: chatId)
        guard syncingState == .syncing(fromPts: 101) else {
            fatalError("CHALLENGE FAILED: Expected .syncing(fromPts: 101), got \(syncingState)")
        }

        // Apply difference response: caught up to 104, journal ends
        let catchupAction = await coordinator.applyDifferenceResponse(
            chatId: chatId,
            newPts: 104,
            hasMore: false,
            snapshotRequired: false
        )

        // If ephemeral update 120 had been buffered, it would be in replayedUpdates or disrupt replay
        guard case .completedCatchup(let replayed) = catchupAction else {
            fatalError("CHALLENGE FAILED: Expected .completedCatchup, got \(catchupAction)")
        }

        guard replayed.count == 1, replayed[0].pts == 105 else {
            fatalError("CHALLENGE FAILED: Replayed count should be exactly 1 (pts 105), but got \(replayed.map { $0.pts }). Ephemeral update leaked into buffer!")
        }
        print("✓ Verified: Only update 105 replayed. Ephemeral update 120 was NEVER buffered into eventBuffers.")

        let finalPts = await coordinator.currentPts(for: chatId)
        guard finalPts == 105 else {
            fatalError("CHALLENGE FAILED: Final PTS should be 105, got \(finalPts)")
        }

        // Step 5: Adversarial Stress: 1,000 Rapid Ephemeral Updates with Divergent PTS
        print("\n[STEP 5] Adversarial Stress: 1,000 rapid ephemeral updates with PTS from 200 to 50,000...")
        for i in 1...1000 {
            let arbitraryPts = UInt64(200 + i * 50)
            let res = await coordinator.evaluateIncomingUpdate(chatId: chatId, pts: arbitraryPts, ptsCount: 0)
            guard res == .applyImmediately else {
                fatalError("CHALLENGE FAILED: Burst ephemeral update \(i) failed with \(res)")
            }
        }
        let ptsAfterBurst = await coordinator.currentPts(for: chatId)
        guard ptsAfterBurst == 105 else {
            fatalError("CHALLENGE FAILED: Persistent PTS corrupted after ephemeral burst: \(ptsAfterBurst)")
        }
        let stateAfterBurst = await coordinator.currentState(for: chatId)
        guard stateAfterBurst == .idle else {
            fatalError("CHALLENGE FAILED: State corrupted after ephemeral burst: \(stateAfterBurst)")
        }
        print("✓ Verified: 1,000 ephemeral updates processed cleanly. PTS remains 105, State remains .idle.")

        // Step 6: Verify contiguous update 106 immediately applies after the 1,000-burst
        print("\n[STEP 6] Contiguous update 106 after 1,000 ephemeral burst...")
        let res106 = await coordinator.evaluateIncomingUpdate(chatId: chatId, pts: 106, ptsCount: 1)
        guard res106 == .applyImmediately else {
            fatalError("CHALLENGE FAILED: Contiguous update 106 failed: \(res106)")
        }
        let ptsAfter106 = await coordinator.currentPts(for: chatId)
        guard ptsAfter106 == 106 else {
            fatalError("CHALLENGE FAILED: Contiguous update 106 failed to advance PTS: \(ptsAfter106)")
        }
        print("✓ Verified: Contiguous update 106 applies cleanly, advancing persistent PTS to 106.")

        print("\n================================================================================")
        print(" ALL PTS EPHEMERAL ADVERSARIAL CHALLENGES PASSED EMPIRICALLY WITH ZERO DEFECTS!")
        print("================================================================================")
    }
}
