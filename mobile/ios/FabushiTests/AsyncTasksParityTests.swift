import XCTest
@testable import Fabushi

final class AsyncTasksParityTests: XCTestCase {
    func testTaskDecoderAcceptsCanonicalDesktopProjection() {
        let task = MobileAsyncTask(json: [
            "kind": "subagent",
            "id": "task-1",
            "label": "Research",
            "status": "running",
            "startedAtMs": 1_725_235_200_000.0,
            "detail": "rearmed after a Host restart",
            "subagentType": "task",
        ])

        XCTAssertEqual(task?.kind, "subagent")
        XCTAssertEqual(task?.id, "task-1")
        XCTAssertEqual(task?.label, "Research")
        XCTAssertEqual(task?.detail, "rearmed after a Host restart")
        XCTAssertEqual(task?.subagentType, "task")
    }

    func testTaskDecoderRejectsMalformedOrNonRunningRows() {
        XCTAssertNil(MobileAsyncTask(json: [
            "kind": "shell",
            "id": "shell-1",
            "label": "Build",
            "status": "completed",
            "startedAtMs": 10.0,
        ]))
        XCTAssertNil(MobileAsyncTask(json: [
            "kind": "unknown",
            "id": "task-2",
            "label": "Unknown",
            "status": "running",
            "startedAtMs": 10.0,
        ]))
        XCTAssertNil(MobileAsyncTask(json: [
            "kind": "cloud-agent",
            "id": "",
            "label": "Cloud",
            "status": "running",
            "startedAtMs": 10.0,
        ]))
    }
}
