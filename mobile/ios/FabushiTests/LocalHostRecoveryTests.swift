import XCTest
@testable import Fabushi

@MainActor
private final class HostRecoveryStub: MahayanaHostRequesting {
    let result: Result<Any, Error>

    init(result: Result<Any, Error>) {
        self.result = result
    }

    func request(
        method: String,
        params: [String: Any]
    ) async throws -> MahayanaHostJSONResult {
        switch result {
        case .success(let value):
            return MahayanaHostJSONResult(value: value)
        case .failure(let error):
            throw error
        }
    }
}

final class LocalHostRecoveryTests: XCTestCase {
    func testCanonicalSendMessageCompletionProjectsAsSettledVisibleAssistantMessage() {
        let operationId = "operation-send-message"

        XCTAssertTrue(isMobileBotVisibleAssistantCompletion([
            "type": "chat.message",
            "operationId": operationId,
            "role": "assistant",
            "text": "final delivered result",
        ], operationId: operationId))

        XCTAssertTrue(isMobileBotVisibleAssistantCompletion([
            "type": "chat.message",
            "operationId": operationId,
            "role": "assistant",
            "text": "",
            "attachment": ["url": "https://example.test/result.png"],
        ], operationId: operationId))

        XCTAssertFalse(isMobileBotVisibleAssistantCompletion([
            "type": "chat.message",
            "operationId": operationId,
            "role": "user",
            "text": "not an assistant completion",
        ], operationId: operationId))
        XCTAssertFalse(isMobileBotVisibleAssistantCompletion([
            "type": "chat.delta",
            "operationId": operationId,
            "role": "assistant",
            "text": "still streaming",
        ], operationId: operationId))
        XCTAssertFalse(isMobileBotVisibleAssistantCompletion([
            "type": "chat.message",
            "operationId": "newer-operation",
            "role": "assistant",
            "text": "stale completion",
        ], operationId: operationId))
        XCTAssertFalse(isMobileBotVisibleAssistantCompletion([
            "type": "chat.message",
            "operationId": operationId,
            "role": "assistant",
            "text": "",
        ], operationId: operationId))
    }

    @MainActor
    func testRecoverableHostFailureSettlesRequestThenServesNextGeneration() async throws {
        let failed = HostRecoveryStub(
            result: .failure(MahayanaHostRuntime.HostError.invalidResponse)
        )
        let recovered = HostRecoveryStub(result: .success("recovered"))
        var factoryCalls = 0
        let supervisor = MahayanaLocalHostSupervisor(
            host: failed,
            factory: {
                factoryCalls += 1
                return recovered
            }
        )
        let coordinator = MahayanaCoordinator(hostSupervisor: supervisor)

        do {
            _ = try await coordinator.request(method: "first")
            XCTFail("failed host request must settle as an error and must not replay")
        } catch {
            XCTAssertTrue(error.localizedDescription.contains("无效响应"))
        }

        XCTAssertEqual(factoryCalls, 1)
        XCTAssertEqual(coordinator.hostGeneration, 2)

        let next = try await coordinator.request(method: "second")
        XCTAssertEqual(next.value as? String, "recovered")
        XCTAssertEqual(factoryCalls, 1)
    }

    @MainActor
    func testStaleHostFailureCannotReplaceNewerGeneration() throws {
        let first = HostRecoveryStub(result: .success("first"))
        let second = HostRecoveryStub(result: .success("second"))
        var factoryCalls = 0
        let supervisor = MahayanaLocalHostSupervisor(
            host: first,
            factory: {
                factoryCalls += 1
                return second
            }
        )
        let observed = supervisor.generation

        XCTAssertTrue(try supervisor.recoverAfterFailure(observedGeneration: observed))
        XCTAssertFalse(try supervisor.recoverAfterFailure(observedGeneration: observed))
        XCTAssertEqual(supervisor.generation, 2)
        XCTAssertEqual(factoryCalls, 1)
    }

    @MainActor
    func testBusinessRequestFailureDoesNotRestartHost() async {
        let failed = HostRecoveryStub(
            result: .failure(MahayanaHostRuntime.HostError.requestFailed("business-rule"))
        )
        var factoryCalls = 0
        let supervisor = MahayanaLocalHostSupervisor(
            host: failed,
            factory: {
                factoryCalls += 1
                return HostRecoveryStub(result: .success("unexpected"))
            }
        )
        let coordinator = MahayanaCoordinator(hostSupervisor: supervisor)

        do {
            _ = try await coordinator.request(method: "write")
            XCTFail("business error should surface")
        } catch {
            XCTAssertTrue(error.localizedDescription.contains("business-rule"))
        }

        XCTAssertEqual(coordinator.hostGeneration, 1)
        XCTAssertEqual(factoryCalls, 0)
    }
}


