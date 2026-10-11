import XCTest
@testable import Fabushi

private actor TestRemoteRunnerTransport: RemoteRunnerTransport {
    private(set) var methods: [String] = []
    private(set) var params: [CoordinatorPayload] = []

    func dispatch(method: String, params: CoordinatorPayload) async throws -> CoordinatorPayload {
        methods.append(method)
        self.params.append(params)
        return .object([
            "method": .string(method),
            "params": params,
        ])
    }

    func recordedMethods() -> [String] { methods }
    func recordedParams() -> [CoordinatorPayload] { params }
}

private actor TestLocalCapabilityBackend: IOSLocalCapabilityBackend {
    private(set) var capabilities: [LocalCapabilityRunner.Capability] = []

    func execute(
        capability: LocalCapabilityRunner.Capability,
        params: CoordinatorPayload
    ) async throws -> CoordinatorPayload {
        capabilities.append(capability)
        return .object(["local": .string(capability.rawValue)])
    }

    func recordedCapabilities() -> [LocalCapabilityRunner.Capability] { capabilities }
}

final class RunnerParityTests: XCTestCase {
    func testBoxExecAlwaysUsesRemoteRunnerOnIOS() async throws {
        let transport = TestRemoteRunnerTransport()
        let server = IOSBoxExecEntrypoint.makeServer(transport: transport)

        let reply = try await server.execute(.shell(command: "pwd", workingDirectory: "/workspace"))

        let boxMethods = await transport.recordedMethods()
        XCTAssertEqual(boxMethods, ["box.exec"])
        guard case .object(let object) = reply else {
            return XCTFail("expected remote reply object")
        }
        XCTAssertEqual(object["method"], .string("box.exec"))
    }

    func testBoxExecPreservesTypedWriteReadAndStreamPolicyPayloads() async throws {
        let transport = TestRemoteRunnerTransport()
        let server = IOSBoxExecEntrypoint.makeServer(transport: transport)

        _ = try await server.execute(.write(
            path: "/workspace/a.bin",
            fileBytes: "AQID",
            toolCallId: "tool-write"
        ))
        _ = try await server.execute(.redactedRead(
            path: "/workspace/secret.txt",
            offset: 3,
            limit: 9
        ))
        _ = try await server.execute(.shellStreamWithPolicy(
            command: "long-task",
            workingDirectory: "/workspace",
            timeoutMs: 5_000,
            hardTimeoutMs: 30_000,
            timeoutBehavior: .background,
            closeStdin: false
        ))
        _ = try await server.execute(.writeShellStdin(
            shellId: 17,
            data: "hello",
            closeStdin: true
        ))

        let params = await transport.recordedParams()
        XCTAssertEqual(params.count, 4)

        guard case .object(let writeRoot) = params[0],
              case .object(let writeArgs)? = writeRoot["args"] else {
            return XCTFail("expected typed write payload")
        }
        XCTAssertEqual(writeRoot["case"], .string("writeArgs"))
        XCTAssertEqual(writeArgs["path"], .string("/workspace/a.bin"))
        XCTAssertEqual(writeArgs["fileBytes"], .string("AQID"))
        XCTAssertEqual(writeArgs["toolCallId"], .string("tool-write"))

        guard case .object(let readRoot) = params[1],
              case .object(let readArgs)? = readRoot["args"] else {
            return XCTFail("expected typed redacted read payload")
        }
        XCTAssertEqual(readRoot["case"], .string("redactedReadArgs"))
        XCTAssertEqual(readArgs["offset"], .number(3))
        XCTAssertEqual(readArgs["limit"], .number(9))

        guard case .object(let streamRoot) = params[2],
              case .object(let streamArgs)? = streamRoot["args"] else {
            return XCTFail("expected typed stream payload")
        }
        XCTAssertEqual(streamRoot["case"], .string("shellStreamArgs"))
        XCTAssertEqual(streamArgs["timeout"], .number(5_000))
        XCTAssertEqual(streamArgs["hardTimeout"], .number(30_000))
        XCTAssertEqual(streamArgs["timeoutBehavior"], .number(2))
        XCTAssertEqual(streamArgs["closeStdin"], .bool(false))

        guard case .object(let stdinRoot) = params[3],
              case .object(let stdinArgs)? = stdinRoot["args"] else {
            return XCTFail("expected typed stdin payload")
        }
        XCTAssertEqual(stdinRoot["case"], .string("writeShellStdinArgs"))
        XCTAssertEqual(stdinArgs["shellId"], .number(17))
        XCTAssertEqual(stdinArgs["data"], .string("hello"))
        XCTAssertEqual(stdinArgs["closeStdin"], .bool(true))
    }

    func testProductionLocalExecutorOnlyRunsAllowlistedCapabilitiesLocally() async throws {
        let transport = TestRemoteRunnerTransport()
        let local = TestLocalCapabilityBackend()
        let executor = IOSProductionLocalExecutor(
            localBackend: local,
            remoteTransport: transport
        )

        let localReply = try await executor.execute(method: "clipboardRead")
        XCTAssertEqual(localReply, .object(["local": .string("clipboardRead")]))
        let localCapabilities = await local.recordedCapabilities()
        XCTAssertEqual(localCapabilities, [.clipboardRead])

        _ = try await executor.execute(
            method: "shell",
            params: .object(["command": .string("pwd")])
        )
        let remoteMethods = await transport.recordedMethods()
        XCTAssertEqual(remoteMethods, ["local-exec.shell"])
    }

