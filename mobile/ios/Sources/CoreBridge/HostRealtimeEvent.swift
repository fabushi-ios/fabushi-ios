import Foundation
import os

/// Real-time asynchronous push events delivered across `telegram_bridge_register_event_callback`.
public enum HostRealtimeEvent: Sendable, Equatable {
    case newMessage(id: Int64, chatId: Int64, senderId: UInt64, text: String, timestamp: Int64)
    case messageEdited(chatId: Int64, payloadJson: String)
    case messageDeleted(chatId: Int64, messageIds: [Int64])
    case reactionUpdated(chatId: Int64, msgId: Int64, payloadJson: String)
    case syncDifference(pts: UInt64, hasMore: Bool)
    case connectionStateChanged(state: String)
    case authStateChanged(authenticated: Bool, userId: UInt64?)
    case accountSwitched(userId: UInt64, generation: UInt64)
    case vadBargeIn(rms: Float, count: UInt64)
    case cuaEmergencyTakeover(generation: UInt64)
    case taskStateUpdated(taskId: String, state: String)
    case languageChanged(language: String)
    case raw(type: String, payloadJson: String)

    public static func decode(eventType: String, payloadJson: String) -> HostRealtimeEvent {
        guard let data = payloadJson.data(using: .utf8),
              let dict = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        else {
            return .raw(type: eventType, payloadJson: payloadJson)
        }

        switch eventType {
        case "newMessage":
            let id = (dict["id"] as? NSNumber)?.int64Value ?? 0
            let chatId = (dict["chat_id"] as? NSNumber)?.int64Value ?? 0
            let senderId = (dict["sender_id"] as? NSNumber)?.uint64Value ?? 0
            let text = dict["text"] as? String ?? ""
            let ts = (dict["timestamp"] as? NSNumber)?.int64Value
                ?? (dict["timestamp_ms"] as? NSNumber)?.int64Value
                ?? 0
            return .newMessage(id: id, chatId: chatId, senderId: senderId, text: text, timestamp: ts)

        case "messageEdited":
            let chatId = (dict["chat_id"] as? NSNumber)?.int64Value ?? 0
            return .messageEdited(chatId: chatId, payloadJson: payloadJson)

        case "messageDeleted", "deleteMessages":
            let chatId = (dict["chat_id"] as? NSNumber)?.int64Value ?? 0
            let ids = (dict["message_ids"] as? [NSNumber])?.map(\.int64Value) ?? []
            return .messageDeleted(chatId: chatId, messageIds: ids)

        case "reactionUpdated":
            let chatId = (dict["chat_id"] as? NSNumber)?.int64Value ?? 0
            let msgId = (dict["msg_id"] as? NSNumber)?.int64Value
                ?? (dict["message_id"] as? NSNumber)?.int64Value
                ?? 0
            return .reactionUpdated(chatId: chatId, msgId: msgId, payloadJson: payloadJson)

        case "syncDifference":
            let pts = (dict["pts"] as? NSNumber)?.uint64Value ?? 0
            let hasMore = dict["has_more"] as? Bool ?? false
            return .syncDifference(pts: pts, hasMore: hasMore)

        case "connectionStateChanged":
            let state = dict["state"] as? String ?? "unknown"
            return .connectionStateChanged(state: state)

        case "authStateChanged":
            let auth = dict["authenticated"] as? Bool ?? false
            let uid = (dict["user_id"] as? NSNumber)?.uint64Value
            return .authStateChanged(authenticated: auth, userId: uid)

        case "accountSwitched":
            let uid = (dict["user_id"] as? NSNumber)?.uint64Value ?? 0
            let gen = (dict["generation"] as? NSNumber)?.uint64Value ?? 0
            return .accountSwitched(userId: uid, generation: gen)

        case "vadBargeIn":
            let rms = (dict["rms"] as? NSNumber)?.floatValue ?? 0
            let count = (dict["barge_in_count"] as? NSNumber)?.uint64Value ?? 0
            return .vadBargeIn(rms: rms, count: count)

        case "cuaEmergencyTakeover":
            let gen = (dict["generation"] as? NSNumber)?.uint64Value ?? 0
            return .cuaEmergencyTakeover(generation: gen)

        case "taskStateUpdated":
            let taskId = dict["taskId"] as? String ?? ""
            let state = dict["state"] as? String ?? "pending"
            return .taskStateUpdated(taskId: taskId, state: state)

        case "languageChanged":
            let lang = dict["language"] as? String ?? "en-US"
            return .languageChanged(language: lang)

        default:
            return .raw(type: eventType, payloadJson: payloadJson)
        }
    }
}

/// Thread-safe Swift 6 `Sendable` fan-out broadcaster backed by `OSAllocatedUnfairLock`.
public final class HostEventBroadcaster: Sendable {
    private let subscribers = OSAllocatedUnfairLock(
        initialState: [UUID: AsyncStream<HostRealtimeEvent>.Continuation]()
    )

    public init() {}

    public func makeStream() -> AsyncStream<HostRealtimeEvent> {
        let id = UUID()
        return AsyncStream(bufferingPolicy: .unbounded) { continuation in
            subscribers.withLock { map in
                map[id] = continuation
            }
            continuation.onTermination = { [weak self] _ in
                self?.subscribers.withLock { map in
                    _ = map.removeValue(forKey: id)
                }
            }
        }
    }

    public func publish(_ event: HostRealtimeEvent) {
        let targets = subscribers.withLock { Array($0.values) }
        for continuation in targets {
            continuation.yield(event)
        }
    }

    public func publishRaw(eventType: String, payloadJson: String) {
        publish(HostRealtimeEvent.decode(eventType: eventType, payloadJson: payloadJson))
    }

    public func finishAll() {
        let targets = subscribers.withLock { map -> [AsyncStream<HostRealtimeEvent>.Continuation] in
            let all = Array(map.values)
            map.removeAll()
            return all
        }
        for continuation in targets {
            continuation.finish()
        }
    }
}
