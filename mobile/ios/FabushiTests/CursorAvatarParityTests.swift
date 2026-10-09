import Foundation
import XCTest
@testable import Fabushi

final class CursorAvatarParityTests: XCTestCase {
    private func credentials() -> AccountMcpCredentials {
        AccountMcpCredentials(
            getAccessToken: { _ in "avatar-token" },
            getMachineId: { "avatar-machine" }
        )
    }

    func testPreferredHttpsAvatarBecomesBoundedImageDataURLWithoutCredentialLeak() async {
        let image = Data([0x89, 0x50, 0x4e, 0x47])
        let client = IOSCursorDashboardClient(
            credentials: credentials(),
            backendURL: URL(string: "https://backend.example.test")!,
            requestExecutor: { request in
                if request.url?.lastPathComponent == "GetMe" {
                    let data = try JSONSerialization.data(withJSONObject: [
                        "profilePictureUrl": "https://cdn.example.test/me.png",
                    ])
                    return (
                        data,
                        HTTPURLResponse(
                            url: request.url!,
                            statusCode: 200,
                            httpVersion: "HTTP/2",
                            headerFields: ["Content-Type": "application/json"]
                        )!
                    )
                }
                XCTAssertEqual(request.url?.absoluteString, "https://cdn.example.test/me.png")
                XCTAssertNil(request.value(forHTTPHeaderField: "Authorization"))
                XCTAssertEqual(request.timeoutInterval, 10)
                return (
                    image,
                    HTTPURLResponse(
                        url: request.url!,
                        statusCode: 200,
                        httpVersion: "HTTP/2",
                        headerFields: [
                            "Content-Type": "image/png; charset=binary",
                            "Content-Length": "\(image.count)",
                        ]
                    )!
                )
            }
        )
        XCTAssertEqual(
            await client.getCursorAvatarDataURL(authId: "github|123"),
            "data:image/png;base64,\(image.base64EncodedString())"
        )
    }

    func testGithubFallbackUsesNumericSubjectAndRejectsOversizedAvatar() async {
        let client = IOSCursorDashboardClient(
            credentials: credentials(),
            backendURL: URL(string: "https://backend.example.test")!,
            requestExecutor: { request in
                if request.url?.lastPathComponent == "GetMe" {
                    let data = try JSONSerialization.data(withJSONObject: [:])
                    return (
                        data,
                        HTTPURLResponse(
                            url: request.url!,
                            statusCode: 200,
                            httpVersion: "HTTP/2",
                            headerFields: ["Content-Type": "application/json"]
                        )!
                    )
                }
                XCTAssertEqual(request.url?.host, "avatars.githubusercontent.com")
                XCTAssertEqual(request.url?.path, "/u/987")
                return (
                    Data([1]),
                    HTTPURLResponse(
                        url: request.url!,
                        statusCode: 200,
                        httpVersion: "HTTP/2",
                        headerFields: [
                            "Content-Type": "image/jpeg",
                            "Content-Length": "1048577",
                        ]
                    )!
                )
            }
        )
        XCTAssertNil(await client.getCursorAvatarDataURL(authId: "github|987"))
        XCTAssertNil(await client.getCursorAvatarDataURL(authId: "github|not-a-number"))
    }
}
