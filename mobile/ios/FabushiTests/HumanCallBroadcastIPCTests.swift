import Foundation
import XCTest
@testable import Fabushi

final class HumanCallBroadcastIPCTests: XCTestCase {
    func testBroadcastIdentifiersRemainBoundToShippingTargets() {
        XCTAssertEqual(HumanCallBroadcastIPC.appGroupIdentifier, "group.com.ombhrum.fabushi.call")
        XCTAssertEqual(HumanCallBroadcastIPC.extensionBundleIdentifier, "com.ombhrum.fabushi.broadcast")
        XCTAssertEqual(HumanCallBroadcastIPC.metadataFilename, "human-call-broadcast-metadata.json")
    }

    func testBroadcastMetadataRoundTripsSessionFenceAndFrameIdentity() throws {
        let metadata = HumanCallBroadcastFrameMetadata(
            sessionID: "call-7:3",
            sequence: 9,
            state: .frame,
            frameFilename: "human-call-broadcast-frame-9.jpg",
            timestampNanoseconds: 123_456,
            orientation: 6
        )
        let encoded = try JSONEncoder().encode(metadata)
        XCTAssertEqual(try JSONDecoder().decode(HumanCallBroadcastFrameMetadata.self, from: encoded), metadata)
    }
}
