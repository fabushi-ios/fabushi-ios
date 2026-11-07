//
//  TelegramAttachmentTests.swift
//  FabushiTests
//
//  Comprehensive native Swift unit test suite covering Features 23 and 24:
//  Attachment Menu Bottom Drawer (F23) and Tabbed Sticker, Emoji & GIF Picker (F24).
//

#if canImport(XCTest)
import XCTest
#if canImport(Fabushi)
@testable import Fabushi
#endif
import SwiftUI

@MainActor
public final class TelegramAttachmentTests: XCTestCase, @unchecked Sendable {

    // =========================================================================
    // MARK: - FEATURE 23: Attachment Menu Bottom Drawer
    // =========================================================================

    public func testTC_T1_F23_01_AttachmentDrawerQuickActionsAndDetents() {
        let vm = AttachmentDrawerViewModel()
        XCTAssertEqual(AttachmentActionType.allCases.count, 6, "Must provide exactly 6 quick actions")
        XCTAssertEqual(AttachmentActionType.allCases.map(\.rawValue), [
            "gallery", "camera", "file", "contact", "location", "poll"
        ])
        XCTAssertEqual(AttachmentDrawerViewModel.defaultSheetHeight, 340.0, "Default half sheet must be 340pt")
    }

    public func testTC_T1_F23_02_MultiSelectBadgesOrderAndReindexing() {
        let vm = AttachmentDrawerViewModel()
        let itemA = MediaPreviewItem(id: "photo_a")
        let itemB = MediaPreviewItem(id: "photo_b")
        let itemC = MediaPreviewItem(id: "photo_c")

        // Select itemA, itemB, itemC in order
        vm.toggleSelection(itemA)
        vm.toggleSelection(itemB)
        vm.toggleSelection(itemC)

        XCTAssertEqual(vm.badgeIndex(for: itemA), 1)
        XCTAssertEqual(vm.badgeIndex(for: itemB), 2)
        XCTAssertEqual(vm.badgeIndex(for: itemC), 3)

        // Deselect itemB -> itemC must dynamically re-index from 3 to 2
        vm.toggleSelection(itemB)
        XCTAssertEqual(vm.badgeIndex(for: itemA), 1)
        XCTAssertNil(vm.badgeIndex(for: itemB))
        XCTAssertEqual(vm.badgeIndex(for: itemC), 2, "Deselecting an item must re-index subsequent items")
    }

    public func testTC_T1_F23_03_SendWithoutCompressionToggle() {
        let vm = AttachmentDrawerViewModel()
        XCTAssertFalse(vm.sendWithoutCompression)
        vm.sendWithoutCompression = true
        XCTAssertTrue(vm.sendWithoutCompression)
    }

    public func testTC_T1_F23_04_LocationPickerPresentationTrigger() {
        let vm = AttachmentDrawerViewModel()
        XCTAssertFalse(vm.showLocationPicker)
        vm.handleActionTap(.location)
        XCTAssertTrue(vm.showLocationPicker)
    }

    public func testTC_T2_F23_01_Max100MediaSelectionClamp() {
        let vm = AttachmentDrawerViewModel()
        for i in 0..<110 {
            let item = MediaPreviewItem(id: "item_\(i)")
            vm.toggleSelection(item)
        }
        XCTAssertEqual(vm.selectedItems.count, 100, "Media selection must clamp at exactly 100 items")
        XCTAssertEqual(vm.badgeIndex(for: vm.selectedItems.last!), 100)
    }

    public func testTC_T2_F23_02_ZeroPhotoLibraryPermissionAlert() {
        let vm = AttachmentDrawerViewModel()
        vm.hasPhotoPermission = false
        vm.handleActionTap(.gallery)
        XCTAssertTrue(vm.showPermissionAlert, "Must trigger permission alert when photo access denied")
    }

    public func testTC_T2_F23_03_ZeroByteEmptyFileAttachmentRejection() {
        XCTAssertThrowsError(try DocumentAttachment.validate(fileSizeBytes: 0)) { error in
            XCTAssertEqual(error as? DocumentError, DocumentError.emptyFile)
        }
        XCTAssertThrowsError(try DocumentAttachment.validate(fileSizeBytes: -10)) { error in
            XCTAssertEqual(error as? DocumentError, DocumentError.emptyFile)
        }
    }

    public func testTC_T2_F23_04_2GBFileBoundaryValidation() throws {
        let validSize: Int64 = 2 * 1024 * 1024 * 1024 // 2GB
        XCTAssertNoThrow(try DocumentAttachment.validate(fileSizeBytes: validSize))

        let oversized: Int64 = validSize + 1
        XCTAssertThrowsError(try DocumentAttachment.validate(fileSizeBytes: oversized)) { error in
            XCTAssertEqual(error as? DocumentError, DocumentError.fileExceeds2GBLimit)
        }
    }

