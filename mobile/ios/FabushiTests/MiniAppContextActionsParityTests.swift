import UIKit
import XCTest
@testable import Fabushi

final class MiniAppContextActionsParityTests: XCTestCase {
    func testExternalContextURLsAllowOnlyHttpAndHttps() {
        XCTAssertEqual(
            MiniAppNativeContextActions.httpURL("https://example.com/image.png")?.scheme,
            "https"
        )
        XCTAssertEqual(
            MiniAppNativeContextActions.httpURL("http://example.com/page")?.scheme,
            "http"
        )
        XCTAssertNil(MiniAppNativeContextActions.httpURL("javascript:alert(1)"))
        XCTAssertNil(MiniAppNativeContextActions.httpURL("file:///tmp/private.png"))
        XCTAssertNil(MiniAppNativeContextActions.httpURL("data:text/html,hello"))
    }

    @MainActor
    func testDataImageDecoderAcceptsImageDataAndRejectsNonImageData() throws {
        let renderer = UIGraphicsImageRenderer(size: CGSize(width: 2, height: 2))
        let image = renderer.image { context in
            UIColor.black.setFill()
            context.fill(CGRect(x: 0, y: 0, width: 2, height: 2))
        }
        let png = try XCTUnwrap(image.pngData())
        let dataURL = "data:image/png;base64,\(png.base64EncodedString())"
        XCTAssertNotNil(MiniAppNativeContextActions.imageFromDataURL(dataURL))
        XCTAssertNil(MiniAppNativeContextActions.imageFromDataURL("data:text/plain;base64,SGVsbG8="))
        XCTAssertNil(MiniAppNativeContextActions.imageFromDataURL("data:image/png;base64,not-image"))
    }

    func testContextProbeExposesOnlyImageAndLinkURLs() {
        XCTAssertTrue(miniAppNativeContextMenuJavaScript.contains("target.closest('img')"))
        XCTAssertTrue(miniAppNativeContextMenuJavaScript.contains("target.closest('a[href]')"))
        XCTAssertTrue(miniAppNativeContextMenuJavaScript.contains("srcURL"))
        XCTAssertTrue(miniAppNativeContextMenuJavaScript.contains("linkURL"))
        XCTAssertFalse(miniAppNativeContextMenuJavaScript.contains("localStorage"))
        XCTAssertFalse(miniAppNativeContextMenuJavaScript.contains("document.cookie"))
        XCTAssertFalse(miniAppNativeContextMenuJavaScript.contains("innerHTML"))
    }

    func testShippingMiniAppWebViewFencesAndOwnsNativeContextActions() throws {
        let repositoryRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let webSurface = try String(
            contentsOf: repositoryRoot.appendingPathComponent(
                "mobile/ios/Fabushi/MiniAppWebMcpSurface.swift"
            ),
            encoding: .utf8
        )
        let nativeActions = try String(
            contentsOf: repositoryRoot.appendingPathComponent(
                "mobile/ios/Fabushi/MiniAppNativeContextActions.swift"
            ),
            encoding: .utf8
        )

        XCTAssertTrue(webSurface.contains("name: miniAppNativeContextMenuHandler"))
        XCTAssertTrue(webSurface.contains("message.frameInfo.isMainFrame"))
        XCTAssertTrue(webSurface.contains("isTrustedWebMcpBridgeURL("))
        XCTAssertTrue(webSurface.contains("generatedMiniAppPreviewOriginHost"))
        XCTAssertTrue(webSurface.contains("nativeContextActions.present(payload: body, from: webView)"))
        XCTAssertTrue(nativeActions.contains("UIPasteboard.general.image"))
        XCTAssertTrue(nativeActions.contains("UIPasteboard.general.string"))
        XCTAssertTrue(nativeActions.contains("UIActivityViewController("))
        XCTAssertTrue(nativeActions.contains("UIApplication.shared.open(link)"))
        XCTAssertTrue(nativeActions.contains("mime.hasPrefix(\"image/\")"))
        XCTAssertFalse(nativeActions.contains("write(to:"))
    }
}
