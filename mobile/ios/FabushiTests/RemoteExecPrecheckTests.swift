import XCTest
@testable import Fabushi

final class RemoteExecPrecheckTests: XCTestCase {
    private enum UnexpectedNetwork: Error { case called }

    private func client() -> IOSRemoteExecDaemonClient {
        IOSRemoteExecDaemonClient(
            descriptor: IOSCursorSandBoxConnection(
                networkToken: "network",
                execDaemonAuthToken: "auth",
                execDaemonURL: "https://box.example.test",
                vncURL: "https://box.example.test/vnc",
                gatewayURL: "https://box.example.test/gateway",
                gatewayToken: "gateway",
                forkVNCBaseURL: "https://box.example.test/fork"
            ),
            requestExecutor: { _ in
                throw UnexpectedNetwork.called
            }
        )
    }

    func testBoxReadRejectsTraversalBeforeNetworkDispatch() async throws {
        let params = try CoordinatorPayload.fromFoundation([
            "case": "readArgs",
            "args": [
                "path": "/workspace/../private/secret.txt",
                "toolCallId": "read-1",
            ],
        ])

        do {
            _ = try await client().dispatch(method: "local-exec.box.exec", params: params)
            XCTFail("expected workspace path rejection")
        } catch let error as IOSRemoteExecDaemonError {
            guard case .invalidRequest(let detail) = error else {
                return XCTFail("unexpected error: \(error)")
            }
            XCTAssertTrue(detail.contains("/workspace"))
        } catch {
            XCTFail("unexpected error: \(error)")
        }
    }

    func testProcessRejectsOutOfScopeWorkingDirectoryBeforeNetworkDispatch() async throws {
        let params = try CoordinatorPayload.fromFoundation([
            "program": "git",
            "args": ["diff", "--"],
            "workingDirectory": "/private/tmp",
        ])

        do {
            _ = try await client().dispatch(method: "local-exec.process.exec", params: params)
            XCTFail("expected working directory rejection")
        } catch let error as IOSRemoteExecDaemonError {
            guard case .invalidRequest(let detail) = error else {
                return XCTFail("unexpected error: \(error)")
            }
            XCTAssertTrue(detail.contains("/workspace"))
        } catch {
            XCTFail("unexpected error: \(error)")
        }
    }
}
