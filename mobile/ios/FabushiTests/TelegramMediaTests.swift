//
//  TelegramMediaTests.swift
//  FabushiTests
//
//  Comprehensive native Swift unit test suite covering Features 25 and 26:
//  Fullscreen Interactive Media Gallery Viewer (F25) and
//  Chunked Media Upload & Streaming Bridge (F26).
//

#if canImport(XCTest)
import XCTest
#if canImport(Fabushi)
@testable import Fabushi
#endif
import SwiftUI

@MainActor
public final class TelegramMediaTests: XCTestCase, @unchecked Sendable {

    // =========================================================================
    // MARK: - FEATURE 25: Fullscreen Interactive Media Gallery Viewer
    // =========================================================================

    public func testTC_T1_F25_01_MediaViewerInitialization() {
        let items = [
            TelegramMediaItem(id: "m1", messageId: 10, chatId: 1, type: .photo, width: 1920, height: 1080),
            TelegramMediaItem(id: "m2", messageId: 11, chatId: 1, type: .video, width: 1280, height: 720, durationSeconds: 15.0)
        ]
        let vm = MediaViewerViewModel(items: items, initialItemId: "m1")
        XCTAssertEqual(vm.items.count, 2)
        XCTAssertEqual(vm.selectedItemId, "m1")
        XCTAssertEqual(vm.currentIndex, 0)
        XCTAssertEqual(vm.currentItem?.type, .photo)
    }

    public func testTC_T1_F25_02_VelocitySwipeDownDismissPhysics() {
        let items = [TelegramMediaItem(id: "m1", messageId: 1, chatId: 1)]
        let vm = MediaViewerViewModel(items: items)

        // 1. Distance below 160pt with low velocity snaps back
        var didDismiss = false
        vm.handleDragEnded(translation: CGSize(width: 0, height: 100), velocityY: 200) {
            didDismiss = true
        }
        XCTAssertFalse(didDismiss, "Distance 100pt (<160pt) and velocity 200pt/s must not dismiss")
        XCTAssertEqual(vm.dismissOffsetY, 0.0, "Snapback must restore dismissOffsetY to 0")

        // 2. Distance >= 160pt triggers dismissal
        vm.handleDragEnded(translation: CGSize(width: 0, height: 160), velocityY: 100) {
            didDismiss = true
        }
        XCTAssertTrue(didDismiss, "Distance >= 160pt must trigger dismissal")

        // 3. High velocity >= 800pt/s triggers dismissal even with small distance (e.g. 50pt)
        didDismiss = false
        vm.handleDragEnded(translation: CGSize(width: 0, height: 50), velocityY: 850) {
            didDismiss = true
        }
        XCTAssertTrue(didDismiss, "Velocity >= 800pt/s must trigger velocity dismissal")
    }

    public func testTC_T1_F25_03_PinchToZoomScalingBounds() {
        let items = [TelegramMediaItem(id: "m1", messageId: 1, chatId: 1)]
        let vm = MediaViewerViewModel(items: items)

        // Zoom in 2.0x
        vm.handlePinch(scale: 2.0)
        XCTAssertEqual(vm.currentScale, 2.0)

        // Zoom out beyond minimum (0.2x) -> must clamp to 1.0x
        vm.handlePinch(scale: 0.2)
        XCTAssertEqual(vm.currentScale, 1.0)

        // Zoom in beyond maximum (10.0x) -> must clamp to 5.0x
        vm.handlePinch(scale: 10.0)
        XCTAssertEqual(vm.currentScale, 5.0)
    }

    public func testTC_T1_F25_04_DoubleTapZoomToggle() {
        let items = [TelegramMediaItem(id: "m1", messageId: 1, chatId: 1)]
        let vm = MediaViewerViewModel(items: items)

        XCTAssertEqual(vm.currentScale, 1.0)
        vm.handleDoubleTap()
        XCTAssertEqual(vm.currentScale, 2.5, "First double-tap must zoom to 2.5x")

        vm.handleDoubleTap()
        XCTAssertEqual(vm.currentScale, 1.0, "Second double-tap must return to 1.0x")
    }

    public func testTC_T1_F25_05_BottomFilmstripScrubberSync() {
        let items = (1...5).map { TelegramMediaItem(id: "m\($0)", messageId: Int64($0), chatId: 1) }
        let vm = MediaViewerViewModel(items: items)

        XCTAssertEqual(vm.currentIndex, 0)
        vm.selectItem(at: 3)
        XCTAssertEqual(vm.currentIndex, 3)
        XCTAssertEqual(vm.selectedItemId, "m4")
    }

    public func testTC_T2_F25_02_SimultaneousPinchAndSwipeDownPanRouting() {
        let items = [TelegramMediaItem(id: "m1", messageId: 1, chatId: 1)]
        let vm = MediaViewerViewModel(items: items)

        // Set zoom to 2.5x
        vm.currentScale = 2.5

        // Drag down: should route to panOffset, NOT dismissOffsetY
        vm.handleDragChanged(translation: CGSize(width: 10, height: 80))
        XCTAssertEqual(vm.dismissOffsetY, 0.0, "Zoomed drag must not trigger dismiss offset")
        XCTAssertEqual(vm.panOffset.height, 80.0, "Zoomed drag must accumulate pan offset")
    }

