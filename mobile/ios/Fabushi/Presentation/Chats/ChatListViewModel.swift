//
//  ChatListViewModel.swift
//  Fabushi
//
//  MainActor ViewModel consuming TelegramBridgeRuntime.shared.eventStream,
//  handling optimistic mutations, PTS sequence continuity synchronization,
//  and 64-bit client nonce generation.
//

import Foundation
import Combine
import Security

// MARK: - Chat List Row & Swipe Metrics
public enum ChatListRowMetrics: Sendable {
    public static let rowHeight: CGFloat = 72.0
    public static let standardRowHeight: CGFloat = 76.0
    public static let avatarSize: CGFloat = 54.0
    public static let avatarLeadingPadding: CGFloat = 12.0
    public static let spacing: CGFloat = 12.0
    public static let contentHorizontalSpacing: CGFloat = 10.0
    public static let trailingPadding: CGFloat = 14.0
    public static let pinnedBadgeSize: CGFloat = 16.0
    public static let swipeActionThreshold: CGFloat = 80.0
}

// MARK: - Folder Tabs
public enum ChatFolderTab: String, CaseIterable, Identifiable, Sendable {
    case all = "All"
    case unread = "Unread"
    case personal = "Personal"
    case work = "Work"
    case groups = "Groups"
    case channels = "Channels"

    public var id: String { rawValue }
}

// MARK: - Telegram Chat Summary
public struct TelegramChatSummary: Identifiable, Hashable, Sendable, Codable {
    public let id: Int64
    public var title: String
    public var kind: String // "direct" | "group" | "channel" | "saved" | "secret"
    public var unreadCount: Int
    public var lastMessage: String
    public var timestamp: Int64
    public var isPinned: Bool
    public var isMuted: Bool
    public var currentPts: UInt64
    public var draft: String?

    public init(
        id: Int64,
        title: String,
        kind: String = "direct",
        unreadCount: Int = 0,
        lastMessage: String = "",
        timestamp: Int64 = 0,
        isPinned: Bool = false,
        isMuted: Bool = false,
        currentPts: UInt64 = 0,
        draft: String? = nil
    ) {
        self.id = id
        self.title = title
        self.kind = kind
        self.unreadCount = unreadCount
        self.lastMessage = lastMessage
        self.timestamp = timestamp
        self.isPinned = isPinned
        self.isMuted = isMuted
        self.currentPts = currentPts
        self.draft = draft
    }
}

// MARK: - Nonce Generator for 24-Hour Deduplication
public enum NonceGenerator {
    public static func nextNonce() -> UInt64 {
        var raw: UInt64 = 0
        let status = withUnsafeMutableBytes(of: &raw) { buf in
            SecRandomCopyBytes(kSecRandomDefault, buf.count, buf.baseAddress!)
        }
        if status == errSecSuccess && raw != 0 {
            return raw
        }
        return UInt64.random(in: 1...UInt64.max)
    }
}

// MARK: - Send Message Payload
public struct SendMessagePayload: Encodable, Sendable {
    public let chat_id: Int64
    public let text: String
    public let sender_id: UInt64
    public let reply_to: Int64?
    public let client_nonce: UInt64

    public init(chat_id: Int64, text: String, sender_id: UInt64 = 1, reply_to: Int64? = nil, client_nonce: UInt64 = NonceGenerator.nextNonce()) {
        self.chat_id = chat_id
        self.text = text
        self.sender_id = sender_id
        self.reply_to = reply_to
        self.client_nonce = client_nonce
    }
}

