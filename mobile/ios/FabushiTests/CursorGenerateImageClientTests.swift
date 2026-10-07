import XCTest
@testable import Fabushi

final class CursorGenerateImageClientTests: XCTestCase {
    func testProtoRequestMatchesDesktopRunGenerateImageFields() {
        let body = IOSCursorGenerateImageProto.encodeRequest(.init(
            description: "cat",
            referenceImages: [],
            modelId: "model",
            maxMode: true
        ))
        XCTAssertEqual(Array(body), [
            0x0a, 0x03, 0x63, 0x61, 0x74,
            0x1a, 0x05, 0x6d, 0x6f, 0x64, 0x65, 0x6c,
            0x20, 0x01,
        ])
    }

    func testProtoResponsePreservesSuccessAndRestrictedErrorOneof() throws {
        let success = Data([
            0x0a, 0x11,
            0x0a, 0x04, 0x59, 0x57, 0x4a, 0x6a,
            0x12, 0x09, 0x69, 0x6d, 0x61, 0x67, 0x65, 0x2f, 0x70, 0x6e, 0x67,
        ])
        XCTAssertEqual(
            try IOSCursorGenerateImageProto.decodeResponse(success),
            .success(.init(imageData: "YWJj", mimeType: "image/png"))
        )
        let restricted = Data([
            0x12, 0x0b,
            0x0a, 0x07, 0x62, 0x6c, 0x6f, 0x63, 0x6b, 0x65, 0x64,
            0x10, 0x01,
        ])
        XCTAssertEqual(
            try IOSCursorGenerateImageProto.decodeResponse(restricted),
            .error(message: "blocked", modelRestricted: true)
        )
    }

    func testConcreteClientUsesAiServiceConnectRouteAndCanonicalCredentials() async throws {
        let credentials = AccountMcpCredentials(
            getAccessToken: { _ in "token-1" },
            getMachineId: { "machine-1" }
        )
        let success = Data([
            0x0a, 0x11,
            0x0a, 0x04, 0x59, 0x57, 0x4a, 0x6a,
            0x12, 0x09, 0x69, 0x6d, 0x61, 0x67, 0x65, 0x2f, 0x70, 0x6e, 0x67,
        ])
        let client = IOSCursorGenerateImageClient(
            credentials: credentials,
            backendURL: URL(string: "https://api.example.test")!,
            requestExecutor: { request in
                XCTAssertEqual(
                    request.url?.absoluteString,
                    "https://api.example.test/aiserver.v1.AiService/RunGenerateImage"
                )
                XCTAssertEqual(request.httpMethod, "POST")
                XCTAssertEqual(request.value(forHTTPHeaderField: "Content-Type"), "application/proto")
                XCTAssertEqual(request.value(forHTTPHeaderField: "Connect-Protocol-Version"), "1")
                XCTAssertEqual(request.value(forHTTPHeaderField: "Authorization"), "Bearer token-1")
                XCTAssertTrue(
                    request.value(forHTTPHeaderField: "x-cursor-checksum")?
                        .hasSuffix("machine-1") == true
                )
                XCTAssertEqual(
                    request.httpBody,
                    IOSCursorGenerateImageProto.encodeRequest(.init(
                        description: "avatar",
                        referenceImages: [],
                        modelId: "model-1",
                        maxMode: false
                    ))
                )
                return (
                    success,
                    HTTPURLResponse(
                        url: request.url!,
                        statusCode: 200,
                        httpVersion: nil,
                        headerFields: nil
                    )!
                )
            }
        )
        XCTAssertEqual(
            try await client.runGenerateImage(.init(
                description: " avatar ",
                referenceImages: [],
                modelId: "model-1",
                maxMode: false
            )),
            .success(.init(imageData: "YWJj", mimeType: "image/png"))
        )
    }

    func testAvatarGeneratedDataUrlFailsClosed() {
        XCTAssertEqual(
            AvatarImagePolicy.data(fromImageDataURL: "data:image/png;base64,YWJj"),
            Data("abc".utf8)
        )
        XCTAssertNil(AvatarImagePolicy.data(fromImageDataURL: "https://example.test/avatar.png"))
        XCTAssertNil(AvatarImagePolicy.data(fromImageDataURL: "data:text/plain;base64,YWJj"))
        XCTAssertNil(AvatarImagePolicy.data(fromImageDataURL: "data:image/png,YWJj"))
    }
}
