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


private final class TestURLRequestCapture: @unchecked Sendable {
    private let lock = NSLock()
    private var request: URLRequest?
    func record(_ request: URLRequest) { lock.lock(); defer { lock.unlock() }; self.request = request }
    var value: URLRequest? { lock.lock(); defer { lock.unlock() }; return request }
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
    func testAgentToolsMaterializerKeepsSmallTextInline() {
        XCTAssertNil(
            IOSAgentToolsOutputMaterializer.prepare(
                content: "small",
                thresholdBytes: 5
            )
        )
    }

    func testAgentToolsMaterializerBuildsBoundedWorkspaceLocation() throws {
        let prepared = try XCTUnwrap(
            IOSAgentToolsOutputMaterializer.prepare(
                content: "alpha\nbeta\ngamma",
                thresholdBytes: 4,
                maxSizeBytes: 10,
                identifier: "fixed-id"
            )
        )
        XCTAssertEqual(prepared.path, "/workspace/agent-tools/fixed-id.txt")
        XCTAssertEqual(prepared.sizeBytes, 10)
        XCTAssertEqual(prepared.lineCount, 2)
        XCTAssertEqual(
            String(
                data: try XCTUnwrap(Data(base64Encoded: prepared.fileBytesBase64)),
                encoding: .utf8
            ),
            "alpha\nbeta"
        )
    }

    func testAgentToolsMaterializerDoesNotSplitUTF8AtBound() throws {
        let prepared = try XCTUnwrap(
            IOSAgentToolsOutputMaterializer.prepare(
                content: "12345🙂tail",
                thresholdBytes: 1,
                maxSizeBytes: 7,
                identifier: "utf8"
            )
        )
        let data = try XCTUnwrap(Data(base64Encoded: prepared.fileBytesBase64))
        XCTAssertEqual(String(data: data, encoding: .utf8), "12345")
        XCTAssertEqual(prepared.sizeBytes, 5)
    }

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
            closeStdin: false,
            requestedSandboxPolicy: .object([
                "type": .number(2),
                "networkAccess": .bool(false),
                "readBoundary": .number(2),
                "additionalReadPaths": .array([.string("/workspace/shared")]),
            ]),
            adminCommandDenylist: ["rm -rf *", "aws:*s3 rm*"]
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
        XCTAssertEqual(
            streamArgs["requestedSandboxPolicy"],
            .object([
                "type": .number(2),
                "networkAccess": .bool(false),
                "readBoundary": .number(2),
                "additionalReadPaths": .array([.string("/workspace/shared")]),
            ])
        )
        XCTAssertEqual(
            streamArgs["adminCommandDenylist"],
            .array([.string("rm -rf *"), .string("aws:*s3 rm*")])
        )

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
        let recordedMethods = await transport.recordedMethods()
        XCTAssertEqual(
            recordedMethods,
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
            _ = try await transport.dispatch(method: "local-exec.shell", params: .object([:]))
            XCTFail("remote execution must require an active account/Host generation")
        } catch let error as IOSRemoteRunnerFenceError {
            XCTAssertEqual(error, .unavailable)
        }
        XCTAssertEqual(dispatchCount, 0)

