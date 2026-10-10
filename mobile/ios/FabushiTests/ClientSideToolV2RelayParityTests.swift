import XCTest
@testable import Fabushi

@MainActor
private final class ClientSideToolRelayTestHost: MahayanaHostRequesting {
    var nextValue: Any = NSNull()

    func request(method: String, params: [String: Any]) async throws -> MahayanaHostJSONResult {
        MahayanaHostJSONResult(value: nextValue)
    }
}

final class ClientSideToolV2RelayParityTests: XCTestCase {
    private func protoVarint(_ value: UInt64) -> [UInt8] {
        var value = value
        var output = [UInt8]()
        repeat {
            var byte = UInt8(value & 0x7f)
            value >>= 7
            if value != 0 { byte |= 0x80 }
            output.append(byte)
        } while value != 0
        return output
    }

    private func protoField(_ field: Int, varint: UInt64) -> [UInt8] {
        protoVarint(UInt64(field << 3)) + protoVarint(varint)
    }

    private func protoField(_ field: Int, string: String) -> [UInt8] {
        protoField(field, bytes: Array(string.utf8))
    }

    private func protoField(_ field: Int, bytes: [UInt8]) -> [UInt8] {
        protoVarint(UInt64((field << 3) | 2)) + protoVarint(UInt64(bytes.count)) + bytes
    }

    private func rendererEvent(
        kind: String,
        agentId: String = "agent-tool",
        sequence: Int,
        messageType: String? = nil,
        bytes: [UInt8]? = nil
    ) -> MobileToolResultRendererEvent {
        var object: [String: Any] = [
            "version": 1,
            "kind": kind,
            "accountSlot": "host",
            "agentId": agentId,
            "epoch": "epoch-tool",
            "sequence": sequence,
        ]
        object["messageType"] = messageType ?? NSNull()
        object["bytes"] = bytes?.map(Int.init) ?? NSNull()
        return MobileToolResultRendererEvent.fromFoundation(object)!
    }

    private func callMessage(_ callID: String) -> ClientSideToolV2WireMessage {
        let id = Array(callID.utf8)
        return .init(
            messageType: "aiserver.v1.ClientSideToolV2Call",
            bytes: Data([0x1a, UInt8(id.count)] + id)
        )
    }

    private func resultMessage(_ callID: String) -> ClientSideToolV2WireMessage {
        let id = Array(callID.utf8)
        return .init(
            messageType: "aiserver.v1.ClientSideToolV2Result",
            bytes: Data([0x9a, 0x02, UInt8(id.count)] + id)
        )
    }

    func testTransportParserRejectsMalformedIdentityVersionSequenceAndWireShape() {
        let validBytes = callMessage("call-parse").bytes
        let base: [String: Any] = [
            "version": 1,
            "kind": "call",
            "accountSlot": "host",
            "agentId": "agent-parse",
            "epoch": "epoch-parse",
            "sequence": 1,
            "message": [
                "encoding": "protobuf-base64",
                "messageType": "aiserver.v1.ClientSideToolV2Call",
                "bytes": validBytes,
            ],
        ]
        XCTAssertNotNil(ClientSideToolV2TransportEvent.fromFoundation(base))

        var badVersion = base
        badVersion["version"] = 2
        XCTAssertNil(ClientSideToolV2TransportEvent.fromFoundation(badVersion))

        var emptyAgent = base
        emptyAgent["agentId"] = ""
        XCTAssertNil(ClientSideToolV2TransportEvent.fromFoundation(emptyAgent))

        var zeroSequence = base
        zeroSequence["sequence"] = 0
        XCTAssertNil(ClientSideToolV2TransportEvent.fromFoundation(zeroSequence))

        var unsafeSequence = base
        unsafeSequence["sequence"] = 9_007_199_254_740_992.0
        XCTAssertNil(ClientSideToolV2TransportEvent.fromFoundation(unsafeSequence))

        var wrongMessageType = base
        wrongMessageType["message"] = [
            "encoding": "protobuf-base64",
            "messageType": "aiserver.v1.ClientSideToolV2Result",
            "bytes": validBytes,
        ]
        XCTAssertNil(ClientSideToolV2TransportEvent.fromFoundation(wrongMessageType))

        var nonCanonicalBase64 = base
        nonCanonicalBase64["message"] = [
            "encoding": "protobuf-base64",
            "messageType": "aiserver.v1.ClientSideToolV2Call",
            "bytes": validBytes + "\n",
        ]
        XCTAssertNil(ClientSideToolV2TransportEvent.fromFoundation(nonCanonicalBase64))
    }

