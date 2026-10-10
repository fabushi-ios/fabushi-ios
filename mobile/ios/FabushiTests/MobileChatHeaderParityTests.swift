import XCTest
@testable import Fabushi

final class MobileChatHeaderParityTests: XCTestCase {
    func testHeaderStatusUsesCanonicalRunningState() {
        let idle = MobileBotSummary(
            id: "agent-idle",
            name: "Idle",
            description: "idle",
            isRunning: false
        )
        let running = MobileBotSummary(
            id: "agent-running",
            name: "Running",
            description: "running",
            isRunning: true
        )

        XCTAssertNil(mobileConversationHeaderStatus(idle))
        XCTAssertEqual(mobileConversationHeaderStatus(running), "Working")
    }

    func testHeaderStatusDoesNotDependOnPresentationAvatarState() {
        let runningWithIdleAvatar = MobileBotSummary(
            id: "agent-running-idle-avatar",
            name: "Running",
            description: "running",
            isRunning: true,
            avatarState: .idle
        )
        let stoppedWithWorkingAvatar = MobileBotSummary(
            id: "agent-stopped-working-avatar",
            name: "Stopped",
            description: "stopped",
            isRunning: false,
            avatarState: .working
        )

        XCTAssertEqual(mobileConversationHeaderStatus(runningWithIdleAvatar), "Working")
        XCTAssertNil(mobileConversationHeaderStatus(stoppedWithWorkingAvatar))
    }
}
