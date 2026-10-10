import XCTest
@testable import Fabushi

@MainActor
private final class ArchitectureTestPort: CoordinatorPort {
    var frames: [CoordinatorFrame] = []
    var closed = false

    func post(_ frame: CoordinatorFrame) {
        frames.append(frame)
    }

    func close() {
        closed = true
    }
}

final class IOSCoordinatorArchitectureTests: XCTestCase {
    func testResyncUnionIsStableAndDeduplicated() {
        XCTAssertEqual(
            CoordinatorResync.unionDisabledTools(
                ["one": ["a", "b"]],
                ["one": ["b", "c"], "two": ["d"]]
            ),
            ["one": ["a", "b", "c"], "two": ["d"]]
        )
    }

    func testPortAccessGuardRequiresActiveTrustedScene() {
        XCTAssertTrue(CoordinatorPortAccessGuard.isTrusted(.init(
            requestedSceneID: "scene-1",
            trustedSceneID: "scene-1",
            isForeground: true
        )))
        XCTAssertFalse(CoordinatorPortAccessGuard.isTrusted(.init(
            requestedSceneID: "scene-1",
            trustedSceneID: "scene-2",
            isForeground: true
        )))
        XCTAssertThrowsError(try CoordinatorPortAccessGuard.requireTrusted(.init(
            requestedSceneID: "scene-1",
            trustedSceneID: "scene-1",
            isForeground: false
        )))
    }

    @MainActor
    func testRendererServerReplaysOnlyAfterServingHandshake() {
        let port = ArchitectureTestPort()
        var servingCount = 0
        let server = RendererPortServer(
            port: port,
            dispatch: { _, _ in .ok(.null) },
            onServing: { servingCount += 1 }
        )

        XCTAssertEqual(servingCount, 0)
        server.receive(.hello(protocolVersion: CoordinatorProtocol.version))
        XCTAssertEqual(server.phase, .serving)
        XCTAssertEqual(servingCount, 1)
        XCTAssertEqual(port.frames.first, .ready(protocolVersion: CoordinatorProtocol.version))
    }

    @MainActor
    func testControlServerHandshakeAndExecutorReply() async {
        let port = ArchitectureTestPort()
        let executors = CoordinatorControlExecutors()
        executors.register("ping") { args in
            .object(["ok": .bool(true), "echo": args])
        }
        let server = CoordinatorControlServer(
            port: port,
            executors: executors,
            onEvent: { _, _ in },
            onProblem: { _ in }
        )

        server.receive(.hello(protocolVersion: CoordinatorProtocol.version))
        XCTAssertEqual(port.frames.first, .ready(protocolVersion: CoordinatorProtocol.version))

        server.receive(.request(requestId: "r-1", method: "ping", args: .string("hello")))
        await Task.yield()

        XCTAssertTrue(port.frames.contains(.reply(
            requestId: "r-1",
            outcome: .ok(.object(["ok": .bool(true), "echo": .string("hello")]))
        )))
    }

    func testWebSurfacePolicyDoesNotExposeLoopbackDesktopSurface() throws {
        XCTAssertEqual(
            PreloadWebSurfacePolicy.decide(try XCTUnwrap(URL(string: "fabushi://auth/callback"))),
            .appCallback(try XCTUnwrap(URL(string: "fabushi://auth/callback")))
        )
        guard case .blocked(let reason) = PreloadWebSurfacePolicy.decide(
            try XCTUnwrap(URL(string: "http://127.0.0.1:8765/devtools"))
        ) else {
            return XCTFail("loopback surface should be blocked")
        }
        XCTAssertTrue(reason.contains("loopback"))
    }

    func testCoordinatorRelaunchBackoffIsBounded() {
        XCTAssertEqual(CoordinatorTelemetry.relaunchDelayMilliseconds(attempt: 0), 250)
        XCTAssertEqual(CoordinatorTelemetry.relaunchDelayMilliseconds(attempt: 1), 500)
        XCTAssertEqual(CoordinatorTelemetry.relaunchDelayMilliseconds(attempt: 20), 10_000)
    }

    @MainActor
    func testIOSConnectivityTracksRecentResume() {
        var now = Date(timeIntervalSince1970: 100)
        let connectivity = IOSConnectivity(now: { now }, isOnline: { true })
        XCTAssertFalse(connectivity.recentResume())
        connectivity.noteResume()
        XCTAssertTrue(connectivity.recentResume())
        now = now.addingTimeInterval(61)
        XCTAssertFalse(connectivity.recentResume())
        XCTAssertEqual(connectivity.telemetryStamps()["client_online"], "true")
    }
}
