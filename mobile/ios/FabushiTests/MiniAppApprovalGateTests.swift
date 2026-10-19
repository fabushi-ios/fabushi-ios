import XCTest
@testable import Fabushi

@MainActor
final class MiniAppApprovalGateTests: XCTestCase {
    func testCancellationResolvesPendingApprovalWithoutUserAction() async {
        let gate = MiniAppApprovalGate()
        let pending = Task { await gate.wait() }
        await Task.yield()

        pending.cancel()

        let approved = await pending.value
        XCTAssertFalse(approved)
    }

    func testApprovalGateAcceptsOnlyTheFirstDecision() async {
        let gate = MiniAppApprovalGate()
        let pending = Task { await gate.wait() }
        await Task.yield()

        gate.resolve(true)
        gate.resolve(false)

        let approved = await pending.value
        XCTAssertTrue(approved)
    }

    func testCancellationAfterResolutionCannotChangeApprovalDecision() async {
        let gate = MiniAppApprovalGate()
        let pending = Task { await gate.wait() }
        await Task.yield()

        gate.resolve(true)
        pending.cancel()

        let approved = await pending.value
        XCTAssertTrue(approved)
    }
}
