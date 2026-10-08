import Foundation
import XCTest
@testable import Fabushi

final class HumanCallSystemCoordinatorTests: XCTestCase {
    func testPushDescriptorRequiresCanonicalCallIdentity() {
        XCTAssertNil(HumanCallPushDescriptor.parse([
            "generation": 1,
            "displayName": "Alice",
        ]))
        XCTAssertNil(HumanCallPushDescriptor.parse([
            "callId": "   ",
            "generation": 1,
        ]))
    }

    func testPushDescriptorPreservesGenerationAndPresentation() throws {
        let descriptor = try XCTUnwrap(HumanCallPushDescriptor.parse([
            "callId": "4A1D9F9E-22F5-4D36-BF5B-34A1D4F2BA19",
            "generation": 7,
            "displayName": " Alice ",
            "hasVideo": true,
        ]))
        XCTAssertEqual(descriptor.callId, "4A1D9F9E-22F5-4D36-BF5B-34A1D4F2BA19")
        XCTAssertEqual(descriptor.generation, 7)
        XCTAssertEqual(descriptor.displayName, "Alice")
        XCTAssertTrue(descriptor.hasVideo)
    }

    func testPushDescriptorFailsClosedOnNegativeGenerationAndDefaultsPresentation() throws {
        XCTAssertNil(HumanCallPushDescriptor.parse([
            "callId": "call-1",
            "generation": -1,
        ]))
        let descriptor = try XCTUnwrap(HumanCallPushDescriptor.parse([
            "callId": "call-2",
        ]))
        XCTAssertEqual(descriptor.generation, 0)
        XCTAssertEqual(descriptor.displayName, "Fabushi 通话")
        XCTAssertFalse(descriptor.hasVideo)
    }

    func testVoIPTokenEncodingIsStableLowercaseHex() {
        XCTAssertEqual(
            HumanCallSystemCoordinator.hexadecimalToken(Data([0x00, 0x0a, 0xff])),
            "000aff"
        )
    }
}
