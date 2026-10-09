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

    func testWaveformLevelClampsAndNormalizesRecorderPower() {
        XCTAssertEqual(VoiceRecorder.normalizedWaveformLevel(decibels: -80), 0, accuracy: 0.0001)
        XCTAssertEqual(VoiceRecorder.normalizedWaveformLevel(decibels: -60), 0, accuracy: 0.0001)
        XCTAssertEqual(VoiceRecorder.normalizedWaveformLevel(decibels: -30), 0.5, accuracy: 0.0001)
        XCTAssertEqual(VoiceRecorder.normalizedWaveformLevel(decibels: 0), 1, accuracy: 0.0001)
        XCTAssertEqual(VoiceRecorder.normalizedWaveformLevel(decibels: 4), 1, accuracy: 0.0001)
    }
}
