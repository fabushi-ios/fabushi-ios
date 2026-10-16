import Foundation
import XCTest
@testable import Fabushi

private func transcribeCredentials() -> AccountMcpCredentials {
    AccountMcpCredentials(
        getAccessToken: { _ in "token" },
        getMachineId: { "machine-id" }
    )
}

private func appendTranscribeVarint(_ value: UInt64, to data: inout Data) {
    var value = value
    repeat {
        var byte = UInt8(value & 0x7f)
        value >>= 7
        if value != 0 { byte |= 0x80 }
        data.append(byte)
    } while value != 0
}

private func transcribeBytesField(_ field: Int, _ bytes: Data, to data: inout Data) {
    appendTranscribeVarint(UInt64((field << 3) | 2), to: &data)
    appendTranscribeVarint(UInt64(bytes.count), to: &data)
    data.append(bytes)
}

final class CursorTranscribeParityTests: XCTestCase {
    func testTranscribeUsesCanonicalAiServiceContractAndNormalizesInputs() async throws {
        let client = IOSCursorDashboardClient(
            credentials: transcribeCredentials(),
            backendURL: URL(string: "https://backend.example.test")!,
            requestExecutor: { request in
                XCTAssertEqual(
                    request.url?.path,
                    "/aiserver.v1.AiService/TranscribeAudio"
                )
                XCTAssertEqual(request.timeoutInterval, 60)
                XCTAssertEqual(
                    request.value(forHTTPHeaderField: "Content-Type"),
                    "application/proto"
                )
                XCTAssertEqual(
                    request.value(forHTTPHeaderField: "Connect-Protocol-Version"),
                    "1"
                )
                var expected = Data()
                transcribeBytesField(1, Data([1, 2, 3]), to: &expected)
                transcribeBytesField(2, Data("audio/webm".utf8), to: &expected)
                transcribeBytesField(3, Data("en-US".utf8), to: &expected)
                XCTAssertEqual(request.httpBody, expected)

                var response = Data()
                transcribeBytesField(1, Data("hello".utf8), to: &response)
                appendTranscribeVarint(UInt64(2 << 3), to: &response)
                appendTranscribeVarint(123, to: &response)
                return (
                    response,
                    HTTPURLResponse(
                        url: request.url!,
                        statusCode: 200,
                        httpVersion: "HTTP/2",
                        headerFields: ["Content-Type": "application/proto"]
                    )!
                )
            }
        )

        let result = try await client.transcribeAudio(
            audio: Data([1, 2, 3]),
            mimeType: "audio/webm;codecs=opus"
        )
        XCTAssertEqual(result.text, "hello")
        XCTAssertEqual(result.transcriptionTimeMs, 123)
    }

    func testTranscribeRejectsEmptyAudioBeforeNetwork() async {
        let client = IOSCursorDashboardClient(
            credentials: transcribeCredentials(),
            backendURL: URL(string: "https://backend.example.test")!,
            requestExecutor: { _ in
                XCTFail("Empty audio must fail before transport.")
                throw CancellationError()
            }
        )
        do {
            _ = try await client.transcribeAudio(
                audio: Data(),
                mimeType: "audio/webm"
            )
            XCTFail("Expected empty-audio rejection")
        } catch let error as IOSCursorDashboardError {
            XCTAssertEqual(error.message, "Cannot transcribe empty audio.")
        } catch {
            XCTFail("Unexpected error: \(error)")
        }
    }
}
