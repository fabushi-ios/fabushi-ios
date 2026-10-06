import XCTest
@testable import Fabushi

@MainActor
final class RemoteComputerRebuildTests: XCTestCase {
    private final class FakeSource: RemoteComputerRebuildSource {
        var supportsManagedLifecycle = true
        var migrationValue: Any = NSNull()
        var migrationReads = 0
        var updateForces: [Bool] = []
        var recreateCalls = 0
        var nextUpdateValue: Any = ["status": "started"]
        var nextRecreateValue: Any = [
            "status": "started",
            "operationId": "reset-op",
        ]

        func getMigrationStatus() async throws -> Any {
            migrationReads += 1
            return migrationValue
        }

        func update(force: Bool) async throws -> Any {
            updateForces.append(force)
            return nextUpdateValue
        }

        func recreate() async throws -> Any {
            recreateCalls += 1
            migrationValue = [
                "operationId": "reset-op",
                "phase": "done",
                "detail": "migration complete",
            ]
            return nextRecreateValue
        }

    }

    func testAgentComputerScopeIsExplicitAndStable() {
        let scoped = RemoteComputerScope(
            accountScopeKey: "account-a",
            agentID: "agent-7",
            agentName: "Research"
        )
        XCTAssertEqual(scoped.scopeKey, "account-a:agent-7")
        XCTAssertEqual(scoped.displayTitle, "Research 的电脑")

        let accountOnly = RemoteComputerScope(
            accountScopeKey: "account-a",
            agentID: nil,
            agentName: nil
        )
        XCTAssertEqual(accountOnly.scopeKey, "account-a:account")
        XCTAssertEqual(accountOnly.displayTitle, "我的电脑")
        XCTAssertNotEqual(scoped.scopeKey, accountOnly.scopeKey)
    }



    func testComputerShellModelProjectsStatusMonitorsAndTaskActivity() {
        let status: [String: Any] = [
            "state": "running",
            "vncUrl": "https://vnc.example/view",
            "pull": ["percent": 0.25],
            "windows": [["id": "window-1"]],
            "handoff": [
                "requestId": "handoff-1",
                "instruction": "Complete sign-in",
                "snapshotDataUrl": "data:image/png;base64,abc",
            ],
        ]
        let projection = RemoteComputerShellModel.projectStatus(
            status,
            readState: .known
        )
        XCTAssertEqual(projection.phase, .pulling)
        XCTAssertTrue(projection.isStatusKnown)
        XCTAssertFalse(projection.isStatusUnavailable)
        XCTAssertEqual(projection.pullPercent, 0.25)
        XCTAssertEqual(projection.vncURL, "https://vnc.example/view")
        XCTAssertEqual(projection.handoff?.requestID, "handoff-1")
        XCTAssertEqual(projection.windows.count, 1)

        let subagents: [Any] = [
            [
                "status": "running",
                "subagentType": "computerUse",
                "subagentId": "sub-1",
                "title": "  Research  ",
            ],
            [
                "status": "done",
                "subagentType": "computerUse",
                "subagentId": "sub-2",
            ],
        ]
        let monitors = RemoteComputerShellModel.projectMonitors(
            subagents: subagents
        ) { id in
            id == "sub-1"
                ? ["state": "running", "vncUrl": "https://vnc.example/sub"] as [String: Any]
                : nil
        }
        XCTAssertEqual(monitors.map(\.subagentID), ["sub-1"])
        XCTAssertEqual(monitors.first?.title, "Research")
        XCTAssertTrue(RemoteComputerShellModel.isComputerUseTaskActive([
            ["subagentType": "computerUse"],
        ]))
    }