    public func testTC_T2_F25_03_DeletedMediaWhileViewing() {
        let items = [
            TelegramMediaItem(id: "m1", messageId: 101, chatId: 1),
            TelegramMediaItem(id: "m2", messageId: 102, chatId: 1)
        ]
        let vm = MediaViewerViewModel(items: items, initialItemId: "m1")

        var didDismiss = false
        vm.handleMessageDeleted(messageId: 101) {
            didDismiss = true
        }

        XCTAssertEqual(vm.deletedMessageToast, "Message Deleted")
        XCTAssertFalse(vm.items.contains { $0.messageId == 101 })
    }

    public func testTC_T2_F25_04_ExtremeAspectRatioPanoramic() {
        let panoramic = TelegramMediaItem(
            id: "pano",
            messageId: 99,
            chatId: 1,
            width: 20000,
            height: 1000
        )
        let aspectRatio = panoramic.width / max(1, panoramic.height)
        XCTAssertEqual(aspectRatio, 20.0, "Panoramic ratio must calculate correctly without overflow")
    }

    // =========================================================================
    // MARK: - FEATURE 26: Chunked Media Streaming Bridge
    // =========================================================================

    public func testTC_T1_F26_01_ResumableChunkPartitioning5MB() throws {
        let fiveMB = 5 * 1024 * 1024 // 5 MB = 5,242,880 bytes
        let sampleData = Data(repeating: 0xAB, count: fiveMB)

        let chunks = try MediaTransferManager.partition(data: sampleData, chunkSize: 512 * 1024)
        XCTAssertEqual(chunks.count, 10, "5MB file must cleanly partition into exactly 10 chunks of 512KB")
        XCTAssertEqual(chunks.first?.count, 512 * 1024)
        XCTAssertEqual(chunks.last?.count, 512 * 1024)
    }

    public func testTC_T1_F26_02_SHA256CASVerification() {
        let sample = "Hello Telegram Chunked CAS Verification".data(using: .utf8)!
        let hash = MediaTransferManager.computeSHA256(data: sample)
        XCTAssertEqual(hash.count, 64, "SHA-256 hash string must be 64 hexadecimal characters")
        // Check reproducibility
        let hash2 = MediaTransferManager.computeSHA256(data: sample)
        XCTAssertEqual(hash, hash2)
    }

    public func testTC_T1_F26_03_UploadProgressMonotonicity() async throws {
        let manager = MediaTransferManager()
        let tmpDir = FileManager.default.temporaryDirectory
        let testFileURL = tmpDir.appendingPathComponent("test_5chunk.dat")
        let totalSize = 5 * 512 * 1024
        try Data(repeating: 0x42, count: totalSize).write(to: testFileURL)
        defer { try? FileManager.default.removeItem(at: testFileURL) }

        var reportedFractions: [Double] = []

        let stream = AsyncStream<MediaTransferManager.TransferProgress> { continuation in
            Task {
                _ = try? await manager.uploadMedia(
                    fileURL: testFileURL,
                    mimeType: "application/octet-stream",
                    fileName: "test_5chunk.dat",
                    mockBridge: true,
                    progressStream: continuation
                )
                continuation.finish()
            }
        }

        for await event in stream {
            if case .progress(_, let frac, _, _) = event {
                reportedFractions.append(frac)
            }
        }

        XCTAssertEqual(reportedFractions.count, 5, "Must emit 5 progress updates for 5 chunks")
        // Monotonic check: 0.2, 0.4, 0.6, 0.8, 1.0
        XCTAssertEqual(reportedFractions, [0.2, 0.4, 0.6, 0.8, 1.0])
    }

    public func testTC_T2_F26_03_ZeroLengthChunkHandlingThrows() {
        XCTAssertThrowsError(try MediaTransferManager.partition(data: Data(), chunkSize: 512 * 1024)) { error in
            XCTAssertEqual(error as? MediaTransferError, MediaTransferError.emptyFile)
        }
    }

    public func testTC_T2_F26_05_LocalCacheHitBypassesNetwork() async throws {
        let manager = MediaTransferManager()
        let cacheDir = try MediaTransferManager.mediaCacheDirectory()
        let fakeSha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        let cachedFile = cacheDir.appendingPathComponent("\(fakeSha256).jpg")
        try "cached_data".data(using: .utf8)!.write(to: cachedFile)
        defer { try? FileManager.default.removeItem(at: cachedFile) }

        let resultURL = try await manager.downloadMedia(
            mediaId: "media_cached",
            fileSha256: fakeSha256,
            totalBytes: 11,
            fileName: "photo.jpg"
        )

        XCTAssertEqual(resultURL.path, cachedFile.path, "Cache hit must immediately return local file URL")
    }
}
#endif
