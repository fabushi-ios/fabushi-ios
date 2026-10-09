import XCTest
@testable import Fabushi

final class AccessCoverParityTests: XCTestCase {
    func testSandAccessWireProjectionMatchesDesktopContract() {
        XCTAssertEqual(
            AccessCoverModel.project(stateWire: 1, reasonWire: 1),
            .init(state: .granted, reason: .none)
        )
        XCTAssertEqual(
            AccessCoverModel.project(stateWire: 2, reasonWire: 2),
            .init(state: .unavailable, reason: .teamPrivacyMode)
        )
        XCTAssertEqual(
            AccessCoverModel.project(stateWire: 3, reasonWire: 9),
            .init(state: .paymentRequired, reason: .paywallTeamAdmin)
        )
        XCTAssertEqual(
            AccessCoverModel.project(stateWire: 99, reasonWire: 99),
            .unknown
        )
    }

    func testAccessCoverRequiresCanonicalRosterFailureAndAllSuppressorsClear() {
        let visible = AccessCoverGateInput(
            rosterFailureCode: ACCESS_BLOCKED_FAILURE_CODE,
            hasReachedBox: false,
            isShowingRestoredRoster: false,
            isComputerRebuildLocked: false
        )
        XCTAssertTrue(AccessCoverModel.shouldShowCover(visible))

        XCTAssertFalse(AccessCoverModel.shouldShowCover(.init(
            rosterFailureCode: "network",
            hasReachedBox: false,
            isShowingRestoredRoster: false,
            isComputerRebuildLocked: false
        )))
        XCTAssertFalse(AccessCoverModel.shouldShowCover(.init(
            rosterFailureCode: ACCESS_BLOCKED_FAILURE_CODE,
            hasReachedBox: true,
            isShowingRestoredRoster: false,
            isComputerRebuildLocked: false
        )))
        XCTAssertFalse(AccessCoverModel.shouldShowCover(.init(
            rosterFailureCode: ACCESS_BLOCKED_FAILURE_CODE,
            hasReachedBox: false,
            isShowingRestoredRoster: true,
            isComputerRebuildLocked: false
        )))
        XCTAssertFalse(AccessCoverModel.shouldShowCover(.init(
            rosterFailureCode: ACCESS_BLOCKED_FAILURE_CODE,
            hasReachedBox: false,
            isShowingRestoredRoster: false,
            isComputerRebuildLocked: true
        )))
    }

    func testAccessCopyPreservesDesktopReasonSpecificActions() {
        XCTAssertEqual(
            AccessCoverModel.noticeCopy(for: .init(
                state: .paymentRequired,
                reason: .freeTrialAvailable
            ))?.action,
            "Start Trial"
        )
        XCTAssertEqual(
            AccessCoverModel.noticeCopy(for: .init(
                state: .paymentRequired,
                reason: .paywallIndividual
            ))?.action,
            "Upgrade"
        )
        XCTAssertEqual(
            AccessCoverModel.noticeCopy(for: .init(
                state: .unavailable,
                reason: .notOffered
            ))?.action,
            nil
        )
        XCTAssertNil(AccessCoverModel.noticeCopy(for: .checking))
        XCTAssertNil(AccessCoverModel.noticeCopy(for: .unknown))
    }