    func testComputerShellModelPreservesVNCAndCursorSemantics() {
        let special = "https://vnc.example/sand-special-treatment-v1/vnc.html?path=websockify%3Ftoken%3Ddisplay-7"
        let dimensions = RemoteComputerShellModel.vncDimensions(special)
        XCTAssertEqual(dimensions.width, 2048)
        XCTAssertEqual(dimensions.height, 2048)

        let identity = RemoteComputerShellModel.vncIdentity(special)
        XCTAssertEqual(identity.host, "vnc.example")
        XCTAssertEqual(identity.display, "display-7")

        let viewer = RemoteComputerShellModel.vncViewerURL(
            "https://vnc.example/vnc.html?foo=1",
            interactive: true
        )
        let query = URLComponents(
            url: try XCTUnwrap(viewer),
            resolvingAgainstBaseURL: false
        )?.queryItems ?? []
        XCTAssertEqual(query.first(where: { $0.name == "autoconnect" })?.value, "true")
        XCTAssertEqual(query.first(where: { $0.name == "resize" })?.value, "scale")
        XCTAssertEqual(query.first(where: { $0.name == "reconnect" })?.value, "true")
        XCTAssertEqual(query.first(where: { $0.name == "sandInteractive" })?.value, "1")

        let first = RemoteComputerShellModel.projectCursor(
            [
                "agentId": "agent-1",
                "type": "move",
                "x": 10,
                "y": 20,
            ],
            previous: nil,
            nowMilliseconds: 1_000
        )
        XCTAssertEqual(first?.sequence, 1)
        XCTAssertNil(first?.lastMovedAtMilliseconds)

        let click = RemoteComputerShellModel.projectCursor(
            [
                "agentId": "agent-1",
                "type": "click",
                "x": 11,
                "y": 20,
            ],
            previous: first,
            nowMilliseconds: 1_200
        )
        XCTAssertEqual(click?.sequence, 2)
        XCTAssertEqual(click?.clickSequence, 1)
        XCTAssertEqual(click?.millisecondsSinceMove, 0)
        let presentation = RemoteComputerShellModel.cursorPresentation(
            click,
            hasFrame: true
        )
        XCTAssertTrue(presentation.isGliding)
        XCTAssertTrue(presentation.isVisible)
        XCTAssertEqual(presentation.press?.key, 1)
        XCTAssertEqual(presentation.press?.delayMilliseconds, 500)
    }

    func testComputerShellModelPreservesStageSessionWarmPoolAndSelection() throws {
        XCTAssertEqual(
            RemoteComputerShellModel.stageCopy(
                isScreenLoading: true,
                isScreenUnavailable: false,
                subjectLabel: "Agent",
                isEmptyLoading: false,
                pullPercent: nil
            ).message,
            "Switching to Agent's screen…"
        )
        XCTAssertTrue(
            RemoteComputerShellModel.stageCopy(
                isScreenLoading: false,
                isScreenUnavailable: true,
                subjectLabel: "Agent",
                isEmptyLoading: false,
                pullPercent: nil
            ).hasRetry
        )
        XCTAssertEqual(
            RemoteComputerShellModel.retainWarmVNCSources(
                ["b", "a", "c"],
                source: "a",
                maxWarm: 2
            ),
            ["a", "b"]
        )

        let session = RemoteComputerShellModel.parseVNCSession(
            "{\"phase\":\"rfb_disconnect\",\"clean\":true}"
        )
        XCTAssertEqual(session?.phase, .disconnect)
        XCTAssertEqual(session?.clean, true)

        let monitors = [
            RemoteComputerShellMonitor(
                subagentID: "a",
                title: "A",
                vncURL: "https://a.example",
                handoff: nil
            ),
            RemoteComputerShellMonitor(
                subagentID: "b",
                title: "B",
                vncURL: "https://b.example",
                handoff: .init(
                    requestID: "handoff",
                    instruction: "Help",
                    snapshotDataURL: nil
                )
            ),
        ]
        XCTAssertEqual(
            RemoteComputerShellModel.firstSelectedMonitor(monitors, requested: nil),
            "b"
        )
        XCTAssertEqual(
            RemoteComputerShellModel.stepSelectedMonitor(
                monitors,
                current: "b",
                delta: 1
            ),
            "a"
        )
        XCTAssertEqual(
            RemoteComputerShellModel.handoffStatusLabel("dismissed").label,
            "Skipped"
        )
        XCTAssertTrue(
            RemoteComputerShellModel.handoffStatusLabel("unknown").muted
        )
    }



