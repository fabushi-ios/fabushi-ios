import XCTest
@testable import Fabushi

final class VoiceRecorderParityTests: XCTestCase {
    @MainActor
    func testDesktopRecordingDurationBoundariesArePreserved() {
        XCTAssertEqual(VoiceRecorder.minimumRecordingDuration, 0.5)
        XCTAssertEqual(VoiceRecorder.maximumRecordingDuration, 300)
        XCTAssertFalse(VoiceRecorder.shouldTranscribe(duration: 0.499))
        XCTAssertTrue(VoiceRecorder.shouldTranscribe(duration: 0.5))
        XCTAssertFalse(VoiceRecorder.reachedMaximumDuration(299.999))
        XCTAssertTrue(VoiceRecorder.reachedMaximumDuration(300))
    }

    @MainActor
    func testWaveformLevelClampsAndNormalizesRecorderPower() {
        XCTAssertEqual(VoiceRecorder.normalizedWaveformLevel(decibels: -80), 0, accuracy: 0.0001)
        XCTAssertEqual(VoiceRecorder.normalizedWaveformLevel(decibels: -60), 0, accuracy: 0.0001)
        XCTAssertEqual(VoiceRecorder.normalizedWaveformLevel(decibels: -30), 0.5, accuracy: 0.0001)
        XCTAssertEqual(VoiceRecorder.normalizedWaveformLevel(decibels: 0), 1, accuracy: 0.0001)
        XCTAssertEqual(VoiceRecorder.normalizedWaveformLevel(decibels: 4), 1, accuracy: 0.0001)
    }
    @MainActor
    func testWaveformHistoryUsesRealBoundedMeterSamples() {
        var samples: [Double] = []
        for index in 0..<40 {
            samples = VoiceRecorder.appendingWaveformSample(
                samples,
                level: Double(index) / 20,
                limit: 9
            )
        }
        XCTAssertEqual(samples.count, 9)
        XCTAssertTrue(samples.allSatisfy { $0 >= 0 && $0 <= 1 })
        XCTAssertEqual(samples.last, 1, accuracy: 0.0001)
    }

    @MainActor
    func testVoiceFailureCodesMatchDesktopUserFacingClasses() {
        XCTAssertEqual(
            VoiceRecorderErrorCode.microphonePermissionDenied.rawValue,
            "MICROPHONE_PERMISSION_DENIED"
        )
        XCTAssertEqual(
            VoiceRecorderErrorCode.audioDeviceUnavailable.rawValue,
            "AUDIO_DEVICE_UNAVAILABLE"
        )
        XCTAssertEqual(
            VoiceRecorderErrorCode.recordingError.rawValue,
            "RECORDING_ERROR"
        )
        XCTAssertFalse(
            VoiceRecorder.userMessage(for: .microphonePermissionDenied).isEmpty
        )
        XCTAssertFalse(
            VoiceRecorder.userMessage(for: .audioDeviceUnavailable).isEmpty
        )
        XCTAssertFalse(
            VoiceRecorder.userMessage(for: .recordingError).isEmpty
        )
    }

}