    func testProductionLocalExecutorAcceptsOnlyExactRemoteNamespaces() async throws {
        let transport = TestRemoteRunnerTransport()
        let executor = IOSProductionLocalExecutor(remoteTransport: transport)

        for method in ["shell", "shell.exec", "process", "process.spawn", "box", "box.read"] {
            _ = try await executor.execute(method: method)
        }
        XCTAssertEqual(
            await transport.recordedMethods(),
            [
                "local-exec.shell",
                "local-exec.shell.exec",
                "local-exec.process",
                "local-exec.process.spawn",
                "local-exec.box",
                "local-exec.box.read",
            ]
        )

        for method in ["shellish", "processManager", "boxes", "unknown"] {
            do {
                _ = try await executor.execute(method: method)
                XCTFail("\(method) must fail closed")
            } catch is LocalCapabilityRunner.RunnerError {
            } catch {
                XCTFail("unexpected error for \(method): \(error)")
            }
        }
    }

    @MainActor
    func testGenerationFencedRemoteRunnerRejectsMissingAndStaleIdentity() async throws {
        var identity: IOSRemoteRunnerExecutionIdentity? = nil
        var dispatchCount = 0
        let transport = IOSGenerationFencedRemoteRunnerTransport(
            currentIdentity: { identity },
            dispatch: { _, _ in
                dispatchCount += 1
                return .object(["ok": .bool(true)])
            }
        )

        do {
            _ = try await transport.dispatch(method: "local-exec.shell")
            XCTFail("remote execution must require an active account/Host generation")
        } catch let error as IOSRemoteRunnerFenceError {
            XCTAssertEqual(error, .unavailable)
        }
        XCTAssertEqual(dispatchCount, 0)

        identity = .init(accountScope: "account-1", accountRevision: 7, hostGeneration: 11)
        let stable = try await transport.dispatch(method: "local-exec.shell")
        XCTAssertEqual(stable, .object(["ok": .bool(true)]))
        XCTAssertEqual(dispatchCount, 1)

        let staleTransport = IOSGenerationFencedRemoteRunnerTransport(
            currentIdentity: { identity },
            dispatch: { _, _ in
                dispatchCount += 1
                identity = .init(accountScope: "account-2", accountRevision: 8, hostGeneration: 12)
                return .object(["stale": .bool(true)])
            }
        )
        do {
            _ = try await staleTransport.dispatch(method: "local-exec.process.spawn")
            XCTFail("stale account/Host results must be discarded")
        } catch let error as IOSRemoteRunnerFenceError {
            XCTAssertEqual(error, .stale)
        }
        XCTAssertEqual(dispatchCount, 2)
    }

    func testShippingCoordinatorComposesRemoteRunnerIntoCanonicalHostGraph() throws {
        let iosRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let source = try String(
            contentsOf: iosRoot
                .deletingLastPathComponent()
                .deletingLastPathComponent()
                .appendingPathComponent("source/mahayana-agent-coordinator/MahayanaCoordinator.swift"),
            encoding: .utf8
        )
        XCTAssertTrue(source.contains("remoteTransport: IOSGenerationFencedRemoteRunnerTransport("))
        XCTAssertTrue(source.contains("accountRevision: localToolApprovalScopeRevision"))
        XCTAssertTrue(source.contains("hostGeneration: hostSupervisor.generation"))
        XCTAssertTrue(source.contains("let result = try await self.request(method: method, params: object)"))
        XCTAssertFalse(source.contains("remoteTransport: IOSFailClosedRemoteRunnerTransport()"))
    }

    func testProductionRemoteRunnerScrubsDesktopOnlyEnvironmentVariables() async throws {
        let transport = TestRemoteRunnerTransport()
        let executor = IOSProductionLocalExecutor(remoteTransport: transport)

        _ = try await executor.execute(
            method: "shell",
            params: .object([
                "command": .string("pwd"),
                "env": .object([
                    "SAFE_VALUE": .string("kept"),
                    "ELECTRON_RUN_AS_NODE": .string("1"),
                    "SSH_AUTH_SOCK": .string("/tmp/agent.sock"),
                    "DBUS_SESSION_BUS_ADDRESS": .string("unix:path=/tmp/dbus"),
                    "XDG_RUNTIME_DIR": .string("/run/user/1000"),
                    "WAYLAND_DISPLAY": .string("wayland-0"),
                ]),
            ])
        )

        let recorded = await transport.recordedParams()
        guard case .object(let root) = try XCTUnwrap(recorded.first),
              case .object(let environment)? = root["env"] else {
            return XCTFail("expected sanitized env payload")
        }
        XCTAssertEqual(environment["SAFE_VALUE"], .string("kept"))
        XCTAssertNil(environment["ELECTRON_RUN_AS_NODE"])
        for key in ShellExecEnvironmentFilter.socketEnvironmentVariablesToScrub {
            XCTAssertNil(environment[key], "desktop socket variable should not cross Remote Runner boundary: \(key)")
        }
    }

    func testInvariantViolationLogUsesStableEventEnvelope() throws {
        let line = IOSLocalExecInvariantViolationLog.line(name: "generation-mismatch")
        let data = try XCTUnwrap(line.data(using: .utf8))
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: String])
        XCTAssertEqual(object["event"], IOSLocalExecInvariantViolationLog.event)
        XCTAssertEqual(object["name"], "generation-mismatch")
    }

    func testMimeLookupUsesNativeUniformTypeIdentifiers() {
        XCTAssertEqual(IOSMimeTypes.lookup(path: "image.png"), "image/png")
        XCTAssertNil(IOSMimeTypes.lookup(path: "README"))
    }
}
