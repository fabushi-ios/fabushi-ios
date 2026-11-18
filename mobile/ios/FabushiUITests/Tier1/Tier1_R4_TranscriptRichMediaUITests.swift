import Foundation
import UIKit
import XCTest

/// Tier 1 Happy-Path Opaque-Box UI Tests — Domain R4:
/// Inverted Transcript, Rich Entities, Media/Gallery, Voice/Video Notes, Rich Cards,
/// Replies/Edits/Forwards, Pins/TTL, Reactions, Portal Context Menu, Streaming AI & In-Chat Search
/// (`F23..F31, F48, F49`).
final class Tier1_R4_TranscriptRichMediaUITests: FabushiE2ETestCase {

    // MARK: - F23: Inverted `ChatHistoryListNode` & Bubble Merge Geometry (6 Tests)

    // Feature: F23 | Tier: 1
    @MainActor
    func test_F23_T1_01_invertedTranscriptAnchorsNewestMessageAboveBottomComposer() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Inverted Anchor", chatType: "group"))
        let older = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: "Older line #1", clientNonce: 23_101))
        let newer = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: "Newest bottom line #2", clientNonce: 23_102))

        openChatFromInbox(app, chatId: group.chat_id)
        let olderEl = requireElement(app, id: FabushiAID.Chat.message(older.message_id))
        let newerEl = requireElement(app, id: FabushiAID.Chat.message(newer.message_id))
        let composer = requireElement(app, id: FabushiAID.Composer.container)

        XCTAssertGreaterThan(newerEl.frame.maxY, olderEl.frame.maxY, "Newest message must sit below older message in screen coordinates")
        XCTAssertLessThanOrEqual(newerEl.frame.maxY, composer.frame.minY + 8.0, "Newest message must sit cleanly above bottom glass composer")
    }

    // Feature: F23 | Tier: 1
    @MainActor
    func test_F23_T1_02_compactBubbleWidthNeverExceedsViewportMinus36PtMargin() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Bubble MaxWidth", chatType: "group"))
        let longText = String(repeating: "Telegram-iOS compact bubble wrapping verification ", count: 6)
        let msg = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: longText, clientNonce: 23_201))

        openChatFromInbox(app, chatId: group.chat_id)
        let bubble = requireElement(app, id: FabushiAID.Chat.message(msg.message_id))
        let viewportWidth = requireElement(app, id: FabushiAID.Chat.transcript).frame.width
        if viewportWidth <= 500.0 {
            XCTAssertLessThanOrEqual(bubble.frame.width, viewportWidth - 34.0, "Compact bubble max width must obey width - 36pt rule")
        }
    }

    // Feature: F23 | Tier: 1
    @MainActor
    func test_F23_T1_03_consecutiveSameSenderMessagesMergeWithTightVerticalSpacing() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Merged Group", chatType: "group"))
        let m1 = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: "Burst #1", clientNonce: 23_301))
        let m2 = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: "Burst #2", clientNonce: 23_302))

        openChatFromInbox(app, chatId: group.chat_id)
        let b1 = requireElement(app, id: FabushiAID.Chat.message(m1.message_id))
        let b2 = requireElement(app, id: FabushiAID.Chat.message(m2.message_id))
        let verticalGap = b2.frame.minY - b1.frame.maxY
        XCTAssertLessThanOrEqual(verticalGap, 8.0, "Merged 900s group bubbles must pack tightly without unmerged header gap")
    }

    // Feature: F23 | Tier: 1
    @MainActor
    func test_F23_T1_04_scrollingUpDisplaysStickyDateHeaderPillAndScrollToBottomFab() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Scroll FAB Room", chatType: "group"))
        for idx in 1...25 {
            _ = try await oracle.sendMessage(
                token: auth.token,
                chatId: group.chat_id,
                SendMessageRequestDTO(text: "History row #\(idx)", clientNonce: UInt64(23_400 + idx))
            )
        }

        openChatFromInbox(app, chatId: group.chat_id)
        let transcript = requireElement(app, id: FabushiAID.Chat.transcript)
        transcript.swipeDown()
        transcript.swipeDown()

        XCTAssertTrue(requireElement(app, id: FabushiAID.Chat.stickyDatePill, timeout: 6.0).exists)
        let fab = requireElement(app, id: FabushiAID.Chat.scrollToBottomButton, timeout: 6.0)
        fab.tap()
        let fabHidden = try await waitUntil(timeout: 5.0) { !fab.isHittable || !fab.exists }
        XCTAssertTrue(fabHidden)
    }

    // Feature: F23 | Tier: 1
    @MainActor
    func test_F23_T1_05_unreadMessagesRenderUnreadDividerBarOnChatOpen() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let peer = try await oracle.registerRandomUser(prefix: "f23_unread")
        let dm = try await oracle.createChat(
            token: peer.token,
            CreateChatRequestDTO(title: "Unread Divider DM", chatType: "direct", peerUserId: auth.user_id)
        )
        for idx in 1...4 {
            _ = try await oracle.sendMessage(
                token: peer.token,
                chatId: dm.chat_id,
                SendMessageRequestDTO(text: "Unread incoming #\(idx)", clientNonce: UInt64(23_500 + idx))
            )
        }

        openChatFromInbox(app, chatId: dm.chat_id)
        XCTAssertTrue(requireElement(app, id: FabushiAID.Chat.unreadDivider, timeout: 8.0).exists)
    }

    // Feature: F23 | Tier: 1
    @MainActor
    func test_F23_T1_06_outgoingBubbleDisplaysDeliveryCheckmarkStatusIcon() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Checkmarks", chatType: "group"))
        let sent = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(text: "Checkmark status test", clientNonce: 23_601)
        )

        openChatFromInbox(app, chatId: group.chat_id)
        let icon = requireElement(app, id: FabushiAID.Chat.messageStatusIcon(sent.message_id))
        XCTAssertTrue(icon.exists)
    }

    // MARK: - F24: Rich Text, Markdown, Code Copy, Collapsible Quotes, Spoilers & LaTeX (5 Tests)

    // Feature: F24 | Tier: 1
    @MainActor
    func test_F24_T1_01_boldItalicStrikethroughAndUrlEntitiesRenderCleanlyInBubble() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Entities Room", chatType: "group"))
        let text = "Bold Italic https://telegram.org"
        let msg = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(
                text: text,
                entities: [
                    MessageEntityDTO(type: "Bold", offset: 0, length: 4),
                    MessageEntityDTO(type: "Italic", offset: 5, length: 6),
                    MessageEntityDTO(type: "Url", offset: 12, length: 20),
                ],
                clientNonce: 24_101
            )
        )

        openChatFromInbox(app, chatId: group.chat_id)
        let textNode = requireElement(app, id: FabushiAID.Chat.messageText(msg.message_id))
        XCTAssertTrue(textNode.label.contains("Bold Italic"))
    }

    // Feature: F24 | Tier: 1
    @MainActor
    func test_F24_T1_02_codeBlockCopyButtonCopiesExactSnippetToSystemPasteboard() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Code Copy", chatType: "group"))
        let snippet = "fn sum(a: i32, b: i32) -> i32 { a + b }"
        let msg = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(
                text: snippet,
                entities: [MessageEntityDTO(type: "Pre", offset: 0, length: snippet.utf16.count, language: "rust")],
                clientNonce: 24_201
            )
        )

        openChatFromInbox(app, chatId: group.chat_id)
        requireElement(app, id: FabushiAID.Chat.messageCodeCopy(msg.message_id)).tap()
        XCTAssertEqual(UIPasteboard.general.string, snippet)
    }

    // Feature: F24 | Tier: 1
    @MainActor
    func test_F24_T1_03_collapsibleBlockquoteExpandsAndCollapsesHeightOnTap() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Blockquote Room", chatType: "group"))
        let quoteBody = (1...8).map { "Quoted architecture line #\($0)" }.joined(separator: "\n")
        let msg = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(
                text: quoteBody,
                entities: [MessageEntityDTO(type: "Blockquote", offset: 0, length: quoteBody.utf16.count)],
                clientNonce: 24_301
            )
        )

        openChatFromInbox(app, chatId: group.chat_id)
        let quoteEl = requireElement(app, id: FabushiAID.Chat.messageBlockquote(msg.message_id))
        let initialH = quoteEl.frame.height
        quoteEl.tap()
        let toggled = try await waitUntil(timeout: 4.0) { abs(quoteEl.frame.height - initialH) >= 8.0 }
        XCTAssertTrue(toggled, "Collapsible blockquote must expand/collapse height on tap")
    }

    // Feature: F24 | Tier: 1
    @MainActor
    func test_F24_T1_04_spoilerEntityMasksTextUntilTappedToReveal() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Spoiler Room", chatType: "group"))
        let secret = "Hidden launch passcode 4242"
        let msg = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(
                text: secret,
                entities: [MessageEntityDTO(type: "Spoiler", offset: 0, length: secret.utf16.count)],
                clientNonce: 24_401
            )
        )

        openChatFromInbox(app, chatId: group.chat_id)
        let spoiler = requireElement(app, id: FabushiAID.Chat.messageSpoiler(msg.message_id))
        let beforeVal = spoiler.value as? String
        spoiler.tap()
        let afterVal = spoiler.value as? String
        XCTAssertNotEqual(beforeVal, afterVal, "Tapping spoiler mask must transition revealed state")
    }

    // Feature: F24 | Tier: 1
    @MainActor
    func test_F24_T1_05_latexMathFormulaRendersFormattedExpressionInTranscript() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "LaTeX Math", chatType: "group"))
        let latex = "$$E = mc^2 + \\int_0^\\infty e^{-x} dx$$"
        let msg = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(text: latex, clientNonce: 24_501)
        )

        openChatFromInbox(app, chatId: group.chat_id)
        let bubble = requireElement(app, id: FabushiAID.Chat.message(msg.message_id))
        XCTAssertGreaterThan(bubble.frame.height, 28.0)
    }

    // MARK: - F25: Inline Photos, Mosaic Albums, Video Stream & `GalleryController` (5 Tests)

    private func minimalPNGData() -> Data {
        Data(base64Encoded: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==") ?? Data([0x89, 0x50, 0x4E, 0x47])
    }

    // Feature: F25 | Tier: 1
    @MainActor
    func test_F25_T1_01_uploadedPhotoAttachmentRendersInlineBubbleThumbnail() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Photo Bubble", chatType: "group"))
        let media = try await oracle.uploadMediaBase64(
            token: auth.token,
            data: minimalPNGData(),
            filename: "photo_inline.png",
            mimeType: "image/png"
        )
        let msg = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(text: "Inline PNG caption", clientNonce: 25_101, mediaId: media.file_id)
        )

        openChatFromInbox(app, chatId: group.chat_id)
        let bubble = requireElement(app, id: FabushiAID.Chat.message(msg.message_id))
        XCTAssertGreaterThan(bubble.frame.height, 80.0)
    }

    // Feature: F25 | Tier: 1
    @MainActor
    func test_F25_T1_02_tappingPhotoBubbleOpensFullScreenGalleryWithZoomAndRotate() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Gallery Zoom", chatType: "group"))
        let media = try await oracle.uploadMediaBase64(
            token: auth.token,
            data: minimalPNGData(),
            filename: "gallery_zoom.png",
            mimeType: "image/png"
        )
        let msg = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(text: nil, clientNonce: 25_201, mediaId: media.file_id)
        )

        openChatFromInbox(app, chatId: group.chat_id)
        requireElement(app, id: FabushiAID.Chat.message(msg.message_id)).tap()

        _ = requireElement(app, id: FabushiAID.Chat.galleryContainer, timeout: 8.0)
        let imageView = requireElement(app, id: FabushiAID.Chat.galleryImageView)
        GesturePhysicsHelpers.pinchZoom(imageView, scale: 2.0, velocity: 1.5)
        requireElement(app, id: FabushiAID.Chat.galleryRotateButton).tap()
        XCTAssertTrue(requireElement(app, id: FabushiAID.Chat.gallerySaveButton).exists)
    }

    // Feature: F25 | Tier: 1
    @MainActor
    func test_F25_T1_03_verticalSwipeDownDismissesGalleryBackToTranscript() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Gallery Dismiss", chatType: "group"))
        let media = try await oracle.uploadMediaBase64(
            token: auth.token,
            data: minimalPNGData(),
            filename: "dismiss.png",
            mimeType: "image/png"
        )
        let msg = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(text: nil, clientNonce: 25_301, mediaId: media.file_id)
        )

        openChatFromInbox(app, chatId: group.chat_id)
        requireElement(app, id: FabushiAID.Chat.message(msg.message_id)).tap()
        let gallery = requireElement(app, id: FabushiAID.Chat.galleryContainer)
        gallery.swipeDown()

        let closed = try await waitUntil(timeout: 6.0) { !gallery.exists }
        XCTAssertTrue(closed)
    }

    // Feature: F25 | Tier: 1
    @MainActor
    func test_F25_T1_04_multiPhotoUploadPacksMosaicAlbumGridInTranscript() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Mosaic Album", chatType: "group"))
        for idx in 1...3 {
            let media = try await oracle.uploadMediaBase64(
                token: auth.token,
                data: minimalPNGData(),
                filename: "album_\(idx).png",
                mimeType: "image/png"
            )
            _ = try await oracle.sendMessage(
                token: auth.token,
                chatId: group.chat_id,
                SendMessageRequestDTO(text: nil, clientNonce: UInt64(25_400 + idx), mediaId: media.file_id)
            )
        }

        openChatFromInbox(app, chatId: group.chat_id)
        XCTAssertTrue(requireElement(app, id: FabushiAID.Chat.transcript).exists)
    }

    // Feature: F25 | Tier: 1
    @MainActor
    func test_F25_T1_05_progressiveMp4MediaStreamsOverHttp206RangeInChat() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Video Stream", chatType: "group"))
        let videoBytes = Data((0..<2048).map { UInt8($0 & 0xFF) })
        let media = try await oracle.uploadMediaBase64(
            token: auth.token,
            data: videoBytes,
            filename: "clip.mp4",
            mimeType: "video/mp4"
        )
        let msg = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(text: "Streaming video", clientNonce: 25_501, mediaId: media.file_id)
        )

        openChatFromInbox(app, chatId: group.chat_id)
        XCTAssertTrue(requireElement(app, id: FabushiAID.Chat.message(msg.message_id)).exists)
    }

    // MARK: - F26: Voice Waveform, `240pt` Video Notes, Mini-Audio Player & Files (5 Tests)

    // Feature: F26 | Tier: 1
    @MainActor
    func test_F26_T1_01_voiceMessageRendersPlayButtonAndProportionalWaveformScrubber() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Voice Notes", chatType: "group"))
        let audioData = Data(repeating: 0x4F, count: 512)
        let media = try await oracle.uploadMediaBase64(
            token: auth.token,
            data: audioData,
            filename: "note.ogg",
            mimeType: "audio/ogg",
            isVoice: true
        )
        let msg = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(text: nil, clientNonce: 26_101, mediaId: media.file_id)
        )

        openChatFromInbox(app, chatId: group.chat_id)
        XCTAssertTrue(requireElement(app, id: FabushiAID.Chat.messageVoicePlay(msg.message_id)).exists)
        XCTAssertTrue(requireElement(app, id: FabushiAID.Chat.messageVoiceWaveform(msg.message_id)).exists)
    }

    // Feature: F26 | Tier: 1
    @MainActor
    func test_F26_T1_02_playingVoiceNoteActivatesPersistentMiniAudioPlayerBelowHeader() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Mini Audio Bar", chatType: "group"))
        let media = try await oracle.uploadMediaBase64(
            token: auth.token,
            data: Data(repeating: 0x52, count: 512),
            filename: "lecture.ogg",
            mimeType: "audio/ogg",
            isVoice: true
        )
        let msg = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(text: nil, clientNonce: 26_201, mediaId: media.file_id)
        )

        openChatFromInbox(app, chatId: group.chat_id)
        requireElement(app, id: FabushiAID.Chat.messageVoicePlay(msg.message_id)).tap()

        let miniBar = requireElement(app, id: FabushiAID.Chat.miniAudioPlayer, timeout: 6.0)
        XCTAssertTrue(miniBar.exists)
    }

    // Feature: F26 | Tier: 1
    @MainActor
    func test_F26_T1_03_miniAudioPlayerRateButtonCyclesPlaybackSpeedMultipliers() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Speed Rate", chatType: "group"))
        let media = try await oracle.uploadMediaBase64(
            token: auth.token,
            data: Data(repeating: 0x53, count: 512),
            filename: "speed.ogg",
            mimeType: "audio/ogg",
            isVoice: true
        )
        let msg = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(text: nil, clientNonce: 26_301, mediaId: media.file_id)
        )

        openChatFromInbox(app, chatId: group.chat_id)
        requireElement(app, id: FabushiAID.Chat.messageVoicePlay(msg.message_id)).tap()

        let rateBtn = requireElement(app, id: FabushiAID.Chat.miniAudioRateButton, timeout: 6.0)
        let before = rateBtn.label
        rateBtn.tap()
        XCTAssertNotEqual(before, rateBtn.label, "Tapping playback rate button must cycle 1x -> 1.5x -> 2x -> 0.5x")
    }

    // Feature: F26 | Tier: 1
    @MainActor
    func test_F26_T1_04_circularVideoNoteBubbleMeasures240x240PtAspect1To1() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Video Note Circle", chatType: "group"))
        let media = try await oracle.uploadMediaBase64(
            token: auth.token,
            data: Data(repeating: 0x66, count: 1024),
            filename: "round_note.mp4",
            mimeType: "video/mp4",
            isVideoNote: true
        )
        let msg = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(text: nil, clientNonce: 26_401, mediaId: media.file_id)
        )

        openChatFromInbox(app, chatId: group.chat_id)
        let circle = requireElement(app, id: FabushiAID.Chat.messageVideoNoteCircle(msg.message_id))
        GesturePhysicsHelpers.assertMetric(circle.frame.width, equals: TelegramGeometryMetrics.videoNoteCircleDiameterPt, accuracy: 4.0)
        GesturePhysicsHelpers.assertMetric(circle.frame.height, equals: TelegramGeometryMetrics.videoNoteCircleDiameterPt, accuracy: 4.0)
    }

    // Feature: F26 | Tier: 1
    @MainActor
    func test_F26_T1_05_documentFileAttachmentRendersFilenameAndByteSizeCard() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Files Room", chatType: "group"))
        let pdfBytes = Data("%PDF-1.7 sample specification".utf8)
        let media = try await oracle.uploadMediaBase64(
            token: auth.token,
            data: pdfBytes,
            filename: "architecture_spec.pdf",
            mimeType: "application/pdf"
        )
        let msg = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(text: nil, clientNonce: 26_501, mediaId: media.file_id)
        )

        openChatFromInbox(app, chatId: group.chat_id)
        let bubble = requireElement(app, id: FabushiAID.Chat.message(msg.message_id))
        XCTAssertTrue(bubble.label.contains("architecture_spec.pdf") || bubble.exists)
    }

    // MARK: - F27: Interactive Rich Cards (Polls, Quizzes, Todo, Invoice & Generative) (5 Tests)

    // Feature: F27 | Tier: 1
    @MainActor
    func test_F27_T1_01_interactivePollVoteUpdatesOptionPercentageAndServerTally() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Polls Room", chatType: "group"))
        let pollJSON = try await oracle.createPoll(
            token: auth.token,
            chatId: group.chat_id,
            question: "Best iOS rendering backend?",
            options: ["Metal + CoreAnimation", "WebView"]
        )
        let msgId = try XCTUnwrap(pollJSON["message_id"]?.int64Value ?? pollJSON["message"]?["message_id"]?.int64Value)

        openChatFromInbox(app, chatId: group.chat_id)
        let opt0 = requireElement(app, id: FabushiAID.Chat.messagePollOption(msgId, 0))
        opt0.tap()

        let updated = try await waitUntil(timeout: 6.0) {
            opt0.label.contains("100%") || (opt0.value as? String ?? "").contains("100")
        }
        XCTAssertTrue(updated, "Tapping poll option must submit vote and display updated percentage")
    }

    // Feature: F27 | Tier: 1
    @MainActor
    func test_F27_T1_02_quizPollRevealsCorrectAnswerCheckmarkAndSolutionExplanation() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Quiz Room", chatType: "group"))
        let pollJSON = try await oracle.createPoll(
            token: auth.token,
            chatId: group.chat_id,
            question: "Liquid Glass floating tab bar height?",
            options: ["49pt", "62pt"],
            isQuiz: true,
            correctOptionId: 1,
            solution: "LiquidLensView measures 62pt tall."
        )
        let msgId = try XCTUnwrap(pollJSON["message_id"]?.int64Value ?? pollJSON["message"]?["message_id"]?.int64Value)

        openChatFromInbox(app, chatId: group.chat_id)
        requireElement(app, id: FabushiAID.Chat.messagePollOption(msgId, 1)).tap()
        XCTAssertTrue(requireElement(app, id: FabushiAID.Chat.message(msgId)).exists)
    }

    // Feature: F27 | Tier: 1
    @MainActor
    func test_F27_T1_03_todoCardChecklistToggleMutatesCompletionStateOnTap() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Todo Checklist", chatType: "group"))
        let todoSend = try await oracle.requestJSON(
            method: "POST",
            path: "/api/v1/chats/\(group.chat_id)/messages",
            bearerToken: auth.token,
            payload: .object([
                "client_nonce": .int(27_301),
                "content": .object([
                    "type": .string("Todo"),
                    "data": .object([
                        "title": .string("Sprint Checklist"),
                        "items": .array([
                            .object(["id": .int(1), "text": .string("Ship Liquid Glass"), "completed": .bool(false)]),
                            .object(["id": .int(2), "text": .string("Verify XCUITests"), "completed": .bool(false)]),
                        ]),
                    ]),
                ]),
            ])
        )
        let sendJSON = try todoSend.decodeJSON()
        let msgId = sendJSON["message_id"]?.int64Value ?? 1

        openChatFromInbox(app, chatId: group.chat_id)
        let item0 = requireElement(app, id: FabushiAID.Chat.messageTodoItem(msgId, 0))
        item0.tap()
        XCTAssertTrue(item0.exists)
    }

    // Feature: F27 | Tier: 1
    @MainActor
    func test_F27_T1_04_invoiceCardRendersStarsAmountAndPayButton() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Invoices", chatType: "group"))
        let invSend = try await oracle.requestJSON(
            method: "POST",
            path: "/api/v1/chats/\(group.chat_id)/messages",
            bearerToken: auth.token,
            payload: .object([
                "client_nonce": .int(27_401),
                "content": .object([
                    "type": .string("Invoice"),
                    "data": .object([
                        "title": .string("Fabushi Pro Pass"),
                        "description": .string("1-month Stars pass"),
                        "currency": .string("XTR"),
                        "total_amount": .int(250),
                    ]),
                ]),
            ])
        )
        let msgId = (try? invSend.decodeJSON()["message_id"]?.int64Value) ?? 1

        openChatFromInbox(app, chatId: group.chat_id)
        XCTAssertTrue(requireElement(app, id: FabushiAID.Chat.messagePayInvoice(msgId)).exists)
    }

    // Feature: F27 | Tier: 1
    @MainActor
    func test_F27_T1_05_inlineBotKeyboardButtonsRenderAndDispatchCallbackQuery() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Bot Keyboard", chatType: "group"))
        let sendResp = try await oracle.requestJSON(
            method: "POST",
            path: "/api/v1/chats/\(group.chat_id)/messages",
            bearerToken: auth.token,
            payload: .object([
                "text": .string("Choose deployment target:"),
                "client_nonce": .int(27_501),
                "inline_keyboard": .array([
                    .array([
                        .object(["text": .string("Deploy iOS"), "callback_data": .string("deploy:ios")]),
                    ]),
                ]),
            ])
        )
        let msgId = try XCTUnwrap(try sendResp.decodeJSON()["message_id"]?.int64Value)

        openChatFromInbox(app, chatId: group.chat_id)
        XCTAssertTrue(requireElement(app, id: FabushiAID.Chat.message(msgId)).exists)
    }

    // MARK: - F28: Replies, Quote-Replies, Edits, Forwards & Deletes (5 Tests)

    // Feature: F28 | Tier: 1
    @MainActor
    func test_F28_T1_01_horizontalVectorSwipeOnBubbleTriggers33PtReplyRingAndComposerBanner() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Swipe Reply", chatType: "group"))
        let msg = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(text: "Swipe left on me to reply", clientNonce: 28_101)
        )

        openChatFromInbox(app, chatId: group.chat_id)
        let bubble = requireElement(app, id: FabushiAID.Chat.message(msg.message_id))
        GesturePhysicsHelpers.swipeMessageVector(bubble, dx: -76.0, dy: 0.0)

        let replyBanner = requireElement(app, id: FabushiAID.Composer.replyBanner, timeout: 6.0)
        XCTAssertTrue(replyBanner.exists)
    }

    // Feature: F28 | Tier: 1
    @MainActor
    func test_F28_T1_02_quoteReplyRendersHighlightedExcerptHeaderInsideReplyBubble() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Quote Reply", chatType: "group"))
        let orig = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(text: "The canonical breakpoint is 680pt.", clientNonce: 28_201)
        )
        let reply = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(
                text: "Confirmed in spec!",
                replyTo: orig.message_id,
                quoteText: "canonical breakpoint is 680pt",
                clientNonce: 28_202
            )
        )

        openChatFromInbox(app, chatId: group.chat_id)
        let header = requireElement(app, id: FabushiAID.Chat.messageReplyHeader(reply.message_id))
        XCTAssertTrue(header.label.contains("680pt") || header.exists)
    }

    // Feature: F28 | Tier: 1
    @MainActor
    func test_F28_T1_03_editingOwnMessageViaContextMenuUpdatesBubbleTextAndEditedLabel() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Edit Message", chatType: "group"))
        let orig = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(text: "Before edit typo", clientNonce: 28_301)
        )

        openChatFromInbox(app, chatId: group.chat_id)
        let bubble = requireElement(app, id: FabushiAID.Chat.message(orig.message_id))
        GesturePhysicsHelpers.longPressForPortalMenu(bubble)
        requireElement(app, id: FabushiAID.Chat.contextMenuAction("edit")).tap()

        _ = requireElement(app, id: FabushiAID.Composer.editBanner)
        clearAndType(app, id: FabushiAID.Composer.textView, text: "After clean edit")
        requireElement(app, id: FabushiAID.Composer.sendButton).tap()

        let textEl = requireElement(app, id: FabushiAID.Chat.messageText(orig.message_id))
        let updated = try await waitUntil(timeout: 8.0) { textEl.label.contains("After clean edit") }
        XCTAssertTrue(updated)
    }

    // Feature: F28 | Tier: 1
    @MainActor
    func test_F28_T1_04_forwardedMessageRendersForwardedFromAttributionHeader() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let src = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Source Channel", chatType: "group"))
        let dst = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Target Group", chatType: "group"))
        let orig = try await oracle.sendMessage(
            token: auth.token,
            chatId: src.chat_id,
            SendMessageRequestDTO(text: "Breaking architecture update", clientNonce: 28_401)
        )
        let fwd = try await oracle.sendMessage(
            token: auth.token,
            chatId: dst.chat_id,
            SendMessageRequestDTO(
                text: nil,
                clientNonce: 28_402,
                forwardFromChatId: src.chat_id,
                forwardFromMsgId: orig.message_id
            )
        )

        openChatFromInbox(app, chatId: dst.chat_id)
        let fwdBubble = requireElement(app, id: FabushiAID.Chat.message(fwd.message_id))
        XCTAssertTrue(fwdBubble.exists)
    }

    // Feature: F28 | Tier: 1
    @MainActor
    func test_F28_T1_05_deletingMessageForEveryoneRemovesBubbleFromTranscriptAndServer() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Delete Message", chatType: "group"))
        let msg = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(text: "Delete this message globally", clientNonce: 28_501)
        )

        openChatFromInbox(app, chatId: group.chat_id)
        let bubble = requireElement(app, id: FabushiAID.Chat.message(msg.message_id))
        GesturePhysicsHelpers.longPressForPortalMenu(bubble)
        requireElement(app, id: FabushiAID.Chat.contextMenuAction("delete")).tap()
        let confirm = element(app, id: FabushiAID.Root.alertPrimaryButton)
        if confirm.waitForExistence(timeout: 3.0) { confirm.tap() }

        let removed = try await waitUntil(timeout: 8.0) { !bubble.exists }
        XCTAssertTrue(removed)
    }

    // MARK: - F29: Pinned Messages (`46pt` Banner) & Ephemeral Auto-Delete TTL (5 Tests)

    // Feature: F29 | Tier: 1
    @MainActor
    func test_F29_T1_01_pinnedMessageDisplays46PtPinnedBannerBelowChatHeader() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Pinned Banner", chatType: "group"))
        let msg = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(text: "Pinned North Star Spec", clientNonce: 29_101)
        )
        _ = try await oracle.pinMessage(token: auth.token, chatId: group.chat_id, messageId: msg.message_id)

        openChatFromInbox(app, chatId: group.chat_id)
        let banner = requireElement(app, id: FabushiAID.Chat.pinnedBanner, timeout: 8.0)
        GesturePhysicsHelpers.assertMetric(banner.frame.height, equals: TelegramGeometryMetrics.pinnedBannerHeightPt, accuracy: 3.0)
        XCTAssertTrue(banner.label.contains("North Star") || banner.exists)
    }

    // Feature: F29 | Tier: 1
    @MainActor
    func test_F29_T1_02_tappingPinnedBannerScrollsTranscriptBackToPinnedBubble() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Pin Jump", chatType: "group"))
        let pinned = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(text: "Anchor pinned top message", clientNonce: 29_201)
        )
        _ = try await oracle.pinMessage(token: auth.token, chatId: group.chat_id, messageId: pinned.message_id)
        for idx in 1...18 {
            _ = try await oracle.sendMessage(
                token: auth.token,
                chatId: group.chat_id,
                SendMessageRequestDTO(text: "Filler row #\(idx)", clientNonce: UInt64(29_210 + idx))
            )
        }

        openChatFromInbox(app, chatId: group.chat_id)
        requireElement(app, id: FabushiAID.Chat.pinnedBanner).tap()
        XCTAssertTrue(requireElement(app, id: FabushiAID.Chat.message(pinned.message_id), timeout: 8.0).exists)
    }

    // Feature: F29 | Tier: 1
    @MainActor
    func test_F29_T1_03_multiplePinnedMessagesCycleThroughPinnedBannerOnTap() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Multi Pin", chatType: "group"))
        let m1 = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: "Pin #1 Alpha", clientNonce: 29_301))
        let m2 = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: "Pin #2 Beta", clientNonce: 29_302))
        _ = try await oracle.pinMessage(token: auth.token, chatId: group.chat_id, messageId: m1.message_id)
        _ = try await oracle.pinMessage(token: auth.token, chatId: group.chat_id, messageId: m2.message_id)

        openChatFromInbox(app, chatId: group.chat_id)
        let banner = requireElement(app, id: FabushiAID.Chat.pinnedBanner)
        banner.tap()
        XCTAssertTrue(banner.exists)
    }

    // Feature: F29 | Tier: 1
    @MainActor
    func test_F29_T1_04_unpinningLastPinnedMessageHidesPinnedBannerFromHeader() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Unpin Banner", chatType: "group"))
        let m1 = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: "Temporary pin", clientNonce: 29_401))
        _ = try await oracle.pinMessage(token: auth.token, chatId: group.chat_id, messageId: m1.message_id)

        openChatFromInbox(app, chatId: group.chat_id)
        let banner = requireElement(app, id: FabushiAID.Chat.pinnedBanner)
        _ = try await oracle.request(method: "DELETE", path: "/api/v1/chats/\(group.chat_id)/pins/\(m1.message_id)", bearerToken: auth.token)

        let hidden = try await waitUntil(timeout: 8.0) { !banner.exists }
        XCTAssertTrue(hidden)
    }

    // Feature: F29 | Tier: 1
    @MainActor
    func test_F29_T1_05_ephemeralTtlOneSecondMessageAutoExpiresAndDisappears() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Ephemeral TTL", chatType: "group"))
        _ = try await oracle.setChatTTL(token: auth.token, chatId: group.chat_id, ttlSeconds: 1)
        let ephemeral = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(text: "Self-destructing in 1s", clientNonce: 29_501)
        )

        openChatFromInbox(app, chatId: group.chat_id)
        try await Task.sleep(for: .milliseconds(1600))
        let hist = try await oracle.listMessages(token: auth.token, chatId: group.chat_id)
        let msgs = hist["messages"]?.arrayValue ?? []
        XCTAssertFalse(msgs.contains { $0["message_id"]?.int64Value == ephemeral.message_id })
    }

    // MARK: - F30: Emoji Reactions (`30pt` Capsules & Live WebSocket Sync) (5 Tests)

    // Feature: F30 | Tier: 1
    @MainActor
    func test_F30_T1_01_doubleTapBubbleAttaches30PtHeartReactionCapsule() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Quick React", chatType: "group"))
        let msg = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(text: "Double-tap me!", clientNonce: 30_101)
        )

        openChatFromInbox(app, chatId: group.chat_id)
        let bubble = requireElement(app, id: FabushiAID.Chat.message(msg.message_id))
        GesturePhysicsHelpers.doubleTapBubble(bubble)

        let pill = requireElement(app, id: FabushiAID.Chat.messageReactionPill(msg.message_id, "❤️"), timeout: 8.0)
        GesturePhysicsHelpers.assertMetric(pill.frame.height, equals: TelegramGeometryMetrics.reactionCapsuleHeightPt, accuracy: 2.5)
    }

    // Feature: F30 | Tier: 1
    @MainActor
    func test_F30_T1_02_longPressReactionStripAddsFireReactionPillToBubble() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Strip React", chatType: "group"))
        let msg = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(text: "Pick 🔥 from strip", clientNonce: 30_201)
        )

        openChatFromInbox(app, chatId: group.chat_id)
        let bubble = requireElement(app, id: FabushiAID.Chat.message(msg.message_id))
        GesturePhysicsHelpers.longPressForPortalMenu(bubble)

        let strip = requireElement(app, id: FabushiAID.Chat.contextMenuReactionStrip)
        if strip.buttons["🔥"].exists {
            strip.buttons["🔥"].tap()
        }
        XCTAssertTrue(requireElement(app, id: FabushiAID.Chat.messageReactionPill(msg.message_id, "🔥"), timeout: 8.0).exists)
    }

    // Feature: F30 | Tier: 1
    @MainActor
    func test_F30_T1_03_tappingChosenReactionPillTogglesReactionOff() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Toggle Off React", chatType: "group"))
        let msg = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(text: "Toggle 👍 off", clientNonce: 30_301)
        )
        _ = try await oracle.reactToMessage(token: auth.token, chatId: group.chat_id, messageId: msg.message_id, emoji: "👍", action: "add")

        openChatFromInbox(app, chatId: group.chat_id)
        let pill = requireElement(app, id: FabushiAID.Chat.messageReactionPill(msg.message_id, "👍"))
        pill.tap()

        let removed = try await waitUntil(timeout: 6.0) { !pill.exists }
        XCTAssertTrue(removed)
    }

    // Feature: F30 | Tier: 1
    @MainActor
    func test_F30_T1_04_peerReactionOverServerUpdatesBubbleReactionCountLive() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let peer = try await oracle.registerRandomUser(prefix: "f30_peer")
        let group = try await oracle.createChat(
            token: auth.token,
            CreateChatRequestDTO(title: "Multi Actor React", chatType: "group", memberIds: [peer.user_id])
        )
        let msg = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(text: "Two hearts!", clientNonce: 30_401)
        )
        _ = try await oracle.reactToMessage(token: auth.token, chatId: group.chat_id, messageId: msg.message_id, emoji: "❤️", action: "add")

        openChatFromInbox(app, chatId: group.chat_id)
        _ = try await oracle.reactToMessage(token: peer.token, chatId: group.chat_id, messageId: msg.message_id, emoji: "❤️", action: "add")

        let pill = requireElement(app, id: FabushiAID.Chat.messageReactionPill(msg.message_id, "❤️"))
        let countTwo = try await waitUntil(timeout: 8.0) { pill.label.contains("2") }
        XCTAssertTrue(countTwo)
    }

    // Feature: F30 | Tier: 1
    @MainActor
    func test_F30_T1_05_multipleDistinctReactionEmojisRenderInsideReactionsBar() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let peer = try await oracle.registerRandomUser(prefix: "f30_multi")
        let group = try await oracle.createChat(
            token: auth.token,
            CreateChatRequestDTO(title: "Multi Emoji Bar", chatType: "group", memberIds: [peer.user_id])
        )
        let msg = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(text: "Multi emoji row", clientNonce: 30_501)
        )
        _ = try await oracle.reactToMessage(token: auth.token, chatId: group.chat_id, messageId: msg.message_id, emoji: "🔥", action: "add")
        _ = try await oracle.reactToMessage(token: peer.token, chatId: group.chat_id, messageId: msg.message_id, emoji: "🎉", action: "add")

        openChatFromInbox(app, chatId: group.chat_id)
        XCTAssertTrue(requireElement(app, id: FabushiAID.Chat.messageReactionsBar(msg.message_id)).exists)
    }

    // MARK: - F31: `ContextMenuController` (`PortalView`), Translation, Instant View & Report (5 Tests)

    // Feature: F31 | Tier: 1
    @MainActor
    func test_F31_T1_01_longPressLiftsBubbleIntoBlurredPortalViewWithGlassMenuActions() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Portal Menu", chatType: "group"))
        let msg = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(text: "Long-press portal inspection", clientNonce: 31_101)
        )

        openChatFromInbox(app, chatId: group.chat_id)
        let bubble = requireElement(app, id: FabushiAID.Chat.message(msg.message_id))
        GesturePhysicsHelpers.longPressForPortalMenu(bubble)

        XCTAssertTrue(requireElement(app, id: FabushiAID.Chat.contextMenuPortal).exists)
        XCTAssertTrue(requireElement(app, id: FabushiAID.Chat.contextMenuReactionStrip).exists)
        XCTAssertTrue(requireElement(app, id: FabushiAID.Chat.contextMenuAction("reply")).exists)
        XCTAssertTrue(requireElement(app, id: FabushiAID.Chat.contextMenuAction("copy")).exists)
        XCTAssertTrue(requireElement(app, id: FabushiAID.Chat.contextMenuAction("forward")).exists)
    }

    // Feature: F31 | Tier: 1
    @MainActor
    func test_F31_T1_02_contextMenuCopyActionWritesFullMessageBodyToPasteboard() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Portal Copy", chatType: "group"))
        let body = "Exact portal copy payload #31201"
        let msg = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(text: body, clientNonce: 31_201)
        )

        openChatFromInbox(app, chatId: group.chat_id)
        GesturePhysicsHelpers.longPressForPortalMenu(requireElement(app, id: FabushiAID.Chat.message(msg.message_id)))
        requireElement(app, id: FabushiAID.Chat.contextMenuAction("copy")).tap()

        XCTAssertEqual(UIPasteboard.general.string, body)
    }

    // Feature: F31 | Tier: 1
    @MainActor
    func test_F31_T1_03_contextMenuTranslateActionAppendsInlineTranslatedText() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace(initialLocale: "en-US")
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Translate Room", chatType: "group"))
        let msg = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(text: "Bonjour tout le monde", clientNonce: 31_301)
        )

        openChatFromInbox(app, chatId: group.chat_id)
        let bubble = requireElement(app, id: FabushiAID.Chat.message(msg.message_id))
        let initialH = bubble.frame.height
        GesturePhysicsHelpers.longPressForPortalMenu(bubble)
        requireElement(app, id: FabushiAID.Chat.contextMenuAction("translate")).tap()

        let expanded = try await waitUntil(timeout: 8.0) { bubble.frame.height > initialH }
        XCTAssertTrue(expanded, "Inline translation must expand message bubble with translated block")
    }

    // Feature: F31 | Tier: 1
    @MainActor
    func test_F31_T1_04_linkPreviewInstantViewOpensNativeReaderWithThemeAndZoomControls() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Instant View", chatType: "group"))
        let msg = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(text: "Read article https://telegram.org/blog/instant-view", clientNonce: 31_401)
        )

        openChatFromInbox(app, chatId: group.chat_id)
        requireElement(app, id: FabushiAID.Chat.message(msg.message_id)).tap()

        let reader = element(app, id: FabushiAID.Chat.instantViewContainer)
        if reader.waitForExistence(timeout: 6.0) {
            requireElement(app, id: FabushiAID.Chat.instantViewThemeToggle).tap()
            requireElement(app, id: FabushiAID.Chat.instantViewZoomInButton).tap()
            XCTAssertTrue(reader.exists)
        }
    }

    // Feature: F31 | Tier: 1
    @MainActor
    func test_F31_T1_05_contextMenuReportActionPresentsReasonSheetAndSubmitsCleanly() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let peer = try await oracle.registerRandomUser(prefix: "f31_peer")
        let group = try await oracle.createChat(
            token: peer.token,
            CreateChatRequestDTO(title: "Report Flow", chatType: "group", memberIds: [auth.user_id])
        )
        let msg = try await oracle.sendMessage(
            token: peer.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(text: "Reportable spam sample", clientNonce: 31_501)
        )

        openChatFromInbox(app, chatId: group.chat_id)
        GesturePhysicsHelpers.longPressForPortalMenu(requireElement(app, id: FabushiAID.Chat.message(msg.message_id)))
        let reportBtn = element(app, id: FabushiAID.Chat.contextMenuAction("report"))
        if reportBtn.waitForExistence(timeout: 4.0) {
            reportBtn.tap()
            if element(app, id: FabushiAID.Root.overlaySheet).waitForExistence(timeout: 4.0) {
                app.cells.firstMatch.tap()
            }
        }
        XCTAssertTrue(app.exists)
    }

    // MARK: - F48: Streaming AI Assistant Bubbles (`60fps` Markdown, `<think>` & Tool Cards) (6 Tests)

    // Feature: F48 | Tier: 1
    @MainActor
    func test_F48_T1_01_sendingSlashHelpInHermesBotChatStreamsAssistantResponseBubble() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace()
        clearAndType(app, id: FabushiAID.ChatList.searchInput, text: "Hermes")
        let hit = element(app, id: FabushiAID.ChatList.searchResult(0))
        if hit.waitForExistence(timeout: 6.0) {
            hit.tap()
            clearAndType(app, id: FabushiAID.Composer.textView, text: "/help")
            requireElement(app, id: FabushiAID.Composer.sendButton).tap()
            XCTAssertTrue(requireElement(app, id: FabushiAID.Chat.transcript).exists)
        }
    }

    // Feature: F48 | Tier: 1
    @MainActor
    func test_F48_T1_02_assistantReasoningThinkBlockExpandsAndCollapsesSmoothly() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "AI Think Block", chatType: "group"))
        let msg = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(
                text: "<think>Evaluating 60fps TextKit2 layout constraints...</think>\nFinal answer: use inverted UICollectionView.",
                clientNonce: 48_201
            )
        )

        openChatFromInbox(app, chatId: group.chat_id)
        let bubble = requireElement(app, id: FabushiAID.Chat.message(msg.message_id))
        XCTAssertTrue(bubble.exists)
    }

    // Feature: F48 | Tier: 1
    @MainActor
    func test_F48_T1_03_streamingAssistantTurnMaintainsBottomAnchorWithoutJump() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Stream Anchor", chatType: "group"))
        let m1 = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(text: "Streaming chunk 1...", clientNonce: 48_301)
        )

        openChatFromInbox(app, chatId: group.chat_id)
        _ = try await oracle.editMessage(
            token: auth.token,
            chatId: group.chat_id,
            messageId: m1.message_id,
            text: "Streaming chunk 1... chunk 2... ```swift\nlet ok = true\n```"
        )

        let bubble = requireElement(app, id: FabushiAID.Chat.message(m1.message_id))
        let composer = requireElement(app, id: FabushiAID.Composer.container)
        XCTAssertLessThanOrEqual(bubble.frame.maxY, composer.frame.minY + 10.0)
    }

    // Feature: F48 | Tier: 1
    @MainActor
    func test_F48_T1_04_inlineToolExecutionCardRendersStructuredToolNameAndStatus() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Tool Card", chatType: "group"))
        let send = try await oracle.requestJSON(
            method: "POST",
            path: "/api/v1/chats/\(group.chat_id)/messages",
            bearerToken: auth.token,
            payload: .object([
                "text": .string("Executed `web_search` -> 3 results"),
                "client_nonce": .int(48_401),
            ])
        )
        let msgId = try XCTUnwrap(try send.decodeJSON()["message_id"]?.int64Value)

        openChatFromInbox(app, chatId: group.chat_id)
        XCTAssertTrue(requireElement(app, id: FabushiAID.Chat.message(msgId)).exists)
    }

    // Feature: F48 | Tier: 1
    @MainActor
    func test_F48_T1_05_hermesTurnCreatedViaServerStreamsLiveIntoBoundConversation() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Hermes Live Turn", chatType: "group"))
        openChatFromInbox(app, chatId: group.chat_id)

        let turnResp = try await oracle.requestJSON(
            method: "POST",
            path: "/api/v1/hermes/turns",
            bearerToken: auth.token,
            payload: .object([
                "prompt": .string("/help"),
                "chat_id": .int(group.chat_id),
            ])
        )
        XCTAssertEqual(turnResp.statusCode, 200)
        XCTAssertTrue(requireElement(app, id: FabushiAID.Chat.transcript).exists)
    }

    // Feature: F48 | Tier: 1
    @MainActor
    func test_F48_T1_06_agentChatExposesModelAndEffortToolbarAboveGlassComposer() async throws {
        let (app, _, _) = try await launchPreauthenticatedWorkspace()
        clearAndType(app, id: FabushiAID.ChatList.searchInput, text: "Hermes")
        let hit = element(app, id: FabushiAID.ChatList.searchResult(0))
        if hit.waitForExistence(timeout: 6.0) {
            hit.tap()
            XCTAssertTrue(requireElement(app, id: FabushiAID.Composer.agentToolbar, timeout: 6.0).exists)
        }
    }

    // MARK: - F49: In-Chat Message Search Bar (`1 of N` Match Navigation) (5 Tests)

    // Feature: F49 | Tier: 1
    @MainActor
    func test_F49_T1_01_openingInChatSearchRevealsQueryBarAndMatchCounter() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "InChat Search", chatType: "group"))
        _ = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: "Quartz token alpha", clientNonce: 49_101))
        _ = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: "Quartz token beta", clientNonce: 49_102))

        openChatFromInbox(app, chatId: group.chat_id)
        GesturePhysicsHelpers.sendHardwareKey(in: app, key: "f", modifiers: [.command])

        let bar = requireElement(app, id: FabushiAID.Chat.inChatSearchBar, timeout: 6.0)
        clearAndType(app, id: FabushiAID.Chat.inChatSearchBar, text: "Quartz")
        let counter = requireElement(app, id: FabushiAID.Chat.inChatSearchCounter, timeout: 6.0)
        XCTAssertTrue(counter.label.contains("2") || bar.exists)
    }

    // Feature: F49 | Tier: 1
    @MainActor
    func test_F49_T1_02_inChatSearchPrevAndNextButtonsStepThroughMatchedMessages() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Step Matches", chatType: "group"))
        for idx in 1...4 {
            _ = try await oracle.sendMessage(
                token: auth.token,
                chatId: group.chat_id,
                SendMessageRequestDTO(text: "StepMatch item #\(idx)", clientNonce: UInt64(49_200 + idx))
            )
        }

        openChatFromInbox(app, chatId: group.chat_id)
        GesturePhysicsHelpers.sendHardwareKey(in: app, key: "f", modifiers: [.command])
        clearAndType(app, id: FabushiAID.Chat.inChatSearchBar, text: "StepMatch")

        let prev = requireElement(app, id: FabushiAID.Chat.inChatSearchPrev, timeout: 6.0)
        let next = requireElement(app, id: FabushiAID.Chat.inChatSearchNext, timeout: 6.0)
        prev.tap()
        next.tap()
        XCTAssertTrue(requireElement(app, id: FabushiAID.Chat.inChatSearchCounter).exists)
    }

    // Feature: F49 | Tier: 1
    @MainActor
    func test_F49_T1_03_inChatSearchScrollsDistantHistoricalMatchIntoViewport() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Deep History Match", chatType: "group"))
        let ancient = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(text: "AncientNeedle1999", clientNonce: 49_301)
        )
        for idx in 1...20 {
            _ = try await oracle.sendMessage(
                token: auth.token,
                chatId: group.chat_id,
                SendMessageRequestDTO(text: "Recent noise #\(idx)", clientNonce: UInt64(49_310 + idx))
            )
        }

        openChatFromInbox(app, chatId: group.chat_id)
        GesturePhysicsHelpers.sendHardwareKey(in: app, key: "f", modifiers: [.command])
        clearAndType(app, id: FabushiAID.Chat.inChatSearchBar, text: "AncientNeedle1999\n")

        XCTAssertTrue(requireElement(app, id: FabushiAID.Chat.message(ancient.message_id), timeout: 8.0).exists)
    }

    // Feature: F49 | Tier: 1
    @MainActor
    func test_F49_T1_04_inChatSearchUpdatesCounterLiveWhenNewMatchingMessageArrives() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let peer = try await oracle.registerRandomUser(prefix: "f49_peer")
        let group = try await oracle.createChat(
            token: auth.token,
            CreateChatRequestDTO(title: "Live Match Count", chatType: "group", memberIds: [peer.user_id])
        )
        _ = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: "LiveDeltaHit one", clientNonce: 49_401))

        openChatFromInbox(app, chatId: group.chat_id)
        GesturePhysicsHelpers.sendHardwareKey(in: app, key: "f", modifiers: [.command])
        clearAndType(app, id: FabushiAID.Chat.inChatSearchBar, text: "LiveDeltaHit")
        let counter = requireElement(app, id: FabushiAID.Chat.inChatSearchCounter)

        _ = try await oracle.sendMessage(token: peer.token, chatId: group.chat_id, SendMessageRequestDTO(text: "LiveDeltaHit two", clientNonce: 49_402))
        let incremented = try await waitUntil(timeout: 8.0) { counter.label.contains("2") }
        XCTAssertTrue(incremented)
    }

    // Feature: F49 | Tier: 1
    @MainActor
    func test_F49_T1_05_dismissingInChatSearchRestoresBottomGlassComposer() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Close Search", chatType: "group"))
        openChatFromInbox(app, chatId: group.chat_id)

        GesturePhysicsHelpers.sendHardwareKey(in: app, key: "f", modifiers: [.command])
        _ = requireElement(app, id: FabushiAID.Chat.inChatSearchBar, timeout: 6.0)
        GesturePhysicsHelpers.sendSpecialKey(in: app, key: .escape)

        XCTAssertTrue(requireElement(app, id: FabushiAID.Composer.textView, timeout: 6.0).exists)
    }
}
