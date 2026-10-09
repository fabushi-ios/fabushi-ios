import XCTest
@testable import Fabushi

final class VoiceRecorderParityTests: XCTestCase {
    func testDesktopRecordingDurationBoundariesArePreserved() {
        XCTAssertEqual(VoiceRecorder.minimumRecordingDuration, 0.5)
        XCTAssertEqual(VoiceRecorder.maximumRecordingDuration, 300)
        XCTAssertFalse(VoiceRecorder.shouldTranscribe(duration: 0.499))
        XCTAssertTrue(VoiceRecorder.shouldTranscribe(duration: 0.5))
        XCTAssertFalse(VoiceRecorder.reachedMaximumDuration(299.999))
        XCTAssertTrue(VoiceRecorder.reachedMaximumDuration(300))
    }
}
