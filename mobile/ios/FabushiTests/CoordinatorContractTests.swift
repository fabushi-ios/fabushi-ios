import XCTest
@testable import Fabushi

private func validatedTestCoordinatorBootstrap() throws -> ValidatedCoordinatorBootstrap {
    try CoordinatorBootstrap(
        processConfig: .init(
            appVersion: "1.0-test",
            isPackaged: false,
            dataDir: "/tmp/fabushi-coordinator-tests",
            localHumanId: "human-test"
        )
    ).validatedForCarrier()
}

@MainActor
private final class TestCoordinatorPort: CoordinatorPort {
    var frames: [CoordinatorFrame] = []
    var closed = false

    func post(_ frame: CoordinatorFrame) {
        frames.append(frame)
    }

    func close() {
        closed = true
    }
}

final class CoordinatorContractTests: XCTestCase {
    func testCurrentMainCoordinatorMethodRegistriesIncludeCanonicalMethods() {
        for method in [
            "fetchLinkMetadata",
            "reportConnectorAuth",
            "reportMcpDiscoveryFailed",
            "loadBoxMcpServers",
            "listBoxMcpToolsRaw",
            "executeBoxMcpToolRaw",
        ] {
            XCTAssertTrue(CoordinatorMainMethodRegistry.contains(method), "missing main method \(method)")
        }
        XCTAssertTrue(CoordinatorMethodRegistry.contains("interruptAgent"))
    }

    func testCoordinatorFrameRoundTripsReferenceWireShape() throws {
        let frame = CoordinatorFrame.request(
            requestId: "r-1",
            method: "sendPrompt",
            args: .object(["text": .string("hello"), "stream": .bool(true)])
        )
        let data = try JSONEncoder().encode(frame)
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        XCTAssertEqual(object["kind"] as? String, "request")
        XCTAssertEqual(object["requestId"] as? String, "r-1")
        XCTAssertEqual(object["method"] as? String, "sendPrompt")
        XCTAssertEqual(try JSONDecoder().decode(CoordinatorFrame.self, from: data), frame)
    }

    func testMalformedRequestWithoutArgsIsRejected() throws {
        let data = #"{"kind":"request","requestId":"r-1","method":"sendPrompt"}"#.data(using: .utf8)!
        XCTAssertThrowsError(try JSONDecoder().decode(CoordinatorFrame.self, from: data))
    }

    @MainActor
    func testRendererPortRequiresHelloBeforeRequest() {
        let port = TestCoordinatorPort()
        let server = RendererPortServer(port: port) { _, _ in .ok(.null) }

        server.receive(.request(requestId: "r-1", method: "sendPrompt", args: .object([:])))

        XCTAssertEqual(server.phase, .settled)
        XCTAssertTrue(port.closed)
        guard case .shutdown(reason: .protocolError, detail: let detail)? = port.frames.last else {
            return XCTFail("expected protocol-error shutdown")
        }
        XCTAssertTrue(detail?.contains("before hello") == true)
    }

    @MainActor
    func testRendererPortHandshakeAndCancellationSettleDeterministically() async {
        let port = TestCoordinatorPort()
        let server = RendererPortServer(port: port) { _, _ in
            try? await Task.sleep(nanoseconds: 2_000_000_000)
            return .ok(.string("late"))
        }

        server.receive(.hello(protocolVersion: CoordinatorProtocol.version))
        XCTAssertEqual(port.frames.first, .ready(protocolVersion: CoordinatorProtocol.version))

        server.receive(.request(requestId: "r-2", method: "sendPrompt", args: .object([:])))
        server.receive(.cancel(requestId: "r-2"))

        XCTAssertTrue(port.frames.contains(.reply(
            requestId: "r-2",
            outcome: .failed(.init(code: CoordinatorProtocol.cancelled, message: "request cancelled"))
        )))
        XCTAssertEqual(server.phase, .serving)
    }

    @MainActor
    func testInProcessCarrierProvidesHandshakeAndRequestReply() async throws {
        let pair = InProcessCoordinatorPort.makePair(bootstrap: try validatedTestCoordinatorBootstrap())
        let server = RendererPortServer(port: pair.server) { method, args in
            .ok(.object(["method": .string(method), "args": args]))
        }
        let client = CoordinatorControlPortClient(port: pair.client, autoStart: false)

        pair.server.onFrame = { [weak server] frame in server?.receive(frame) }
        pair.server.onClose = { [weak server] in server?.portClosed() }
        pair.client.onFrame = { [weak client] frame in client?.receive(frame) }
        pair.client.onClose = { [weak client] in client?.portClosed() }

        client.start()
        XCTAssertTrue(client.readyObserved)

        let response = try await client.call(
            method: "sendPrompt",
            args: .object(["text": .string("hello")])
        )
        XCTAssertEqual(
            response,
            .object([
                "method": .string("sendPrompt"),
                "args": .object(["text": .string("hello")]),
            ])
        )
    }

