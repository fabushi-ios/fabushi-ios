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
