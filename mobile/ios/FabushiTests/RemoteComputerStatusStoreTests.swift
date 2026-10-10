import XCTest
@testable import Fabushi

@MainActor
final class RemoteComputerStatusStoreTests: XCTestCase {
    private enum FakeError: Error {
        case failed
    }

    @MainActor
    private final class FakeSource: RemoteComputerAgentBoxSourcing {
        var statusCalls: [String] = []
        var ensureCalls: [String] = []
        var releaseCalls: [(String, String)] = []
        var statusValues: [String: RemoteComputerAgentBoxSnapshot?] = [:]
        var ensureValues: [String: RemoteComputerAgentBoxSnapshot] = [:]
        var statusContinuation: CheckedContinuation<RemoteComputerAgentBoxSnapshot?, Error>?
        var ensureContinuation: CheckedContinuation<RemoteComputerAgentBoxSnapshot, Error>?
        var suspendStatus = false
        var suspendEnsure = false
        var failEnsure = false

        func status(agentID: String) async throws -> RemoteComputerAgentBoxSnapshot? {
            statusCalls.append(agentID)
            if suspendStatus {
                return try await withCheckedThrowingContinuation { continuation in
                    statusContinuation = continuation
                }
            }
            return statusValues[agentID] ?? nil
        }

        func ensure(agentID: String) async throws -> RemoteComputerAgentBoxSnapshot {
            ensureCalls.append(agentID)
            if failEnsure {
                throw FakeError.failed
            }
            if suspendEnsure {
                return try await withCheckedThrowingContinuation { continuation in
                    ensureContinuation = continuation
                }
            }
            guard let value = ensureValues[agentID] else {
                throw FakeError.failed
            }
            return value
        }

        func release(agentID: String, trigger: String) async throws {
            releaseCalls.append((agentID, trigger))
        }
    }

    private func snapshot(
        _ agentID: String,
        host: String,
        state: String = "running",
        pressure: RemoteComputerAgentBoxDiskPressureSnapshot? = nil
    ) -> RemoteComputerAgentBoxSnapshot {
        .init(
            agentID: agentID,
            state: state,
            vncURL: URL(string: "https://\(host)/vnc.html"),
            imageUpdateAvailable: false,
            diskPressure: pressure
        )
    }

    private func scope(_ agentID: String) -> RemoteComputerScope {
        .init(
            accountScopeKey: "account-a",
            agentID: agentID,
            agentName: agentID
        )
    }

    private func waitUntil(
        _ predicate: @escaping @MainActor () -> Bool,
        iterations: Int = 200
    ) async {
        for _ in 0..<iterations {
            if predicate() { return }
            await Task.yield()
        }
    }

    func testStatusReadDedupesTimesOutAndAcceptsLateResult() async {
        let source = FakeSource()
        source.failEnsure = true
        source.suspendStatus = true
        let owner = RemoteComputerAgentBoxOwner(
            source: source,
            statusTimeoutMilliseconds: 5
        )

        await owner.connect(scope: scope("agent-a"))
        await waitUntil { source.statusContinuation != nil }
        XCTAssertEqual(source.statusCalls, ["agent-a"])

        let duplicate = Task { @MainActor in
            await owner.refresh(agentID: "agent-a")
        }
        await Task.yield()
        XCTAssertEqual(source.statusCalls, ["agent-a"])

        try? await Task.sleep(nanoseconds: 20_000_000)
        XCTAssertEqual(owner.readState(for: "agent-a"), .error)
        XCTAssertNil(owner.status(for: "agent-a"))

        source.statusContinuation?.resume(
            returning: snapshot("agent-a", host: "late.agent.example")
        )
        source.statusContinuation = nil
        await duplicate.value
        await waitUntil { owner.status(for: "agent-a") != nil }

        XCTAssertEqual(owner.readState(for: "agent-a"), .known)
        XCTAssertEqual(owner.status(for: "agent-a")?.vncURL?.host, "late.agent.example")
        owner.dispose()
    }

    func testEnsureDedupesAndLateScopeGenerationCannotOverwriteNewAgent() async {
        let source = FakeSource()
        source.suspendEnsure = true
        let owner = RemoteComputerAgentBoxOwner(
            source: source,
            statusTimeoutMilliseconds: 1_000
        )

        let first = Task { @MainActor in
            await owner.ensure(agentID: "agent-a")
        }
        await waitUntil { source.ensureContinuation != nil }

        let duplicate = Task { @MainActor in
            await owner.ensure(agentID: "agent-a")
        }
        await Task.yield()
        XCTAssertEqual(source.ensureCalls, ["agent-a"])

        source.ensureContinuation?.resume(
            returning: snapshot("agent-a", host: "agent-a.example")
        )
        source.ensureContinuation = nil
        await first.value
        await duplicate.value
        XCTAssertEqual(owner.status(for: "agent-a")?.vncURL?.host, "agent-a.example")
        owner.dispose()
    }