    func testHostActivityProjectionKeepsOnlyRunningComputerUseState() {
        let snapshot = IOSRemoteComputerHostActivitySource.project(
            agentID: "agent-a",
            subagentEvent: [
                "subagents": [
                    [
                        "id": "computer-1",
                        "status": "running",
                        "subagentType": "computerUse",
                    ],
                    [
                        "id": "research-1",
                        "status": "running",
                        "subagentType": "research",
                    ],
                    [
                        "id": "computer-done",
                        "status": "done",
                        "subagentType": "computerUse",
                    ],
                ],
            ],
            taskEvent: [
                "tasks": [
                    [
                        "id": "task-1",
                        "status": "running",
                        "subagentType": "computerUse",
                    ],
                ],
            ]
        )

        XCTAssertEqual(snapshot.agentID, "agent-a")
        XCTAssertEqual(snapshot.runningComputerSubagentIDs, ["computer-1"])
        XCTAssertTrue(snapshot.isComputerUseTaskActive)
        XCTAssertTrue(snapshot.isActive)
    }

    @MainActor
    func testHostActivityOwnerFencesStaleAgentRefresh() async throws {
        var continuations: [
            String: CheckedContinuation<RemoteComputerHostActivitySnapshot, Error>
        ] = [:]
        let owner = RemoteComputerHostActivityOwner { agentID in
            try await withCheckedThrowingContinuation {
                (continuation: CheckedContinuation<RemoteComputerHostActivitySnapshot, Error>) in
                continuations[agentID] = continuation
            }
        }

        let first = Task { await owner.refresh(agentID: "agent-a") }
        await Task.yield()
        let second = Task { await owner.refresh(agentID: "agent-b") }
        await Task.yield()

        let secondContinuation = try XCTUnwrap(continuations["agent-b"])
        secondContinuation.resume(returning: .init(
            agentID: "agent-b",
            runningComputerSubagentIDs: ["computer-b"],
            isComputerUseTaskActive: true
        ))
        await second.value

        let firstContinuation = try XCTUnwrap(continuations["agent-a"])
        firstContinuation.resume(returning: .init(
            agentID: "agent-a",
            runningComputerSubagentIDs: ["computer-a"],
            isComputerUseTaskActive: true
        ))
        await first.value

        XCTAssertEqual(owner.snapshot.agentID, "agent-b")
        XCTAssertEqual(owner.snapshot.runningComputerSubagentIDs, ["computer-b"])
        XCTAssertFalse(owner.isRefreshing)
    }


    func testRemoteComputerScopeUsesAgentNameInVisibleTitle() {
        let account = RemoteComputerScope(
            accountScopeKey: "account-a",
            agentID: nil,
            agentName: nil
        )
        XCTAssertEqual(account.displayTitle, "我的电脑")

        let agent = RemoteComputerScope(
            accountScopeKey: "account-a",
            agentID: "agent-7",
            agentName: "研究助手"
        )
        XCTAssertEqual(agent.displayTitle, "研究助手 的电脑")
        XCTAssertNotEqual(agent.scopeKey, account.scopeKey)
    }

    func testWebProcessCrashPolicyReloadsThreeTimesThenFailsClosed() {
        var policy = RemoteComputerWebProcessCrashPolicy()

        XCTAssertEqual(policy.recordCrash(atMilliseconds: 1_000), .reload)
        XCTAssertEqual(policy.recordCrash(atMilliseconds: 2_000), .reload)
        XCTAssertEqual(policy.recordCrash(atMilliseconds: 3_000), .reload)
        XCTAssertEqual(policy.recordCrash(atMilliseconds: 4_000), .failClosed)
        XCTAssertEqual(policy.crashCount, 4)
        XCTAssertTrue(policy.failedClosed)
    }

