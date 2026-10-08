import AVFoundation
import XCTest
@testable import Fabushi

final class HumanCallMediaPortTests: XCTestCase {
    @MainActor
    func testPermissionMappingMatchesDesktopCallMediaContract() {
        XCTAssertEqual(HumanCallMediaPort.permission(for: .authorized), .granted)
        XCTAssertEqual(HumanCallMediaPort.permission(for: .denied), .denied)
        XCTAssertEqual(HumanCallMediaPort.permission(for: .restricted), .denied)
        XCTAssertEqual(HumanCallMediaPort.permission(for: .notDetermined), .prompt)
    }

    func testMediaPermissionWireValuesStayStable() {
        XCTAssertEqual(HumanCallMediaPermission.granted.rawValue, "granted")
        XCTAssertEqual(HumanCallMediaPermission.denied.rawValue, "denied")
        XCTAssertEqual(HumanCallMediaPermission.prompt.rawValue, "prompt")
        XCTAssertEqual(HumanCallMediaPermission.notRequested.rawValue, "not-requested")
    }
}

extension HumanCallMediaPortTests {
    func testHumanCallSessionProjectionKeepsLifecycleIdentityAndActions() {
        let record = HumanCallSessionRecord(raw: [
            "id": "call-1",
            "scopeId": "conversation-1",
            "creatorId": "alice",
            "state": "ringing",
            "generation": 3,
            "participantIds": ["alice", "bob"],
            "mediaCapabilities": [
                "microphone": "granted",
                "camera": "denied",
            ],
            "updatedAtMs": 1234,
        ])

        XCTAssertEqual(record?.id, "call-1")
        XCTAssertEqual(record?.scopeId, "conversation-1")
        XCTAssertEqual(record?.generation, 3)
        XCTAssertEqual(record?.participantIds, ["alice", "bob"])
        XCTAssertEqual(record?.mediaCapabilities["microphone"], "granted")
        XCTAssertEqual(record?.stateLabel, "响铃中")
        XCTAssertEqual(record?.canAccept, true)
        XCTAssertEqual(record?.canDecline, true)
        XCTAssertEqual(record?.canHangUp, true)
        XCTAssertEqual(record?.isTerminal, false)
    }

    func testHumanCallSessionProjectionFailsClosedAndTerminalStateDisablesActions() {
        XCTAssertNil(HumanCallSessionRecord(raw: [
            "scopeId": "conversation-1",
            "creatorId": "alice",
            "state": "ringing",
        ]))

        let ended = HumanCallSessionRecord(raw: [
            "id": "call-2",
            "scopeId": "conversation-1",
            "creatorId": "alice",
            "state": "ended",
            "generation": 4,
            "participantIds": ["alice", "bob"],
            "terminalReason": "hangup",
            "updatedAtMs": 5678,
        ])

        XCTAssertEqual(ended?.isTerminal, true)
        XCTAssertEqual(ended?.canAccept, false)
        XCTAssertEqual(ended?.canDecline, false)
        XCTAssertEqual(ended?.canHangUp, false)
        XCTAssertEqual(ended?.terminalReason, "hangup")
    }
}
