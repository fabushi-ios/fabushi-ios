import AVFoundation
import XCTest
@testable import Fabushi

final class HumanCallMediaPortTests: XCTestCase {
    func testPermissionMappingMatchesDesktopCallMediaContract() {
        XCTAssertEqual(HumanCallMediaPort.permission(for: .authorized), .granted)
        XCTAssertEqual(HumanCallMediaPort.permission(for: .denied), .denied)
        XCTAssertEqual(HumanCallMediaPort.permission(for: .restricted), .denied)
        XCTAssertEqual(HumanCallMediaPort.permission(for: .notDetermined), .prompt)
    }

    func testMediaPermissionWireValuesStayStable() {
        XCTAssertEqual(HumanCallMediaPermission.granted.rawValue, "granted")
        XCTAssertEqual(HumanCallMediaPermission.denied.rawValue, "denied")
        XCTAssertEqual(HumanCallMediaPermission.prompt.rawValue, "prompt")
        XCTAssertEqual(HumanCallMediaPermission.notRequested.rawValue, "not-requested")
    }
}