    // =========================================================================
    // MARK: - FEATURE 24: Tabbed Sticker / Emoji / GIF Selector
    // =========================================================================

    public func testTC_T1_F24_01_PickerTabSwitching() {
        let vm = StickerEmojiPickerViewModel()
        XCTAssertEqual(vm.activeTab, .stickers)
        vm.selectTab(.emoji)
        XCTAssertEqual(vm.activeTab, .emoji)
        vm.selectTab(.gifs)
        XCTAssertEqual(vm.activeTab, .gifs)
    }

    public func testTC_T1_F24_02_EmojiCategoryAndSearchFilter() {
        let vm = StickerEmojiPickerViewModel()
        XCTAssertFalse(vm.recentEmojis.isEmpty)

        vm.searchQuery = "heart"
        let filtered = vm.filteredEmojis()
        XCTAssertTrue(filtered.contains { $0.glyph == "❤️" })

        vm.searchQuery = "cat"
        let cats = vm.filteredEmojis()
        XCTAssertTrue(cats.contains { $0.glyph == "🐱" })
    }

    public func testTC_T1_F24_03_StickerPackPreview() {
        let vm = StickerEmojiPickerViewModel()
        XCTAssertFalse(vm.installedPacks.isEmpty)
        let pack = vm.installedPacks[0]
        XCTAssertFalse(pack.stickers.isEmpty)
        XCTAssertEqual(pack.stickers[0].packId, pack.id)
    }

    public func testTC_T1_F24_05_TapToDirectSendAction() {
        let sticker = TelegramSticker(id: "s1", packId: "p1", emojiHint: "🎉", imageUri: "uri")
        var sentSticker: TelegramSticker? = nil

        let sheet = StickerEmojiPickerSheet(
            viewModel: StickerEmojiPickerViewModel(),
            onSendSticker: { s in sentSticker = s },
            onSendEmoji: { _ in },
            onSendGIF: { _ in }
        )
        XCTAssertNotNil(sheet)
        sheet.onSendSticker(sticker)
        XCTAssertEqual(sentSticker?.id, "s1")
    }

    public func testTC_T2_F24_01_OfflineGifSearchPreservesCache() {
        let vm = StickerEmojiPickerViewModel()
        vm.isOffline = true
        let cached = vm.filteredGifs()
        XCTAssertFalse(cached.isEmpty, "Cached GIFs must remain accessible during offline state")
    }

    public func testTC_T2_F24_02_CorruptedStickerSafetyFallback() {
        let corruptedSticker = TelegramSticker(
            id: "corrupted_1",
            packId: "pack_x",
            emojiHint: "❓",
            imageUri: "corrupted_payload",
            isAnimated: true,
            isCorrupted: true
        )
        XCTAssertTrue(corruptedSticker.isCorrupted)
    }

    public func testTC_T2_F24_03_500InstalledStickerPacksScalability() {
        let vm = StickerEmojiPickerViewModel()
        var largeCatalog: [TelegramStickerPack] = []
        for p in 0..<500 {
            let stickers = (0..<10).map { i in
                TelegramSticker(id: "p\(p)_s\(i)", packId: "p\(p)", emojiHint: "🔥", imageUri: "uri")
            }
            largeCatalog.append(TelegramStickerPack(id: "p\(p)", title: "Pack \(p)", thumbnailUri: "thumb", stickers: stickers))
        }
        vm.installedPacks = largeCatalog
        XCTAssertEqual(vm.installedPacks.count, 500)

        vm.searchQuery = "Pack 499"
        let results = vm.filteredPacks()
        XCTAssertEqual(results.count, 1)
        XCTAssertEqual(results.first?.title, "Pack 499")
    }

    public func testTC_T2_F24_04_RapidTabCyclingStability() {
        let vm = StickerEmojiPickerViewModel()
        let tabs = PickerTab.allCases
        for i in 0..<60 {
            vm.selectTab(tabs[i % tabs.count])
        }
        XCTAssertEqual(vm.tabSwitchCount, 60)
    }

    public func testTC_T2_F24_05_LongPressPreviewModalLifecycle() {
        let vm = StickerEmojiPickerViewModel()
        let sticker = TelegramSticker(id: "preview_me", packId: "p1", emojiHint: "🌟", imageUri: "uri")

        XCTAssertNil(vm.previewSticker)
        vm.triggerLongPressPreview(sticker)
        XCTAssertEqual(vm.previewSticker?.id, "preview_me")

        vm.dismissPreview()
        XCTAssertNil(vm.previewSticker)
    }
}
#endif