    func testFirstBoxGateIsStickyAndSuppressesRestoredAccessAndConnectivityFailures() {
        var state = FirstBoxGateState.initial
        state = FirstBoxGate.project(
            previous: state,
            roster: .init(
                loadState: .error,
                isShowingRestoredRoster: false,
                failureCode: nil,
                failureTransportKind: nil
            )
        )
        XCTAssertTrue(state.isAwaitingFirstBox)
        XCTAssertFalse(state.hasReachedBox)

        for suppressed in [
            FirstBoxRosterSnapshot(
                loadState: .error,
                isShowingRestoredRoster: true,
                failureCode: nil,
                failureTransportKind: nil
            ),
            FirstBoxRosterSnapshot(
                loadState: .error,
                isShowingRestoredRoster: false,
                failureCode: ACCESS_BLOCKED_FAILURE_CODE,
                failureTransportKind: nil
            ),
            FirstBoxRosterSnapshot(
                loadState: .error,
                isShowingRestoredRoster: false,
                failureCode: nil,
                failureTransportKind: "network"
            ),
            FirstBoxRosterSnapshot(
                loadState: .error,
                isShowingRestoredRoster: false,
                failureCode: nil,
                failureTransportKind: "dns"
            ),
        ] {
            XCTAssertFalse(
                FirstBoxGate.project(previous: .initial, roster: suppressed).isAwaitingFirstBox
            )
        }

        state = FirstBoxGate.project(
            previous: .initial,
            roster: .init(
                loadState: .ready,
                isShowingRestoredRoster: false,
                failureCode: nil,
                failureTransportKind: nil
            )
        )
        XCTAssertTrue(state.hasReachedBox)
        XCTAssertFalse(state.isAwaitingFirstBox)

        state = FirstBoxGate.project(
            previous: state,
            roster: .init(
                loadState: .error,
                isShowingRestoredRoster: false,
                failureCode: nil,
                failureTransportKind: nil
            )
        )
        XCTAssertTrue(state.hasReachedBox)
        XCTAssertFalse(state.isAwaitingFirstBox)
        XCTAssertEqual(FirstBoxGate.reset(), .initial)
    }
    func testLiveAccessProjectionAcceptsWireAndNamedPayloadsFailClosed() {
        XCTAssertEqual(
            AccessCoverModel.project(
                foundationValue: ["state": 3, "reason": 6]
            ),
            .init(state: .paymentRequired, reason: .freeTrialAvailable)
        )
        XCTAssertEqual(
            AccessCoverModel.project(
                foundationValue: [
                    "access": [
                        "state": "unavailable",
                        "reason": "teamAccessRequired",
                    ],
                ]
            ),
            .init(state: .unavailable, reason: .teamAccessRequired)
        )
        XCTAssertEqual(
            AccessCoverModel.project(foundationValue: ["unexpected": true]),
            .unknown
        )
    }

    func testRosterProjectionRestoresThenReplacesWithCompleteLiveRoster() {
        let restoredBot = MobileBotSummary(
            id: "restored",
            name: "Restored",
            description: "cached"
        )
        let liveBot = MobileBotSummary(
            id: "live",
            name: "Live",
            description: "authoritative"
        )

        let restored = AccessRosterSnapshotProjection.restore([restoredBot])
        XCTAssertEqual(restored.bots, [restoredBot])
        XCTAssertTrue(restored.isShowingRestoredRoster)
        XCTAssertFalse(restored.hasCompleteRoster)
        XCTAssertEqual(restored.loadState, .ready)

        let fetching = AccessRosterSnapshotProjection.beginFetch(restored)
        XCTAssertTrue(fetching.isFetching)
        XCTAssertEqual(fetching.loadState, .ready)

        let complete = AccessRosterSnapshotProjection.complete(
            [liveBot],
            previous: fetching
        )
        XCTAssertEqual(complete.bots, [liveBot])
        XCTAssertTrue(complete.hasCompleteRoster)
        XCTAssertFalse(complete.isShowingRestoredRoster)
        XCTAssertFalse(complete.isFetching)
        XCTAssertEqual(complete.confirmedFetches, 1)
        XCTAssertEqual(complete.transport, .connected)
    }

    func testRosterFailurePreservesRestoredRowsAndSeparatesNetworkFromAccessBlock() {
        let cached = MobileBotSummary(
            id: "cached",
            name: "Cached",
            description: "offline"
        )
        let restored = AccessRosterSnapshotProjection.restore([cached])
        let network = AccessRosterFailureClassifier.failure(
            for: URLError(.dnsLookupFailed),
            access: .unknown
        )
        XCTAssertEqual(network.code, "dns")
        XCTAssertEqual(network.transportKind, "dns")

        let failed = AccessRosterSnapshotProjection.fail(network, previous: restored)
        XCTAssertEqual(failed.bots, [cached])
        XCTAssertTrue(failed.isShowingRestoredRoster)
        XCTAssertEqual(failed.loadState, .ready)
        XCTAssertEqual(failed.transport, .down)

        let blocked = AccessRosterFailureClassifier.failure(
            for: NSError(
                domain: "Fabushi.AccessRoster",
                code: 1,
                userInfo: [NSLocalizedDescriptionKey: "roster denied"]
            ),
            access: .init(state: .paymentRequired, reason: .paywallIndividual)
        )
        XCTAssertEqual(blocked.code, ACCESS_BLOCKED_FAILURE_CODE)
        XCTAssertNil(blocked.transportKind)
    }

