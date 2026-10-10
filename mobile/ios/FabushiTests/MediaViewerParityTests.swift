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
    func testAttachmentSelectionClampsAndPrefersProjectedGallery() {
        var message = ChatMessage(
            id: "m1",
            conversationId: "c1",
            text: "media",
            contentType: "photo",
            mediaFileName: "fallback.jpg",
            mediaBlobId: "fallback",
            mediaMimeType: "image/jpeg",
            mediaSizeBytes: 10,
            contactName: nil,
            latitude: nil,
            longitude: nil,
            pollQuestion: nil,
            pollOptions: [],
            pollMultipleAnswers: false,
            isOutgoing: true,
            time: "10:00",
            replyToMessageId: nil,
            forwardOrigin: nil,
            reactions: [],
            deliveryState: "delivered",
            isEdited: false,
            isPinned: false
        )
        message.mediaAttachments = [
            ChatMediaAttachment(
                id: "a",
                messageId: "m1",
                contentType: "photo",
                fileName: "a.jpg",
                blobId: "a",
                mimeType: "image/jpeg",
                sizeBytes: 11,
                groupIndex: 0
            ),
            ChatMediaAttachment(
                id: "b",
                messageId: "m2",
                contentType: "video",
                fileName: "b.mp4",
                blobId: "b",
                mimeType: "video/mp4",
                sizeBytes: 12,
                groupIndex: 1
            ),
        ]

        XCTAssertEqual(mediaViewerAttachments(for: message).map(\.id), ["a", "b"])
        XCTAssertEqual(clampedMediaAttachmentIndex(-5, count: 2), 0)
        XCTAssertEqual(clampedMediaAttachmentIndex(99, count: 2), 1)
    }

}