// MARK: - PTS Sync Coordinator Actor
public actor PtsSyncCoordinator {
    public enum SyncState: Equatable, Sendable {
        case idle
        case buffering(currentPts: UInt64)
        case syncing(fromPts: UInt64)
        case reconciling
        case snapshotRequired
        case failed(error: String)
    }

    public struct BufferedUpdate: Sendable, Equatable {
        public let pts: UInt64
        public let ptsCount: UInt32
        public let eventJson: String
        public let timestampMs: Int64

        public init(pts: UInt64, ptsCount: UInt32, eventJson: String, timestampMs: Int64) {
            self.pts = pts
            self.ptsCount = ptsCount
            self.eventJson = eventJson
            self.timestampMs = timestampMs
        }

        public var prevPts: UInt64 {
            pts >= UInt64(ptsCount) ? pts - UInt64(ptsCount) : 0
        }
    }

    public enum EvaluationResult: Sendable, Equatable {
        case applyImmediately
        case duplicateIgnored
        case needsDifferenceSync(missingRange: ClosedRange<UInt64>)
    }

    public enum ReconciliationAction: Sendable, Equatable {
        case performSnapshotReload
        case requestNextDifferencePage(fromPts: UInt64)
        case completedCatchup(replayedUpdates: [BufferedUpdate])
    }

    private var currentPtsByChat: [Int64: UInt64] = [:]
    private var stateByChat: [Int64: SyncState] = [:]
    private var eventBuffers: [Int64: [BufferedUpdate]] = [:]

    public init() {}

    public func setInitialPts(chatId: Int64, pts: UInt64) {
        currentPtsByChat[chatId] = pts
        stateByChat[chatId] = .idle
        eventBuffers[chatId] = []
    }

    public func currentPts(for chatId: Int64) -> UInt64 {
        currentPtsByChat[chatId] ?? 0
    }

    public func currentState(for chatId: Int64) -> SyncState {
        stateByChat[chatId] ?? .idle
    }

    /// Evaluates an incoming update event against the chat's current PTS sequence.
    public func evaluateIncomingUpdate(
        chatId: Int64,
        pts: UInt64,
        ptsCount: UInt32,
        eventJson: String = "",
        timestampMs: Int64 = 0
    ) -> EvaluationResult {
        let current = currentPtsByChat[chatId] ?? 0
        let prevPts = pts >= UInt64(ptsCount) ? pts - UInt64(ptsCount) : 0

        // Ephemeral updates (e.g. typing or read receipts with ptsCount == 0)
        // In MTProto, ephemeral events do not advance persistent PTS and must NEVER trigger difference sync.
        if ptsCount == 0 {
            return .applyImmediately
        }

        // Case 1: Exact contiguous update
        if prevPts == current {
            currentPtsByChat[chatId] = pts
            return .applyImmediately
        }

        // Case 2: Stale or duplicate update
        if pts <= current && ptsCount > 0 {
            return .duplicateIgnored
        }

        // Case 3: Gap detected! prevPts > current
        let missingRange = (current + 1)...prevPts
        let buffered = BufferedUpdate(pts: pts, ptsCount: ptsCount, eventJson: eventJson, timestampMs: timestampMs)
        eventBuffers[chatId, default: []].append(buffered)

        stateByChat[chatId] = .syncing(fromPts: current)
        return .needsDifferenceSync(missingRange: missingRange)
    }

    /// Reconciles differences returned by `telegram.syncDifference` / `sync.getDifference`.
    public func applyDifferenceResponse(
        chatId: Int64,
        newPts: UInt64,
        hasMore: Bool,
        snapshotRequired: Bool
    ) -> ReconciliationAction {
        if snapshotRequired {
            stateByChat[chatId] = .snapshotRequired
            eventBuffers[chatId]?.removeAll()
            return .performSnapshotReload
        }

        currentPtsByChat[chatId] = newPts

        if hasMore {
            stateByChat[chatId] = .syncing(fromPts: newPts)
            return .requestNextDifferencePage(fromPts: newPts)
        }

        // Caught up with journal! Drain and replay buffered events
        stateByChat[chatId] = .reconciling
        var replayList: [BufferedUpdate] = []
        if let buffer = eventBuffers[chatId] {
            // Sort by PTS ascending
            let sorted = buffer.sorted { $0.pts < $1.pts }
            var current = currentPtsByChat[chatId] ?? 0
            for item in sorted {
                if item.prevPts == current {
                    replayList.append(item)
                    current = item.pts
                } else if item.pts <= current {
                    // Stale in buffer, drop
                    continue
                } else {
                    // Still a gap in buffer!
                    eventBuffers[chatId] = Array(sorted.drop(while: { $0.pts <= current }))
                    stateByChat[chatId] = .syncing(fromPts: current)
                    currentPtsByChat[chatId] = current
                    return .requestNextDifferencePage(fromPts: current)
                }
            }
            currentPtsByChat[chatId] = current
        }

        eventBuffers[chatId]?.removeAll()
        stateByChat[chatId] = .idle
        return .completedCatchup(replayedUpdates: replayList)
    }
}

