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
            let path = await recorder.value()
            XCTAssertEqual(
                path,
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
    func testHistoricalSubagentTabsMergeRunningOverlayWithoutDroppingCompletedHistory() throws {
        let historical = [
            MobileBotSubagent(
                subagentId: "sub-done",
                subagentType: "task",
                title: "Completed",
                status: "done"
            ),
            MobileBotSubagent(
                subagentId: "sub-running",
                subagentType: "task",
                title: "Old running title",
                status: "running"
            ),
            MobileBotSubagent(
                subagentId: "sub-error",
                subagentType: "ci",
                title: "Failed",
                status: "error"
            ),
        ]
        let running = [
            try XCTUnwrap(MobileAsyncTask(json: [
                "kind": "subagent",
                "id": "sub-running",
                "label": "Live running title",
                "status": "running",
                "startedAtMs": 10.0,
                "subagentType": "research",
            ])),
            try XCTUnwrap(MobileAsyncTask(json: [
                "kind": "subagent",
                "id": "sub-new",
                "label": "New live task",
                "status": "running",
                "startedAtMs": 11.0,
                "subagentType": "task",
            ])),
        ]

        let merged = mergeMobileOutlineSubagents(
            historical: historical,
            running: running
        )

        XCTAssertEqual(
            merged.map(\.subagentId),
            ["sub-done", "sub-running", "sub-error", "sub-new"]
        )
        XCTAssertEqual(merged[0].status, "done")
        XCTAssertEqual(merged[1].status, "running")
        XCTAssertEqual(merged[1].subagentType, "research")
        XCTAssertEqual(merged[1].title, "Live running title")
        XCTAssertEqual(merged[2].status, "error")
        XCTAssertEqual(merged[3].status, "running")
    }

    func testAsyncTaskMetadataUsesVisibleKindAndDetailInsteadOfLiteralPlaceholders() throws {
        let task = try XCTUnwrap(MobileAsyncTask(json: [
            "kind": "shell",
            "id": "shell-1",
            "label": "Build",
            "status": "running",
            "startedAtMs": 1_000.0,
            "detail": "cargo test",
        ]))
        XCTAssertEqual(mobileAsyncTaskMetadata(task), "Shell · cargo test")
    }

    func testAsyncTaskRelativeTimeMatchesDesktopAgeBuckets() {
        let now = Date(timeIntervalSince1970: 1_000_000)
        XCTAssertEqual(mobileAsyncTaskRelativeTime(timestampMs: 999_990_000, now: now), "now")
        XCTAssertEqual(mobileAsyncTaskRelativeTime(timestampMs: 999_700_000, now: now), "5m ago")
        XCTAssertEqual(mobileAsyncTaskRelativeTime(timestampMs: 992_800_000, now: now), "2h ago")
        XCTAssertEqual(mobileAsyncTaskRelativeTime(timestampMs: 827_200_000, now: now), "2d ago")
        XCTAssertEqual(mobileAsyncTaskRelativeTime(timestampMs: 0, now: now), "")
    }

    func testAsyncTaskRequestScopeRejectsAgentReconnectAndGenerationReplacement() {
        let scope = MobileAsyncTasksRequestScope(
            agentId: "agent-a",
            reconnectGeneration: 4,
            generation: 9
        )
        XCTAssertTrue(
            scope.accepts(
                agentId: "agent-a",
                reconnectGeneration: 4,
                generation: 9
            )
        )
        XCTAssertFalse(
            scope.accepts(
                agentId: "agent-b",
                reconnectGeneration: 4,
                generation: 9
            )
        )
        XCTAssertFalse(
            scope.accepts(
                agentId: "agent-a",
                reconnectGeneration: 5,
                generation: 9
            )
        )
        XCTAssertFalse(
            scope.accepts(
                agentId: "agent-a",
                reconnectGeneration: 4,
                generation: 10
            )
        )
    }

}