    func testWebProcessCrashPolicyStartsNewEpisodeAfterWindowOrExplicitReload() {
        var policy = RemoteComputerWebProcessCrashPolicy()

        XCTAssertEqual(policy.recordCrash(atMilliseconds: 1_000), .reload)
        XCTAssertEqual(
            policy.recordCrash(
                atMilliseconds: 1_000 + RemoteComputerWebProcessCrashPolicy.crashWindowMilliseconds
            ),
            .reload
        )
        XCTAssertEqual(policy.crashCount, 1)

        _ = policy.recordCrash(atMilliseconds: 62_000)
        policy.resetForExplicitReload()
        XCTAssertEqual(policy.crashCount, 0)
        XCTAssertNil(policy.lastCrashAtMilliseconds)
        XCTAssertFalse(policy.failedClosed)
        XCTAssertEqual(policy.recordCrash(atMilliseconds: 62_100), .reload)
    }

    func testOperationIDFencesStaleTerminalMigrationAndDoneSettles() {
        let operationA = RemoteComputerRebuildOperationID(value: "op-a")
        let operationB = RemoteComputerRebuildOperationID(value: "op-b")
        var state = RemoteComputerRebuildState.initial(boxPhase: "running")

        state = RemoteComputerRebuildReducer.reduce(
            state,
            .box(boxID: "forever-box", phase: "running", at: 1)
        )
        state = RemoteComputerRebuildReducer.reduce(
            state,
            .request(
                kind: .reset,
                operationID: operationA,
                source: nil,
                at: 2
            )
        )
        state = RemoteComputerRebuildReducer.reduce(
            state,
            .migration(operationID: operationB, phase: .done, at: 3)
        )

        XCTAssertFalse(state.hasTerminalMigration)
        XCTAssertEqual(state.kind, .reset)

        state = RemoteComputerRebuildReducer.reduce(
            state,
            .migration(operationID: operationA, phase: .done, at: 4)
        )
        XCTAssertTrue(state.hasTerminalMigration)

        state = RemoteComputerRebuildReducer.reduce(
            state,
            .deactivate(at: 5)
        )
        XCTAssertNil(state.kind)
        XCTAssertEqual(state.lastResolution, .settled)
        XCTAssertEqual(state.lastResolutionKind, .reset)
    }

    func testFailedAndDeactivatePreserveDesktopTerminalSemantics() {
        var failed = RemoteComputerRebuildState.initial()
        failed = RemoteComputerRebuildReducer.reduce(
            failed,
            .request(
                kind: .update,
                operationID: nil,
                source: .migration,
                at: 1
            )
        )
        failed = RemoteComputerRebuildReducer.reduce(
            failed,
            .migration(
                operationID: .init(value: "update-op"),
                phase: .failed,
                at: 2
            )
        )
        XCTAssertNil(failed.kind)
        XCTAssertEqual(failed.lastResolution, .failed)
        XCTAssertEqual(failed.lastResolutionKind, .update)

        var cancelled = RemoteComputerRebuildState.initial()
        cancelled = RemoteComputerRebuildReducer.reduce(
            cancelled,
            .request(
                kind: .recover,
                operationID: .init(value: "recover-op"),
                source: nil,
                at: 3
            )
        )
        cancelled = RemoteComputerRebuildReducer.reduce(
            cancelled,
            .deactivate(at: 4)
        )
        XCTAssertNil(cancelled.kind)
        XCTAssertEqual(cancelled.lastResolution, .cancelled)
        XCTAssertEqual(cancelled.lastResolutionKind, .recover)
    }

