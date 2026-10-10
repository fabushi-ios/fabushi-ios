import XCTest
@testable import Fabushi

private struct ImmediateDelayClock: SandDelayClock {
    func sleep(milliseconds: Int) async {}
}

final class SharedNativeNodeAdaptersParityTests: XCTestCase {
    func testDelayAdapterAcceptsNonPositiveDurations() async {
        await delayWith(ImmediateDelayClock(), ms: -1)
    }

    func testAtomicWriteCreatesAndReplacesFile() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let path = root.appendingPathComponent("nested/value.txt").path
        try writeFileAtomic(targetPath: path, string: "one")
        XCTAssertEqual(try String(contentsOfFile: path, encoding: .utf8), "one")
        try writeFileAtomic(targetPath: path, string: "two")
        XCTAssertEqual(try String(contentsOfFile: path, encoding: .utf8), "two")
        try? FileManager.default.removeItem(at: root)
    }

    func testJwtScopeExpiryAndBackendSelection() throws {
        func b64url(_ data: Data) -> String {
            data.base64EncodedString()
                .replacingOccurrences(of: "+", with: "-")
                .replacingOccurrences(of: "/", with: "_")
                .replacingOccurrences(of: "=", with: "")
        }
        let payload = try JSONSerialization.data(withJSONObject: ["sub":"user-1","email":"a@example.com","exp":2_000_000_000])
        let token = "x.\(b64url(payload)).y"
        XCTAssertEqual(parseJwtPayload(token)?.sub, "user-1")
        XCTAssertEqual(accountCacheScope(token).count, 64)
        XCTAssertFalse(isTokenExpiringSoon(token, nowMs: 1_000))
        XCTAssertEqual(getConfiguredBackendUrl([:]), "https://api2.cursor.sh/")
        XCTAssertEqual(getAuthClientId("http://localhost:3000/", env: [:]), DEV_AUTH_CLIENT_ID)
        XCTAssertEqual(getAuthClientId("https://api2.cursor.sh/", env: [:]), PROD_AUTH_CLIENT_ID)
    }

    func testImageContentTypeCapAndDataUrl() throws {
        let url = try XCTUnwrap(URL(string: "https://example.com/a.png"))
        let response = try XCTUnwrap(HTTPURLResponse(
            url: url,
            statusCode: 200,
            httpVersion: nil,
            headerFields: ["Content-Type":"image/png; charset=binary","Content-Length":"3"]
        ))
        XCTAssertEqual(imageContentType(response), "image/png")
        let body = Data([1,2,3])
        XCTAssertEqual(readCappedImageBytes(response, chunks: [body], maxBytes: 3), body)
        XCTAssertNil(readCappedImageBytes(response, chunks: [body], maxBytes: 2))
        XCTAssertEqual(responseToImageDataUrl(response, body: body, maxBytes: 3), "data:image/png;base64,AQID")
    }

    func testPathUtilitiesEnforceInclusiveAndSymlinkResolvedContainment() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let child = root.appendingPathComponent("a/b").path
        XCTAssertTrue(isPathWithin(parent: root.path, child: child))
        XCTAssertFalse(isPathWithin(parent: root.path, child: root.path))
        XCTAssertTrue(isPathWithin(parent: root.path, child: root.path, isInclusive: true))
        XCTAssertEqual(filePathFromFileUrl(root.absoluteString), root.path)
        XCTAssertEqual(try containWithin(roots: [root.path], path: child), child)
        try? FileManager.default.removeItem(at: root)
    }

    func testIOSLocalInferenceNeverSimulatesDesktopCLIProcesses() {
        XCTAssertNil(resolveCodexCliPath())
        XCTAssertNil(resolveClaudeCodeCliPath())

        let status = getLocalInferenceCliStatus()
        XCTAssertFalse(status.codex.installed)
        XCTAssertFalse(status.codex.authenticated)
        XCTAssertNil(status.codex.executablePath)
        XCTAssertEqual(status.codex.route, .nativeProvider)

        XCTAssertFalse(status.claudeCode.installed)
        XCTAssertFalse(status.claudeCode.authenticated)
        XCTAssertNil(status.claudeCode.executablePath)
        XCTAssertEqual(status.claudeCode.route, .remoteRunner)
    }

    func testVariantMetadataAndHeaders() {
        let dev = ["SAND_CLIENT_APP_VERSION":"1.2.3-beta","SAND_PACKAGED":"0"]
        XCTAssertEqual(getSandVariant(dev), .dev)
        XCTAssertEqual(getSandClientBaseVersion(dev), "1.2.3")
        XCTAssertEqual(getSandClientVersion(dev), "1.2.3-dev")
        XCTAssertEqual(getSandBoxNamespace(dev), .dev)
        XCTAssertEqual(getSandBackendClientHeaders(dev)["x-cursor-client-version"], "1.2.3-dev")

        let lab = ["SAND_CLIENT_APP_VERSION":"2.0.0","SAND_PACKAGED":"1","SAND_LAB":"1"]
        XCTAssertEqual(getSandVariant(lab), .lab)
        XCTAssertEqual(getSandClientVersion(lab), "2.0.0-lab")
    }
}