    @MainActor
    func testRelayFencesEpochsSequencesSettlesAndReplays() {
        let relay = ClientSideToolV2Relay()
        let call = ClientSideToolV2TransportEvent.update(
            version: 1,
            kind: .call,
            accountSlot: "host",
            agentId: "agent-1",
            epoch: "epoch-a",
            sequence: 1,
            message: callMessage("call-7")
        )
        XCTAssertEqual(relay.accept(call)?.bytes, Data([0x1a, 0x06] + Array("call-7".utf8)))
        XCTAssertNil(relay.accept(call))

        let result = ClientSideToolV2TransportEvent.update(
            version: 1,
            kind: .result,
            accountSlot: "host",
            agentId: "agent-1",
            epoch: "epoch-a",
            sequence: 2,
            message: resultMessage("call-7")
        )
        XCTAssertNotNil(relay.accept(result))
        XCTAssertEqual(relay.replay().map(\.kind), ["call", "result"])

        let nextEpoch = ClientSideToolV2TransportEvent.update(
            version: 1,
            kind: .call,
            accountSlot: "host",
            agentId: "agent-1",
            epoch: "epoch-b",
            sequence: 1,
            message: callMessage("call-8")
        )
        XCTAssertNotNil(relay.accept(nextEpoch))
        XCTAssertNil(relay.accept(.update(
            version: 1,
            kind: .result,
            accountSlot: "host",
            agentId: "agent-1",
            epoch: "epoch-a",
            sequence: 3,
            message: resultMessage("call-7")
        )))
        XCTAssertEqual(relay.replay().map(\.kind), ["call"])

        XCTAssertNotNil(relay.accept(.reset(
            version: 1,
            accountSlot: "host",
            agentId: "agent-1",
            epoch: "epoch-b",
            sequence: 2
        )))
        XCTAssertTrue(relay.replay().isEmpty)
    }

    @MainActor
    func testRelayRejectsInvalidWireAndOrphanResult() {
        let relay = ClientSideToolV2Relay()
        XCTAssertNil(relay.accept(.update(
            version: 2,
            kind: .call,
            accountSlot: "host",
            agentId: "agent-1",
            epoch: "epoch-a",
            sequence: 1,
            message: callMessage("call-1")
        )))
        XCTAssertNil(relay.accept(.update(
            version: 1,
            kind: .call,
            accountSlot: "other",
            agentId: "agent-1",
            epoch: "epoch-a",
            sequence: 1,
            message: callMessage("call-1")
        )))
        XCTAssertNil(relay.accept(.update(
            version: 1,
            kind: .result,
            accountSlot: "host",
            agentId: "agent-1",
            epoch: "epoch-a",
            sequence: 1,
            message: resultMessage("missing")
        )))
    }

    @MainActor
    func testCoordinatorProductionReceiveOwnsRelayProjection() async {
        let host = ClientSideToolRelayTestHost()
        let supervisor = MahayanaLocalHostSupervisor(host: host, factory: { host })
        let coordinator = MahayanaCoordinator(hostSupervisor: supervisor)
        var projected: [(String, CoordinatorPayload)] = []
        coordinator.setRendererEventSink { family, payload in projected.append((family, payload)) }

        let callID = Array("call-9".utf8)
        host.nextValue = [
            "channel": ClientSideToolV2Transport.family,
            "payload": [
                "version": 1,
                "kind": "call",
                "accountSlot": "host",
                "agentId": "agent-9",
                "epoch": "epoch-9",
                "sequence": 1,
                "message": [
                    "encoding": "protobuf-base64",
                    "messageType": "aiserver.v1.ClientSideToolV2Call",
                    "bytes": Data([0x1a, UInt8(callID.count)] + callID).base64EncodedString(),
                ],
            ],
        ]

        _ = await coordinator.dispatchTransport(method: "feature.receive", args: .object([:]))
        XCTAssertEqual(projected.count, 1)
        XCTAssertEqual(projected.first?.0, ClientSideToolV2Transport.family)
        guard case .object(let payload)? = projected.first?.1 else {
            return XCTFail("Coordinator must project a typed renderer event")
        }
        XCTAssertEqual(payload["agentId"], .string("agent-9"))
        XCTAssertEqual(payload["kind"], .string("call"))

        coordinator.beginShutdown()
        var replayedAfterShutdown = 0
        coordinator.setRendererEventSink { _, _ in replayedAfterShutdown += 1 }
        XCTAssertEqual(replayedAfterShutdown, 0)
    }