    func testHealthyDisconnectReconnectTracksTransportTeardown() {
        var state = RemoteComputerRebuildState.initial()
        state = RemoteComputerRebuildReducer.reduce(
            state,
            .box(boxID: "forever-box", phase: "running", at: 1)
        )
        state = RemoteComputerRebuildReducer.reduce(
            state,
            .request(
                kind: .reconnecting,
                operationID: nil,
                source: nil,
                at: 2
            )
        )
        state = RemoteComputerRebuildReducer.reduce(
            state,
            .connection(isConnected: false, at: 3)
        )

        XCTAssertTrue(state.hasLeftHealthy)
        XCTAssertEqual(state.teardownObserved, .transport)
        XCTAssertFalse(state.isConnected)

        state = RemoteComputerRebuildReducer.reduce(
            state,
            .box(boxID: "forever-box", phase: "starting", at: 4)
        )
        state = RemoteComputerRebuildReducer.reduce(
            state,
            .box(boxID: "forever-box", phase: "running", at: 5)
        )
        state = RemoteComputerRebuildReducer.reduce(
            state,
            .connection(isConnected: true, at: 6)
        )

        XCTAssertTrue(state.isConnected)
        XCTAssertTrue(state.reconnectedSinceLeft)
        XCTAssertEqual(state.boxPhase, "running")
        XCTAssertNotNil(state.readySince)
    }

    func testMigrationHistoryResetsWhenOperationChanges() {
        var accumulator = RemoteComputerMigrationAccumulator()
        XCTAssertTrue(accumulator.ingest(.init(
            operationID: .init(value: "op-a"),
            phase: .backingUp,
            detail: "backup"
        )))
        XCTAssertTrue(accumulator.ingest(.init(
            operationID: .init(value: "op-a"),
            phase: .moving,
            detail: "moving"
        )))
        XCTAssertEqual(accumulator.snapshot.phases, [.backingUp, .moving])

        XCTAssertTrue(accumulator.ingest(.init(
            operationID: .init(value: "op-b"),
            phase: .creating,
            detail: "new operation"
        )))
        XCTAssertEqual(accumulator.snapshot.operationID?.value, "op-b")
        XCTAssertEqual(accumulator.snapshot.phases, [.creating])

        XCTAssertTrue(accumulator.ingest(.init(
            operationID: .init(value: "op-b"),
            phase: .done,
            detail: "done"
        )))
        XCTAssertEqual(accumulator.snapshot.phase, .done)
        XCTAssertEqual(accumulator.snapshot.phases, [.creating])
    }

    func testForeverBoxProjectionCoversHydratedDesktopPhases() {
        XCTAssertEqual(
            RemoteComputerForeverBoxProjection.phase(.init(
                agentID: "forever-box",
                state: "running",
                pullPercent: 0.4,
                vncURL: "https://vnc.example",
                imageUpdateAvailable: true
            )),
            "pulling"
        )
        XCTAssertEqual(
            RemoteComputerForeverBoxProjection.phase(.init(
                agentID: "forever-box",
                state: "running",
                pullPercent: nil,
                vncURL: "https://vnc.example",
                imageUpdateAvailable: false
            )),
            "running"
        )
        XCTAssertEqual(
            RemoteComputerForeverBoxProjection.phase(.init(
                agentID: "forever-box",
                state: "running",
                pullPercent: nil,
                vncURL: nil,
                imageUpdateAvailable: false
            )),
            "local"
        )
        XCTAssertEqual(
            RemoteComputerForeverBoxProjection.phase(.init(
                agentID: "forever-box",
                state: "hibernated",
                pullPercent: nil,
                vncURL: nil,
                imageUpdateAvailable: false
            )),
            "sleeping"
        )
        XCTAssertEqual(
            RemoteComputerForeverBoxProjection.phase(.init(
                agentID: "forever-box",
                state: "stopped",
                pullPercent: nil,
                vncURL: nil,
                imageUpdateAvailable: false
            ), isStarting: true),
            "starting"
        )
    }

