import XCTest
@testable import Fabushi

@MainActor
final class RemoteComputerStatusStoreTests: XCTestCase {
    private final class FakeSource: RemoteComputerStatusStoreSourcing {
        var readCalls: [String] = []
        var ensureCalls: [String] = []
        var handBackCalls: [(String, String)] = []
        var readHandler: ((String) async throws -> RemoteComputerAgentBoxSnapshot?)?
        var ensureHandler: ((String) async throws -> RemoteComputerAgentBoxSnapshot?)?

        func read(agentID: String) async throws -> RemoteComputerAgentBoxSnapshot? {
            readCalls.append(agentID)
            return try await readHandler?(agentID)
        }

        func ensure(agentID: String) async throws -> RemoteComputerAgentBoxSnapshot? {
            ensureCalls.append(agentID)
            return try await ensureHandler?(agentID)
        }

        func handBack(agentID: String, trigger: String) async throws {
            handBackCalls.append((agentID, trigger))
        }
    }

    private func snapshot(
        _ agentID: String,
        host: String = "agent.example"
    ) -> RemoteComputerAgentBoxSnapshot {
        .init(
            agentID: agentID,
            state: "running",
            vncURL: URL(string: "https://\(host)/vnc.html"),
            imageUpdateAvailable: false
        )
    }

    private func yieldUntil(
        attempts: Int = 200,
        _ predicate: @escaping @MainActor () -> Bool
    ) async {
        for _ in 0..<attempts {
            if predicate() { return }
            await Task.yield()
        }
    }

    func testReadDedupesAndCachesResolvedStatus() async throws {
        let source = FakeSource()
        var continuation: CheckedContinuation<RemoteComputerAgentBoxSnapshot?, Error>?
        source.readHandler = { _ in
            try await withCheckedThrowingContinuation { continuation = $0 }
        }
        let store = RemoteComputerStatusStore(source: source)
        let release = store.retain("agent-a")
        store.connect()
        store.refresh("agent-a")
        store.refresh("agent-a")
        await yieldUntil { source.readCalls.count == 1 }

        XCTAssertEqual(source.readCalls, ["agent-a"])
        continuation?.resume(returning: snapshot("agent-a"))
        await yieldUntil { store.status(for: "agent-a") != nil }

        XCTAssertEqual(store.readState(for: "agent-a"), .known)
        XCTAssertEqual(store.status(for: "agent-a")?.vncURL?.host, "agent.example")
        release()
        store.dispose()
    }

    func testTimeoutMarksErrorButAcceptsLateResultForSameAttempt() async {
        let source = FakeSource()
        var continuation: CheckedContinuation<RemoteComputerAgentBoxSnapshot?, Error>?
        source.readHandler = { _ in
            try await withCheckedThrowingContinuation { continuation = $0 }
        }
        let store = RemoteComputerStatusStore(
            source: source,
            deadline: .init(timeoutNanoseconds: 1_000_000)
        )
        let release = store.retain("agent-a")
        store.connect()
        await yieldUntil { store.readState(for: "agent-a") == .error }

        XCTAssertEqual(store.readState(for: "agent-a"), .error)
        XCTAssertNil(store.status(for: "agent-a"))

        continuation?.resume(returning: snapshot("agent-a", host: "late.example"))
        await yieldUntil { store.status(for: "agent-a")?.vncURL?.host == "late.example" }

        XCTAssertEqual(store.readState(for: "agent-a"), .known)
        release()
        store.dispose()
    }

    func testNewerIngestFencesLateReadResult() async {
        let source = FakeSource()
        var continuation: CheckedContinuation<RemoteComputerAgentBoxSnapshot?, Error>?
        source.readHandler = { _ in
            try await withCheckedThrowingContinuation { continuation = $0 }
        }
        let store = RemoteComputerStatusStore(
            source: source,
            deadline: .init(timeoutNanoseconds: 1_000_000)
        )
        let release = store.retain("agent-a")
        store.connect()
        await yieldUntil { store.readState(for: "agent-a") == .error }

        store.ingest(status: snapshot("agent-a", host: "new.example"))
        continuation?.resume(returning: snapshot("agent-a", host: "stale.example"))
        for _ in 0..<20 { await Task.yield() }

        XCTAssertEqual(store.status(for: "agent-a")?.vncURL?.host, "new.example")
        release()
        store.dispose()
    }

