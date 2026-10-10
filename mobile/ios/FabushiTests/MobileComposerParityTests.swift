import XCTest
@testable import Fabushi

final class MobileComposerParityTests: XCTestCase {
    private let attachment = MobileComposerAttachment(
        id: "abc123",
        name: "notes.txt",
        path: "/private/fabushi/notes.txt",
        mimeType: "text/plain",
        sizeBytes: 42
    )

    func testAttachmentOnlyDraftIsSendable() {
        XCTAssertTrue(mobileComposerHasPayload(text: "", attachments: [attachment]))
        XCTAssertTrue(mobileComposerHasPayload(text: "hello", attachments: []))
        XCTAssertFalse(mobileComposerHasPayload(text: "  \n ", attachments: []))
        XCTAssertEqual(mobileComposerAttachmentLimit, 6)
    }

    func testVoiceTranscriptInsertsInsteadOfOverwritingExistingDraft() {
        XCTAssertEqual(
            mergeMobileComposerVoiceTranscript(existing: "Existing draft", transcript: "new words"),
            "Existing draft new words"
        )
        XCTAssertEqual(
            mergeMobileComposerVoiceTranscript(existing: "Existing draft ", transcript: "new words"),
            "Existing draft new words"
        )
        XCTAssertEqual(
            mergeMobileComposerVoiceTranscript(existing: "", transcript: "  hello  "),
            "hello"
        )
    }

    func testAttachmentCommandPayloadPreservesStoredIdentityAndMetadata() {
        let payload = mobileComposerAttachmentCommandPayload(attachment)
        XCTAssertEqual(payload["id"] as? String, "abc123")
        XCTAssertEqual(payload["name"] as? String, "notes.txt")
        XCTAssertEqual(payload["path"] as? String, "/private/fabushi/notes.txt")
        XCTAssertEqual(payload["mimeType"] as? String, "text/plain")
        XCTAssertEqual(payload["sizeBytes"] as? Int, 42)
    }

    func testAttachmentLimitsMatchDesktopComposerContract() {
        XCTAssertEqual(AttachmentLimits.attachmentByteLimit(forName: "notes.txt"), 25 * 1024 * 1024)
        XCTAssertEqual(AttachmentLimits.attachmentByteLimit(forName: "clip.mp4"), 200 * 1024 * 1024)
    }

    func testUnnamedStageFallbackMatchesDesktop() {
        XCTAssertEqual(
            mobileComposerStageFileName(
                proposedName: "",
                fallbackLastPathComponent: "",
                mimeType: "image/png"
            ),
            "image.png"
        )
        XCTAssertEqual(
            mobileComposerStageFileName(
                proposedName: nil,
                fallbackLastPathComponent: "",
                mimeType: "application/octet-stream"
            ),
            "file"
        )
        XCTAssertEqual(
            mobileComposerStageFileName(
                proposedName: " notes.txt ",
                fallbackLastPathComponent: "ignored.bin",
                mimeType: nil
            ),
            "notes.txt"
        )
    }

    func testStageFailureNoticeAggregatesLikeDesktop() {
        XCTAssertEqual(
            mobileComposerStageFailureNotice([
                .init(name: "a.txt", reason: .tooLarge),
                .init(name: "b.mp4", reason: .tooLarge),
            ]),
            "2 files are too large to attach (max 25 MB, or 200 MB for video)."
        )
        XCTAssertEqual(
            mobileComposerStageFailureNotice([
                .init(name: "empty.txt", reason: .empty),
            ]),
            "\"empty.txt\" is empty, so it wasn't attached."
        )
        XCTAssertEqual(
            mobileComposerStageFailureNotice([
                .init(name: "a.txt", reason: .empty),
                .init(name: "b.txt", reason: .failed),
            ]),
            "2 files couldn't be attached."
        )
    }
}