    @MainActor
    func testToolResultProjectionMergesAuthoritativeShellAndEditProtobuf() {
        let store = MobileToolResultStore()

        let terminalParams = protoField(1, string: "printf hi")
            + protoField(2, string: "/repo")
            + protoField(5, varint: 0)
        let shellCall = protoField(1, varint: 15)
            + protoField(3, string: "shell-1")
            + protoField(23, bytes: terminalParams)
        XCTAssertTrue(store.consume(rendererEvent(
            kind: "call",
            sequence: 1,
            messageType: "aiserver.v1.ClientSideToolV2Call",
            bytes: shellCall
        )))
        XCTAssertEqual(store.cardsByAgent["agent-tool"]?.first?.status, .running)
        XCTAssertEqual(store.cardsByAgent["agent-tool"]?.first?.command, "printf hi")
        XCTAssertEqual(store.cardsByAgent["agent-tool"]?.first?.workingDirectory, "/repo")

        let terminalResult = protoField(1, string: "hi")
            + protoField(7, string: "/repo/next")
            + protoField(9, varint: 1)
            + protoField(12, string: "hi")
        let shellResult = protoField(1, varint: 15)
            + protoField(24, bytes: terminalResult)
            + protoField(35, string: "shell-1")
        XCTAssertTrue(store.consume(rendererEvent(
            kind: "result",
            sequence: 2,
            messageType: "aiserver.v1.ClientSideToolV2Result",
            bytes: shellResult
        )))
        let shell = store.cardsByAgent["agent-tool"]?.first
        XCTAssertEqual(shell?.status, .success)
        XCTAssertEqual(shell?.output, "hi")
        XCTAssertEqual(shell?.workingDirectory, "/repo/next")
        XCTAssertFalse(shell?.isStreaming ?? true)

        let editParams = protoField(1, string: "Sources/App.swift")
        let editCall = protoField(1, varint: 7)
            + protoField(3, string: "edit-1")
            + protoField(13, bytes: editParams)
        XCTAssertTrue(store.consume(rendererEvent(
            kind: "call",
            sequence: 3,
            messageType: "aiserver.v1.ClientSideToolV2Call",
            bytes: editCall
        )))
        let chunk = protoField(1, string: "@@ -1 +1 @@\n-old\n+new\n")
        let diff = protoField(1, bytes: chunk)
        let editResultPayload = protoField(1, bytes: diff)
            + protoField(2, varint: 1)
        let editResult = protoField(1, varint: 7)
            + protoField(10, bytes: editResultPayload)
            + protoField(35, string: "edit-1")
        XCTAssertTrue(store.consume(rendererEvent(
            kind: "result",
            sequence: 4,
            messageType: "aiserver.v1.ClientSideToolV2Result",
            bytes: editResult
        )))
        let edit = store.cardsByAgent["agent-tool"]?.first(where: { $0.toolCallId == "edit-1" })
        XCTAssertEqual(edit?.kind, .fileEdit)
        XCTAssertEqual(edit?.status, .success)
        XCTAssertTrue(edit?.diff.contains("+new") == true)

        XCTAssertTrue(store.consume(rendererEvent(kind: "reset", sequence: 5)))
        XCTAssertNil(store.cardsByAgent["agent-tool"])
    }

    @MainActor
    func testToolResultProjectionFailsClosedForOrphansUnsupportedAndPermissionErrors() {
        let store = MobileToolResultStore()
        let orphanResult = protoField(1, varint: 15)
            + protoField(35, string: "missing")
        XCTAssertFalse(store.consume(rendererEvent(
            kind: "result",
            sequence: 1,
            messageType: "aiserver.v1.ClientSideToolV2Result",
            bytes: orphanResult
        )))

        let unsupportedCall = protoField(1, varint: 5)
            + protoField(3, string: "read-1")
        XCTAssertFalse(store.consume(rendererEvent(
            kind: "call",
            sequence: 2,
            messageType: "aiserver.v1.ClientSideToolV2Call",
            bytes: unsupportedCall
        )))

        let terminalParams = protoField(1, string: "rm protected")
        let shellCall = protoField(1, varint: 15)
            + protoField(3, string: "shell-denied")
            + protoField(23, bytes: terminalParams)
        XCTAssertTrue(store.consume(rendererEvent(
            kind: "call",
            sequence: 3,
            messageType: "aiserver.v1.ClientSideToolV2Call",
            bytes: shellCall
        )))
        let error = protoField(1, string: "Permission denied: policy")
        let shellError = protoField(1, varint: 15)
            + protoField(8, bytes: error)
            + protoField(35, string: "shell-denied")
        XCTAssertTrue(store.consume(rendererEvent(
            kind: "result",
            sequence: 4,
            messageType: "aiserver.v1.ClientSideToolV2Result",
            bytes: shellError
        )))
        let denied = store.cardsByAgent["agent-tool"]?.first
        XCTAssertEqual(denied?.status, .denied)
        XCTAssertEqual(denied?.summary, "Permission denied: policy")

        XCTAssertFalse(store.consume(rendererEvent(
            kind: "result",
            sequence: 4,
            messageType: "aiserver.v1.ClientSideToolV2Result",
            bytes: shellError
        )))
    }

    @MainActor
    func testCompatibilityCallsFailClosedAndClearOnSettlement() {
        let relay = ClientSideToolV2Relay()
        XCTAssertNoThrow(try relay.begin(callID: "compat-1", toolName: "read"))
        XCTAssertThrowsError(try relay.begin(callID: "compat-1", toolName: "read"))
        let settled = try? relay.settle(callID: "compat-1", output: .string("ok"))
        XCTAssertEqual(settled?.tool, "read")
        XCTAssertEqual(settled?.output, .string("ok"))
        relay.clear()
        XCTAssertThrowsError(try relay.settle(callID: "compat-1", output: .null))
    }
}
