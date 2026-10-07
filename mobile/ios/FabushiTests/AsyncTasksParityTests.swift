import XCTest
@testable import Fabushi

private actor CloudAgentRequestRecorder {
    private(set) var path: String?

    func record(_ value: String?) {
        path = value
    }

    func value() -> String? {
        path
    }
}

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

    func testBackgroundComposerStatusUsesCanonicalServiceRoute() async {
        let recorder = CloudAgentRequestRecorder()
        let client = IOSCursorDashboardClient(
            credentials: .init(
                getAccessToken: { _ in "test-token" },
                getMachineId: { "test-machine" }
            ),
            backendURL: URL(string: "https://api.example.test")!,
            requestExecutor: { request in
                await recorder.record(request.url?.path)
                throw URLError(.cancelled)
            }
        )

        do {
            _ = try await client.getBackgroundComposerInfo(bcId: "bc-1")
            XCTFail("status request should have been intercepted")
        } catch {
            XCTAssertEqual(
                await recorder.value(),
                "/aiserver.v1.BackgroundComposerService/GetBackgroundComposerInfo"
            )
        }
    }

    func testCloudAgentStatusClassificationMatchesDesktopPoller() {
        XCTAssertTrue(IOSCloudAgentComposerInfo(status: 1, summary: nil, permanentError: nil).isActive)
        XCTAssertTrue(IOSCloudAgentComposerInfo(status: 4, summary: nil, permanentError: nil).isActive)
        XCTAssertFalse(IOSCloudAgentComposerInfo(status: 2, summary: nil, permanentError: nil).isActive)
        XCTAssertTrue(IOSCloudAgentComposerInfo(status: 3, summary: nil, permanentError: nil).isError)
        XCTAssertTrue(IOSCloudAgentComposerInfo(status: 5, summary: nil, permanentError: nil).isError)
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
