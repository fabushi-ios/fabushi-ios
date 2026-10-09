import XCTest
@testable import Fabushi

final class SharedMediaParityTests:XCTestCase {
    func testExtensionsAndMimeTablesMatchReference() {
        XCTAssertEqual(SharedMediaExtensions.extensionOf("/tmp/PHOTO.JPEG"),".jpeg")
        XCTAssertEqual(SharedMediaExtensions.extensionOf(".env"),"")
        XCTAssertEqual(SharedImageMime.imageMime(fromPath:"a.png"),"image/png")
        XCTAssertEqual(SharedImageMime.servableImageMime(fromPath:"a.heic"),"image/heic")
        XCTAssertEqual(SharedImageMime.extensionFromImageMime("IMAGE/JPEG"),".jpg")
        XCTAssertEqual(SharedImageMime.videoMime(fromPath:"clip.MOV"),"video/quicktime")
        XCTAssertEqual(SharedImageMime.audioMime(fromPath:"voice.opus"),"audio/ogg")
    }
    func testAttachmentLimitsAndPreviewPolicy() {
        XCTAssertEqual(AttachmentLimits.attachmentByteLimit(forName:"clip.mp4"),200*1024*1024)
        XCTAssertEqual(AttachmentLimits.attachmentByteLimit(forName:"notes.txt"),25*1024*1024)
        XCTAssertEqual(AttachmentLimits.formatTooLargeNotice(filename:"clip.mp4"),"\"clip.mp4\" is too large to attach (max 200 MB for video).")
        XCTAssertTrue(AttachmentPreviewPolicy.isTextPreviewableName("main.swift"))
        XCTAssertFalse(AttachmentPreviewPolicy.looksLikeBinary(Data("hello\nworld".utf8)))
        XCTAssertTrue(AttachmentPreviewPolicy.looksLikeBinary(Data([0x41,0x00,0x42])))
    }
    func testAttachmentKindClassificationAndSummary() {
        XCTAssertEqual(AttachmentClassification.classify(.init(mimeType:"application/vnd.example+json")),.json)
        XCTAssertEqual(AttachmentClassification.classify(.init(fileName:"report.docx")),.document)
        XCTAssertEqual(AttachmentClassification.classify(.init(urlOrPath:"https://example.com/a%20b.tar")),.archive)
        XCTAssertEqual(AttachmentSummaryText.formatSentSummary(count:3,kinds:[.init(kind:"image",count:2),.init(kind:"unknown",count:1)]),"Sent 3 files · 2 images, 1 file")
    }
    func testFilePreviewKindsAndMarkdownFlattening() {
        XCTAssertEqual(FilePreviewPolicy.kind(for:"report.xlsx"),.table)
        XCTAssertEqual(FilePreviewPolicy.kind(for:"notes.mdx"),.markdown)
        XCTAssertEqual(FilePreviewPolicy.kind(for:"archive.bin"),.unknown)
        XCTAssertTrue(FilePreviewPolicy.needsBytes(.markdown))
        XCTAssertFalse(FilePreviewPolicy.needsBytes(.image))
        XCTAssertEqual(MarkdownPreview.toPreviewText("# **Title**\n- [link](https://example.com) code"),"Title link code")
    }
    func testImageDimensionsUsesNativeImageIO() {
        let png=Data([
            0x89,0x50,0x4E,0x47,0x0D,0x0A,0x1A,0x0A,0x00,0x00,0x00,0x0D,0x49,0x48,0x44,0x52,
            0x00,0x00,0x00,0x01,0x00,0x00,0x00,0x01,0x08,0x06,0x00,0x00,0x00,0x1F,0x15,0xC4,0x89,
            0x00,0x00,0x00,0x0D,0x49,0x44,0x41,0x54,0x08,0xD7,0x63,0xF8,0xCF,0xC0,0xF0,0x1F,0x00,
            0x05,0x00,0x01,0xFF,0x89,0x99,0x3D,0x1D,0x00,0x00,0x00,0x00,0x49,0x45,0x4E,0x44,0xAE,0x42,0x60,0x82,
        ])
        XCTAssertEqual(ImageFileDimensions.readPng(png),.init(width:1,height:1))
    }
    func testMP4TrackHeaderDimensionsParser() {
        var body=[UInt8](repeating:0,count:84)
        for (value,offset) in [(UInt32(640<<16),76),(UInt32(360<<16),80)] {
            body[offset]=UInt8((value>>24)&0xff); body[offset+1]=UInt8((value>>16)&0xff)
            body[offset+2]=UInt8((value>>8)&0xff); body[offset+3]=UInt8(value&0xff)
        }
        func box(_ type:String,_ body:[UInt8])->[UInt8] {
            let size=UInt32(body.count+8)
            return [UInt8((size>>24)&0xff),UInt8((size>>16)&0xff),UInt8((size>>8)&0xff),UInt8(size&0xff)] + Array(type.utf8) + body
        }
        let data=Data(box("moov",box("trak",box("tkhd",body))))
        XCTAssertEqual(MP4Dimensions.read(data),.init(width:640,height:360))
    }
    func testNativePdfPreviewDetectionAndDesktopPreviewCap() {
        XCTAssertTrue(isNativePdfAttachment(mimeType: "application/pdf", fileName: "report.bin"))
        XCTAssertTrue(isNativePdfAttachment(mimeType: nil, fileName: "Report.PDF"))
        XCTAssertFalse(isNativePdfAttachment(mimeType: "text/plain", fileName: "report.txt"))
        XCTAssertEqual(nativePdfPreviewByteCap, 25 * 1024 * 1024)
    }

}
