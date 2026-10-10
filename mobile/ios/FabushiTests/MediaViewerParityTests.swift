import XCTest
@testable import Fabushi

final class MediaViewerParityTests: XCTestCase {
    func testImageTransformClampsDesktopZoomRangeAndResetsAtFit() {
        var transform = NativeMediaImageTransform()
        transform = transform.zoomed(by: 10)
        XCTAssertEqual(transform.scale, 5)

        transform = transform.panned(by: CGSize(width: 24, height: -9))
        XCTAssertEqual(transform.offset, CGSize(width: 24, height: -9))

        transform = transform.zoomed(by: 0.01)
        XCTAssertEqual(transform, NativeMediaImageTransform())
    }

    func testImageTransformDoesNotPanAtFitScaleAndAccumulatesWhileZoomed() {
        XCTAssertEqual(
            NativeMediaImageTransform().panned(by: CGSize(width: 99, height: 42)),
            NativeMediaImageTransform()
        )

        let zoomed = NativeMediaImageTransform(scale: 2, offset: CGSize(width: 4, height: 5))
            .panned(by: CGSize(width: -1, height: 3))
        XCTAssertEqual(zoomed.scale, 2)
        XCTAssertEqual(zoomed.offset, CGSize(width: 3, height: 8))
    }
}