    func testIngestForeverBoxFencesStaleStatusRead() async {
        let source = FakeSource()
        source.failEnsure = true
        source.suspendStatus = true
        let owner = RemoteComputerAgentBoxOwner(
            source: source,
            statusTimeoutMilliseconds: 1_000
        )

        await owner.connect(scope: scope("agent-a"))
        await waitUntil { source.statusContinuation != nil }

        owner.ingestForeverBox(snapshot("agent-a", host: "fresh.agent.example"))
        source.statusContinuation?.resume(
            returning: snapshot("agent-a", host: "stale.agent.example")
        )
        source.statusContinuation = nil
        await waitUntil { owner.readState(for: "agent-a") == .known }

        XCTAssertEqual(owner.status(for: "agent-a")?.vncURL?.host, "fresh.agent.example")
        owner.dispose()
    }

    func testRecordCacheEvictsOldUnwatchedEntriesButRetainsWatchedEntry() {
        let source = FakeSource()
        let owner = RemoteComputerAgentBoxOwner(source: source)

        owner.ingestForeverBox(snapshot("keep", host: "keep.example"))
        let release = owner.retain(agentID: "keep")

        for index in 0..<40 {
            owner.ingestForeverBox(
                snapshot("agent-\(index)", host: "agent-\(index).example")
            )
        }

        XCTAssertLessThanOrEqual(owner.cachedRecordCount, RemoteComputerAgentBoxOwner.recordLimit)
        XCTAssertEqual(owner.status(for: "keep")?.vncURL?.host, "keep.example")
        XCTAssertEqual(owner.status(for: "agent-39")?.vncURL?.host, "agent-39.example")

        release()
        owner.dispose()
    }

    func testFocusReconnectHydrateWatchedDemandedRecord() async {
        let source = FakeSource()
        let value = snapshot("agent-a", host: "agent-a.example")
        source.ensureValues["agent-a"] = value
        source.statusValues["agent-a"] = value
        let owner = RemoteComputerAgentBoxOwner(source: source)

        await owner.connect(scope: scope("agent-a"))
        await waitUntil { source.statusCalls.count >= 1 }
        let firstStatusCount = source.statusCalls.count
        let firstEnsureCount = source.ensureCalls.count

        owner.noteWindowFocus()
        await waitUntil {
            source.statusCalls.count > firstStatusCount
                && source.ensureCalls.count > firstEnsureCount
        }
        XCTAssertGreaterThan(source.statusCalls.count, firstStatusCount)
        XCTAssertGreaterThan(source.ensureCalls.count, firstEnsureCount)

        let focusStatusCount = source.statusCalls.count
        let focusEnsureCount = source.ensureCalls.count
        await owner.noteReconnect()
        await waitUntil { source.statusCalls.count > focusStatusCount }

        XCTAssertGreaterThan(source.statusCalls.count, focusStatusCount)
        XCTAssertGreaterThan(source.ensureCalls.count, focusEnsureCount)
        XCTAssertEqual(owner.reloadRevision, 1)
        owner.dispose()
    }

    func testDiskPressureComputerActionPresenceResetAndDispose() {
        let source = FakeSource()
        let owner = RemoteComputerAgentBoxOwner(source: source)
        var actions: [RemoteComputerAgentBoxAction] = []
        let unsubscribe = owner.subscribeComputerActions { actions.append($0) }

        let pressure = RemoteComputerAgentBoxDiskPressureSnapshot(
            level: "warning",
            availableBytes: 100,
            totalBytes: 1_000
        )
        owner.ingestBoxDiskPressure(pressure)
        owner.ingestVncUserPresence(isPresent: true)
        owner.ingestComputerAction(
            .init(
                agentID: "agent-a",
                kind: "click",
                x: 0.25,
                y: 0.5
            )
        )

        XCTAssertEqual(owner.diskPressureSnapshot, pressure)
        XCTAssertTrue(owner.vncUserPresent)
        XCTAssertEqual(actions.map(\.kind), ["click"])

        owner.reset()
        XCTAssertNil(owner.diskPressureSnapshot)
        XCTAssertFalse(owner.vncUserPresent)
        XCTAssertEqual(owner.readState, .unknown)

        owner.dispose()
        owner.ingestVncUserPresence(isPresent: true)
        owner.ingestComputerAction(
            .init(agentID: "agent-a", kind: "move", x: 1, y: 1)
        )
        XCTAssertFalse(owner.vncUserPresent)
        XCTAssertEqual(actions.map(\.kind), ["click"])
        unsubscribe()
    }

    func testProjectedStatusCarriesDiskPressureIntoSingleOwner() throws {
        let projected = try IOSRemoteComputerAgentBoxSource.projectStatus(
            [
                "agentId": "agent-a",
                "state": "running",
                "vncUrl": "https://agent-a.example/vnc.html",
                "diskPressure": [
                    "level": "critical",
                    "availableBytes": 64,
                    "totalBytes": 1_024,
                ],
            ],
            expectedAgentID: "agent-a"
        )

        XCTAssertEqual(projected.diskPressure?.level, "critical")
        XCTAssertEqual(projected.diskPressure?.availableBytes, 64)
        XCTAssertEqual(projected.diskPressure?.totalBytes, 1_024)
    }
}
