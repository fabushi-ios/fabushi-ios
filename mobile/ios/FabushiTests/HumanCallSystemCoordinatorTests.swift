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

    func testPushDescriptorFailsClosedOnMissingOrMalformedPresentationFields() {
        XCTAssertNil(HumanCallPushDescriptor.parse([
            "callId": "call-1",
            "generation": -1,
            "displayName": "Alice",
            "hasVideo": false,
        ]))
        XCTAssertNil(HumanCallPushDescriptor.parse([
            "callId": "call-2",
            "displayName": "Alice",
            "hasVideo": false,
        ]))
        XCTAssertNil(HumanCallPushDescriptor.parse([
            "callId": "call-3",
            "generation": 0,
            "hasVideo": false,
        ]))
        XCTAssertNil(HumanCallPushDescriptor.parse([
            "callId": "call-4",
            "generation": 0,
            "displayName": "Alice",
        ]))
        XCTAssertNil(HumanCallPushDescriptor.parse([
            "callId": "call-5",
            "generation": 0,
            "displayName": "   ",
            "hasVideo": true,
        ]))
    }

    func testProviderResetDescriptorsKeepLatestGenerationPerCall() {
        let older = HumanCallPushDescriptor(
            callId: "call-b",
            generation: 2,
            displayName: "Older",
            hasVideo: false
        )
        let newer = HumanCallPushDescriptor(
            callId: "call-b",
            generation: 4,
            displayName: "Newer",
            hasVideo: true
        )
        let other = HumanCallPushDescriptor(
            callId: "call-a",
            generation: 1,
            displayName: "Other",
            hasVideo: false
        )

        let descriptors = HumanCallSystemCoordinator.canonicalProviderResetDescriptors(
            [older, newer, other]
        )

        XCTAssertEqual(descriptors.map(\.callId), ["call-a", "call-b"])
        XCTAssertEqual(descriptors.map(\.generation), [1, 4])
        XCTAssertEqual(descriptors.last?.displayName, "Newer")
    }

    func testVoIPTokenEncodingIsStableLowercaseHex() {
        XCTAssertEqual(
            HumanCallSystemCoordinator.hexadecimalToken(Data([0x00, 0x0a, 0xff])),
            "000aff"
        )
    }
}
