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
    private(set) var platformResolutions: [[String: Any]] = []
    private(set) var platformRejections: [[String: Any]] = []
    var terminalResponses: [[String: Any]]
    var platformRequests: [[String: Any]]

    init(
        terminalResponses: [[String: Any]],
        platformRequests: [[String: Any]] = []
    ) {
        self.terminalResponses = terminalResponses
        self.platformRequests = platformRequests
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
        case "platformExec.next":
            guard !platformRequests.isEmpty else {
                return MahayanaHostJSONResult(value: NSNull())
            }
            return MahayanaHostJSONResult(value: platformRequests.removeFirst())
        case "platformExec.resolve":
            platformResolutions.append(params)
            return MahayanaHostJSONResult(value: ["settled": true])
        case "platformExec.reject":
            platformRejections.append(params)
            return MahayanaHostJSONResult(value: ["settled": true])
        default:
            return MahayanaHostJSONResult(value: [:])
        }
    }
}

private actor PlatformExecutionProviderStub: IOSRemoteExecutionProviding {
    private(set) var calls: [(String, CoordinatorPayload)] = []
    var result: CoordinatorPayload
    var delayNanoseconds: UInt64

    init(
        result: CoordinatorPayload = .object(["success": .bool(true)]),
        delayNanoseconds: UInt64 = 0
    ) {
        self.result = result
        self.delayNanoseconds = delayNanoseconds
    }

    func dispatch(
        method: String,
        params: CoordinatorPayload
    ) async throws -> CoordinatorPayload {
        calls.append((method, params))
        if delayNanoseconds > 0 {
            try await Task.sleep(nanoseconds: delayNanoseconds)
        }
        return result
    }

    func recordedMethods() -> [String] {
        calls.map { $0.0 }
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
            [
                "feature.execute",
                "feature.awaitOperation",
                "platformExec.next",
                "feature.awaitOperation",
            ]
        )
    }

    @MainActor
    func testAwaitTurnServicesRealPlatformExecThroughProductionExecutor() async throws {
        let host = AwaitTurnHostStub(
            terminalResponses: [
                ["status": "pending"],
                ["status": "completed"],
            ],
            platformRequests: [[
                "requestId": "platform-1",
                "method": "process.exec",
                "params": [
                    "program": "git",
                    "args": ["status", "--short"],
                ],
            ]]
        )
        let provider = PlatformExecutionProviderStub(
            result: .object([
                "success": .bool(true),
                "stdout": .string("clean"),
            ])
        )
        let supervisor = MahayanaLocalHostSupervisor(host: host, factory: { host })
        let coordinator = MahayanaCoordinator(
            hostSupervisor: supervisor,
            remoteExecutionProvider: provider
        )
        coordinator.updateAccountSettingsScope("account-1")

        _ = try await coordinator.request(
            method: "feature.execute",
            params: [
                "awaitTurn": true,
                "command": [
                    "type": "chat.send",
                    "requestId": "send-platform",
                    "text": "inspect the workspace",
                ],
            ]
        )

        XCTAssertEqual(
            await provider.recordedMethods(),
            ["local-exec.process.exec"]
        )
        XCTAssertEqual(host.platformResolutions.count, 1)
        XCTAssertEqual(host.platformResolutions.first?["requestId"] as? String, "platform-1")
        XCTAssertTrue(host.platformRejections.isEmpty)
    }

    @MainActor
    func testPlatformExecRejectsStaleAccountRevisionBeforeRemoteDispatch() async throws {
        let host = AwaitTurnHostStub(
            terminalResponses: [],
            platformRequests: [[
                "requestId": "platform-stale",
                "method": "shell.exec",
                "params": ["command": "pwd"],
            ]]
        )
        let provider = PlatformExecutionProviderStub()
        let supervisor = MahayanaLocalHostSupervisor(host: host, factory: { host })
        let coordinator = MahayanaCoordinator(
            hostSupervisor: supervisor,
            remoteExecutionProvider: provider
        )
        coordinator.updateAccountSettingsScope("account-a")
        let oldIdentity = IOSRemoteRunnerExecutionIdentity(
            accountScope: "account-a",
            accountRevision: 1,
            hostGeneration: 1
        )
        coordinator.updateAccountSettingsScope("account-b")

        XCTAssertTrue(
            try await coordinator.servicePendingPlatformExecution(
                expectedIdentity: oldIdentity
            )
        )
        XCTAssertTrue(await provider.recordedMethods().isEmpty)
        XCTAssertEqual(
            host.platformRejections.first?["requestId"] as? String,
            "platform-stale"
        )
        XCTAssertEqual(
            host.platformRejections.first?["code"] as? String,
            "stale_execution_scope"
        )
    }

    @MainActor
    func testPlatformExecCancellationRejectsLeasedBrokerRequest() async throws {
        let host = AwaitTurnHostStub(
            terminalResponses: [],
            platformRequests: [[
                "requestId": "platform-cancel",
                "method": "process.exec",
                "params": ["program": "git"],
            ]]
        )
        let provider = PlatformExecutionProviderStub(delayNanoseconds: 5_000_000_000)
        let supervisor = MahayanaLocalHostSupervisor(host: host, factory: { host })
        let coordinator = MahayanaCoordinator(
            hostSupervisor: supervisor,
            remoteExecutionProvider: provider
        )
        coordinator.updateAccountSettingsScope("account-cancel")
        let identity = IOSRemoteRunnerExecutionIdentity(
            accountScope: "account-cancel",
            accountRevision: 1,
            hostGeneration: 1
        )

        let task = Task { @MainActor in
            try await coordinator.servicePendingPlatformExecution(
                expectedIdentity: identity
            )
        }
        for _ in 0..<8 {
            await Task.yield()
        }
        task.cancel()
        do {
            _ = try await task.value
            XCTFail("cancelled platform execution must not commit a result")
        } catch is CancellationError {
            // expected
        }

        XCTAssertTrue(host.platformResolutions.isEmpty)
        XCTAssertEqual(
            host.platformRejections.first?["requestId"] as? String,
            "platform-cancel"
        )
        XCTAssertEqual(
            host.platformRejections.first?["code"] as? String,
            "cancelled"
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
