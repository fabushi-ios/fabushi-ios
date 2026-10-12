import XCTest
@testable import Fabushi

private actor LogoFetchCounter {
    var calls = 0
    func increment() { calls += 1 }
    func value() -> Int { calls }
}

final class SharedMcpResultAssetsParityTests: XCTestCase {
    func testGeneratedResultFactoryPreservesSuccessMetadata() {
        let original = SandMcpResult(result: .success(.init(
            content: [.init(content: .text(.init(text: "old")))],
            isError: true,
            structuredContent: .object(["x": .number(1)])
        )))
        let rebuilt = generatedMcpResultFactory.success(
            original,
            content: [generatedMcpResultFactory.textItem("new")]
        )
        guard case .success(let success) = rebuilt.result else {
            return XCTFail("expected success")
        }
        XCTAssertTrue(success.isError)
        XCTAssertEqual(success.structuredContent, .object(["x": .number(1)]))
        XCTAssertEqual(success.content, [.init(content: .text(.init(text: "new")))])
        XCTAssertEqual(
            generatedMcpResultFactory.error("boom"),
            .init(result: .error("boom"))
        )
    }

    func testSavedImageDescriptionAndAugmentation() async {
        let original = SandMcpResult(result: .success(.init(
            content: [
                .init(content: .image(.init(data: "abc", mimeType: "image/png"))),
                .init(content: .text(.init(text: "done"))),
            ],
            isError: false,
            structuredContent: nil
        )))
        let augmented = await augmentMcpResultWithSavedImages(original) { data, mime in
            XCTAssertEqual(data, "abc")
            XCTAssertEqual(mime, "image/png")
            return .init(fileUrl: "file:///tmp/image.png", width: 100, height: 80)
        }
        guard case .success(let success) = augmented.result else {
            return XCTFail("expected success")
        }
        XCTAssertEqual(success.content.count, 3)
        guard case .text(let note) = success.content[1].content else {
            return XCTFail("expected saved-image note")
        }
        XCTAssertTrue(note.text.contains("file:///tmp/image.png (100x80)"))
        XCTAssertTrue(note.text.contains("SendMessage"))
    }

    func testImagePersistenceFailureIsIsolated() async {
        let original = SandMcpResult(result: .success(.init(
            content: [.init(content: .image(.init(data: "abc", mimeType: "image/png")))],
            isError: false,
            structuredContent: nil
        )))
        let result = await augmentMcpResultWithSavedImages(original) { _, _ in
            throw URLError(.cannotWriteToFile)
        }
        XCTAssertEqual(result, original)
    }

    func testPluginLogoRequiresKnownHttpsUrlAndCachesResult() async throws {
        await clearPluginLogoCacheForTesting()
        rememberPluginLogoUrl("https://cdn.example.test/logo.png")
        let counter = LogoFetchCounter()
        let png = Data([0x89, 0x50, 0x4e, 0x47])
        let fetch: MarketplaceLogoFetch = { url, timeout in
            await counter.increment()
            XCTAssertEqual(url.absoluteString, "https://cdn.example.test/logo.png")
            XCTAssertEqual(timeout, 12)
            let response = try XCTUnwrap(HTTPURLResponse(
                url: url,
                statusCode: 200,
                httpVersion: nil,
                headerFields: [
                    "Content-Type": "image/png",
                    "Content-Length": String(png.count),
                ]
            ))
            return (png, response)
        }

        let first = await resolvePluginLogo(
            "https://cdn.example.test/logo.png",
            fetch: fetch
        )
        let second = await resolvePluginLogo(
            "https://cdn.example.test/logo.png",
            fetch: fetch
        )
        XCTAssertEqual(first, "data:image/png;base64,iVBORw==")
        XCTAssertEqual(second, first)
        let calls = await counter.value()
        XCTAssertEqual(calls, 1)

        let unknown = await resolvePluginLogo(
            "https://cdn.example.test/unknown.png",
            fetch: fetch
        )
        XCTAssertNil(unknown)
        let insecure = await resolvePluginLogo(
            "http://cdn.example.test/logo.png",
            isKnown: { _ in true },
            fetch: fetch
        )
        XCTAssertNil(insecure)
    }

    func testPluginLogoRejectsOversizedBody() async throws {
        await clearPluginLogoCacheForTesting()
        let urlString = "https://cdn.example.test/large.png"
        let body = Data(repeating: 1, count: LOGO_MAX_BYTES + 1)
        let result = await resolvePluginLogo(
            urlString,
            isKnown: { _ in true },
            fetch: { url, _ in
                let response = try XCTUnwrap(HTTPURLResponse(
                    url: url,
                    statusCode: 200,
                    httpVersion: nil,
                    headerFields: [
                        "Content-Type": "image/png",
                        "Content-Length": String(body.count),
                    ]
                ))
                return (body, response)
            }
        )
        XCTAssertNil(result)
    }
}