// MARK: - ChatListViewModel
@MainActor
public final class ChatListViewModel: ObservableObject {
    @Published public private(set) var chats: [TelegramChatSummary] = []
    @Published public var selectedFolder: ChatFolderTab = .all
    @Published public var searchText: String = ""
    @Published public private(set) var isLoading: Bool = false
    @Published public private(set) var typingIndicators: [Int64: String] = [:] // chatId -> typing status
    @Published public private(set) var syncStateByChat: [Int64: PtsSyncCoordinator.SyncState] = [:]

    private let runtime: TelegramBridgeRuntime
    public let ptsCoordinator: PtsSyncCoordinator
    private var eventListenTask: Task<Void, Never>?
    private var typingTimers: [Int64: Task<Void, Never>] = [:]

    // MARK: - Initializers
    /// Default initializer isolated to MainActor, safely evaluating .shared inside constructor body.
    @MainActor
    public init() {
        self.runtime = TelegramBridgeRuntime.shared
        self.ptsCoordinator = PtsSyncCoordinator()
    }

    /// Designated initializer for dependency injection and testing.
    public init(runtime: TelegramBridgeRuntime, ptsCoordinator: PtsSyncCoordinator = PtsSyncCoordinator()) {
        self.runtime = runtime
        self.ptsCoordinator = ptsCoordinator
    }

    deinit {
        eventListenTask?.cancel()
        typingTimers.values.forEach { $0.cancel() }
    }

    public var filteredChats: [TelegramChatSummary] {
        let folderFiltered: [TelegramChatSummary]
        switch selectedFolder {
        case .all:
            folderFiltered = chats
        case .unread:
            folderFiltered = chats.filter { $0.unreadCount > 0 }
        case .personal:
            folderFiltered = chats.filter { $0.kind == "direct" }
        case .work:
            folderFiltered = chats.filter { $0.kind == "saved" || $0.kind == "work" }
        case .groups:
            folderFiltered = chats.filter { $0.kind == "group" }
        case .channels:
            folderFiltered = chats.filter { $0.kind == "channel" }
        }

        let search = searchText.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard !search.isEmpty else { return folderFiltered }
        return folderFiltered.filter {
            $0.title.lowercased().contains(search) || $0.lastMessage.lowercased().contains(search)
        }
    }

    // MARK: - Lifecycle & Initial Data Load
    public func start() async {
        guard !isLoading else { return }
        isLoading = true
        defer { isLoading = false }

        startEventStreamListener()
        await loadChats()
    }

    public func loadChats() async {
        do {
            let fetchedChats: [TelegramChatSummary] = try await runtime.dispatch(command: "telegram.getChats")
            // Sort chats: pinned first, then descending timestamp
            self.chats = fetchedChats.sorted { lhs, rhs in
                if lhs.isPinned != rhs.isPinned { return lhs.isPinned }
                return lhs.timestamp > rhs.timestamp
            }
            // Initialize PTS trackers
            for chat in self.chats {
                await ptsCoordinator.setInitialPts(chatId: chat.id, pts: chat.currentPts)
            }
        } catch {
            print("[ChatListViewModel] Failed to load chats: \(error.localizedDescription)")
        }
    }

    // MARK: - Event Stream Listener
    public func startEventStreamListener() {
        eventListenTask?.cancel()
        eventListenTask = Task { [weak self] in
            guard let self else { return }
            for await event in self.runtime.eventStream {
                guard !Task.isCancelled else { break }
                self.handleIncomingEvent(event)
            }
        }
    }

    public func handleIncomingEvent(_ event: TelegramEvent) {
        switch event {
        case .newMessage(let id, let chatId, _, let text, let timestamp, _):
            updateChatWithMessage(chatId: chatId, messageId: id, text: text, timestamp: timestamp)

        case .editMessage(let id, let chatId, let text, let editTimestamp):
            updateChatEditedMessage(chatId: chatId, messageId: id, text: text, timestamp: editTimestamp)

        case .deleteMessages(let chatId, _):
            Task { await self.refreshChatSummary(chatId: chatId) }

        case .userTyping(let chatId, _, let action):
            handleUserTyping(chatId: chatId, action: action)

        case .readHistory(let chatId, _):
            markChatReadLocally(chatId: chatId)

        case .ptsUpdate(let pts, let ptsCount):
            print("[ChatListViewModel] Received global ptsUpdate: pts=\(pts) count=\(ptsCount)")

        case .raw(let type, let json):
            if type == "chatUpdated" {
                handleChatUpdatedRaw(json: json)
            }

        default:
            break
        }
    }

