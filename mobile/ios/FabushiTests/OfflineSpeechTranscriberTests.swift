import Speech
import XCTest
@testable import Fabushi

final class OfflineSpeechTranscriberTests: XCTestCase {
    @MainActor
    func testRecognitionRequestRequiresOnDeviceRecognitionWithoutPartials() {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("offline-speech-policy.m4a")
        let request = OfflineSpeechTranscriber.makeRequest(fileURL: url)
        XCTAssertTrue(request.requiresOnDeviceRecognition)
        XCTAssertFalse(request.shouldReportPartialResults)
        XCTAssertEqual(request.url, url)
    }

    func testShippingBundleDeclaresSpeechRecognitionPurpose() {
        let value = Bundle.main.object(forInfoDictionaryKey: "NSSpeechRecognitionUsageDescription") as? String
        XCTAssertNotNil(value)
        XCTAssertFalse(value?.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ?? true)
    }
}
