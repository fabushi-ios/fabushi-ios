import Foundation

// MARK: - Strongly Typed Telegram Events
public enum TelegramEvent: Sendable, Equatable {
    case newMessage(id: Int64, chatId: Int64, senderId: Int64, text: String, timestamp: Int64, replyTo: Int64? = nil)
    case editMessage(id: Int64, chatId: Int64, text: String, editTimestamp: Int64)
    case deleteMessages(chatId: Int64, messageIds: [Int64])
    case ptsUpdate(pts: UInt64, ptsCount: UInt32)
    case syncDifference(pts: UInt64, hasMore: Bool)
    case userTyping(chatId: Int64, userId: Int64, action: String)
    case readHistory(chatId: Int64, maxId: Int64)
    case reactionUpdated(chatId: Int64, messageId: Int64, emoji: String, count: Int, chosenByMe: Bool)
    case userStatusChanged(userId: Int64, isOnline: Bool)
    case connectionStateChanged(state: String)
    case taskStateUpdated(taskId: String, state: String)
    case toolApprovalRequested(approvalId: String, toolName: String, summary: String)
    case raw(type: String, json: String)
}

// MARK: - Event Stream Pipeline
public final class TelegramEventStream: @unchecked Sendable {
    private let lock = NSLock()
    private var continuations: [UUID: AsyncStream<TelegramEvent>.Continuation] = [:]

    public init() {}

    public var stream: AsyncStream<TelegramEvent> {
        makeStream()
    }

    public func makeStream() -> AsyncStream<TelegramEvent> {
        let id = UUID()
        return AsyncStream(bufferingPolicy: .unbounded) { continuation in
            lock.lock()
            continuations[id] = continuation
            lock.unlock()

            continuation.onTermination = { [weak self] _ in
                guard let self else { return }
                self.lock.lock()
                self.continuations.removeValue(forKey: id)
                self.lock.unlock()
            }
        }
    }

    public func yield(_ event: TelegramEvent) {
        lock.lock()
        let targets = Array(continuations.values)
        lock.unlock()
        for continuation in targets {
            continuation.yield(event)
        }
    }

    public func finish() {
        lock.lock()
        let targets = Array(continuations.values)
        continuations.removeAll()
        lock.unlock()
        for continuation in targets {
            continuation.finish()
        }
    }

    public func parseAndYield(type: String, json: String) {
        let event = Self.parseEvent(type: type, json: json)
        yield(event)
    }

    public static func parseEvent(type: String, json: String) -> TelegramEvent {
        guard let data = json.data(using: .utf8),
              let dict = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        else {
            return .raw(type: type, json: json)
        }

        switch type {
        case "newMessage":
            let id = (dict["id"] as? NSNumber)?.int64Value ?? 0
            let chatId = (dict["chat_id"] as? NSNumber)?.int64Value ?? 0
            let senderId = (dict["sender_id"] as? NSNumber)?.int64Value ?? 0
            let text = dict["text"] as? String ?? ""
            let ts = (dict["timestamp"] as? NSNumber)?.int64Value ?? 0
            let replyTo = (dict["reply_to"] as? NSNumber)?.int64Value
            return .newMessage(id: id, chatId: chatId, senderId: senderId, text: text, timestamp: ts, replyTo: replyTo)

        case "editMessage":
            let id = (dict["id"] as? NSNumber)?.int64Value ?? 0
            let chatId = (dict["chat_id"] as? NSNumber)?.int64Value ?? 0
            let text = dict["text"] as? String ?? ""
            let ts = (dict["edit_timestamp"] as? NSNumber)?.int64Value ?? 0
            return .editMessage(id: id, chatId: chatId, text: text, editTimestamp: ts)

        case "deleteMessages":
            let chatId = (dict["chat_id"] as? NSNumber)?.int64Value ?? 0
            let rawIds = dict["message_ids"] as? [NSNumber] ?? []
            let ids = rawIds.map { $0.int64Value }
            return .deleteMessages(chatId: chatId, messageIds: ids)

        case "ptsUpdate":
            let pts = (dict["pts"] as? NSNumber)?.uint64Value ?? 0
            let count = (dict["pts_count"] as? NSNumber)?.uint32Value ?? 0
            return .ptsUpdate(pts: pts, ptsCount: count)

        case "syncDifference":
            let pts = (dict["pts"] as? NSNumber)?.uint64Value ?? 0
            let more = dict["has_more"] as? Bool ?? false
            return .syncDifference(pts: pts, hasMore: more)

        case "userTyping":
            let chatId = (dict["chat_id"] as? NSNumber)?.int64Value ?? 0
            let userId = (dict["user_id"] as? NSNumber)?.int64Value ?? 0
            let action = dict["action"] as? String ?? "typing"
            return .userTyping(chatId: chatId, userId: userId, action: action)

        case "readHistory":
            let chatId = (dict["chat_id"] as? NSNumber)?.int64Value ?? 0
            let maxId = (dict["max_id"] as? NSNumber)?.int64Value ?? 0
            return .readHistory(chatId: chatId, maxId: maxId)

        case "reactionUpdated":
            let chatId = (dict["chat_id"] as? NSNumber)?.int64Value ?? 0
            let msgId = (dict["message_id"] as? NSNumber)?.int64Value ?? 0
            let emoji = dict["emoji"] as? String ?? ""
            let count = dict["count"] as? Int ?? 0
            let mine = dict["chosen_by_me"] as? Bool ?? false
            return .reactionUpdated(chatId: chatId, messageId: msgId, emoji: emoji, count: count, chosenByMe: mine)

        case "userStatusChanged":
            let userId = (dict["user_id"] as? NSNumber)?.int64Value ?? 0
            let isOnline = dict["is_online"] as? Bool ?? false
            return .userStatusChanged(userId: userId, isOnline: isOnline)

        case "connectionStateChanged":
            let state = dict["state"] as? String ?? "unknown"
            return .connectionStateChanged(state: state)

        case "taskStateUpdated":
            let taskId = dict["taskId"] as? String ?? ""
            let state = dict["state"] as? String ?? "pending"
            return .taskStateUpdated(taskId: taskId, state: state)

        case "toolApprovalRequested":
            let approvalId = dict["approvalId"] as? String ?? ""
            let toolName = dict["toolName"] as? String ?? ""
            let summary = dict["summary"] as? String ?? ""
            return .toolApprovalRequested(approvalId: approvalId, toolName: toolName, summary: summary)

        default:
            return .raw(type: type, json: json)
        }
    }
}