        identity = .init(accountScope: "account-1", accountRevision: 7, hostGeneration: 11)
        let stable = try await transport.dispatch(method: "local-exec.shell", params: .object([:]))
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
            _ = try await staleTransport.dispatch(method: "local-exec.process.spawn", params: .object([:]))
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
        let capture = TestURLRequestCapture()
        let client = IOSRemoteExecDaemonClient(
            descriptor: descriptor,
            requestExecutor: { request in
                capture.record(request)

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
                "timeout": .number(5_000),
                "isBackground": .bool(true),
                "timeoutBehavior": .number(2),
                "hardTimeout": .number(30_000),
                "requestedSandboxPolicy": .object([
                    "type": .number(2),
                    "networkAccess": .bool(true),
                    "additionalReadwritePaths": .array([.string("/workspace/out")]),
                    "additionalReadonlyPaths": .array([.string("/workspace/input")]),
                    "debugOutputDir": .string("/workspace/debug"),
                    "disableTmpWrite": .bool(true),
                    "allowlistEscalated": .bool(false),
                    "enableSharedBuildCache": .bool(true),
                    "networkPolicy": .object([
                        "version": .number(1),
                        "defaultAction": .number(2),
                        "deny": .array([.string("blocked.example")]),
                        "allow": .array([.string("api.example")]),
                        "logging": .object([
                            "decisionLogPath": .string("/workspace/network.log"),
                            "logFormat": .string("jsonl"),
                        ]),
                    ]),
                    "networkPolicyStrict": .bool(true),
                    "captureDenies": .bool(true),
                    "skipStatsigDefaults": .bool(false),
                    "readBoundary": .number(2),
                    "additionalReadPaths": .array([.string("/workspace/shared")]),
                ]),
                "adminCommandDenylist": .array([
                    .string("rm -rf *"),
                    .string("aws:*s3 rm*"),
                ]),
            ])
        )
        XCTAssertEqual(capture.value?.url?.path, "/agent.v1.ExecService/Exec")
        XCTAssertEqual(capture.value?.value(forHTTPHeaderField: "Authorization"), "Bearer secret")
        XCTAssertEqual(capture.value?.value(forHTTPHeaderField: "Content-Type"), "application/proto")
        XCTAssertEqual(capture.value?.value(forHTTPHeaderField: "Connect-Protocol-Version"), "1")
        guard let body = capture.value?.httpBody else { return XCTFail("missing ExecServerMessage body") }
        XCTAssertNotNil(body.range(of: Data("pwd".utf8)))
        XCTAssertNotNil(body.range(of: Data("/workspace".utf8)))

        func readVarint(_ bytes: [UInt8], index: inout Int) -> UInt64? {
            var value: UInt64 = 0
            var shift: UInt64 = 0
            while index < bytes.count, shift < 64 {
                let byte = bytes[index]
                index += 1
                value |= UInt64(byte & 0x7f) << shift
                if byte & 0x80 == 0 { return value }
                shift += 7
            }
            return nil
        }
        func protobufFields(_ data: Data) -> [(UInt64, UInt8, Data)]? {
            let bytes = [UInt8](data)
            var index = 0
            var fields: [(UInt64, UInt8, Data)] = []
            while index < bytes.count {
                guard let key = readVarint(bytes, index: &index) else { return nil }
                let field = key >> 3
                let wire = UInt8(key & 7)
                switch wire {
                case 0:
                    let start = index
                    guard readVarint(bytes, index: &index) != nil else { return nil }
                    fields.append((field, wire, Data(bytes[start..<index])))
                case 2:
                    guard let rawLength = readVarint(bytes, index: &index),
                          rawLength <= UInt64(Int.max)
                    else { return nil }
                    let length = Int(rawLength)
                    guard length <= bytes.count - index else { return nil }
                    let end = index + length
                    fields.append((field, wire, Data(bytes[index..<end])))
                    index = end
                default:
                    return nil
                }
            }
            return fields
        }
        func firstBytes(_ field: UInt64, in data: Data) -> Data? {
            protobufFields(data)?.first(where: { $0.0 == field && $0.1 == 2 })?.2
        }
        func allStrings(_ field: UInt64, in data: Data) -> [String] {
            (protobufFields(data) ?? [])
                .filter { $0.0 == field && $0.1 == 2 }
                .compactMap { String(data: $0.2, encoding: .utf8) }
        }
        func firstVarint(_ field: UInt64, in data: Data) -> UInt64? {
            guard let raw = protobufFields(data)?.first(where: { $0.0 == field && $0.1 == 0 })?.2 else {
                return nil
            }
            var index = 0
            return readVarint([UInt8](raw), index: &index)
        }

        let shellArgs = try XCTUnwrap(firstBytes(14, in: body))
        XCTAssertEqual(firstVarint(3, in: shellArgs), 5_000)
        XCTAssertEqual(firstVarint(11, in: shellArgs), 1)
        XCTAssertEqual(firstVarint(13, in: shellArgs), 2)
        XCTAssertEqual(firstVarint(14, in: shellArgs), 30_000)
        let policy = try XCTUnwrap(firstBytes(9, in: shellArgs))
        XCTAssertEqual(firstVarint(1, in: policy), 2)
        XCTAssertEqual(firstVarint(2, in: policy), 1)
        XCTAssertEqual(allStrings(3, in: policy), ["/workspace/out"])
        XCTAssertEqual(allStrings(4, in: policy), ["/workspace/input"])
        XCTAssertEqual(String(data: try XCTUnwrap(firstBytes(5, in: policy)), encoding: .utf8), "/workspace/debug")
        XCTAssertEqual(firstVarint(7, in: policy), 1)
        XCTAssertEqual(firstVarint(8, in: policy), 0)
        XCTAssertEqual(firstVarint(9, in: policy), 1)
        XCTAssertEqual(firstVarint(11, in: policy), 1)
        XCTAssertEqual(firstVarint(12, in: policy), 1)
        XCTAssertEqual(firstVarint(13, in: policy), 0)
        XCTAssertEqual(firstVarint(14, in: policy), 2)
        XCTAssertEqual(allStrings(15, in: policy), ["/workspace/shared"])
        XCTAssertEqual(allStrings(22, in: shellArgs), ["rm -rf *", "aws:*s3 rm*"])

        let network = try XCTUnwrap(firstBytes(10, in: policy))
        XCTAssertEqual(firstVarint(1, in: network), 1)
        XCTAssertEqual(firstVarint(2, in: network), 2)
        XCTAssertEqual(allStrings(3, in: network), ["blocked.example"])
        XCTAssertEqual(allStrings(4, in: network), ["api.example"])
        let logging = try XCTUnwrap(firstBytes(5, in: network))
        XCTAssertEqual(String(data: try XCTUnwrap(firstBytes(1, in: logging)), encoding: .utf8), "/workspace/network.log")
        XCTAssertEqual(String(data: try XCTUnwrap(firstBytes(2, in: logging)), encoding: .utf8), "jsonl")

        guard case .object(let value) = result else { return XCTFail("expected shell result") }
        XCTAssertEqual(value["success"], .bool(true))
        XCTAssertEqual(value["code"], .number(0))
        XCTAssertEqual(value["stdout"], .string("ok\n"))
        XCTAssertEqual(value["cwd"], .string("/workspace"))
    }


    func testRemoteExecDaemonShellPreservesCanonicalRejectedResult() async throws {
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
                func frame(_ payload: Data, flags: UInt8 = 0) -> Data {
                    let n = UInt32(payload.count)
                    return Data([flags, UInt8(n >> 24), UInt8(n >> 16), UInt8(n >> 8), UInt8(n)]) + payload
                }

                let rejected = field(1, bytes: Data("rm -rf /workspace".utf8))
                    + field(2, bytes: Data("/workspace".utf8))
                    + field(3, bytes: Data("Command is not allowed".utf8))
                let shellEvent = field(5, bytes: rejected)
                let clientMessage = field(14, bytes: shellEvent)
                let element = field(1, bytes: clientMessage)
                let body = frame(element) + frame(Data("{}".utf8), flags: 0x02)
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
                "command": .string("rm -rf /workspace"),
                "workingDirectory": .string("/workspace"),
            ])
        )
        guard case .object(let root) = result,
              case .object(let value)? = root["value"] else {
            return XCTFail("expected canonical rejected shell result")
        }
        XCTAssertEqual(root["case"], .string("rejected"))
        XCTAssertEqual(value["command"], .string("rm -rf /workspace"))
        XCTAssertEqual(value["workingDirectory"], .string("/workspace"))
        XCTAssertEqual(value["reason"], .string("Command is not allowed"))
    }

    func testRemoteExecDaemonShellPreservesCanonicalReadonlyPermissionDeniedResult() async throws {
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
                    return Data([flags, UInt8(n >> 24), UInt8(n >> 16), UInt8(n >> 8), UInt8(n)]) + payload
                }

                let denied = field(1, bytes: Data("cat /workspace/secret.txt".utf8))
                    + field(2, bytes: Data("/workspace".utf8))
                    + field(3, bytes: Data("Command blocked by permissions configuration".utf8))
                    + vfield(4, value: 1)
                let shellEvent = field(6, bytes: denied)
                let clientMessage = field(14, bytes: shellEvent)
                let element = field(1, bytes: clientMessage)
                let body = frame(element) + frame(Data("{}".utf8), flags: 0x02)
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
                "command": .string("cat /workspace/secret.txt"),
                "workingDirectory": .string("/workspace"),
            ])
        )
        guard case .object(let root) = result,
              case .object(let value)? = root["value"] else {
            return XCTFail("expected canonical permission-denied shell result")
        }
        XCTAssertEqual(root["case"], .string("permissionDenied"))
        XCTAssertEqual(value["command"], .string("cat /workspace/secret.txt"))
        XCTAssertEqual(value["workingDirectory"], .string("/workspace"))
        XCTAssertEqual(value["error"], .string("Command blocked by permissions configuration"))
        XCTAssertEqual(value["isReadonly"], .bool(true))
    }

    func testRemoteExecDaemonWriteUsesCanonicalBoxBytesAndResult() async throws {
        let descriptor = IOSCursorSandBoxConnection(
            networkToken: "",
            execDaemonAuthToken: "secret",
            execDaemonURL: "https://exec.example.test",
            vncURL: "",
            gatewayURL: "",
            gatewayToken: "",
            forkVNCBaseURL: ""
        )
        let capture = TestURLRequestCapture()
        let client = IOSRemoteExecDaemonClient(
            descriptor: descriptor,
            requestExecutor: { request in
                capture.record(request)

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
                    field(1, bytes: Data("/workspace/cloud-agent.jsonl".utf8))
                    + vfield(2, value: 1)
                    + vfield(3, value: 35)
                    + field(4, bytes: Data("after-write".utf8))
                let writeResult = field(1, bytes: success)
                let clientMessage = field(3, bytes: writeResult)
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

        let bytes = Data("{\"role\":\"assistant\",\"text\":\"done\"}\n".utf8)
        let result = try await client.dispatch(
            method: "local-exec.box.exec",
            params: .object([
                "case": .string("writeArgs"),
                "args": .object([
                    "path": .string("/workspace/cloud-agent.jsonl"),
                    "fileBytes": .string(bytes.base64EncodedString()),
                    "toolCallId": .string("tool-dump"),
                    "returnFileContentAfterWrite": .bool(true),
                    "encodingHint": .string("utf-8"),
                ]),
            ])
        )
        guard let body = capture.value?.httpBody else { return XCTFail("missing write request body") }
        XCTAssertNotNil(body.range(of: Data("/workspace/cloud-agent.jsonl".utf8)))
        XCTAssertNotNil(body.range(of: Data("tool-dump".utf8)))
        XCTAssertNotNil(body.range(of: Data("utf-8".utf8)))
        XCTAssertNotNil(body.range(of: bytes))
        guard case .object(let value) = result else { return XCTFail("expected write result") }
        XCTAssertEqual(value["case"], .string("success"))
        XCTAssertEqual(value["success"], .bool(true))
        XCTAssertEqual(value["path"], .string("/workspace/cloud-agent.jsonl"))
        XCTAssertEqual(value["linesCreated"], .number(1))
        XCTAssertEqual(value["fileSize"], .number(35))
        XCTAssertEqual(value["fileContentAfterWrite"], .string("after-write"))
    }

    func testRemoteExecDaemonWritePreservesTypedTerminalOutcomes() throws {
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
        func event(_ resultField: UInt64, payload: Data) throws -> IOSRemoteExecDaemonWire.Event {
            let writeResult = field(resultField, bytes: payload)
            let clientMessage = field(3, bytes: writeResult)
            return try IOSRemoteExecDaemonWire.decodeElement(field(1, bytes: clientMessage))
        }

        let permission =
            field(1, bytes: Data("/workspace/locked.txt".utf8))
            + field(2, bytes: Data("/workspace".utf8))
            + field(3, bytes: Data("write".utf8))
            + field(4, bytes: Data("read-only mount".utf8))
            + vfield(5, value: 1)
        XCTAssertEqual(
            try event(3, payload: permission),
            .write([
                "case": "permissionDenied",
                "path": "/workspace/locked.txt",
                "directory": "/workspace",
                "operation": "write",
                "error": "read-only mount",
                "isReadonly": true,
            ])
        )
        XCTAssertEqual(
            try event(4, payload: field(1, bytes: Data("/workspace/full.txt".utf8))),
            .write([
                "case": "noSpace",
                "path": "/workspace/full.txt",
            ])
        )
        XCTAssertEqual(
            try event(
                5,
                payload:
                    field(1, bytes: Data("/workspace/error.txt".utf8))
                    + field(2, bytes: Data("disk error".utf8))
            ),
            .write([
                "case": "error",
                "path": "/workspace/error.txt",
                "error": "disk error",
            ])
        )
        XCTAssertEqual(
            try event(
                6,
                payload:
                    field(1, bytes: Data("/workspace/rejected.txt".utf8))
                    + field(2, bytes: Data("policy".utf8))
            ),
            .write([
                "case": "rejected",
                "path": "/workspace/rejected.txt",
                "reason": "policy",
            ])
        )
    }

    func testRemoteExecDaemonLsUsesCanonicalBoxTreeResult() async throws {
        let descriptor = IOSCursorSandBoxConnection(
            networkToken: "",
            execDaemonAuthToken: "secret",
            execDaemonURL: "https://exec.example.test",
            vncURL: "",
            gatewayURL: "",
            gatewayToken: "",
            forkVNCBaseURL: ""
        )
        let capture = TestURLRequestCapture()
        let client = IOSRemoteExecDaemonClient(
            descriptor: descriptor,
            requestExecutor: { request in
                capture.record(request)

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

                let file = field(1, bytes: Data("task.jsonl".utf8))
                let nestedFile = field(1, bytes: Data("nested.jsonl".utf8))
                let nestedDirectory =
                    field(1, bytes: Data("/workspace/transcripts/nested".utf8))
                    + field(3, bytes: nestedFile)
                    + vfield(4, value: 1)
                let tree =
                    field(1, bytes: Data("/workspace/transcripts".utf8))
                    + field(2, bytes: nestedDirectory)
                    + field(3, bytes: file)
                    + vfield(4, value: 1)
                let success = field(1, bytes: tree)
                let lsResult = field(1, bytes: success)
                let clientMessage = field(8, bytes: lsResult)
                let element = field(1, bytes: clientMessage)
                return (
                    frame(element) + frame(Data("{}".utf8), flags: 0x02),
                    HTTPURLResponse(
                        url: request.url!,
                        statusCode: 200,
                        httpVersion: "HTTP/1.1",
                        headerFields: ["Content-Type": "application/connect+proto"]
                    )!
                )
            }
        )

        let result = try await client.dispatch(
            method: "local-exec.box.exec",
            params: .object([
                "case": .string("lsArgs"),
                "args": .object([
                    "path": .string("/workspace/transcripts"),
                    "ignore": .array([.string("*.tmp")]),
                    "toolCallId": .string("tool-ls"),
                    "sandboxPolicy": .object([
                        "type": .number(2),
                        "networkAccess": .bool(false),
                        "readBoundary": .number(2),
                        "additionalReadPaths": .array([.string("/workspace/shared")]),
                    ]),
                    "timeoutMs": .number(2_000),
                ]),
            ])
        )

        guard let body = capture.value?.httpBody else {
            return XCTFail("missing ls ExecServerMessage body")
        }
        XCTAssertNotNil(body.range(of: Data("/workspace/transcripts".utf8)))
        XCTAssertNotNil(body.range(of: Data("*.tmp".utf8)))
        XCTAssertNotNil(body.range(of: Data("tool-ls".utf8)))

        func readVarint(_ bytes: [UInt8], index: inout Int) -> UInt64? {
            var value: UInt64 = 0
            var shift: UInt64 = 0
            while index < bytes.count, shift < 64 {
                let byte = bytes[index]
                index += 1
                value |= UInt64(byte & 0x7f) << shift
                if byte & 0x80 == 0 { return value }
                shift += 7
            }
            return nil
        }
        func firstLengthDelimitedField(_ number: UInt64, in data: Data) -> Data? {
            let bytes = [UInt8](data)
            var index = 0
            while index < bytes.count {
                guard let key = readVarint(bytes, index: &index) else { return nil }
                switch key & 7 {
                case 0:
                    guard readVarint(bytes, index: &index) != nil else { return nil }
                case 2:
                    guard let rawLength = readVarint(bytes, index: &index),
                          rawLength <= UInt64(Int.max)
                    else { return nil }
                    let length = Int(rawLength)
                    guard index <= bytes.count, length <= bytes.count - index else { return nil }
                    let end = index + length
                    if key >> 3 == number { return Data(bytes[index..<end]) }
                    index = end
                default:
                    return nil
                }
            }
            return nil
        }
        func firstVarintField(_ number: UInt64, in data: Data) -> UInt64? {
            let bytes = [UInt8](data)
            var index = 0
            while index < bytes.count {
                guard let key = readVarint(bytes, index: &index) else { return nil }
                switch key & 7 {
                case 0:
                    guard let value = readVarint(bytes, index: &index) else { return nil }
                    if key >> 3 == number { return value }
                case 2:
                    guard let rawLength = readVarint(bytes, index: &index),
                          rawLength <= UInt64(Int.max)
                    else { return nil }
                    let length = Int(rawLength)
                    guard index <= bytes.count, length <= bytes.count - index else { return nil }
                    index += length
                default:
                    return nil
                }
            }
            return nil
        }
        guard let lsArgs = firstLengthDelimitedField(8, in: body) else {
            return XCTFail("missing canonical ExecServerMessage field-8 LsArgs")
        }
        let sandboxPolicy = try XCTUnwrap(firstLengthDelimitedField(4, in: lsArgs))
        XCTAssertEqual(firstVarintField(1, in: sandboxPolicy), 2)
        XCTAssertEqual(firstVarintField(2, in: sandboxPolicy), 0)
        XCTAssertEqual(firstVarintField(14, in: sandboxPolicy), 2)
        XCTAssertEqual(
            String(
                data: try XCTUnwrap(firstLengthDelimitedField(15, in: sandboxPolicy)),
                encoding: .utf8
            ),
            "/workspace/shared"
        )
        XCTAssertEqual(firstVarintField(5, in: lsArgs), 2_000)

        guard case .object(let value) = result else {
            return XCTFail("expected ls result")
        }
        XCTAssertEqual(value["case"], .string("success"))
        guard case .object(let root)? = value["directoryTreeRoot"] else {
            return XCTFail("expected directory tree root")
        }
        XCTAssertEqual(root["absPath"], .string("/workspace/transcripts"))
        XCTAssertEqual(root["childrenWereProcessed"], .bool(true))
        XCTAssertEqual(root["childrenFiles"], .array([.object(["name": .string("task.jsonl")])]))
        guard case .array(let childrenDirs)? = root["childrenDirs"],
              childrenDirs.count == 1,
              case .object(let nested) = childrenDirs[0]
        else {
            return XCTFail("expected recursively projected nested directory")
        }
        XCTAssertEqual(nested["absPath"], .string("/workspace/transcripts/nested"))
        XCTAssertEqual(nested["childrenWereProcessed"], .bool(true))
        XCTAssertEqual(nested["childrenFiles"], .array([.object(["name": .string("nested.jsonl")])]))
    }

    func testRemoteExecDaemonDeleteFetchAndGrepPreserveCanonicalTypedContracts() throws {
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
        func readVarint(_ bytes: [UInt8], index: inout Int) -> UInt64? {
            var value: UInt64 = 0
            var shift: UInt64 = 0
            while index < bytes.count, shift < 64 {
                let byte = bytes[index]
                index += 1
                value |= UInt64(byte & 0x7f) << shift
                if byte & 0x80 == 0 { return value }
                shift += 7
            }
            return nil
        }
        func firstLengthDelimitedField(_ number: UInt64, in data: Data) -> Data? {
            let bytes = [UInt8](data)
            var index = 0
            while index < bytes.count {
                guard let key = readVarint(bytes, index: &index) else { return nil }
                switch key & 7 {
                case 0:
                    guard readVarint(bytes, index: &index) != nil else { return nil }
                case 2:
                    guard let rawLength = readVarint(bytes, index: &index),
                          rawLength <= UInt64(Int.max)
                    else { return nil }
                    let length = Int(rawLength)
                    guard index <= bytes.count, length <= bytes.count - index else { return nil }
                    let end = index + length
                    if key >> 3 == number { return Data(bytes[index..<end]) }
                    index = end
                default:
                    return nil
                }
            }
            return nil
        }
        func firstVarintField(_ number: UInt64, in data: Data) -> UInt64? {
            let bytes = [UInt8](data)
            var index = 0
            while index < bytes.count {
                guard let key = readVarint(bytes, index: &index) else { return nil }
                switch key & 7 {
                case 0:
                    guard let value = readVarint(bytes, index: &index) else { return nil }
                    if key >> 3 == number { return value }
                case 2:
                    guard let rawLength = readVarint(bytes, index: &index),
                          rawLength <= UInt64(Int.max)
                    else { return nil }
                    let length = Int(rawLength)
                    guard index <= bytes.count, length <= bytes.count - index else { return nil }
                    index += length
                default:
                    return nil
                }
            }
            return nil
        }

        let deleteRequest = try IOSRemoteExecDaemonWire.deleteRequest(
            id: 7,
            execID: "delete-exec",
            path: "/workspace/old.txt",
            toolCallID: "tool-delete"
        )
        let deleteArgs = try XCTUnwrap(firstLengthDelimitedField(4, in: deleteRequest))
        XCTAssertEqual(
            String(data: try XCTUnwrap(firstLengthDelimitedField(1, in: deleteArgs)), encoding: .utf8),
            "/workspace/old.txt"
        )
        XCTAssertEqual(
            String(data: try XCTUnwrap(firstLengthDelimitedField(2, in: deleteArgs)), encoding: .utf8),
            "tool-delete"
        )

        let fetchRequest = try IOSRemoteExecDaemonWire.fetchRequest(
            id: 8,
            execID: "fetch-exec",
            url: "https://example.test/readme",
            toolCallID: "tool-fetch"
        )
        let fetchArgs = try XCTUnwrap(firstLengthDelimitedField(20, in: fetchRequest))
        XCTAssertEqual(
            String(data: try XCTUnwrap(firstLengthDelimitedField(1, in: fetchArgs)), encoding: .utf8),
            "https://example.test/readme"
        )
        XCTAssertEqual(
            String(data: try XCTUnwrap(firstLengthDelimitedField(2, in: fetchArgs)), encoding: .utf8),
            "tool-fetch"
        )

        let grepRequest = try IOSRemoteExecDaemonWire.grepRequest(
            id: 9,
            execID: "grep-exec",
            pattern: "TODO",
            path: "/workspace",
            glob: "*.swift",
            outputMode: "files",
            contextBefore: 2,
            contextAfter: 3,
            context: nil,
            caseInsensitive: true,
            type: "swift",
            headLimit: 20,
            multiline: false,
            sort: "path",
            sortAscending: true,
            toolCallID: "tool-grep",
            sandboxPolicy: nil,
            offset: 1
        )
        let grepArgs = try XCTUnwrap(firstLengthDelimitedField(5, in: grepRequest))
        XCTAssertEqual(
            String(data: try XCTUnwrap(firstLengthDelimitedField(1, in: grepArgs)), encoding: .utf8),
            "TODO"
        )
        XCTAssertEqual(
            String(data: try XCTUnwrap(firstLengthDelimitedField(2, in: grepArgs)), encoding: .utf8),
            "/workspace"
        )
        XCTAssertEqual(firstVarintField(5, in: grepArgs), 2)
        XCTAssertEqual(firstVarintField(6, in: grepArgs), 3)
        XCTAssertEqual(firstVarintField(8, in: grepArgs), 1)
        XCTAssertEqual(firstVarintField(10, in: grepArgs), 20)
        XCTAssertEqual(firstVarintField(13, in: grepArgs), 1)
        XCTAssertEqual(firstVarintField(16, in: grepArgs), 1)

        let deleteSuccess =
            field(1, bytes: Data("/workspace/old.txt".utf8))
            + field(2, bytes: Data("old.txt".utf8))
            + vfield(3, value: 3)
            + field(4, bytes: Data("old".utf8))
        XCTAssertEqual(
            try IOSRemoteExecDaemonWire.decodeElement(
                field(1, bytes: field(4, bytes: field(1, bytes: deleteSuccess)))
            ),
            .delete([
                "case": "success",
                "path": "/workspace/old.txt",
                "deletedFile": "old.txt",
                "fileSize": Int64(3),
                "prevContent": "old",
            ])
        )

        let fetchSuccess =
            field(1, bytes: Data("https://example.test/readme".utf8))
            + field(2, bytes: Data("hello".utf8))
            + vfield(3, value: 200)
            + field(4, bytes: Data("text/plain".utf8))
        XCTAssertEqual(
            try IOSRemoteExecDaemonWire.decodeElement(
                field(1, bytes: field(20, bytes: field(1, bytes: fetchSuccess)))
            ),
            .fetch([
                "case": "success",
                "url": "https://example.test/readme",
                "content": "hello",
                "statusCode": 200,
                "contentType": "text/plain",
            ])
        )

        let filesResult =
            field(1, bytes: Data("/workspace/App.swift".utf8))
            + vfield(2, value: 1)
            + vfield(3, value: 0)
            + vfield(4, value: 0)
            + vfield(5, value: 20)
            + vfield(6, value: 1)
        let union = field(2, bytes: filesResult)
        let mapEntry =
            field(1, bytes: Data("workspace".utf8))
            + field(2, bytes: union)
        let grepSuccess =
            field(1, bytes: Data("TODO".utf8))
            + field(2, bytes: Data("/workspace".utf8))
            + field(3, bytes: Data("files".utf8))
            + field(4, bytes: mapEntry)
        let grepEvent = try IOSRemoteExecDaemonWire.decodeElement(
            field(1, bytes: field(5, bytes: field(1, bytes: grepSuccess)))
        )
        guard case .grep(let grepJSON) = grepEvent,
              let grepData = grepJSON.data(using: .utf8),
              let grepObject = try JSONSerialization.jsonObject(with: grepData) as? [String: Any]
        else {
            return XCTFail("expected canonical grep success projection")
        }
        XCTAssertEqual(grepObject["case"] as? String, "success")
        XCTAssertEqual(grepObject["pattern"] as? String, "TODO")
        XCTAssertEqual(grepObject["path"] as? String, "/workspace")
        XCTAssertEqual(grepObject["outputMode"] as? String, "files")
        let workspaceResults = try XCTUnwrap(grepObject["workspaceResults"] as? [String: Any])
        let workspace = try XCTUnwrap(workspaceResults["workspace"] as? [String: Any])
        XCTAssertEqual(workspace["case"] as? String, "files")
        XCTAssertEqual(workspace["files"] as? [String], ["/workspace/App.swift"])
        XCTAssertEqual(workspace["totalFiles"] as? Int, 1)
        XCTAssertEqual(workspace["headLimitApplied"] as? Int, 20)
        XCTAssertEqual(workspace["offsetApplied"] as? Int, 1)
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

        let processCapture = TestURLRequestCapture()
        let processClient = IOSRemoteExecDaemonClient(
            descriptor: descriptor,
            requestExecutor: { request in
                processCapture.record(request)

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
        guard let processBody = processCapture.value?.httpBody else {
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
        let capture = TestURLRequestCapture()
        let client = IOSRemoteExecDaemonClient(
            descriptor: descriptor,
            requestExecutor: { request in
                capture.record(request)
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
                "driverPath": .string("/workspace/.sand-browser/driver-v2.mjs"),
                "driverDirectory": .string("/workspace/.sand-browser"),
                "driverSource": .string(driver),
                "resultMarker": .string("__SAND_BROWSER_RESULT__"),
                "request": .object([
                    "op": .string("refresh"),
                    "display": .number(0),
                    "cdpPort": .number(9222),
                    "viewId": .string("main"),
                    "expectedGeneration": .string("generation-7"),
                    "screenshotPath": .string("/workspace/.sand-browser/shot-browser-call-1.png"),
                ]),
                "screenshotPath": .string("/workspace/.sand-browser/shot-browser-call-1.png"),
                "toolCallId": .string("browser-call-1"),
            ])
        )
        guard let body = capture.value?.httpBody else {
            return XCTFail("missing browser ExecServerMessage body")
        }
        XCTAssertEqual(capture.value?.timeoutInterval, 120)
        XCTAssertNotNil(body.range(of: Data("/workspace/.sand-browser/driver-v2.mjs".utf8)))
        XCTAssertNotNil(body.range(of: Data("chmod 700 /workspace/.sand-browser".utf8)))
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
        XCTAssertTrue(mcp.contains("return try await self.dashboard.ensureSandBox()"))
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