    func testBannerProjectsUpdateResetRecoverAndReconnectVariants() {
        for kind in [
            RemoteComputerRebuildKind.update,
            .reset,
            .recover,
        ] {
            var state = RemoteComputerRebuildState.initial(boxPhase: "running")
            state = RemoteComputerRebuildReducer.reduce(
                state,
                .request(
                    kind: kind,
                    operationID: .init(value: "\(kind.rawValue)-op"),
                    source: kind == .update ? .request : nil,
                    at: 1
                )
            )
            let presentation = RemoteComputerRebuildPresentation.project(
                state: state,
                migration: .empty
            )
            XCTAssertNotNil(presentation)
            XCTAssertEqual(
                presentation?.accessibilityIdentifier,
                "remote-computer-rebuild-\(kind.rawValue)"
            )
        }

        var reconnecting = RemoteComputerRebuildState.initial(
            boxPhase: "running"
        )
        reconnecting = RemoteComputerRebuildReducer.reduce(
            reconnecting,
            .request(
                kind: .reconnecting,
                operationID: nil,
                source: nil,
                at: 2
            )
        )
        reconnecting = RemoteComputerRebuildReducer.reduce(
            reconnecting,
            .connection(isConnected: false, at: 3)
        )
        XCTAssertEqual(
            RemoteComputerRebuildPresentation.project(
                state: reconnecting,
                migration: .empty
            )?.reconnectVariant,
            .network
        )

        reconnecting = RemoteComputerRebuildReducer.reduce(
            reconnecting,
            .box(
                boxID: "forever-box",
                phase: "starting",
                at: 4
            )
        )
        XCTAssertEqual(
            RemoteComputerRebuildPresentation.project(
                state: reconnecting,
                migration: .empty
            )?.reconnectVariant,
            .restarting
        )
    }

    func testShippingOwnerHydratesReconnectAndSettlesReset() async {
        let source = FakeSource()
        source.migrationValue = [
            "operationId": "existing-op",
            "phase": "backing-up",
            "detail": "restored after launch",
        ]
        var now: Int64 = 100
        let owner = RemoteComputerRebuildOwner(
            source: source,
            now: {
                now += 1
                return now
            }
        )

        await owner.connect()
        XCTAssertEqual(source.migrationReads, 1)
        XCTAssertEqual(owner.migrationSnapshot.operationID?.value, "existing-op")
        XCTAssertEqual(owner.migrationSnapshot.phases, [.backingUp])

        owner.noteNavigationFinished()
        await owner.noteReconnect()
        XCTAssertEqual(source.migrationReads, 2)
        XCTAssertEqual(owner.state.boxPhase, "running")
        XCTAssertTrue(owner.state.isConnected)

        await owner.requestReset()
        XCTAssertEqual(source.recreateCalls, 1)
        XCTAssertEqual(source.migrationReads, 3)
        XCTAssertNil(owner.state.kind)
        XCTAssertEqual(owner.state.lastResolution, .settled)
        XCTAssertEqual(owner.state.lastResolutionKind, .reset)
        XCTAssertEqual(owner.migrationSnapshot.operationID?.value, "reset-op")
        XCTAssertEqual(owner.migrationSnapshot.phase, .done)

        owner.dispose()
    }

    func testShippingOwnerUpdateAndReconnectUseSingleInjectedSource() async {
        let source = FakeSource()
        let owner = RemoteComputerRebuildOwner(
            source: source,
            now: { 200 }
        )
        await owner.connect()
        owner.noteNavigationFinished()

        await owner.requestUpdate(force: true)
        XCTAssertEqual(source.updateForces, [true])

        await owner.requestReconnect()
        XCTAssertEqual(owner.reloadRevision, 1)
        XCTAssertEqual(source.updateForces, [true])
        XCTAssertTrue(owner.state.isPending == false)

        owner.dispose()
    }

    func testUnavailableManagedLifecycleFailsClosedWithoutCallingBackend() async {
        let source = FakeSource()
        source.supportsManagedLifecycle = false
        let owner = RemoteComputerRebuildOwner(
            source: source,
            now: { 300 }
        )

        await owner.connect()
        XCTAssertFalse(owner.managedLifecycleAvailable)
        XCTAssertEqual(source.migrationReads, 0)

        await owner.requestUpdate()
        await owner.requestReset()
        await owner.requestRecover()
        XCTAssertTrue(source.updateForces.isEmpty)
        XCTAssertEqual(source.recreateCalls, 0)
        XCTAssertNotNil(owner.requestError)

        await owner.requestReconnect()
        XCTAssertEqual(owner.reloadRevision, 1)

        owner.dispose()
    }
}
