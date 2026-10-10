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
}
