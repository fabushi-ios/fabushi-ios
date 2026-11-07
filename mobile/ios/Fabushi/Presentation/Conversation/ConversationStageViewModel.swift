//
//  ConversationStageViewModel.swift
//  Fabushi
//
//  Production view model for Telegram 1:1 conversation stage.
//  Coordinates inverted message list, optimistic sending with 64-bit client nonce,
//  hysteretic scroll tracking, draft persistence, pinned message carousel, and real-time push events.
//

import Combine
import Foundation
import SwiftUI

@MainActor
public final class ConversationStageViewModel: ObservableObject {
    public let chatId: Int64
    public let currentUserId: Int64
    public let isSecretChat: Bool

    // Messages array: Index 0 = physical bottom / newest message (inverted coordinate space)
    @Published public private(set) var messages: [TelegramStageMessage] = []
    @Published public private(set) var pinnedMessages: [TelegramStageMessage] = []
    @Published public var activePinnedIndex: Int = 0

    // Scroll & Hysteresis state
    @Published public var scrollDistanceToBottom: CGFloat = 0.0
    @Published public var unreadScrolledCount: Int = 0
    @Published public var isBottomButtonVisible: Bool = false

    // Highlight & Selection state
    @Published public var highlightedMessageId: Int64? = nil
    @Published public var activeReplyMessage: TelegramStageMessage? = nil
    @Published public var revealedSpoilers: Set<String> = []

    // Draft state
    @Published public var draftText: String = ""

    // Milestone 4: Drawer & Fullscreen Media Viewer Navigation States
    @Published public var showAttachmentDrawer: Bool = false
    @Published public var showStickerPicker: Bool = false
    @Published public var activeMediaViewerVM: MediaViewerViewModel? = nil

    private var eventTask: Task<Void, Never>?
    private let draftKey: String

    public init(
        chatId: Int64,
        currentUserId: Int64 = 1,
        isSecretChat: Bool = false,
        initialMessages: [TelegramStageMessage] = []
    ) {
        self.chatId = chatId
        self.currentUserId = currentUserId
        self.isSecretChat = isSecretChat
        self.draftKey = "telegram_draft_\(chatId)"

        if let savedDraft = UserDefaults.standard.string(forKey: draftKey) {
            self.draftText = savedDraft
        }

        if !initialMessages.isEmpty {
            self.messages = initialMessages.sorted { $0.timestamp > $1.timestamp }
            self.pinnedMessages = self.messages.filter { $0.isPinned }
        }

        startEventListening()
    }

    deinit {
        eventTask?.cancel()
    }

    // MARK: - Draft Management
    public func saveDraft() {
        let trimmed = draftText.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.isEmpty {
            UserDefaults.standard.removeObject(forKey: draftKey)
        } else {
            UserDefaults.standard.set(trimmed, forKey: draftKey)
        }
    }

    // MARK: - Initial Loading
    public func loadInitialMessages() async {
        do {
            struct GetMessagesParams: Encodable {
                let chat_id: Int64
            }
            let fetched: [TelegramStageMessage] = try await TelegramBridgeRuntime.shared.dispatch(
                command: "telegram.getMessages",
                params: GetMessagesParams(chat_id: chatId)
            )
            self.messages = fetched.sorted { $0.timestamp > $1.timestamp }
            self.pinnedMessages = self.messages.filter { $0.isPinned }
        } catch {
            // Keep existing messages on bridge fallback/uninitialized
            if self.messages.isEmpty {
                self.pinnedMessages = self.messages.filter { $0.isPinned }
            }
        }
    }

    // MARK: - Message Transmission (Optimistic insertion with 64-bit client nonce)
    public func sendTextMessage(_ text: String, replyTo: Int64? = nil) async {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }

        let nonce = NonceGenerator.nextNonce()
        let provisionalId = -Int64(Date().timeIntervalSince1970 * 1000)
        let nowMs = Int64(Date().timeIntervalSince1970 * 1000)

        let provisionalMessage = TelegramStageMessage(
            id: provisionalId,
            chatId: chatId,
            senderId: currentUserId,
            text: trimmed,
            timestamp: nowMs,
            replyTo: replyTo ?? activeReplyMessage?.id,
            clientNonce: nonce,
            status: .pending,
            isPinned: false,
            isOutgoing: true
        )

        // Optimistically insert at top of array (physical bottom)
        messages.insert(provisionalMessage, at: 0)
        draftText = ""
        saveDraft()
        activeReplyMessage = nil

        struct SendMessageResponse: Decodable {
            let id: Int64
            let status: String?
        }

