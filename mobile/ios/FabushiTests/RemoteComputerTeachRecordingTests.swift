import XCTest
@testable import Fabushi

@MainActor
final class RemoteComputerTeachRecordingTests: XCTestCase {
    private final class FakeSource: RemoteComputerTeachRecordingSourcing {
        var statusValue = IDLE_TEACH_RECORDING_STATUS
        var startValue = TeachRecordingStatus(
            state: .recording,
            agentId: "agent-a",
            startedAtMs: 1_000,
            maxDurationMs: SAND_TEACH_MAX_DURATION_MS,
            capturePath: "/tmp/fabushi-teach/demo.mp4"
        )
        var stopValue = IDLE_TEACH_RECORDING_STATUS
        var statusCalls = 0
        var starts: [(String, String)] = []
        var stops: [(String, Bool)] = []

        func status() async throws -> TeachRecordingStatus {
            statusCalls += 1
            return statusValue
        }

        func start(agentID: String, entryPoint: String) async throws -> TeachRecordingStatus {
            starts.append((agentID, entryPoint))
            return startValue
        }

        func stop(agentID: String, save: Bool) async throws -> TeachRecordingStatus {
            stops.append((agentID, save))
            return stopValue
        }
    }

    private final class FakeCapture: RemoteComputerTeachCapturing {
        enum Failure: Error { case start }
        var isRecording = false
        var failStart = false
        var starts: [String] = []
        var stops: [Bool] = []

        func start(path: String) async throws {
            if failStart { throw Failure.start }
            starts.append(path)
            isRecording = true
        }

        func stop(save: Bool) async {
            stops.append(save)
            isRecording = false
        }
    }

    func testSourceProjectsHostTeachStatusAndNativeCaptureTarget() throws {
        let projected = try IOSRemoteComputerTeachRecordingSource.projectStatus([
            "type": "teach.changed",
            "status": [
                "state": "recording",
                "agentId": "agent-a",
                "startedAtMs": 1_234,
                "maxDurationMs": 600_000,
                "capturePath": "/sandbox/teach/demo.mp4",
            ],
        ])
        XCTAssertEqual(projected.state, .recording)
        XCTAssertEqual(projected.agentId, "agent-a")
        XCTAssertEqual(projected.startedAtMs, 1_234)
        XCTAssertEqual(projected.maxDurationMs, 600_000)
        XCTAssertEqual(projected.capturePath, "/sandbox/teach/demo.mp4")

        let idle = try IOSRemoteComputerTeachRecordingSource.projectStatus([
            "type": "teach.changed",
            "status": [
                "state": "idle",
                "maxDurationMs": 600_000,
            ],
        ])
        XCTAssertEqual(idle, IDLE_TEACH_RECORDING_STATUS)
    }

    func testOwnerStartsNativeCaptureThenStopsThroughSameHostOwner() async {
        let source = FakeSource()
        let capture = FakeCapture()
        var now = 900
        let owner = RemoteComputerTeachRecordingOwner(
            source: source,
            capture: capture,
            now: { now }
        )

        owner.arm(agentID: "agent-a", entryPoint: "fullscreen_title_bar")
        await owner.start(agentID: "agent-a", entryPoint: "fullscreen_title_bar")

        XCTAssertEqual(source.starts.count, 1)
        XCTAssertEqual(source.starts.first?.0, "agent-a")
        XCTAssertEqual(source.starts.first?.1, "fullscreen_title_bar")
        XCTAssertEqual(capture.starts, ["/tmp/fabushi-teach/demo.mp4"])
        XCTAssertEqual(owner.status.state, .recording)
        XCTAssertNil(owner.armed)

        now = 2_500
        XCTAssertEqual(owner.elapsedMilliseconds, 1_500)

        await owner.stop(save: true)
        XCTAssertEqual(capture.stops.last, true)
        XCTAssertEqual(source.stops.count, 1)
        XCTAssertEqual(source.stops.first?.0, "agent-a")
        XCTAssertEqual(source.stops.first?.1, true)
        XCTAssertEqual(owner.status.state, .idle)
    }

    func testCaptureStartupFailureDiscardsHostRecordingAndRollsBack() async {
        let source = FakeSource()
        let capture = FakeCapture()
        capture.failStart = true
        let owner = RemoteComputerTeachRecordingOwner(
            source: source,
            capture: capture
        )

        await owner.start(agentID: "agent-a", entryPoint: "screen_hover")

        XCTAssertEqual(owner.status.state, .idle)
        XCTAssertEqual(source.stops.count, 1)
        XCTAssertEqual(source.stops.first?.0, "agent-a")
        XCTAssertEqual(source.stops.first?.1, false)
        XCTAssertNotNil(owner.errorMessage)
        XCTAssertFalse(capture.isRecording)
    }

    func testReconnectHealsRecordingWithoutCreatingSecondStateOwner() async {
        let source = FakeSource()
        source.statusValue = .init(
            state: .recording,
            agentId: "agent-a",
            startedAtMs: 2_000,
            maxDurationMs: SAND_TEACH_MAX_DURATION_MS,
            capturePath: "/tmp/recovered-teach/demo.mp4"
        )
        let capture = FakeCapture()
        let owner = RemoteComputerTeachRecordingOwner(
            source: source,
            capture: capture
        )

        await owner.connect()
        XCTAssertEqual(source.statusCalls, 1)
        XCTAssertEqual(owner.status.state, .recording)
        XCTAssertEqual(capture.starts, ["/tmp/recovered-teach/demo.mp4"])

        source.statusValue = IDLE_TEACH_RECORDING_STATUS
        await owner.noteReconnect()
        XCTAssertEqual(source.statusCalls, 2)
        XCTAssertEqual(owner.status.state, .idle)
        XCTAssertEqual(capture.stops.last, false)
    }

    func testResetDiscardsActiveTeachSessionAndClearsArm() async {
        let source = FakeSource()
        let capture = FakeCapture()
        let owner = RemoteComputerTeachRecordingOwner(
            source: source,
            capture: capture
        )

        owner.arm(agentID: "agent-a", entryPoint: "composer_menu")
        await owner.start(agentID: "agent-a", entryPoint: "composer_menu")
        XCTAssertEqual(owner.status.state, .recording)

        owner.reset()
        for _ in 0..<20 where source.stops.isEmpty {
            await Task.yield()
        }

        XCTAssertEqual(owner.status.state, .idle)
        XCTAssertNil(owner.armed)
        XCTAssertEqual(source.stops.last?.0, "agent-a")
        XCTAssertEqual(source.stops.last?.1, false)
    }

    func testAgentBoxProjectionPreservesTeachHandoffGate() throws {
        let snapshot = try IOSRemoteComputerAgentBoxSource.projectStatus(
            [
                "agentId": "agent-a",
                "state": "running",
                "vncUrl": "https://agent-box.example/vnc.html",
                "handoff": [
                    "requestId": "handoff-1",
                    "instruction": "Complete sign in",
                ],
            ],
            expectedAgentID: "agent-a"
        )
        XCTAssertTrue(snapshot.hasHandoff)

        let clear = try IOSRemoteComputerAgentBoxSource.projectStatus(
            [
                "agentId": "agent-a",
                "state": "running",
                "vncUrl": "https://agent-box.example/vnc.html",
            ],
            expectedAgentID: "agent-a"
        )
        XCTAssertFalse(clear.hasHandoff)
    }
}
