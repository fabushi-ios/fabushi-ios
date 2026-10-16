import XCTest
@testable import Fabushi

@MainActor
final class HiddenChatsParityTests: XCTestCase {
    private func bot(id: String = "bot-1", hidden: Bool = false) -> MobileBotSummary {
        MobileBotSummary(id: id, name: "Bot", description: "test", hidden: hidden)
    }

    func testOptimisticHideConfirmsWithoutSecondRosterOwner() async throws {
        let controller = MobileHiddenChatsMutationController()
        controller.setScope(accountScopeKey: "acct-a", activeAgentId: nil)
        var roster = [bot()]
        var calls: [(String, Bool)] = []

        try await controller.setAgentHidden(
            agentId: "bot-1",
            isHidden: true,
            readAgent: { id in roster.first(where: { $0.id == id }) },
            onOptimisticChange: { id, hidden in
                if let index = roster.firstIndex(where: { $0.id == id }) {
                    roster[index] = roster[index].replacingHidden(hidden)
                }
            },
            call: { id, hidden in calls.append((id, hidden)) },
            onRollback: { id, _, previous in
                if let index = roster.firstIndex(where: { $0.id == id }) {
                    roster[index] = roster[index].replacingHidden(previous)
                }
            }
        )

        XCTAssertTrue(roster[0].hidden)
        XCTAssertEqual(calls.count, 1)
        XCTAssertFalse(controller.isPending("bot-1"))
        XCTAssertEqual(controller.projectAgents([bot(hidden: false)])[0].hidden, true)
        controller.ingestAgents([bot(hidden: true)])
        XCTAssertEqual(controller.projectAgents([bot(hidden: true)])[0].hidden, true)
    }

    func testNonTransportFailureRollsBackOptimisticState() async {
        let controller = MobileHiddenChatsMutationController()
        controller.setScope(accountScopeKey: "acct-a", activeAgentId: nil)
        var roster = [bot()]

        do {
            try await controller.setAgentHidden(
                agentId: "bot-1",
                isHidden: true,
                readAgent: { id in roster.first(where: { $0.id == id }) },
                onOptimisticChange: { id, hidden in
                    if let index = roster.firstIndex(where: { $0.id == id }) {
                        roster[index] = roster[index].replacingHidden(hidden)
                    }
                },
                call: { _, _ in
                    throw NSError(domain: "test", code: 1)
                },
                onRollback: { id, _, previous in
                    if let index = roster.firstIndex(where: { $0.id == id }) {
                        roster[index] = roster[index].replacingHidden(previous)
                    }
                }
            )
            XCTFail("Expected mutation failure")
        } catch {
            XCTAssertFalse(roster[0].hidden)
            XCTAssertFalse(controller.isPending("bot-1"))
        }
    }

    func testTransportFailureHoldsOptimisticStateAndReconnectRetries() async {
        let controller = MobileHiddenChatsMutationController()
        controller.setScope(accountScopeKey: "acct-a", activeAgentId: "agent-a")
        var roster = [bot()]
        var retries = 0

        do {
            try await controller.setAgentHidden(
                agentId: "bot-1",
                isHidden: true,
                readAgent: { id in roster.first(where: { $0.id == id }) },
                onOptimisticChange: { id, hidden in
                    if let index = roster.firstIndex(where: { $0.id == id }) {
                        roster[index] = roster[index].replacingHidden(hidden)
                    }
                },
                call: { _, _ in
                    throw IOSCoordinatorPortClient.PortError(
                        code: "source/transport-failure",
                        message: "offline"
                    )
                },
                onRollback: { _, _, _ in XCTFail("Transport failure must not roll back") }
            )
            XCTFail("Expected transport failure")
        } catch {
            XCTAssertTrue(roster[0].hidden)
            XCTAssertTrue(controller.isPending("bot-1"))
        }

        controller.noteReconnect(
            call: { _, _ in retries += 1 },
            onRollback: { _, _, _ in XCTFail("Successful retry must not roll back") }
        )
        await Task.yield()
        XCTAssertEqual(retries, 1)
        XCTAssertFalse(controller.isPending("bot-1"))
    }

    func testScopeChangeAndDisposeFenceStaleMutationCompletion() async {
        let controller = MobileHiddenChatsMutationController()
        controller.setScope(accountScopeKey: "acct-a", activeAgentId: "agent-a")
        var roster = [bot()]
        var continuation: CheckedContinuation<Void, Error>?

        let task = Task { @MainActor in
            try await controller.setAgentHidden(
                agentId: "bot-1",
                isHidden: true,
                readAgent: { id in roster.first(where: { $0.id == id }) },
                onOptimisticChange: { id, hidden in
                    if let index = roster.firstIndex(where: { $0.id == id }) {
                        roster[index] = roster[index].replacingHidden(hidden)
                    }
                },
                call: { _, _ in
                    try await withCheckedThrowingContinuation { next in continuation = next }
                },
                onRollback: { _, _, _ in XCTFail("Stale completion must be fenced") }
            )
        }
        await Task.yield()
        XCTAssertTrue(roster[0].hidden)

        controller.setScope(accountScopeKey: "acct-b", activeAgentId: "agent-b")
        continuation?.resume()
        _ = try? await task.value
        XCTAssertFalse(controller.isPending("bot-1"))

        controller.dispose()
        XCTAssertTrue(controller.disposed)
        XCTAssertNil(controller.accountScopeKey)
        XCTAssertNil(controller.activeAgentId)
    }
}