    func testRosterPersistenceIsAccountScopedAndRejectsCrossAccountReuse() throws {
        let suite = "AccessCoverParityTests.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }

        let bot = MobileBotSummary(
            id: "agent-1",
            name: "Agent One",
            description: "Persisted",
            unread: true,
            lastMessagePreview: "hello",
            updatedAtMs: 42,
            isRunning: true,
            memberIds: ["peer-1"]
        )
        AccessRosterPersistence.save(
            [bot],
            accountScopeKey: "account-a",
            defaults: defaults
        )

        XCTAssertEqual(
            AccessRosterPersistence.load(
                accountScopeKey: "account-a",
                defaults: defaults
            ),
            [bot]
        )
        XCTAssertTrue(
            AccessRosterPersistence.load(
                accountScopeKey: "account-b",
                defaults: defaults
            ).isEmpty
        )
    }

    func testAccessCoverCompositionUsesStructuredRosterFailureAndSuppressors() {
        let blocked = AccessRosterFailure(
            code: ACCESS_BLOCKED_FAILURE_CODE,
            message: "blocked",
            transportKind: nil
        )
        let roster = AccessRosterSnapshot(
            bots: [],
            hasCompleteRoster: false,
            isShowingRestoredRoster: false,
            loadState: .error,
            failure: blocked,
            isFetching: false,
            confirmedFetches: 0,
            transport: .connected
        )
        let state = AccessCoverComposition.project(
            access: .init(state: .paymentRequired, reason: .paywallIndividual),
            roster: roster,
            firstBox: .initial,
            isComputerRebuildLocked: false
        )
        XCTAssertTrue(state.isVisible)
        XCTAssertTrue(state.isError)

        XCTAssertFalse(
            AccessCoverComposition.project(
                access: state.access,
                roster: roster,
                firstBox: .initial,
                isComputerRebuildLocked: true
            ).isVisible
        )
    }

    func testRosterSelectionPersistenceIsAccountScoped() throws {
        let suite = "AccessRosterSelectionTests.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }

        let selected = AccessRosterSelectionState(
            currentAgentID: "agent-2",
            isLoadPending: false
        )
        AccessRosterSelectionPersistence.save(
            selected,
            accountScopeKey: "account-a",
            defaults: defaults
        )

        XCTAssertEqual(
            AccessRosterSelectionPersistence.load(
                accountScopeKey: "account-a",
                defaults: defaults
            ),
            selected
        )
        XCTAssertEqual(
            AccessRosterSelectionPersistence.load(
                accountScopeKey: "account-b",
                defaults: defaults
            ),
            .empty
        )

        AccessRosterSelectionPersistence.clear(
            accountScopeKey: "account-a",
            defaults: defaults
        )
        XCTAssertEqual(
            AccessRosterSelectionPersistence.load(
                accountScopeKey: "account-a",
                defaults: defaults
            ),
            .empty
        )
    }

    func testRosterSelectionFencesMissingPendingAgentUntilSettle() {
        let selected = AccessRosterSelectionProjection.select(
            "missing",
            previous: .empty
        )
        XCTAssertEqual(
            selected,
            .init(currentAgentID: "missing", isLoadPending: true)
        )

        let reconciledWhilePending = AccessRosterSelectionProjection.reconcile(
            selected,
            agentIDs: ["agent-1", "agent-2"],
            isRosterComplete: true
        )
        XCTAssertEqual(reconciledWhilePending, selected)

        let settled = AccessRosterSelectionProjection.settle(
            reconciledWhilePending,
            attemptedAgentID: "missing",
            completeAgentIDs: ["agent-1", "agent-2"]
        )
        XCTAssertEqual(
            settled,
            .init(currentAgentID: "agent-1", isLoadPending: false)
        )
    }