    // MARK: - Optimistic UI State Mutations
    public func updateChatWithMessage(chatId: Int64, messageId: Int64, text: String, timestamp: Int64) {
        guard let index = chats.firstIndex(where: { $0.id == chatId }) else {
            // New chat discovered
            let newChat = TelegramChatSummary(
                id: chatId,
                title: "Chat #\(chatId)",
                unreadCount: 1,
                lastMessage: text,
                timestamp: timestamp
            )
            chats.insert(newChat, at: 0)
            return
        }

        chats[index].lastMessage = text
        chats[index].timestamp = timestamp
        chats[index].unreadCount += 1

        // Move active chat to top of unpinned group or top of pinned group
        let updated = chats.remove(at: index)
        if updated.isPinned {
            chats.insert(updated, at: 0)
        } else {
            let insertIndex = chats.firstIndex(where: { !$0.isPinned }) ?? 0
            chats.insert(updated, at: insertIndex)
        }
    }

    public func updateChatEditedMessage(chatId: Int64, messageId: Int64, text: String, timestamp: Int64) {
        guard let index = chats.firstIndex(where: { $0.id == chatId }) else { return }
        chats[index].lastMessage = text
        chats[index].timestamp = timestamp
    }

    public func handleUserTyping(chatId: Int64, action: String) {
        typingIndicators[chatId] = action == "typing" ? "is typing..." : action
        typingTimers[chatId]?.cancel()
        typingTimers[chatId] = Task { [weak self] in
            try? await Task.sleep(nanoseconds: 5_000_000_000) // 5 seconds auto-clear
            guard !Task.isCancelled else { return }
            self?.typingIndicators.removeValue(forKey: chatId)
        }
    }

    public func markChatReadLocally(chatId: Int64) {
        guard let index = chats.firstIndex(where: { $0.id == chatId }) else { return }
        chats[index].unreadCount = 0
    }

    private func handleChatUpdatedRaw(json: String) {
        guard let data = json.data(using: .utf8),
              let dict = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              let chatId = (dict["chat_id"] as? NSNumber)?.int64Value,
              let index = chats.firstIndex(where: { $0.id == chatId })
        else { return }

        if let title = dict["title"] as? String { chats[index].title = title }
        if let isPinned = dict["is_pinned"] as? Bool { chats[index].isPinned = isPinned }
        if let isMuted = dict["is_muted"] as? Bool { chats[index].isMuted = isMuted }
        if let unreadCount = dict["unread_count"] as? Int { chats[index].unreadCount = unreadCount }
    }

    // MARK: - Swipe Actions
    public func togglePin(chatId: Int64) {
        guard let index = chats.firstIndex(where: { $0.id == chatId }) else { return }
        chats[index].isPinned.toggle()
        let updated = chats.remove(at: index)
        if updated.isPinned {
            chats.insert(updated, at: 0)
        } else {
            let targetIdx = chats.firstIndex(where: { !$0.isPinned }) ?? chats.endIndex
            chats.insert(updated, at: targetIdx)
        }
    }

    public func toggleMute(chatId: Int64) {
        guard let index = chats.firstIndex(where: { $0.id == chatId }) else { return }
        chats[index].isMuted.toggle()
    }

    public func markAsRead(chatId: Int64) {
        markChatReadLocally(chatId: chatId)
    }

    public func deleteChat(chatId: Int64) {
        chats.removeAll(where: { $0.id == chatId })
    }

    public func setChatsDirectly(_ newChats: [TelegramChatSummary]) {
        self.chats = newChats
    }

    private func refreshChatSummary(chatId: Int64) async {
        // Refreshes single chat summary if needed
    }
}

// MARK: - TelegramChatSummary Presentation Extensions
extension TelegramChatSummary {
    public var formattedTime: String {
        guard timestamp > 0 else { return "" }
        let timeInterval = TimeInterval(timestamp > 1_000_000_000_000 ? timestamp / 1000 : timestamp)
        let date = Date(timeIntervalSince1970: timeInterval)
        let formatter = DateFormatter()
        formatter.dateFormat = "HH:mm"
        return formatter.string(from: date)
    }

    public var displayPreview: String {
        if let draft = draft, !draft.isEmpty {
            return "Draft: \(draft)"
        }
        return lastMessage.isEmpty ? "No messages yet" : lastMessage
    }
}

