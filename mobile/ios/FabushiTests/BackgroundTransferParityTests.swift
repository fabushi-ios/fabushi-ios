import XCTest
@testable import Fabushi

private actor StubBackgroundTransferScheduler: IOSBackgroundTransferScheduling {
    private(set) var requests: [(URL, String?)] = []

    func enqueueDownload(url: URL, fileName: String?) async throws -> Int {
        requests.append((url, fileName))
        return 42
    }

    func recordedCount() -> Int { requests.count }
}

final class BackgroundTransferParityTests: XCTestCase {
    func testBackgroundTransferURLPolicyIsHttpsOnlyAndCredentialFree() throws {
        XCTAssertEqual(
            try IOSBackgroundTransferService.validatedRemoteURL(
                "https://example.com/file.bin"
            ).scheme,
            "https"
        )
        XCTAssertThrowsError(
            try IOSBackgroundTransferService.validatedRemoteURL(
                "http://example.com/file.bin"
            )
        )
        XCTAssertThrowsError(
            try IOSBackgroundTransferService.validatedRemoteURL(
                "https://user:secret@example.com/file.bin"
            )
        )
    }

    func testBackgroundTransferFileNameRejectsPathTraversal() throws {
        XCTAssertEqual(
            try IOSBackgroundTransferService.validatedFileName("report.pdf"),
            "report.pdf"
        )
        XCTAssertThrowsError(
            try IOSBackgroundTransferService.validatedFileName("../report.pdf")
        )
        XCTAssertThrowsError(
            try IOSBackgroundTransferService.validatedFileName("folder/report.pdf")
        )
    }

    func testNativeCapabilityBackendSchedulesRealBackgroundSessionWork() async throws {
        let scheduler = StubBackgroundTransferScheduler()
        let backend = IOSNativeLocalCapabilityBackend(
            backgroundTransfers: scheduler
        )

        let reply = try await backend.execute(
            capability: .backgroundTransfer,
            params: .object([
                "url": .string("https://example.com/file.bin"),
                "fileName": .string("file.bin"),
            ])
        )

        let scheduledCount = await scheduler.recordedCount()
        XCTAssertEqual(scheduledCount, 1)
        guard case .object(let object) = reply else {
            return XCTFail("expected background-transfer result")
        }
        XCTAssertEqual(object["accepted"], .bool(true))
        XCTAssertEqual(object["taskIdentifier"], .number(42))
        XCTAssertEqual(
            object["sessionIdentifier"],
            .string(IOSBackgroundTransferService.sessionIdentifier)
        )
    }
}