    @MainActor
    func testControlPortRejectsPendingCallWithDesktopDisconnectOnPortClose() async {
        let port = TestCoordinatorPort()
        let client = CoordinatorControlPortClient(port: port, autoStart: false)
        client.start()
        client.receive(.ready(protocolVersion: CoordinatorProtocol.version))

        let pending = Task { @MainActor in
            try await client.call(method: "pending", args: .object([:]))
        }
        await Task.yield()
        client.portClosed()

        do {
            _ = try await pending.value
            XCTFail("pending control call must fail when the port closes")
        } catch let error as ControlPortCallError {
            XCTAssertEqual(error, .init(code: CoordinatorProtocol.disconnected, message: "control port closed"))
        } catch {
            XCTFail("unexpected error: \(error)")
        }
        XCTAssertEqual(client.settlement, .portClosed)
        XCTAssertTrue(port.closed)
    }

    @MainActor
    func testControlPortProtocolBreachPreservesDisconnectCauseForPendingCall() async {
        let port = TestCoordinatorPort()
        let client = CoordinatorControlPortClient(port: port, autoStart: false)
        client.start()
        client.receive(.ready(protocolVersion: CoordinatorProtocol.version))

        let pending = Task { @MainActor in
            try await client.call(method: "pending", args: .object([:]))
        }
        await Task.yield()
        client.receive(.request(requestId: "server-r-1", method: "illegal", args: .object([:])))

        do {
            _ = try await pending.value
            XCTFail("pending control call must fail on protocol breach")
        } catch let error as ControlPortCallError {
            XCTAssertEqual(
                error,
                .init(
                    code: CoordinatorProtocol.disconnected,
                    message: "main posted a client-direction request frame"
                )
            )
        } catch {
            XCTFail("unexpected error: \(error)")
        }
        XCTAssertEqual(client.settlement, .protocolBreach("main posted a client-direction request frame"))
        XCTAssertTrue(port.frames.contains(.shutdown(
            reason: .protocolError,
            detail: "main posted a client-direction request frame"
        )))
    }

    @MainActor
    func testControlPortPeerShutdownUsesDetailAndLocalShutdownUsesRequestedCause() async {
        let peerPort = TestCoordinatorPort()
        let peerClient = CoordinatorControlPortClient(port: peerPort, autoStart: false)
        peerClient.start()
        peerClient.receive(.ready(protocolVersion: CoordinatorProtocol.version))
        let peerPending = Task { @MainActor in
            try await peerClient.call(method: "pending-peer", args: .object([:]))
        }
        await Task.yield()
        peerClient.receive(.shutdown(reason: .protocolError, detail: "peer protocol failure"))
        do {
            _ = try await peerPending.value
            XCTFail("peer shutdown must reject pending calls")
        } catch let error as ControlPortCallError {
            XCTAssertEqual(error, .init(code: CoordinatorProtocol.disconnected, message: "peer protocol failure"))
        } catch {
            XCTFail("unexpected error: \(error)")
        }

        let localPort = TestCoordinatorPort()
        let localClient = CoordinatorControlPortClient(port: localPort, autoStart: false)
        localClient.start()
        localClient.receive(.ready(protocolVersion: CoordinatorProtocol.version))
        let localPending = Task { @MainActor in
            try await localClient.call(method: "pending-local", args: .object([:]))
        }
        await Task.yield()
        localClient.shutdown()
        do {
            _ = try await localPending.value
            XCTFail("local shutdown must reject pending calls")
        } catch let error as ControlPortCallError {
            XCTAssertEqual(error, .init(code: CoordinatorProtocol.disconnected, message: "shutdown requested"))
        } catch {
            XCTFail("unexpected error: \(error)")
        }
        XCTAssertTrue(localPort.frames.contains(.shutdown(reason: .requested, detail: nil)))
    }

    @MainActor
    func testIOSCoordinatorRestartReplacesPortAndFencesStaleGeneration() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        defer { try? FileManager.default.removeItem(at: directory) }

        let main = try IOSMainRuntime(
            appDataDirectory: directory,
            featureHostTest: true
        )
        let runtime = IOSCoordinatorRuntime(main: main)

        XCTAssertThrowsError(try runtime.start()) { error in
            XCTAssertEqual(error as? CoordinatorCarrierError, .missingLocalHumanIdentity)
        }

        main.applySettledHostLocalHumanIdentity("human-1")
        let first = try runtime.start()
        XCTAssertEqual(first.clientPort.bootstrap.value.processConfig.localHumanId, "human-1")
        XCTAssertEqual(runtime.state, .running(generation: 1))
        XCTAssertTrue(runtime.accepts(generation: 1))

