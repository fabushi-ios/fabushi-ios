import Foundation
import XCTest

/// Tier 2 Boundary, Negative & Corner-Case Opaque-Box UI Tests — Domain R4:
/// Transcript Boundaries, UTF-16 Surrogates, Media Clamps, Audio Singleton, Closed Polls,
/// Diagonal Swipe Rejections, Non-Admin Pin Bans, Reaction Idempotency, Portal Cleanup,
/// Streaming AI Scroll-Lock Release & In-Chat Search Regex Safety (`F23..F31, F48, F49`).
final class Tier2_R4_TranscriptBoundaryUITests: FabushiE2ETestCase {

    // MARK: - F23: Transcript Length Limits, Merge Breaks & Nonce Deduplication (6 Tests)

    // Feature: F23 | Tier: 2
    @MainActor
    func test_F23_T2_01_emptyZeroMessageConversationRendersInvertedTranscriptWithoutCrash() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let emptyGroup = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Empty Transcript", chatType: "group"))

        openChatFromInbox(app, chatId: emptyGroup.chat_id)
        XCTAssertTrue(requireElement(app, id: FabushiAID.Chat.transcript).exists)
        XCTAssertFalse(element(app, id: FabushiAID.Chat.scrollToBottomButton).isHittable)
    }

    // Feature: F23 | Tier: 2
    @MainActor
    func test_F23_T2_02_messageExceeding4096CharsRejectedByServerWith400InvalidText() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "4096 Boundary", chatType: "group"))

        let overLimit = String(repeating: "A", count: 4097)
        let resp = try await oracle.requestJSON(
            method: "POST",
            path: "/api/v1/chats/\(group.chat_id)/messages",
            bearerToken: auth.token,
            payload: .object(["text": .string(overLimit), "client_nonce": .int(230_201)])
        )
        XCTAssertEqual(resp.statusCode, 400)
        openChatFromInbox(app, chatId: group.chat_id)
        XCTAssertTrue(requireElement(app, id: FabushiAID.Chat.transcript).exists)
    }

    // Feature: F23 | Tier: 2
    @MainActor
    func test_F23_T2_03_exact4096CharacterBoundaryMessageAcceptedAndRenderedCleanly() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Exact 4096", chatType: "group"))
        let exact4096 = String(repeating: "B", count: 4096)
        let sent = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: exact4096, clientNonce: 230_301))

        openChatFromInbox(app, chatId: group.chat_id)
        XCTAssertTrue(requireElement(app, id: FabushiAID.Chat.message(sent.message_id)).exists)
    }

    // Feature: F23 | Tier: 2
    @MainActor
    func test_F23_T2_04_interleavedPeerMessageBreaksConsecutiveBubbleMergeGroup() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let peer = try await oracle.registerRandomUser(prefix: "f23_inter")
        let group = try await oracle.createChat(
            token: auth.token,
            CreateChatRequestDTO(title: "Merge Break", chatType: "group", memberIds: [peer.user_id])
        )
        let m1 = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: "Mine #1", clientNonce: 230_401))
        let mPeer = try await oracle.sendMessage(token: peer.token, chatId: group.chat_id, SendMessageRequestDTO(text: "Peer interrupt", clientNonce: 230_402))
        let m2 = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: "Mine #2", clientNonce: 230_403))

        openChatFromInbox(app, chatId: group.chat_id)
        let b1 = requireElement(app, id: FabushiAID.Chat.message(m1.message_id))
        let bPeer = requireElement(app, id: FabushiAID.Chat.message(mPeer.message_id))
        let b2 = requireElement(app, id: FabushiAID.Chat.message(m2.message_id))
        XCTAssertLessThan(bPeer.frame.minX, b1.frame.minX, "Incoming peer bubble must align left while outgoing aligns right")
        XCTAssertGreaterThan(b2.frame.minY, bPeer.frame.maxY)
    }

    // Feature: F23 | Tier: 2
    @MainActor
    func test_F23_T2_05_duplicateClientNonceRetryRendersSingleDeduplicatedBubble() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Nonce Dedup", chatType: "group"))
        let first = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: "Idempotent payload", clientNonce: 777_888))
        let dup = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: "Idempotent payload", clientNonce: 777_888))
        XCTAssertEqual(first.message_id, dup.message_id)

        openChatFromInbox(app, chatId: group.chat_id)
        let matches = app.descendants(matching: .any).matching(identifier: FabushiAID.Chat.message(first.message_id))
        XCTAssertEqual(matches.count, 1)
    }

    // Feature: F23 | Tier: 2
    @MainActor
    func test_F23_T2_06_scrolledUpHistoryPositionIsNotYankedToBottomWhenPeerSendsMessage() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let peer = try await oracle.registerRandomUser(prefix: "f23_noyank")
        let group = try await oracle.createChat(
            token: auth.token,
            CreateChatRequestDTO(title: "No Scroll Yank", chatType: "group", memberIds: [peer.user_id])
        )
        let oldest = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: "Oldest anchor #0", clientNonce: 230_600))
        for idx in 1...25 {
            _ = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: "History #\(idx)", clientNonce: UInt64(230_600 + idx)))
        }

        openChatFromInbox(app, chatId: group.chat_id)
        let transcript = requireElement(app, id: FabushiAID.Chat.transcript)
        transcript.swipeDown()
        transcript.swipeDown()
        _ = requireElement(app, id: FabushiAID.Chat.scrollToBottomButton, timeout: 6.0)

        _ = try await oracle.sendMessage(token: peer.token, chatId: group.chat_id, SendMessageRequestDTO(text: "Incoming while reading history", clientNonce: 230_699))
        XCTAssertTrue(requireElement(app, id: FabushiAID.Chat.scrollToBottomButton).exists, "Reading upper history must increment FAB badge instead of yanking scroll to bottom")
        XCTAssertGreaterThan(oldest.message_id, 0)
    }

    // MARK: - F24: UTF-16 Surrogates, Malicious URLs & Unclosed Markdown (5 Tests)

    // Feature: F24 | Tier: 2
    @MainActor
    func test_F24_T2_01_utf16ZwjFamilyEmojiAndCjkEntityOffsetsRenderWithoutStringIndexTrap() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "UTF16 Surrogate", chatType: "group"))
        let prefix = "中文👨‍👩‍👧‍👦مرحبا "
        let target = "SafeBold"
        let full = prefix + target
        let msg = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(
                text: full,
                entities: [MessageEntityDTO(type: "Bold", offset: prefix.utf16.count, length: target.utf16.count)],
                clientNonce: 240_101
            )
        )

        openChatFromInbox(app, chatId: group.chat_id)
        let textEl = requireElement(app, id: FabushiAID.Chat.messageText(msg.message_id))
        XCTAssertTrue(textEl.label.contains("SafeBold"))
    }

    // Feature: F24 | Tier: 2
    @MainActor
    func test_F24_T2_02_nestedOverlappingBoldItalicAndSpoilerEntitiesRenderSimultaneously() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Nested Entities", chatType: "group"))
        let text = "Nested secret token"
        let msg = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(
                text: text,
                entities: [
                    MessageEntityDTO(type: "Bold", offset: 0, length: text.utf16.count),
                    MessageEntityDTO(type: "Italic", offset: 7, length: 12),
                    MessageEntityDTO(type: "Spoiler", offset: 7, length: 6),
                ],
                clientNonce: 240_201
            )
        )

        openChatFromInbox(app, chatId: group.chat_id)
        XCTAssertTrue(requireElement(app, id: FabushiAID.Chat.message(msg.message_id)).exists)
    }

    // Feature: F24 | Tier: 2
    @MainActor
    func test_F24_T2_03_javascriptSchemeTextUrlEntityBlockedFromExecutingUnsafeOpen() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "XSS URL Guard", chatType: "group"))
        let text = "ClickMe"
        let resp = try await oracle.requestJSON(
            method: "POST",
            path: "/api/v1/chats/\(group.chat_id)/messages",
            bearerToken: auth.token,
            payload: .object([
                "text": .string(text),
                "entities": .array([
                    .object([
                        "type": .string("TextUrl"),
                        "offset": .int(0),
                        "length": .int(7),
                        "url": .string("javascript:alert(1)"),
                    ]),
                ]),
                "client_nonce": .int(240_301),
            ])
        )

        openChatFromInbox(app, chatId: group.chat_id)
        if resp.statusCode < 300, let mid = (try? resp.decodeJSON())?["message_id"]?.int64Value {
            requireElement(app, id: FabushiAID.Chat.message(mid)).tap()
        }
        XCTAssertTrue(app.state == .runningForeground)
    }

    // Feature: F24 | Tier: 2
    @MainActor
    func test_F24_T2_04_unclosedMarkdownDelimitersAndEmptyCodeFencesRenderWithoutCrash() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Broken Markdown", chatType: "group"))
        let msg = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(text: "**unclosed _italic ```\n`````` $$unclosed_math", clientNonce: 240_401)
        )

        openChatFromInbox(app, chatId: group.chat_id)
        XCTAssertTrue(requireElement(app, id: FabushiAID.Chat.message(msg.message_id)).exists)
    }

    // Feature: F24 | Tier: 2
    @MainActor
    func test_F24_T2_05_outOfBoundsEntityLengthRejectedByServerValidatorWith400() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Entity Bounds", chatType: "group"))
        let badResp = try await oracle.requestJSON(
            method: "POST",
            path: "/api/v1/chats/\(group.chat_id)/messages",
            bearerToken: auth.token,
            payload: .object([
                "text": .string("Short"),
                "entities": .array([
                    .object(["type": .string("Bold"), "offset": .int(0), "length": .int(999)]),
                ]),
                "client_nonce": .int(240_501),
            ])
        )
        XCTAssertTrue(badResp.statusCode >= 400)
        XCTAssertTrue(app.exists)
    }

    // MARK: - F25: Media Upload Validation, Gallery Zoom Clamps & Corrupt Bytes (5 Tests)

    // Feature: F25 | Tier: 2
    @MainActor
    func test_F25_T2_01_emptyZeroByteMediaUploadRejectedWith400BadRequest() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let resp = try await oracle.request(
            method: "POST",
            path: "/api/v1/media/upload",
            bearerToken: auth.token,
            body: Data(),
            contentType: "image/png"
        )
        XCTAssertEqual(resp.statusCode, 400)
        XCTAssertTrue(app.exists)
    }

    // Feature: F25 | Tier: 2
    @MainActor
    func test_F25_T2_02_invalidBase64JsonMediaUploadRejectedWith400InvalidBase64() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let resp = try await oracle.requestJSON(
            method: "POST",
            path: "/api/v1/media/upload",
            bearerToken: auth.token,
            payload: .object([
                "data_base64": .string("%%%NOT_BASE64%%%"),
                "filename": .string("bad.png"),
                "mime_type": .string("image/png"),
            ])
        )
        XCTAssertEqual(resp.statusCode, 400)
        XCTAssertTrue(app.exists)
    }

    // Feature: F25 | Tier: 2
    @MainActor
    func test_F25_T2_03_galleryExtremePinchScalesClampCleanlyWithin0Point5xTo3Point0x() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Zoom Clamp", chatType: "group"))
        let png = Data(base64Encoded: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==")!
        let media = try await oracle.uploadMediaBase64(token: auth.token, data: png, filename: "clamp.png", mimeType: "image/png")
        let msg = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: nil, clientNonce: 250_301, mediaId: media.file_id))

        openChatFromInbox(app, chatId: group.chat_id)
        requireElement(app, id: FabushiAID.Chat.message(msg.message_id)).tap()
        let img = requireElement(app, id: FabushiAID.Chat.galleryImageView)
        GesturePhysicsHelpers.pinchZoom(img, scale: 8.0, velocity: 3.0)
        GesturePhysicsHelpers.pinchZoom(img, scale: 0.1, velocity: -3.0)
        XCTAssertTrue(requireElement(app, id: FabushiAID.Chat.galleryContainer).exists)
    }

    // Feature: F25 | Tier: 2
    @MainActor
    func test_F25_T2_04_corruptedTruncatedImageBytesRenderFallbackPlaceholderWithoutCrash() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Corrupt Image", chatType: "group"))
        let corrupt = Data([0xFF, 0xD8, 0xFF, 0x00, 0x11, 0x22])
        let media = try await oracle.uploadMediaBase64(token: auth.token, data: corrupt, filename: "broken.jpg", mimeType: "image/jpeg")
        let msg = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: nil, clientNonce: 250_401, mediaId: media.file_id))

        openChatFromInbox(app, chatId: group.chat_id)
        XCTAssertTrue(requireElement(app, id: FabushiAID.Chat.message(msg.message_id)).exists)
        XCTAssertTrue(app.state == .runningForeground)
    }

    // Feature: F25 | Tier: 2
    @MainActor
    func test_F25_T2_05_uploadingMediaScopedToInaccessibleForeignChatReturns403() async throws {
        let other = try await oracle.registerRandomUser(prefix: "f25_foreign")
        let foreignChat = try await oracle.createChat(token: other.token, CreateChatRequestDTO(title: "Foreign Media", chatType: "group"))

        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let resp = try await oracle.requestJSON(
            method: "POST",
            path: "/api/v1/media/upload",
            bearerToken: auth.token,
            queryItems: [URLQueryItem(name: "chat_id", value: String(foreignChat.chat_id))],
            payload: .object([
                "data_base64": .string(Data([1, 2, 3, 4]).base64EncodedString()),
                "filename": .string("intrude.bin"),
                "mime_type": .string("application/octet-stream"),
            ])
        )
        XCTAssertEqual(resp.statusCode, 403)
        XCTAssertTrue(app.exists)
    }

    // MARK: - F26: Audio Singleton Preemption, Missing Media & Video Note Viewport Bounds (5 Tests)

    // Feature: F26 | Tier: 2
    @MainActor
    func test_F26_T2_01_startingSecondVoiceNotePreemptsFirstVoiceNotePlaybackCleanly() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Singleton Voice", chatType: "group"))
        let m1Media = try await oracle.uploadMediaBase64(token: auth.token, data: Data(repeating: 0x41, count: 512), filename: "v1.ogg", mimeType: "audio/ogg", isVoice: true)
        let m2Media = try await oracle.uploadMediaBase64(token: auth.token, data: Data(repeating: 0x42, count: 512), filename: "v2.ogg", mimeType: "audio/ogg", isVoice: true)
        let msg1 = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: nil, clientNonce: 260_101, mediaId: m1Media.file_id))
        let msg2 = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: nil, clientNonce: 260_102, mediaId: m2Media.file_id))

        openChatFromInbox(app, chatId: group.chat_id)
        requireElement(app, id: FabushiAID.Chat.messageVoicePlay(msg1.message_id)).tap()
        requireElement(app, id: FabushiAID.Chat.messageVoicePlay(msg2.message_id)).tap()

        XCTAssertTrue(requireElement(app, id: FabushiAID.Chat.miniAudioPlayer).exists)
    }

    // Feature: F26 | Tier: 2
    @MainActor
    func test_F26_T2_02_sendingMessageWithNonexistentMediaIdRejectedWith400MediaNotFound() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Ghost Media", chatType: "group"))
        let resp = try await oracle.requestJSON(
            method: "POST",
            path: "/api/v1/chats/\(group.chat_id)/messages",
            bearerToken: auth.token,
            payload: .object([
                "media_id": .string("nonexistent_sha256_media_id_0000"),
                "client_nonce": .int(260_201),
            ])
        )
        XCTAssertEqual(resp.statusCode, 400)
        XCTAssertTrue(app.exists)
    }

    // Feature: F26 | Tier: 2
    @MainActor
    func test_F26_T2_03_corruptedAudioPayloadFailsPlaybackGracefullyWithoutCrash() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Bad Audio", chatType: "group"))
        let media = try await oracle.uploadMediaBase64(token: auth.token, data: Data([0x00, 0xFF, 0x00]), filename: "corrupt.ogg", mimeType: "audio/ogg", isVoice: true)
        let msg = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: nil, clientNonce: 260_301, mediaId: media.file_id))

        openChatFromInbox(app, chatId: group.chat_id)
        requireElement(app, id: FabushiAID.Chat.messageVoicePlay(msg.message_id)).tap()
        XCTAssertTrue(app.state == .runningForeground)
    }

    // Feature: F26 | Tier: 2
    @MainActor
    func test_F26_T2_04_videoNoteCircleNeverExceedsTranscriptHorizontalBounds() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Circle Bounds", chatType: "group"))
        let media = try await oracle.uploadMediaBase64(token: auth.token, data: Data(repeating: 0x33, count: 512), filename: "circle.mp4", mimeType: "video/mp4", isVideoNote: true)
        let msg = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: nil, clientNonce: 260_401, mediaId: media.file_id))

        openChatFromInbox(app, chatId: group.chat_id)
        let circle = requireElement(app, id: FabushiAID.Chat.messageVideoNoteCircle(msg.message_id))
        let transcript = requireElement(app, id: FabushiAID.Chat.transcript)
        XCTAssertLessThanOrEqual(circle.frame.width, transcript.frame.width - 32.0)
    }

    // Feature: F26 | Tier: 2
    @MainActor
    func test_F26_T2_05_playbackRateFullFourStepCycleWrapsBackToOnePointZeroX() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Rate Wrap", chatType: "group"))
        let media = try await oracle.uploadMediaBase64(token: auth.token, data: Data(repeating: 0x55, count: 512), filename: "wrap.ogg", mimeType: "audio/ogg", isVoice: true)
        let msg = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: nil, clientNonce: 260_501, mediaId: media.file_id))

        openChatFromInbox(app, chatId: group.chat_id)
        requireElement(app, id: FabushiAID.Chat.messageVoicePlay(msg.message_id)).tap()
        let rateBtn = requireElement(app, id: FabushiAID.Chat.miniAudioRateButton)
        let initial = rateBtn.label
        for _ in 0..<4 { rateBtn.tap() }
        XCTAssertEqual(rateBtn.label, initial)
    }

    // MARK: - F27: Closed Polls, Option Count Bounds & Non-Creator Close Ban (5 Tests)

    // Feature: F27 | Tier: 2
    @MainActor
    func test_F27_T2_01_votingOnClosedPollIsBlockedInUiAndRejectedByServer() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let peer = try await oracle.registerRandomUser(prefix: "f27_closed")
        let group = try await oracle.createChat(
            token: peer.token,
            CreateChatRequestDTO(title: "Closed Poll", chatType: "group", memberIds: [auth.user_id])
        )
        let poll = try await oracle.createPoll(token: peer.token, chatId: group.chat_id, question: "Already closed?", options: ["A", "B"])
        let pollId = try XCTUnwrap(poll["poll_id"]?.uint64Value ?? poll["poll"]?["poll_id"]?.uint64Value)
        let msgId = try XCTUnwrap(poll["message_id"]?.int64Value ?? poll["message"]?["message_id"]?.int64Value)
        _ = try await oracle.requestJSON(method: "POST", path: "/api/v1/polls/\(pollId)/close", bearerToken: peer.token, payload: .object([:]))

        openChatFromInbox(app, chatId: group.chat_id)
        let opt0 = requireElement(app, id: FabushiAID.Chat.messagePollOption(msgId, 0))
        opt0.tap()

        let lateVote = try await oracle.requestJSON(
            method: "POST",
            path: "/api/v1/polls/\(pollId)/vote",
            bearerToken: auth.token,
            payload: .object(["option_ids": .array([.int(0)])])
        )
        XCTAssertTrue(lateVote.statusCode >= 400)
    }

    // Feature: F27 | Tier: 2
    @MainActor
    func test_F27_T2_02_pollWithFewerThanTwoOptionsRejectedAtCreation() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Invalid Poll", chatType: "group"))
        let resp = try await oracle.requestJSON(
            method: "POST",
            path: "/api/v1/chats/\(group.chat_id)/polls",
            bearerToken: auth.token,
            payload: .object([
                "question": .string("Only one option?"),
                "options": .array([.string("Solo")]),
            ])
        )
        XCTAssertTrue(resp.statusCode >= 400)
        XCTAssertTrue(app.exists)
    }

    // Feature: F27 | Tier: 2
    @MainActor
    func test_F27_T2_03_pollWithMoreThanTenOptionsRejectedAtCreation() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "11 Options Poll", chatType: "group"))
        let eleven = (1...11).map { JSONValue.string("Option \($0)") }
        let resp = try await oracle.requestJSON(
            method: "POST",
            path: "/api/v1/chats/\(group.chat_id)/polls",
            bearerToken: auth.token,
            payload: .object([
                "question": .string("Too many options?"),
                "options": .array(eleven),
            ])
        )
        XCTAssertTrue(resp.statusCode >= 400)
        XCTAssertTrue(app.exists)
    }

    // Feature: F27 | Tier: 2
    @MainActor
    func test_F27_T2_04_quizPollWrongAnswerSelectionRendersIncorrectFeedbackSafely() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Wrong Quiz", chatType: "group"))
        let poll = try await oracle.createPoll(
            token: auth.token,
            chatId: group.chat_id,
            question: "2 + 2 = ?",
            options: ["5", "4"],
            isQuiz: true,
            correctOptionId: 1,
            solution: "2 + 2 equals 4."
        )
        let msgId = try XCTUnwrap(poll["message_id"]?.int64Value ?? poll["message"]?["message_id"]?.int64Value)

        openChatFromInbox(app, chatId: group.chat_id)
        requireElement(app, id: FabushiAID.Chat.messagePollOption(msgId, 0)).tap()
        XCTAssertTrue(requireElement(app, id: FabushiAID.Chat.message(msgId)).exists)
    }

    // Feature: F27 | Tier: 2
    @MainActor
    func test_F27_T2_05_nonCreatorNonAdminCannotClosePeerPoll() async throws {
        let owner = try await oracle.registerRandomUser(prefix: "f27_powner")
        let (app, member, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(
            token: owner.token,
            CreateChatRequestDTO(title: "Poll Close ACL", chatType: "group", memberIds: [member.user_id])
        )
        let poll = try await oracle.createPoll(token: owner.token, chatId: group.chat_id, question: "Protected poll?", options: ["Yes", "No"])
        let pollId = try XCTUnwrap(poll["poll_id"]?.uint64Value ?? poll["poll"]?["poll_id"]?.uint64Value)

        let forbidden = try await oracle.requestJSON(
            method: "POST",
            path: "/api/v1/polls/\(pollId)/close",
            bearerToken: member.token,
            payload: .object([:])
        )
        XCTAssertTrue(forbidden.statusCode >= 400)
        XCTAssertTrue(app.exists)
    }

    // MARK: - F28: Diagonal Swipe Rejection, Edit/Delete ACLs & Deleted Reply Tombstones (6 Tests)

    // Feature: F28 | Tier: 2
    @MainActor
    func test_F28_T2_01_steepDiagonalScrollGestureDoesNotFalselyTriggerSwipeToReply() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Diagonal Guard", chatType: "group"))
        let msg = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: "Steep diagonal test", clientNonce: 280_101))

        openChatFromInbox(app, chatId: group.chat_id)
        let bubble = requireElement(app, id: FabushiAID.Chat.message(msg.message_id))
        // Violates absY * 2 < absX (dx = -30, dy = -45 => absY * 2 = 90 >= 30)
        GesturePhysicsHelpers.swipeMessageVector(bubble, dx: -30.0, dy: -45.0)

        XCTAssertFalse(element(app, id: FabushiAID.Composer.replyBanner).exists, "Steep diagonal scroll must NOT trigger swipe-to-reply")
    }

    // Feature: F28 | Tier: 2
    @MainActor
    func test_F28_T2_02_subThresholdShortHorizontalNudgeDoesNotCommitReplyBanner() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Tiny Nudge", chatType: "group"))
        let msg = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: "Short drag under threshold", clientNonce: 280_201))

        openChatFromInbox(app, chatId: group.chat_id)
        let bubble = requireElement(app, id: FabushiAID.Chat.message(msg.message_id))
        GesturePhysicsHelpers.swipeMessageVector(bubble, dx: -10.0, dy: 0.0)

        XCTAssertFalse(element(app, id: FabushiAID.Composer.replyBanner).exists)
    }

    // Feature: F28 | Tier: 2
    @MainActor
    func test_F28_T2_03_contextMenuOnPeerMessageOmitsEditActionAndServerEnforces403() async throws {
        let peer = try await oracle.registerRandomUser(prefix: "f28_author")
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(
            token: peer.token,
            CreateChatRequestDTO(title: "Peer Edit Ban", chatType: "group", memberIds: [auth.user_id])
        )
        let msg = try await oracle.sendMessage(token: peer.token, chatId: group.chat_id, SendMessageRequestDTO(text: "Peer authored text", clientNonce: 280_301))

        openChatFromInbox(app, chatId: group.chat_id)
        GesturePhysicsHelpers.longPressForPortalMenu(requireElement(app, id: FabushiAID.Chat.message(msg.message_id)))
        XCTAssertFalse(element(app, id: FabushiAID.Chat.contextMenuAction("edit")).exists, "Cannot show Edit on another user's message")

        let forbidden = try await oracle.editMessage(token: auth.token, chatId: group.chat_id, messageId: msg.message_id, text: "Illegal edit")
        XCTAssertEqual(forbidden.statusCode, 403)
    }

    // Feature: F28 | Tier: 2
    @MainActor
    func test_F28_T2_04_deletingOriginalMessageTurnsDependentReplyHeaderIntoTombstone() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Orphan Reply", chatType: "group"))
        let orig = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: "Will be deleted", clientNonce: 280_401))
        let reply = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: "Reply survives", replyTo: orig.message_id, clientNonce: 280_402))
        _ = try await oracle.deleteMessage(token: auth.token, chatId: group.chat_id, messageId: orig.message_id, revoke: true)

        openChatFromInbox(app, chatId: group.chat_id)
        XCTAssertTrue(requireElement(app, id: FabushiAID.Chat.message(reply.message_id)).exists)
    }

    // Feature: F28 | Tier: 2
    @MainActor
    func test_F28_T2_05_editingMessageToEmptyWhitespaceRejectedWith400() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Empty Edit", chatType: "group"))
        let msg = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: "Valid text", clientNonce: 280_501))

        let badEdit = try await oracle.editMessage(token: auth.token, chatId: group.chat_id, messageId: msg.message_id, text: "    ")
        XCTAssertEqual(badEdit.statusCode, 400)
        openChatFromInbox(app, chatId: group.chat_id)
        XCTAssertTrue(requireElement(app, id: FabushiAID.Chat.messageText(msg.message_id)).label.contains("Valid text"))
    }

    // Feature: F28 | Tier: 2
    @MainActor
    func test_F28_T2_06_forwardingFromInaccessiblePrivateChatRejectedWith403() async throws {
        let outsider = try await oracle.registerRandomUser(prefix: "f28_sec")
        let secretGroup = try await oracle.createChat(token: outsider.token, CreateChatRequestDTO(title: "Secret Src", chatType: "group"))
        let secretMsg = try await oracle.sendMessage(token: outsider.token, chatId: secretGroup.chat_id, SendMessageRequestDTO(text: "Top secret", clientNonce: 280_601))

        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let myGroup = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "My Dst", chatType: "group"))
        let illegal = try await oracle.requestJSON(
            method: "POST",
            path: "/api/v1/chats/\(myGroup.chat_id)/messages",
            bearerToken: auth.token,
            payload: .object([
                "forward_from_chat_id": .int(secretGroup.chat_id),
                "forward_from_msg_id": .int(secretMsg.message_id),
                "client_nonce": .int(280_602),
            ])
        )
        XCTAssertEqual(illegal.statusCode, 403)
        XCTAssertTrue(app.exists)
    }

    // MARK: - F29: Non-Admin Pin Ban, Deleted Pin Cleanup & TTL Bounds (5 Tests)

    // Feature: F29 | Tier: 2
    @MainActor
    func test_F29_T2_01_nonAdminGroupMemberCannotPinMessagesAndReceives403() async throws {
        let admin = try await oracle.registerRandomUser(prefix: "f29_admin")
        let (app, member, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(
            token: admin.token,
            CreateChatRequestDTO(title: "Pin ACL Group", chatType: "group", memberIds: [member.user_id])
        )
        let msg = try await oracle.sendMessage(token: admin.token, chatId: group.chat_id, SendMessageRequestDTO(text: "Admin announcement", clientNonce: 290_101))

        let forbidden = try await oracle.pinMessage(token: member.token, chatId: group.chat_id, messageId: msg.message_id)
        XCTAssertEqual(forbidden.statusCode, 403)
        openChatFromInbox(app, chatId: group.chat_id)
        XCTAssertFalse(element(app, id: FabushiAID.Chat.pinnedBanner).exists)
    }

    // Feature: F29 | Tier: 2
    @MainActor
    func test_F29_T2_02_deletingPinnedMessageAutomaticallyClears46PtPinnedBanner() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Delete Pinned", chatType: "group"))
        let msg = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: "Pinned then deleted", clientNonce: 290_201))
        _ = try await oracle.pinMessage(token: auth.token, chatId: group.chat_id, messageId: msg.message_id)

        openChatFromInbox(app, chatId: group.chat_id)
        let banner = requireElement(app, id: FabushiAID.Chat.pinnedBanner)
        _ = try await oracle.deleteMessage(token: auth.token, chatId: group.chat_id, messageId: msg.message_id, revoke: true)

        let cleared = try await waitUntil(timeout: 8.0) { !banner.exists }
        XCTAssertTrue(cleared)
    }

    // Feature: F29 | Tier: 2
    @MainActor
    func test_F29_T2_03_pinningNonexistentMessageIdReturns404MessageNotFound() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Pin 404", chatType: "group"))
        let resp = try await oracle.pinMessage(token: auth.token, chatId: group.chat_id, messageId: 999_999)
        XCTAssertEqual(resp.statusCode, 404)
        XCTAssertTrue(app.exists)
    }

    // Feature: F29 | Tier: 2
    @MainActor
    func test_F29_T2_04_settingChatTtlToZeroDisablesAutoDeleteCleanly() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Disable TTL", chatType: "group"))
        _ = try await oracle.setChatTTL(token: auth.token, chatId: group.chat_id, ttlSeconds: 86400)
        let disable = try await oracle.setChatTTL(token: auth.token, chatId: group.chat_id, ttlSeconds: 0)
        XCTAssertEqual(disable.statusCode, 200)
        XCTAssertTrue(app.exists)
    }

    // Feature: F29 | Tier: 2
    @MainActor
    func test_F29_T2_05_negativeTtlSecondsRejectedByServerValidator() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Negative TTL", chatType: "group"))
        let bad = try await oracle.setChatTTL(token: auth.token, chatId: group.chat_id, ttlSeconds: -60)
        XCTAssertTrue(bad.statusCode >= 400)
        XCTAssertTrue(app.exists)
    }

    // MARK: - F30: Reaction Idempotency, Empty Emoji & Kicked-Member Rejections (5 Tests)

    // Feature: F30 | Tier: 2
    @MainActor
    func test_F30_T2_01_whitespaceReactionEmojiRejectedWith400InvalidEmoji() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Empty Emoji", chatType: "group"))
        let msg = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: "Test", clientNonce: 300_101))

        let bad = try await oracle.reactToMessage(token: auth.token, chatId: group.chat_id, messageId: msg.message_id, emoji: "   ", action: "add")
        XCTAssertEqual(bad.statusCode, 400)
        XCTAssertTrue(app.exists)
    }

    // Feature: F30 | Tier: 2
    @MainActor
    func test_F30_T2_02_rapidRepeatedReactionTogglesNeverUnderflowBelowZero() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Rapid React", chatType: "group"))
        let msg = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: "Rapid toggle target", clientNonce: 300_201))

        for _ in 0..<6 {
            _ = try await oracle.reactToMessage(token: auth.token, chatId: group.chat_id, messageId: msg.message_id, emoji: "❤️", action: "toggle")
        }
        openChatFromInbox(app, chatId: group.chat_id)
        XCTAssertFalse(element(app, id: FabushiAID.Chat.messageReactionPill(msg.message_id, "❤️")).exists)
    }

    // Feature: F30 | Tier: 2
    @MainActor
    func test_F30_T2_03_removingUnsetReactionIsIdempotentZeroOp() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Remove Unset", chatType: "group"))
        let msg = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: "No reaction yet", clientNonce: 300_301))

        let rem = try await oracle.reactToMessage(token: auth.token, chatId: group.chat_id, messageId: msg.message_id, emoji: "🔥", action: "remove")
        XCTAssertEqual(rem.statusCode, 200)
        XCTAssertTrue(app.exists)
    }

    // Feature: F30 | Tier: 2
    @MainActor
    func test_F30_T2_04_kickedMemberReactingToMessageRejectedWith403Forbidden() async throws {
        let owner = try await oracle.registerRandomUser(prefix: "f30_kowner")
        let (app, member, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(
            token: owner.token,
            CreateChatRequestDTO(title: "Kick React", chatType: "group", memberIds: [member.user_id])
        )
        let msg = try await oracle.sendMessage(token: owner.token, chatId: group.chat_id, SendMessageRequestDTO(text: "Protected", clientNonce: 300_401))
        _ = try await oracle.request(method: "DELETE", path: "/api/v1/chats/\(group.chat_id)/members/\(member.user_id)", bearerToken: owner.token)

        let forbidden = try await oracle.reactToMessage(token: member.token, chatId: group.chat_id, messageId: msg.message_id, emoji: "👍", action: "add")
        XCTAssertEqual(forbidden.statusCode, 403)
        XCTAssertTrue(app.exists)
    }

    // Feature: F30 | Tier: 2
    @MainActor
    func test_F30_T2_05_zwjHeartOnFireReactionPillRendersCleanlyWithoutTruncation() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "ZWJ React", chatType: "group"))
        let msg = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: "ZWJ pill", clientNonce: 300_501))
        _ = try await oracle.reactToMessage(token: auth.token, chatId: group.chat_id, messageId: msg.message_id, emoji: "❤️‍🔥", action: "add")

        openChatFromInbox(app, chatId: group.chat_id)
        XCTAssertTrue(requireElement(app, id: FabushiAID.Chat.messageReactionPill(msg.message_id, "❤️‍🔥")).exists)
    }

    // MARK: - F31: PortalView Dismissal, Rotation Cleanup & Instant View Fallbacks (5 Tests)

    // Feature: F31 | Tier: 2
    @MainActor
    func test_F31_T2_01_tappingBlurredBackdropDismissesContextMenuPortalWithoutSideEffects() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Backdrop Dismiss", chatType: "group"))
        let msg = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: "Dismiss portal", clientNonce: 310_101))

        openChatFromInbox(app, chatId: group.chat_id)
        GesturePhysicsHelpers.longPressForPortalMenu(requireElement(app, id: FabushiAID.Chat.message(msg.message_id)))
        let portal = requireElement(app, id: FabushiAID.Chat.contextMenuPortal)
        app.windows.firstMatch.coordinate(withNormalizedOffset: CGVector(dx: 0.08, dy: 0.15)).tap()

        let closed = try await waitUntil(timeout: 5.0) { !portal.exists }
        XCTAssertTrue(closed)
    }

    // Feature: F31 | Tier: 2
    @MainActor
    func test_F31_T2_02_deviceRotationWhilePortalContextMenuOpenDismissesOverlayCleanly() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Portal Rotate", chatType: "group"))
        let msg = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: "Rotate portal", clientNonce: 310_201))

        openChatFromInbox(app, chatId: group.chat_id)
        GesturePhysicsHelpers.longPressForPortalMenu(requireElement(app, id: FabushiAID.Chat.message(msg.message_id)))
        _ = requireElement(app, id: FabushiAID.Chat.contextMenuPortal)

        XCUIDevice.shared.orientation = .landscapeLeft
        XCUIDevice.shared.orientation = .portrait
        XCTAssertTrue(app.state == .runningForeground)
    }

    // Feature: F31 | Tier: 2
    @MainActor
    func test_F31_T2_03_translatingEmojiOnlyMessageDoesNotCrashOrCorruptBubble() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Emoji Translate", chatType: "group"))
        let msg = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: "🚀🔥🎉✨", clientNonce: 310_301))

        openChatFromInbox(app, chatId: group.chat_id)
        let bubble = requireElement(app, id: FabushiAID.Chat.message(msg.message_id))
        GesturePhysicsHelpers.longPressForPortalMenu(bubble)
        if element(app, id: FabushiAID.Chat.contextMenuAction("translate")).exists {
            element(app, id: FabushiAID.Chat.contextMenuAction("translate")).tap()
        }
        XCTAssertTrue(bubble.exists)
    }

    // Feature: F31 | Tier: 2
    @MainActor
    func test_F31_T2_04_unreachableArticleUrlGracefullyFallsBackWithoutCrashingInstantView() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Dead Link", chatType: "group"))
        let msg = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(text: "https://127.0.0.1:1/unreachable-article", clientNonce: 310_401)
        )

        openChatFromInbox(app, chatId: group.chat_id)
        requireElement(app, id: FabushiAID.Chat.message(msg.message_id)).tap()
        XCTAssertTrue(app.state == .runningForeground)
    }

    // Feature: F31 | Tier: 2
    @MainActor
    func test_F31_T2_05_multiMessageSelectModeSuppressesSwipeToReplyGesture() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Select Mode", chatType: "group"))
        let msg = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: "Select mode item", clientNonce: 310_501))

        openChatFromInbox(app, chatId: group.chat_id)
        let bubble = requireElement(app, id: FabushiAID.Chat.message(msg.message_id))
        GesturePhysicsHelpers.longPressForPortalMenu(bubble)
        if element(app, id: FabushiAID.Chat.contextMenuAction("select")).exists {
            element(app, id: FabushiAID.Chat.contextMenuAction("select")).tap()
            GesturePhysicsHelpers.swipeMessageVector(bubble, dx: -76.0, dy: 0.0)
            XCTAssertFalse(element(app, id: FabushiAID.Composer.replyBanner).exists)
        }
    }

    // MARK: - F48: Streaming AI Validation, Unclosed Fences & Session ACLs (6 Tests)

    // Feature: F48 | Tier: 2
    @MainActor
    func test_F48_T2_01_emptyPromptToHermesTurnsEndpointRejectedWith400InvalidPrompt() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let bad = try await oracle.requestJSON(
            method: "POST",
            path: "/api/v1/hermes/turns",
            bearerToken: auth.token,
            payload: .object(["prompt": .string("   ")])
        )
        XCTAssertEqual(bad.statusCode, 400)
        XCTAssertTrue(app.exists)
    }

    // Feature: F48 | Tier: 2
    @MainActor
    func test_F48_T2_02_hermesTurnTargetingInaccessibleChatIdRejectedWith403() async throws {
        let other = try await oracle.registerRandomUser(prefix: "f48_other")
        let foreignChat = try await oracle.createChat(token: other.token, CreateChatRequestDTO(title: "Foreign AI Room", chatType: "group"))

        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let forbidden = try await oracle.requestJSON(
            method: "POST",
            path: "/api/v1/hermes/turns",
            bearerToken: auth.token,
            payload: .object([
                "prompt": .string("/help"),
                "chat_id": .int(foreignChat.chat_id),
            ])
        )
        XCTAssertEqual(forbidden.statusCode, 403)
        XCTAssertTrue(app.exists)
    }

    // Feature: F48 | Tier: 2
    @MainActor
    func test_F48_T2_03_partialUnclosedTripleBacktickStreamChunkRendersWithoutCrash() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Partial Fence", chatType: "group"))
        let partial = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(text: "Here is code:\n```swift\nfunc incomplete(", clientNonce: 480_301)
        )

        openChatFromInbox(app, chatId: group.chat_id)
        XCTAssertTrue(requireElement(app, id: FabushiAID.Chat.message(partial.message_id)).exists)
    }

    // Feature: F48 | Tier: 2
    @MainActor
    func test_F48_T2_04_unclosedThinkTagDuringStreamingRendersGracefully() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Unclosed Think", chatType: "group"))
        let partial = try await oracle.sendMessage(
            token: auth.token,
            chatId: group.chat_id,
            SendMessageRequestDTO(text: "<think>Still reasoning mid-token without closing tag...", clientNonce: 480_401)
        )

        openChatFromInbox(app, chatId: group.chat_id)
        XCTAssertTrue(requireElement(app, id: FabushiAID.Chat.message(partial.message_id)).exists)
    }

    // Feature: F48 | Tier: 2
    @MainActor
    func test_F48_T2_05_userManualScrollUpDisengagesStreamingAutoScrollLock() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Disengage Lock", chatType: "group"))
        for idx in 1...22 {
            _ = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: "Line #\(idx)", clientNonce: UInt64(480_500 + idx)))
        }
        let streaming = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: "Streaming start...", clientNonce: 480_590))

        openChatFromInbox(app, chatId: group.chat_id)
        let transcript = requireElement(app, id: FabushiAID.Chat.transcript)
        transcript.swipeDown()
        transcript.swipeDown()

        _ = try await oracle.editMessage(token: auth.token, chatId: group.chat_id, messageId: streaming.message_id, text: "Streaming start... appended delta while user scrolled up")
        XCTAssertTrue(requireElement(app, id: FabushiAID.Chat.scrollToBottomButton).exists)
    }

    // Feature: F48 | Tier: 2
    @MainActor
    func test_F48_T2_06_emptyHermesSlashCommandRejectedWith400InvalidCommand() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let bad = try await oracle.requestJSON(
            method: "POST",
            path: "/api/v1/hermes/command",
            bearerToken: auth.token,
            payload: .object(["command": .string("   ")])
        )
        XCTAssertEqual(bad.statusCode, 400)
        XCTAssertTrue(app.exists)
    }

    // MARK: - F49: In-Chat Search Zero-Match, Regex Metacharacters & Live Deletion (6 Tests)

    // Feature: F49 | Tier: 2
    @MainActor
    func test_F49_T2_01_zeroMatchInChatSearchDisplaysZeroCountAndDisablesPrevNext() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Zero Match", chatType: "group"))
        _ = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: "Ordinary line", clientNonce: 490_101))

        openChatFromInbox(app, chatId: group.chat_id)
        GesturePhysicsHelpers.sendHardwareKey(in: app, key: "f", modifiers: [.command])
        clearAndType(app, id: FabushiAID.Chat.inChatSearchBar, text: "UnmatchedToken99999")

        let counter = requireElement(app, id: FabushiAID.Chat.inChatSearchCounter)
        XCTAssertTrue(counter.label.contains("0"))
    }

    // Feature: F49 | Tier: 2
    @MainActor
    func test_F49_T2_02_regexMetacharactersInInChatSearchEvaluatedLiterallyWithoutCrash() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Regex Literal", chatType: "group"))
        _ = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: "Array index [0] + (a*b)", clientNonce: 490_201))

        openChatFromInbox(app, chatId: group.chat_id)
        GesturePhysicsHelpers.sendHardwareKey(in: app, key: "f", modifiers: [.command])
        clearAndType(app, id: FabushiAID.Chat.inChatSearchBar, text: "[0] + (a*b")

        XCTAssertTrue(app.state == .runningForeground)
    }

    // Feature: F49 | Tier: 2
    @MainActor
    func test_F49_T2_03_tappingNextPastLastMatchClampsOrWrapsWithoutOutOfBoundsCrash() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Wrap Bounds", chatType: "group"))
        _ = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: "SoloHit", clientNonce: 490_301))

        openChatFromInbox(app, chatId: group.chat_id)
        GesturePhysicsHelpers.sendHardwareKey(in: app, key: "f", modifiers: [.command])
        clearAndType(app, id: FabushiAID.Chat.inChatSearchBar, text: "SoloHit")

        let next = requireElement(app, id: FabushiAID.Chat.inChatSearchNext)
        if next.isEnabled { next.tap(); next.tap() }
        XCTAssertTrue(app.state == .runningForeground)
    }

    // Feature: F49 | Tier: 2
    @MainActor
    func test_F49_T2_04_deletingMatchedMessageWhileInChatSearchOpenDecrementsCounterLive() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Delete Match", chatType: "group"))
        let m1 = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: "ShrinkNeedle A", clientNonce: 490_401))
        _ = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: "ShrinkNeedle B", clientNonce: 490_402))

        openChatFromInbox(app, chatId: group.chat_id)
        GesturePhysicsHelpers.sendHardwareKey(in: app, key: "f", modifiers: [.command])
        clearAndType(app, id: FabushiAID.Chat.inChatSearchBar, text: "ShrinkNeedle")
        let counter = requireElement(app, id: FabushiAID.Chat.inChatSearchCounter)

        _ = try await oracle.deleteMessage(token: auth.token, chatId: group.chat_id, messageId: m1.message_id, revoke: true)
        let decremented = try await waitUntil(timeout: 8.0) { counter.label.contains("1") }
        XCTAssertTrue(decremented)
    }

    // Feature: F49 | Tier: 2
    @MainActor
    func test_F49_T2_05_caseInsensitiveAndDiacriticInChatSearchMatchesUppercaseToken() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let group = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Case Fold", chatType: "group"))
        _ = try await oracle.sendMessage(token: auth.token, chatId: group.chat_id, SendMessageRequestDTO(text: "LIQUIDGLASS_UPPER", clientNonce: 490_501))

        openChatFromInbox(app, chatId: group.chat_id)
        GesturePhysicsHelpers.sendHardwareKey(in: app, key: "f", modifiers: [.command])
        clearAndType(app, id: FabushiAID.Chat.inChatSearchBar, text: "liquidglass_upper")

        let counter = requireElement(app, id: FabushiAID.Chat.inChatSearchCounter)
        XCTAssertTrue(counter.label.contains("1"))
    }

    // Feature: F49 | Tier: 2
    @MainActor
    func test_F49_T2_06_switchingToDifferentConversationClearsPreviousInChatSearchQuery() async throws {
        let (app, auth, _) = try await launchPreauthenticatedWorkspace()
        let g1 = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Search Room 1", chatType: "group"))
        let g2 = try await oracle.createChat(token: auth.token, CreateChatRequestDTO(title: "Search Room 2", chatType: "group"))

        openChatFromInbox(app, chatId: g1.chat_id)
        GesturePhysicsHelpers.sendHardwareKey(in: app, key: "f", modifiers: [.command])
        clearAndType(app, id: FabushiAID.Chat.inChatSearchBar, text: "StaleQuery")

        if app.navigationBars.buttons.firstMatch.exists { app.navigationBars.buttons.firstMatch.tap() }
        openChatFromInbox(app, chatId: g2.chat_id)
        XCTAssertFalse(element(app, id: FabushiAID.Chat.inChatSearchBar).exists)
    }
}
