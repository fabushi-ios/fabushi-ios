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

        for method in ["shell", "shell.exec", "process", "process.spawn", "box", "box.read", "browser", "browser.exec"] {
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
                "local-exec.browser",
                "local-exec.browser.exec",
            ]
        )

        for method in ["shellish", "processManager", "boxes", "browserish", "unknown"] {
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

    @MainActor
    func testGenerationFencedRemoteRunnerRejectsIndependentAccountAndHostGenerationLateResults() async throws {
        var identity = IOSRemoteRunnerExecutionIdentity(
            accountScope: "account-1",
            accountRevision: 7,
            hostGeneration: 11
        )

        let accountSwitch = IOSGenerationFencedRemoteRunnerTransport(
            currentIdentity: { identity },
            dispatch: { _, _ in
                identity = .init(
                    accountScope: "account-2",
                    accountRevision: 8,
                    hostGeneration: 11
                )
                return .object(["late": .bool(true)])
            }
        )
        do {
            _ = try await accountSwitch.dispatch(method: "local-exec.shell.exec", params: .object([:]))
            XCTFail("account switch must discard the late provider result")
        } catch let error as IOSRemoteRunnerFenceError {
            XCTAssertEqual(error, .stale)
        }

        identity = .init(
            accountScope: "account-1",
            accountRevision: 7,
            hostGeneration: 11
        )
        let hostRestart = IOSGenerationFencedRemoteRunnerTransport(
            currentIdentity: { identity },
            dispatch: { _, _ in
                identity = .init(
                    accountScope: "account-1",
                    accountRevision: 7,
                    hostGeneration: 12
                )
                return .object(["late": .bool(true)])
            }
        )
        do {
            _ = try await hostRestart.dispatch(method: "local-exec.box.exec", params: .object([:]))
            XCTFail("Host generation change must discard the late provider result")
        } catch let error as IOSRemoteRunnerFenceError {
            XCTAssertEqual(error, .stale)
        }
    }

    @MainActor
    func testGenerationFencedRemoteRunnerPropagatesProviderFailureAndCancellation() async throws {
        enum ExpectedProviderFailure: Error {
            case failed
        }

        let identity = IOSRemoteRunnerExecutionIdentity(
            accountScope: "account-1",
            accountRevision: 7,
            hostGeneration: 11
        )
        let failing = IOSGenerationFencedRemoteRunnerTransport(
            currentIdentity: { identity },
            dispatch: { _, _ in
                throw ExpectedProviderFailure.failed
            }
        )
        do {
            _ = try await failing.dispatch(method: "local-exec.shell.exec", params: .object([:]))
            XCTFail("provider failure must propagate")
        } catch ExpectedProviderFailure.failed {
        } catch {
            XCTFail("unexpected provider failure: \(error)")
        }

        let cancellable = IOSGenerationFencedRemoteRunnerTransport(
            currentIdentity: { identity },
            dispatch: { _, _ in
                try await Task.sleep(nanoseconds: 5_000_000_000)
                return .object(["unexpected": .bool(true)])
            }
        )
        let task = Task {
            try await cancellable.dispatch(
                method: "local-exec.process.exec",
                params: .object(["program": .string("git")])
            )
        }
        await Task.yield()
        task.cancel()
        do {
            _ = try await task.value
            XCTFail("cancelled provider dispatch must not return a result")
        } catch is CancellationError {
        } catch {
            XCTFail("expected CancellationError, got \(error)")
        }
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
        XCTAssertTrue(source.contains("servicePendingPlatformExecution("))
        XCTAssertTrue(source.contains("self.remoteExecutionProvider.dispatch("))
        XCTAssertTrue(source.contains("surface.dispatchRemoteExecution("))
        XCTAssertTrue(source.contains("IOSClosureRemoteExecutionProvider("))
        XCTAssertFalse(source.contains("let result = try await self.request(method: method, params: object)"))
        XCTAssertFalse(source.contains("remoteTransport: IOSFailClosedRemoteRunnerTransport()"))
    }

    func testShippingRemoteExecutionProviderFailsClosedUntilReviewedProviderIsConfigured() async {
        let provider = IOSUnavailableRemoteExecutionProvider()
        do {
            _ = try await provider.dispatch(
                method: "local-exec.process.exec",
                params: .object(["program": .string("git")])
            )
            XCTFail("shipping fallback must not fabricate process execution")
        } catch let error as IOSRemoteExecutionProviderError {
            XCTAssertEqual(error, .unavailable)
        } catch {
            XCTFail("unexpected unavailable-provider error: \(error)")
        }
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
    func testEnsureSandBoxDescriptorDecodesExecDaemonAuthorityFields() throws {
        var payload = Data()
        func appendVarint(_ value: UInt64, to data: inout Data) {
            var value = value
            repeat {
                var byte = UInt8(value & 0x7f)
                value >>= 7
                if value != 0 { byte |= 0x80 }
                data.append(byte)
            } while value != 0
        }
        func appendBytes(_ field: UInt64, _ value: String, to data: inout Data) {
            let bytes = Data(value.utf8)
            appendVarint((field << 3) | 2, to: &data)
            appendVarint(UInt64(bytes.count), to: &data)
            data.append(bytes)
        }

        appendBytes(4, "network-token", to: &payload)
        appendBytes(5, "exec-auth", to: &payload)
        appendBytes(6, "https://exec.example.test", to: &payload)
        appendBytes(7, "https://vnc.example.test", to: &payload)
        appendBytes(10, "https://gateway.example.test", to: &payload)
        appendBytes(11, "gateway-token", to: &payload)
        appendBytes(12, "https://fork-vnc.example.test", to: &payload)

        let descriptor = try IOSCursorDashboardClient.decodeEnsureSandBoxConnectionForTests(payload)
        XCTAssertEqual(descriptor.networkToken, "network-token")
        XCTAssertEqual(descriptor.execDaemonAuthToken, "exec-auth")
        XCTAssertEqual(descriptor.execDaemonURL, "https://exec.example.test")
        XCTAssertEqual(descriptor.vncURL, "https://vnc.example.test")
        XCTAssertEqual(descriptor.gatewayURL, "https://gateway.example.test")
        XCTAssertEqual(descriptor.gatewayToken, "gateway-token")
        XCTAssertEqual(descriptor.forkVNCBaseURL, "https://fork-vnc.example.test")
        XCTAssertTrue(IOSCursorDashboardClient.ensureSandBoxRequestForTests().isEmpty)
    }

    func testEnsureSandBoxDescriptorFailsClosedWithoutExecutableAuthority() {
        var payload = Data()
        payload.append(0x2a) // field 5, bytes
        payload.append(0x00) // empty exec-daemon auth token
        XCTAssertThrowsError(
            try IOSCursorDashboardClient.decodeEnsureSandBoxConnectionForTests(payload)
        )
    }

    func testRemoteExecDaemonShellUsesCanonicalBearerConnectWire() async throws {
        let descriptor = IOSCursorSandBoxConnection(
            networkToken: "network",
            execDaemonAuthToken: "secret",
            execDaemonURL: "https://exec.example.test",
            vncURL: "",
            gatewayURL: "",
            gatewayToken: "",
            forkVNCBaseURL: ""
        )
        var observed: URLRequest?
        let client = IOSRemoteExecDaemonClient(
            descriptor: descriptor,
            requestExecutor: { request in
                observed = request

                func varint(_ value: UInt64) -> Data {
                    var value = value
                    var data = Data()
                    repeat {
                        var byte = UInt8(value & 0x7f)
                        value >>= 7
                        if value != 0 { byte |= 0x80 }
                        data.append(byte)
                    } while value != 0
                    return data
                }
                func field(_ number: UInt64, bytes: Data) -> Data {
                    varint((number << 3) | 2) + varint(UInt64(bytes.count)) + bytes
                }
                func vfield(_ number: UInt64, value: UInt64) -> Data {
                    varint(number << 3) + varint(value)
                }
                func frame(_ payload: Data, flags: UInt8 = 0) -> Data {
                    let n = UInt32(payload.count)
                    return Data([flags, UInt8(n >> 24), UInt8(n >> 16), UInt8(n >> 8), UInt8(n)])
                        + payload
                }

                let stdout = field(1, bytes: Data("ok\n".utf8))
                let stdoutEvent = field(1, bytes: stdout)
                let stdoutClient = field(14, bytes: stdoutEvent)
                let stdoutElement = field(1, bytes: stdoutClient)

                let exitBody = vfield(1, value: 0) + field(2, bytes: Data("/workspace".utf8))
                let exitEvent = field(3, bytes: exitBody)
                let exitClient = field(14, bytes: exitEvent)
                let exitElement = field(1, bytes: exitClient)

                let body = frame(stdoutElement) + frame(exitElement) + frame(Data("{}".utf8), flags: 0x02)
                let response = HTTPURLResponse(
                    url: request.url!,
                    statusCode: 200,
                    httpVersion: "HTTP/1.1",
                    headerFields: ["Content-Type": "application/connect+proto"]
                )!
                return (body, response)
            }
        )
        let result = try await client.dispatch(
            method: "local-exec.shell.exec",
            params: .object([
                "command": .string("pwd"),
                "workingDirectory": .string("/workspace"),
                "toolCallId": .string("tool-shell"),
            ])
        )
        XCTAssertEqual(observed?.url?.path, "/agent.v1.ExecService/Exec")
        XCTAssertEqual(observed?.value(forHTTPHeaderField: "Authorization"), "Bearer secret")
        XCTAssertEqual(observed?.value(forHTTPHeaderField: "Content-Type"), "application/proto")
        XCTAssertEqual(observed?.value(forHTTPHeaderField: "Connect-Protocol-Version"), "1")
        guard let body = observed?.httpBody else { return XCTFail("missing ExecServerMessage body") }
        XCTAssertNotNil(body.range(of: Data("pwd".utf8)))
        XCTAssertNotNil(body.range(of: Data("/workspace".utf8)))
        guard case .object(let value) = result else { return XCTFail("expected shell result") }
        XCTAssertEqual(value["success"], .bool(true))
        XCTAssertEqual(value["code"], .number(0))
        XCTAssertEqual(value["stdout"], .string("ok\n"))
        XCTAssertEqual(value["cwd"], .string("/workspace"))
    }

    func testRemoteExecDaemonReadAndProcessFailClosedSemantics() async throws {
        let descriptor = IOSCursorSandBoxConnection(
            networkToken: "",
            execDaemonAuthToken: "secret",
            execDaemonURL: "https://exec.example.test",
            vncURL: "",
            gatewayURL: "",
            gatewayToken: "",
            forkVNCBaseURL: ""
        )
        let client = IOSRemoteExecDaemonClient(
            descriptor: descriptor,
            requestExecutor: { request in
                func varint(_ value: UInt64) -> Data {
                    var value = value
                    var data = Data()
                    repeat {
                        var byte = UInt8(value & 0x7f)
                        value >>= 7
                        if value != 0 { byte |= 0x80 }
                        data.append(byte)
                    } while value != 0
                    return data
                }
                func field(_ number: UInt64, bytes: Data) -> Data {
                    varint((number << 3) | 2) + varint(UInt64(bytes.count)) + bytes
                }
                func vfield(_ number: UInt64, value: UInt64) -> Data {
                    varint(number << 3) + varint(value)
                }
                func frame(_ payload: Data, flags: UInt8 = 0) -> Data {
                    let n = UInt32(payload.count)
                    return Data([flags, UInt8(n >> 24), UInt8(n >> 16), UInt8(n >> 8), UInt8(n)])
                        + payload
                }

                let success =
                    field(1, bytes: Data("/workspace/README.md".utf8))
                    + field(2, bytes: Data("hello".utf8))
                    + vfield(3, value: 1)
                    + vfield(4, value: 5)
                let readResult = field(1, bytes: success)
                let clientMessage = field(7, bytes: readResult)
                let element = field(1, bytes: clientMessage)
                let data = frame(element) + frame(Data("{}".utf8), flags: 0x02)
                return (
                    data,
                    HTTPURLResponse(
                        url: request.url!,
                        statusCode: 200,
                        httpVersion: "HTTP/1.1",
                        headerFields: ["Content-Type": "application/connect+proto"]
                    )!
                )
            }
        )

        let read = try await client.dispatch(
            method: "local-exec.box.exec",
            params: .object([
                "case": .string("readArgs"),
                "args": .object([
                    "path": .string("/workspace/README.md"),
                    "toolCallId": .string("tool-read"),
                ]),
            ])
        )
        guard case .object(let value) = read else { return XCTFail("expected read result") }
        XCTAssertEqual(value["content"], .string("hello"))
        XCTAssertEqual(value["success"], .bool(true))

        var observedProcessRequest: URLRequest?
        let processClient = IOSRemoteExecDaemonClient(
            descriptor: descriptor,
            requestExecutor: { request in
                observedProcessRequest = request

                func varint(_ value: UInt64) -> Data {
                    var value = value
                    var data = Data()
                    repeat {
                        var byte = UInt8(value & 0x7f)
                        value >>= 7
                        if value != 0 { byte |= 0x80 }
                        data.append(byte)
                    } while value != 0
                    return data
                }
                func field(_ number: UInt64, bytes: Data) -> Data {
                    varint((number << 3) | 2) + varint(UInt64(bytes.count)) + bytes
                }
                func vfield(_ number: UInt64, value: UInt64) -> Data {
                    varint(number << 3) + varint(value)
                }
                func frame(_ payload: Data, flags: UInt8 = 0) -> Data {
                    let n = UInt32(payload.count)
                    return Data([flags, UInt8(n >> 24), UInt8(n >> 16), UInt8(n >> 8), UInt8(n)])
                        + payload
                }

                let stdoutBody = field(1, bytes: Data(" M file\n".utf8))
                let stdoutEvent = field(1, bytes: stdoutBody)
                let stdoutClient = field(14, bytes: stdoutEvent)
                let stdoutElement = field(1, bytes: stdoutClient)
                let exitBody = vfield(1, value: 0) + field(2, bytes: Data("/workspace".utf8))
                let exitEvent = field(3, bytes: exitBody)
                let exitClient = field(14, bytes: exitEvent)
                let exitElement = field(1, bytes: exitClient)
                let data = frame(stdoutElement) + frame(exitElement) + frame(Data("{}".utf8), flags: 0x02)
                return (
                    data,
                    HTTPURLResponse(
                        url: request.url!,
                        statusCode: 200,
                        httpVersion: "HTTP/1.1",
                        headerFields: ["Content-Type": "application/connect+proto"]
                    )!
                )
            }
        )
        let process = try await processClient.dispatch(
            method: "local-exec.process.exec",
            params: .object([
                "program": .string("git"),
                "args": .array([.string("status"), .string("--short"), .string("quote'arg")]),
                "workingDirectory": .string("/workspace"),
            ])
        )
        guard let processBody = observedProcessRequest?.httpBody else {
            return XCTFail("missing process ExecServerMessage body")
        }
        XCTAssertNotNil(processBody.range(of: Data("git".utf8)))
        XCTAssertNotNil(processBody.range(of: Data("status".utf8)))
        XCTAssertNotNil(processBody.range(of: Data("--short".utf8)))
        XCTAssertNotNil(processBody.range(of: Data("quote".utf8)))
        guard case .object(let processValue) = process else {
            return XCTFail("expected process result")
        }
        XCTAssertEqual(processValue["success"], .bool(true))
        XCTAssertEqual(processValue["code"], .number(0))
        XCTAssertEqual(processValue["stdout"], .string(" M file\n"))

        do {
            _ = try await processClient.dispatch(
                method: "local-exec.unknown",
                params: .object([:])
            )
            XCTFail("unknown remote method must fail closed")
        } catch let error as IOSRemoteExecDaemonError {
            XCTAssertEqual(error, .unsupported("local-exec.unknown"))
        }
    }


    func testRemoteExecDaemonBrowserDriverUsesCurrentGenerationFenceContract() async throws {
        let descriptor = IOSCursorSandBoxConnection(
            networkToken: "",
            execDaemonAuthToken: "secret",
            execDaemonURL: "https://exec.example.test",
            vncURL: "",
            gatewayURL: "",
            gatewayToken: "",
            forkVNCBaseURL: ""
        )
        var observed: URLRequest?
        let client = IOSRemoteExecDaemonClient(
            descriptor: descriptor,
            requestExecutor: { request in
                observed = request
                func varint(_ value: UInt64) -> Data {
                    var value = value
                    var data = Data()
                    repeat {
                        var byte = UInt8(value & 0x7f)
                        value >>= 7
                        if value != 0 { byte |= 0x80 }
                        data.append(byte)
                    } while value != 0
                    return data
                }
                func field(_ number: UInt64, bytes: Data) -> Data {
                    varint((number << 3) | 2) + varint(UInt64(bytes.count)) + bytes
                }
                func vfield(_ number: UInt64, value: UInt64) -> Data {
                    varint(number << 3) + varint(value)
                }
                func frame(_ payload: Data, flags: UInt8 = 0) -> Data {
                    let n = UInt32(payload.count)
                    return Data([flags, UInt8(n >> 24), UInt8(n >> 16), UInt8(n >> 8), UInt8(n)]) + payload
                }
                let output = "__SAND_BROWSER_RESULT__{\"ok\":true,\"summary\":\"Refreshed the current tab\",\"viewId\":\"main\",\"generation\":\"generation-8\",\"screenshot\":true}\n__FABUSHI_BROWSER_SCREENSHOT__AQID\n"
                let stdoutBody = field(1, bytes: Data(output.utf8))
                let stdoutEvent = field(1, bytes: stdoutBody)
                let stdoutClient = field(14, bytes: stdoutEvent)
                let stdoutElement = field(1, bytes: stdoutClient)
                let exitBody = vfield(1, value: 0) + field(2, bytes: Data("/workspace".utf8))
                let exitEvent = field(3, bytes: exitBody)
                let exitClient = field(14, bytes: exitEvent)
                let exitElement = field(1, bytes: exitClient)
                return (
                    frame(stdoutElement) + frame(exitElement) + frame(Data("{}".utf8), flags: 0x02),
                    HTTPURLResponse(
                        url: request.url!,
                        statusCode: 200,
                        httpVersion: "HTTP/1.1",
                        headerFields: ["Content-Type": "application/connect+proto"]
                    )!
                )
            }
        )
        let driver = "const GENERATION_FENCED_OPS = new Set([\"navigate\",\"refresh\",\"back\",\"forward\"]); function requireExpectedGeneration() {} async function navigateHistory() { return \"Page.navigateToHistoryEntry\"; }"
        let result = try await client.dispatch(
            method: "local-exec.browser.exec",
            params: .object([
                "driverVersion": .number(2),
                "driverPath": .string("/tmp/.sand-browser/driver-v2.mjs"),
                "driverDirectory": .string("/tmp/.sand-browser"),
                "driverSource": .string(driver),
                "resultMarker": .string("__SAND_BROWSER_RESULT__"),
                "request": .object([
                    "op": .string("refresh"),
                    "display": .number(0),
                    "cdpPort": .number(9222),
                    "viewId": .string("main"),
                    "expectedGeneration": .string("generation-7"),
                    "screenshotPath": .string("/tmp/.sand-browser/shot-browser-call-1.png"),
                ]),
                "screenshotPath": .string("/tmp/.sand-browser/shot-browser-call-1.png"),
                "toolCallId": .string("browser-call-1"),
            ])
        )
        guard let body = observed?.httpBody else {
            return XCTFail("missing browser ExecServerMessage body")
        }
        XCTAssertNotNil(body.range(of: Data("/tmp/.sand-browser/driver-v2.mjs".utf8)))
        XCTAssertNotNil(body.range(of: Data("__FABUSHI_BROWSER_SCREENSHOT__".utf8)))
        guard case .object(let value) = result else {
            return XCTFail("expected browser result")
        }
        XCTAssertEqual(value["isError"], .bool(false))
        XCTAssertEqual(value["viewId"], .string("main"))
        XCTAssertEqual(value["generation"], .string("generation-8"))
        XCTAssertEqual(value["imageB64"], .string("AQID"))
    }

    func testShippingRemoteProviderReusesAccountScopedMcpSurface() throws {
        let iosRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let sourceRoot = iosRoot
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .appendingPathComponent("source")
        let coordinator = try String(
            contentsOf: sourceRoot
                .appendingPathComponent("mahayana-agent-coordinator/MahayanaCoordinator.swift"),
            encoding: .utf8
        )
        let mcp = try String(
            contentsOf: sourceRoot
                .appendingPathComponent("mahayana-agent-coordinator/coordinator-mcp-manager.swift"),
            encoding: .utf8
        )
        XCTAssertTrue(coordinator.contains("surface.dispatchRemoteExecution("))
        XCTAssertTrue(mcp.contains("let descriptor = try await dashboard.ensureSandBox()"))
        XCTAssertTrue(mcp.contains("IOSRemoteExecDaemonClient("))
        XCTAssertTrue(mcp.contains("guard routedScopeToken() == token"))
        XCTAssertTrue(mcp.contains("try refuseComputerConnectionWhileClientPaused()"))
        let executor = try String(
            contentsOf: sourceRoot
                .appendingPathComponent("local-exec-daemon/production-executor.swift"),
            encoding: .utf8
        )
        XCTAssertTrue(executor.contains(#"case "local-exec.process", "local-exec.process.exec", "local-exec.process.spawn":"#))
        XCTAssertTrue(executor.contains("map(Self.shellQuotedArgument)"))
        XCTAssertTrue(executor.contains(#"appendingPathComponent("agent.v1.ExecService")"#))
        XCTAssertTrue(executor.contains(#"appendingPathComponent("Exec")"#))
    }


}