    func testRosterSelectionRestoreAndCompleteRosterReconciliation() {
        let restored = AccessRosterSelectionState(
            currentAgentID: "agent-2",
            isLoadPending: false
        )
        XCTAssertEqual(
            AccessRosterSelectionProjection.reconcile(
                restored,
                agentIDs: ["agent-1", "agent-2"],
                isRosterComplete: true
            ),
            restored
        )
        XCTAssertEqual(
            AccessRosterSelectionProjection.reconcile(
                restored,
                agentIDs: ["agent-1"],
                isRosterComplete: true
            ),
            .init(currentAgentID: "agent-1", isLoadPending: false)
        )
        XCTAssertEqual(
            AccessRosterSelectionProjection.reconcile(
                restored,
                agentIDs: ["agent-1"],
                isRosterComplete: false
            ),
            restored
        )
    }

    func testRuntimeRosterReadinessIsAccountBoundConnectedLoadedAndSelected() {
        let botA = MobileBotSummary(id: "a", name: "A", description: "")
        let botB = MobileBotSummary(id: "b", name: "B", description: "")
        let ready = AccessRosterSnapshot(
            bots: [botA, botB],
            hasCompleteRoster: true,
            isShowingRestoredRoster: false,
            loadState: .ready,
            failure: nil,
            isFetching: false,
            confirmedFetches: 1,
            transport: .connected
        )
        let projected = AccessRosterReadinessProjection.select(
            accountScopeKey: "account-1",
            roster: ready,
            selectedAgentID: "b",
            isPrivacyBlocked: false
        )
        XCTAssertTrue(projected.isAccountBound)
        XCTAssertTrue(projected.isConnected)
        XCTAssertTrue(projected.isLoaded)
        XCTAssertTrue(projected.hasReachedBox)
        XCTAssertTrue(projected.hasSelectedAgent)
        XCTAssertTrue(projected.isSelectionReady)

        let disconnected = AccessRosterReadinessProjection.select(
            accountScopeKey: "account-1",
            roster: .init(
                bots: [botA],
                hasCompleteRoster: true,
                isShowingRestoredRoster: true,
                loadState: .error,
                failure: .init(
                    code: "DOWN",
                    message: nil,
                    transportKind: "network"
                ),
                isFetching: false,
                confirmedFetches: 0,
                transport: .down
            ),
            selectedAgentID: "a",
            isPrivacyBlocked: false
        )
        XCTAssertTrue(disconnected.hasReachedBox)
        XCTAssertFalse(disconnected.isSelectionReady)
        XCTAssertEqual(disconnected.rosterFailureCode, "DOWN")
        XCTAssertEqual(disconnected.rosterFailureTransportKind, "network")

        let loggedOut = AccessRosterReadinessProjection.select(
            accountScopeKey: nil,
            roster: ready,
            selectedAgentID: "a",
            isPrivacyBlocked: true
        )
        XCTAssertFalse(loggedOut.isAccountBound)
        XCTAssertFalse(loggedOut.hasReachedBox)
        XCTAssertFalse(loggedOut.hasSelectedAgent)
        XCTAssertFalse(loggedOut.isSelectionReady)
        XCTAssertTrue(loggedOut.isPrivacyBlocked)
    }

    func testTerminalProjectionNormalizesLegacyFieldsAndStatusPriority() {
        XCTAssertEqual(
            MobileTerminalOutputModel.normalizeOutput("a\r\nb\rc"),
            "a\nb\nc"
        )
        XCTAssertNil(MobileTerminalOutputModel.project(nil))
        XCTAssertNil(MobileTerminalOutputModel.project(["sessionId": "s"]))

        let snapshot = MobileTerminalOutputModel.project([
            "terminal_instance_id": 42,
            "metadata": [
                "cwd": "/repo",
                "currentCommand": ["command": "npm test"],
            ],
            "output_raw": "one\r\ntwo",
            "exit_code": 0,
        ])
        XCTAssertEqual(
            snapshot,
            .init(
                sessionId: "42",
                command: "npm test",
                cwd: "/repo",
                output: "one\ntwo",
                status: .exited,
                exitCode: 0
            )
        )
        XCTAssertEqual(
            MobileTerminalOutputModel.project([
                "sessionId": "run",
                "command": "build",
                "status": "running",
                "exitCode": 7,
            ])?.status,
            .running
        )
        XCTAssertEqual(
            MobileTerminalOutputModel.project([
                "sessionId": "run",
                "command": "build",
                "exitCode": 7,
            ])?.status,
            .error
        )
    }

}