    func testEnsureDedupesAndReconnectRehydratesDemandedScope() async {
        let source = FakeSource()
        source.readHandler = { [unowned self] agentID in self.snapshot(agentID) }
        var ensureContinuation: CheckedContinuation<RemoteComputerAgentBoxSnapshot?, Error>?
        source.ensureHandler = { _ in
            try await withCheckedThrowingContinuation { ensureContinuation = $0 }
        }
        let store = RemoteComputerStatusStore(source: source)
        let release = store.retain("agent-a")
        store.connect()
        await yieldUntil { source.readCalls.count == 1 }

        store.ensure("agent-a")
        store.ensure("agent-a")
        await yieldUntil { source.ensureCalls.count == 1 }
        XCTAssertTrue(store.hasDemanded("agent-a"))
        XCTAssertEqual(source.ensureCalls, ["agent-a"])

        ensureContinuation?.resume(returning: snapshot("agent-a"))
        await yieldUntil { !store.snapshot(for: "agent-a").isEnsureStarting }

        source.ensureHandler = { [unowned self] agentID in self.snapshot(agentID) }
        store.noteReconnect()
        await yieldUntil {
            source.readCalls.count >= 2 && source.ensureCalls.count >= 2
        }

        XCTAssertGreaterThanOrEqual(source.readCalls.count, 2)
        XCTAssertGreaterThanOrEqual(source.ensureCalls.count, 2)
        release()
        store.dispose()
    }

    func testResetPreservesWatchedRecordButClearsStateAndDemand() async {
        let source = FakeSource()
        source.readHandler = { [unowned self] agentID in self.snapshot(agentID) }
        source.ensureHandler = { [unowned self] agentID in self.snapshot(agentID) }
        let store = RemoteComputerStatusStore(source: source)
        let release = store.retain("agent-a")
        store.connect()
        store.ensure("agent-a")
        await yieldUntil {
            store.status(for: "agent-a") != nil && store.hasDemanded("agent-a")
        }
        store.ingestDiskPressure("critical")
        store.ingestVNCUserPresence(true)

        store.reset()

        XCTAssertNil(store.status(for: "agent-a"))
        XCTAssertEqual(store.readState(for: "agent-a"), .unknown)
        XCTAssertFalse(store.hasDemanded("agent-a"))
        XCTAssertNil(store.diskPressure)
        XCTAssertFalse(store.vncUserPresent)
        release()
        store.dispose()
    }

    func testCacheEvictsOldestUnwatchedRecordAtLimit() {
        let source = FakeSource()
        let store = RemoteComputerStatusStore(source: source)

        for index in 0...RemoteComputerStatusStore.recordLimit {
            store.recordReadFailure("agent-\(index)")
        }

        XCTAssertEqual(store.readState(for: "agent-0"), .unknown)
        XCTAssertEqual(
            store.readState(for: "agent-\(RemoteComputerStatusStore.recordLimit)"),
            .error
        )
        store.dispose()
    }

    func testIngressAndHandBackStayOnSingleStoreOwner() async {
        let source = FakeSource()
        let store = RemoteComputerStatusStore(source: source)
        var actions: [String] = []
        let unsubscribe = store.subscribeComputerActions { value in
            if let value = value as? String {
                actions.append(value)
            }
        }

        store.ingestDiskPressure("warning")
        store.ingestVNCUserPresence(true)
        store.ingestComputerAction("cursor")
        await store.handBack("agent-a", trigger: "dismissed")

        XCTAssertEqual(store.diskPressure, "warning")
        XCTAssertTrue(store.vncUserPresent)
        XCTAssertEqual(actions, ["cursor"])
        XCTAssertEqual(source.handBackCalls.count, 1)
        XCTAssertEqual(source.handBackCalls.first?.0, "agent-a")
        XCTAssertEqual(source.handBackCalls.first?.1, "dismissed")

        unsubscribe()
        store.dispose()
    }

    func testDisposeFencesLateResultAndActionDelivery() async {
        let source = FakeSource()
        var continuation: CheckedContinuation<RemoteComputerAgentBoxSnapshot?, Error>?
        source.readHandler = { _ in
            try await withCheckedThrowingContinuation { continuation = $0 }
        }
        let store = RemoteComputerStatusStore(
            source: source,
            deadline: .init(timeoutNanoseconds: 1_000_000)
        )
        var actions = 0
        _ = store.subscribeComputerActions { _ in actions += 1 }
        let release = store.retain("agent-a")
        store.connect()
        await yieldUntil { store.readState(for: "agent-a") == .error }

        store.dispose()
        store.ingestComputerAction("ignored")
        continuation?.resume(returning: snapshot("agent-a", host: "late.example"))
        for _ in 0..<20 { await Task.yield() }

        XCTAssertEqual(actions, 0)
        XCTAssertNil(store.status(for: "agent-a"))
        release()
    }
}