        do {
            let payload = SendMessagePayload(
                chat_id: chatId,
                text: trimmed,
                sender_id: UInt64(currentUserId),
                reply_to: provisionalMessage.replyTo,
                client_nonce: nonce
            )

            let response: SendMessageResponse = try await TelegramBridgeRuntime.shared.dispatch(
                command: "telegram.sendMessage",
                params: payload
            )

            if let idx = messages.firstIndex(where: { $0.clientNonce == nonce || $0.id == provisionalId }) {
                messages[idx] = TelegramStageMessage(
                    id: response.id,
                    chatId: chatId,
                    senderId: currentUserId,
                    text: trimmed,
                    timestamp: provisionalMessage.timestamp,
                    replyTo: provisionalMessage.replyTo,
                    clientNonce: nonce,
                    status: .sent,
                    isPinned: provisionalMessage.isPinned,
                    isOutgoing: true
                )
            }
        } catch {
            if let idx = messages.firstIndex(where: { $0.clientNonce == nonce || $0.id == provisionalId }) {
                messages[idx].status = .failed
            }
        }
    }

    // MARK: - Scroll Tracking & Hysteresis
    public func updateScrollDistance(_ distance: CGFloat) {
        self.scrollDistanceToBottom = max(0, distance)

        // Update hysteresis appearance/dismissal
        self.isBottomButtonVisible = ScrollToBottomButton.evaluateVisibility(
            distance: scrollDistanceToBottom,
            currentlyVisible: isBottomButtonVisible
        )

        // Auto-reset unread counter on bottom proximity (< 100pt)
        if scrollDistanceToBottom < 100.0 {
            unreadScrolledCount = 0
        }
    }

    public func scrollToBottomAction() {
        unreadScrolledCount = 0
        scrollDistanceToBottom = 0.0
        isBottomButtonVisible = false
    }

    // MARK: - Pinned Messages Navigation
    @discardableResult
    public func jumpToPinnedMessage(at index: Int) -> Int64? {
        guard index >= 0 && index < pinnedMessages.count else { return nil }
        activePinnedIndex = index
        let target = pinnedMessages[index]
        highlightedMessageId = target.id

        // 0.8s flash highlight timer (TC-T1-F17-02)
        Task { [weak self] in
            try? await Task.sleep(nanoseconds: 800_000_000)
            guard let self else { return }
            if self.highlightedMessageId == target.id {
                self.highlightedMessageId = nil
            }
        }

        return target.id
    }

    @discardableResult
    public func cyclePinnedMessages() -> Int64? {
        guard !pinnedMessages.isEmpty else { return nil }
        let nextIndex = (activePinnedIndex + 1) % pinnedMessages.count
        return jumpToPinnedMessage(at: nextIndex)
    }

    public func unpinMessage(id: Int64) {
        pinnedMessages.removeAll { $0.id == id }
        if let idx = messages.firstIndex(where: { $0.id == id }) {
            messages[idx].isPinned = false
        }
        if activePinnedIndex >= pinnedMessages.count {
            activePinnedIndex = max(0, pinnedMessages.count - 1)
        }
    }

    public static let maxPinnedMessages: Int = 5

    @discardableResult
    public func pinMessage(id: Int64) -> Bool {
        guard pinnedMessages.count < Self.maxPinnedMessages else { return false }
        guard let idx = messages.firstIndex(where: { $0.id == id }) else { return false }
        messages[idx].isPinned = true
        if !pinnedMessages.contains(where: { $0.id == id }) {
            pinnedMessages.append(messages[idx])
        }
        return true
    }

    // MARK: - Reactions & Deletion
    public func toggleReaction(messageId: Int64, emoji: String) {
        guard let idx = messages.firstIndex(where: { $0.id == messageId }) else { return }
        var rxList = messages[idx].reactions
        if let rIdx = rxList.firstIndex(where: { $0.emoji == emoji }) {
            if rxList[rIdx].chosenByMe {
                rxList[rIdx].count -= 1
                rxList[rIdx].chosenByMe = false
                if rxList[rIdx].count <= 0 {
                    rxList.remove(at: rIdx)
                }
            } else {
                rxList[rIdx].count += 1
                rxList[rIdx].chosenByMe = true
            }
        } else {
            rxList.append(TelegramReaction(emoji: emoji, count: 1, chosenByMe: true))
        }
        messages[idx].reactions = rxList
    }

    public func deleteMessage(id: Int64) {
        messages.removeAll { $0.id == id }
        pinnedMessages.removeAll { $0.id == id }
    }

    public func revealSpoiler(id: String) {
        revealedSpoilers.insert(id)
    }

    // MARK: - Push Event Stream Processing
    private func startEventListening() {
        eventTask = Task { [weak self] in
            for await event in TelegramBridgeRuntime.shared.eventStream {
                guard let self else { return }
                self.handleIncomingPushEvent(event)
            }
        }
    }

    public func handleIncomingPushEvent(_ event: TelegramEvent) {
        switch event {
        case .newMessage(let id, let cId, let sId, let text, let ts, let replyTo):
            guard cId == self.chatId else { return }

            // Deduplicate if already present
            if let idx = self.messages.firstIndex(where: { $0.id == id }) {
                self.messages[idx].status = .sent
                return
            }

            let isOut = (sId == self.currentUserId)
            let newMsg = TelegramStageMessage(
                id: id,
                chatId: cId,
                senderId: sId,
                text: text,
                timestamp: ts,
                replyTo: replyTo,
                status: .sent,
                isOutgoing: isOut
            )
            self.messages.insert(newMsg, at: 0)

            // Increment unread if user is scrolled away from bottom (>= 100pt)
            if self.scrollDistanceToBottom >= 100.0 {
                self.unreadScrolledCount += 1
            }

        case .readHistory(let cId, let maxId):
            guard cId == self.chatId else { return }
            for idx in self.messages.indices where self.messages[idx].id <= maxId && self.messages[idx].status == .sent {
                self.messages[idx].status = .read
            }

        case .deleteMessages(let cId, let ids):
            guard cId == self.chatId else { return }
            let idSet = Set(ids)
            self.messages.removeAll { idSet.contains($0.id) }
            self.pinnedMessages.removeAll { idSet.contains($0.id) }

        case .reactionUpdated(let cId, let mId, let emoji, let count, let chosenByMe):
            guard cId == self.chatId else { return }
            guard let idx = self.messages.firstIndex(where: { $0.id == mId }) else { return }
            var list = self.messages[idx].reactions
            if let rIdx = list.firstIndex(where: { $0.emoji == emoji }) {
                list[rIdx].count = count
                list[rIdx].chosenByMe = chosenByMe
                if count <= 0 { list.remove(at: rIdx) }
            } else if count > 0 {
                list.append(TelegramReaction(emoji: emoji, count: count, chosenByMe: chosenByMe))
            }
            self.messages[idx].reactions = list

        default:
            break
        }
    }
}