        try runtime.restart()
        let second = try runtime.start()

        XCTAssertFalse(first === second)
        XCTAssertTrue(first.clientPort.isClosed)
        XCTAssertEqual(second.clientPort.bootstrap.value.processConfig.localHumanId, "human-1")
        XCTAssertEqual(runtime.state, .running(generation: 2))
        XCTAssertFalse(runtime.accepts(generation: 1))
        XCTAssertTrue(runtime.accepts(generation: 2))

        main.applySettledHostLocalHumanIdentity(nil)
        XCTAssertThrowsError(try runtime.restart()) { error in
            XCTAssertEqual(error as? CoordinatorCarrierError, .missingLocalHumanIdentity)
        }
        XCTAssertEqual(runtime.state, .stopped)
        XCTAssertTrue(second.clientPort.isClosed)

        main.applySettledHostLocalHumanIdentity("human-2")
        let replacement = try runtime.start()
        XCTAssertEqual(replacement.clientPort.bootstrap.value.processConfig.localHumanId, "human-2")
        XCTAssertEqual(runtime.state, .running(generation: 4))

        runtime.dispose()
        XCTAssertEqual(runtime.state, .disposed)
        XCTAssertTrue(replacement.clientPort.isClosed)
    }

    @MainActor
    func testCoordinatorAccountTransitionClearsIdentityBeforeReplacementAuthorization() async {
        var events: [String] = []
        var localHumanId: String? = "human-1"
        let cleanup = ProductionAccountTransitionCleanup(
            dependencies: .init(
                clearAccountScope: {
                    localHumanId = nil
                    events.append("clear")
                },
                didClearAccountScope: { previous, next in
                    events.append("cleared:\(previous)->\(next ?? "nil")")
                }
            )
        )
        let runtime = CoordinatorAccountRuntime(
            activeSlot: "human-1",
            cleanup: cleanup,
            authorize: { slot, _ in
                XCTAssertNil(localHumanId)
                events.append("authorize:\(slot ?? "nil")")
                localHumanId = slot
                return .ready(slot: slot)
            }
        )

        let result = await runtime.transition(to: "human-2")

        XCTAssertEqual(result, .ready(slot: "human-2"))
        XCTAssertEqual(localHumanId, "human-2")
        XCTAssertEqual(
            events,
            ["clear", "cleared:human-1->human-2", "authorize:human-2"]
        )
    }

    func testCoordinatorBootstrapCodablePreservesLocalHumanIdentity() throws {
        let bootstrap = CoordinatorBootstrap(
            processConfig: .init(
                appVersion: "1.0",
                isPackaged: true,
                dataDir: "/tmp/fabushi",
                localHumanId: "human-42"
            )
        )
        let encoded = try JSONEncoder().encode(bootstrap)
        let decoded = try JSONDecoder().decode(CoordinatorBootstrap.self, from: encoded)
        XCTAssertEqual(decoded.processConfig.localHumanId, "human-42")
        XCTAssertNoThrow(try decoded.validatedForCarrier().requiringLocalHumanIdentity())

        let unscoped = CoordinatorBootstrap(
            processConfig: .init(
                appVersion: "1.0",
                isPackaged: true,
                dataDir: "/tmp/fabushi"
            )
        )
        XCTAssertThrowsError(
            try unscoped.validatedForCarrier().requiringLocalHumanIdentity()
        ) { error in
            XCTAssertEqual(error as? CoordinatorCarrierError, .missingLocalHumanIdentity)
        }
    }

    @MainActor
    func testIOSPreloadPortClientUsesRendererCoordinatorBoundary() async throws {
        let pair = InProcessCoordinatorPort.makePair(bootstrap: try validatedTestCoordinatorBootstrap())
        let server = RendererPortServer(port: pair.server) { method, args in
            .ok(.object([
                "method": .string(method),
                "args": args,
            ]))
        }

        pair.server.onFrame = { [weak server] frame in
            server?.receive(frame)
        }
        pair.server.onClose = { [weak server] in
            server?.portClosed()
        }

        let client = IOSCoordinatorPortClient(port: pair.client)
        XCTAssertTrue(client.readyObserved)

        let response = try await client.request(
            method: "feature.auth.status",
            args: .object(["refresh": .bool(true)])
        )
        XCTAssertEqual(
            response,
            .object([
                "method": .string("feature.auth.status"),
                "args": .object(["refresh": .bool(true)]),
            ])
        )

        client.shutdown()
        XCTAssertEqual(client.settlement, .shutdownRequested)
    }


    @MainActor
    func testCarrierValidatesBootstrapAndPreservesChannelOrdering() throws {
        XCTAssertThrowsError(try CoordinatorBootstrap(
            processConfig: .init(appVersion: "   ", isPackaged: false, dataDir: "/tmp/fabushi")
        ).validatedForCarrier()) { error in
            XCTAssertEqual(error as? CoordinatorCarrierError, .emptyAppVersion)
        }
        XCTAssertThrowsError(try CoordinatorBootstrap(
            processConfig: .init(appVersion: "1.0", isPackaged: false, dataDir: "")
        ).validatedForCarrier()) { error in
            XCTAssertEqual(error as? CoordinatorCarrierError, .emptyDataDirectory)
        }

        let pair = InProcessCoordinatorPort.makePair(bootstrap: try validatedTestCoordinatorBootstrap())
        var observed: [String] = []
        pair.server.onFrame = { _ in observed.append("control") }

        try pair.client.post(.event(family: "data", payload: .null), on: .data)
        pair.client.post(.event(family: "control", payload: .null))
        XCTAssertEqual(observed, [])
        XCTAssertEqual(pair.server.pendingMessageCount, 2)

        pair.server.onDataFrame = { _ in observed.append("data") }
        XCTAssertEqual(observed, ["data", "control"])
        XCTAssertEqual(pair.server.pendingMessageCount, 0)

        try pair.client.post(.event(family: "main", payload: .null), on: .mainData)
        XCTAssertEqual(pair.server.pendingMessageCount, 1)
        pair.server.onMainDataFrame = { _ in observed.append("main") }
        XCTAssertEqual(observed, ["data", "control", "main"])
    }

    @MainActor
    func testCarrierRejectsUnknownChannelAndPostsAfterClose() throws {
        let pair = InProcessCoordinatorPort.makePair(bootstrap: try validatedTestCoordinatorBootstrap())
        XCTAssertThrowsError(try pair.server.acceptEnvelope(.init(
            wireChannel: "coordinator-unknown",
            frame: .event(family: "x", payload: .null)
        ))) { error in
            XCTAssertEqual(error as? CoordinatorCarrierError, .unknownChannel("coordinator-unknown"))
        }

        try pair.client.post(.event(family: "queued", payload: .null), on: .data)
        XCTAssertEqual(pair.server.pendingMessageCount, 1)
        pair.server.close()
        XCTAssertEqual(pair.server.pendingMessageCount, 0)
        XCTAssertThrowsError(try pair.server.post(.event(family: "late", payload: .null), on: .control)) { error in
            XCTAssertEqual(error as? CoordinatorCarrierError, .closed)
        }
    }

    func testSSEDecoderPreservesEventDataBoundaries() {
        var decoder = SSEBlockDecoder()
        XCTAssertEqual(decoder.append("event: transcript\nid: 7\ndata: one\n"), [])
        XCTAssertEqual(
            decoder.append("data: two\n\n"),
            [.init(event: "transcript", id: "7", data: "one\ntwo")]
        )
    }

    @MainActor
    func testClientPauseControlDropsObservedConnectionOnceAndSerializesCoordinatorState() async throws {
        var paused = true
        var dropped = 0
        var coordinatorStates: [Bool] = []
        let control = IOSCoordinatorClientPauseControl(
            isPaused: { paused },
            applyCoordinatorPause: { value in
                coordinatorStates.append(value)
                return value
            },
            dropObservedConnection: {
                dropped += 1
            }
        )

        try await control.synchronize()
        try await control.synchronize()
        XCTAssertTrue(control.isPaused)
        XCTAssertEqual(dropped, 1)
        XCTAssertEqual(coordinatorStates, [true])

        paused = false
        try await control.synchronize()
        XCTAssertFalse(control.isPaused)
        XCTAssertEqual(dropped, 1)
        XCTAssertEqual(coordinatorStates, [true, false])

        paused = true
        try await control.synchronize()
        XCTAssertEqual(dropped, 2)
        XCTAssertEqual(coordinatorStates, [true, false, true])
    }

    @MainActor
    func testClientPauseControlReappliesPauseAfterCoordinatorRelaunch() async throws {
        var coordinatorStates: [Bool] = []
        let control = IOSCoordinatorClientPauseControl(
            isPaused: { true },
            applyCoordinatorPause: { value in
                coordinatorStates.append(value)
                return value
            },
            dropObservedConnection: {}
        )

        try await control.synchronize()
        control.reapplyAfterCoordinatorLaunch()
        try await control.synchronize()

        XCTAssertEqual(coordinatorStates, [true, true])
    }

    func testClientPauseErrorMatchesCanonicalDesktopBlockedMarker() {
        XCTAssertEqual(IOS_SAND_CLIENT_PAUSE_GATE, "sand_client_pause")
        XCTAssertEqual(
            IOSClientPausedError().localizedDescription,
            "sand box blocked by kill switch: SAND_CLIENT_PAUSE\u{001F}\u{001F}"
        )
    }

}