@MainActor
private final class AwaitTurnHostStub: MahayanaHostRequesting {
    private(set) var calls: [(String, [String: Any])] = []
    var terminalResponses: [[String: Any]]

    init(terminalResponses: [[String: Any]]) {
        self.terminalResponses = terminalResponses
    }

    func request(
        method: String,
        params: [String: Any]
    ) async throws -> MahayanaHostJSONResult {
        calls.append((method, params))
        switch method {
        case "feature.execute":
            return MahayanaHostJSONResult(value: [
                "requestId": "send-1",
                "operationId": "operation-1",
            ])
        case "feature.awaitOperation":
            guard !terminalResponses.isEmpty else {
                return MahayanaHostJSONResult(value: ["status": "pending"])
            }
            return MahayanaHostJSONResult(value: terminalResponses.removeFirst())
        case "feature.awaitOperation.cancel":
            return MahayanaHostJSONResult(value: NSNull())
        default:
            return MahayanaHostJSONResult(value: [:])
        }
    }
}

extension LocalHostRecoveryTests {
    @MainActor
    func testAwaitTurnHoldsCoordinatorRequestUntilTerminalCompletion() async throws {
        let host = AwaitTurnHostStub(terminalResponses: [
            ["status": "pending"],
            ["status": "completed"],
        ])
        let supervisor = MahayanaLocalHostSupervisor(host: host, factory: { host })
        let coordinator = MahayanaCoordinator(hostSupervisor: supervisor)

        let result = try await coordinator.request(
            method: "feature.execute",
            params: [
                "awaitTurn": true,
                "source": "workflow-reference",
                "command": [
                    "type": "chat.send",
                    "requestId": "send-1",
                    "text": "run the workflow",
                ],
            ]
        )

        XCTAssertEqual((result.value as? [String: Any])?["operationId"] as? String, "operation-1")
        XCTAssertEqual(
            host.calls.map { $0.0 },
            ["feature.execute", "feature.awaitOperation", "feature.awaitOperation"]
        )
    }

    @MainActor
    func testWorkflowRunTransportStampsAwaitTurnAndSourceBeforeHostDispatch() async throws {
        let host = AwaitTurnHostStub(terminalResponses: [["status": "completed"]])
        let supervisor = MahayanaLocalHostSupervisor(host: host, factory: { host })
        let coordinator = MahayanaCoordinator(hostSupervisor: supervisor)

        let outcome = await coordinator.dispatchTransport(
            method: "feature.execute",
            args: .object([
                "command": .object([
                    "type": .string("workflow.run"),
                    "requestId": .string("workflow-run-1"),
                    "agentId": .string("mahayana-assistant"),
                    "id": .string("release"),
                ]),
            ])
        )

        guard case .ok = outcome else {
            return XCTFail("workflow.run transport should settle successfully")
        }
        XCTAssertEqual(host.calls.map { $0.0 }, ["feature.execute", "feature.awaitOperation"])
        XCTAssertEqual(host.calls.first?.1["awaitTurn"] as? Bool, true)
        XCTAssertEqual(host.calls.first?.1["source"] as? String, "workflow-reference")
    }

    @MainActor
    func testAwaitTurnFalseReturnsAtAcceptanceWithoutTerminalPolling() async throws {
        let host = AwaitTurnHostStub(terminalResponses: [["status": "completed"]])
        let supervisor = MahayanaLocalHostSupervisor(host: host, factory: { host })
        let coordinator = MahayanaCoordinator(hostSupervisor: supervisor)

        _ = try await coordinator.request(
            method: "feature.execute",
            params: [
                "awaitTurn": false,
                "command": [
                    "type": "chat.send",
                    "requestId": "send-2",
                    "text": "ordinary send",
                ],
            ]
        )

        XCTAssertEqual(host.calls.map { $0.0 }, ["feature.execute"])
    }

    @MainActor
    func testAwaitTurnPropagatesTerminalFailure() async {
        let host = AwaitTurnHostStub(terminalResponses: [[
            "status": "failed",
            "code": "provider_error",
            "message": "provider unavailable",
        ]])
        let supervisor = MahayanaLocalHostSupervisor(host: host, factory: { host })
        let coordinator = MahayanaCoordinator(hostSupervisor: supervisor)

        do {
            _ = try await coordinator.request(
                method: "feature.execute",
                params: [
                    "awaitTurn": true,
                    "command": [
                        "type": "chat.send",
                        "requestId": "send-3",
                        "text": "fail this turn",
                    ],
                ]
            )
            XCTFail("awaitTurn must fail with the terminal turn failure")
        } catch {
            XCTAssertTrue(error.localizedDescription.contains("provider_error"))
            XCTAssertTrue(error.localizedDescription.contains("provider unavailable"))
        }
    }
}
