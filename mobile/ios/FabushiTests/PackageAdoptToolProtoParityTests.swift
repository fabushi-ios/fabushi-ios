import XCTest
@testable import Fabushi

final class PackageAdoptToolProtoParityTests: XCTestCase {
    func testAdoptArgsUsesCanonicalSourceAgentFieldOneWire() throws {
        let args = AdoptArgs(sourceAgentId: "agent-a")
        let encoded = args.serializedData()

        XCTAssertEqual(
            encoded,
            Data([0x0a, 0x07] + Array("agent-a".utf8))
        )
        XCTAssertEqual(try AdoptArgs(serializedData: encoded), args)
        XCTAssertEqual(AdoptArgs.protobufTypeName, "agent.v1.AdoptArgs")
    }

    func testAdoptResultSuccessPreservesAllIdsAndOutcomeEnum() throws {
        let value = AdoptResult(
            sourceAgentId: "s",
            targetAgentId: "t",
            projectRootId: "p",
            result: .success(.storeImportCompleted)
        )

        XCTAssertEqual(
            value.serializedData(),
            Data([
                0x0a, 0x01, 0x73,
                0x12, 0x01, 0x74,
                0x1a, 0x01, 0x70,
                0x20, 0x03,
            ])
        )
        XCTAssertEqual(try AdoptResult(serializedData: value.serializedData()), value)
    }

    func testAdoptResultOneofUsesLastWireValueAndPreservesEmptyErrorPresence() throws {
        var wire = Data([0x20, 0x01])
        wire.append(contentsOf: [0x2a, 0x00])

        let decoded = try AdoptResult(serializedData: wire)
        XCTAssertEqual(decoded.result, .error(""))
        XCTAssertEqual(decoded.serializedData(), Data([0x2a, 0x00]))
    }

    func testAdoptToolCallRoundTripsNestedArgsAndResultMessages() throws {
        let call = AdoptToolCall(
            args: AdoptArgs(sourceAgentId: "child"),
            result: AdoptResult(
                sourceAgentId: "child",
                targetAgentId: "parent",
                projectRootId: "project",
                result: .success(.edgeOnly)
            )
        )

        let encoded = call.serializedData()
        XCTAssertEqual(try AdoptToolCall(serializedData: encoded), call)
        XCTAssertEqual(AdoptToolCall.protobufTypeName, "agent.v1.AdoptToolCall")
    }

    func testUnknownFieldsAreSkippedAndUnknownOutcomeValuesArePreserved() throws {
        var wire = Data([
            0x0a, 0x01, 0x73,
            0x20, 0x11,
        ])
        wire.append(contentsOf: [0x48, 0x63])

        let decoded = try AdoptResult(serializedData: wire)
        XCTAssertEqual(decoded.sourceAgentId, "s")
        XCTAssertEqual(decoded.result, .success(AdoptOutcome(rawValue: 17)))
        XCTAssertEqual(
            decoded.serializedData(),
            Data([
                0x0a, 0x01, 0x73,
                0x20, 0x11,
            ])
        )
    }
}
