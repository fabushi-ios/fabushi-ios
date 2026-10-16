import Foundation
import Observation

internal enum ConversationKind: String, Identifiable, Sendable {
    case direct
    case group
    case channel
    case savedMessages
    case secret

    var id: String { rawValue }
    var label: String {
        switch self {
        case .direct: "私聊"
        case .group: "群组"
        case .channel: "频道"
        case .savedMessages: "收藏"
        case .secret: "加密聊天"
        }
    }
}

internal struct ConversationParticipant: Identifiable, Equatable, Sendable {
    var id: String { actorId }
    let actorId: String
    var role: String
    var joinedAtMs: Int64
}

internal struct ConversationSummary: Identifiable, Equatable, Sendable {
    let id: String
    var title: String
    var description: String
    var ownerId: String?
    var participants: [ConversationParticipant]
    var preview: String
    var time: String
    var badge: String
    var kind: ConversationKind
    var unreadCount: Int
    var isPinned: Bool
    var isMuted: Bool
    var isArchived: Bool
    var lastMessageId: String?
    var pinnedMessageIds: [String]
    var folderIds: [String]
    var markedUnread: Bool
}

internal struct MessagingDraft: Equatable, Sendable {
    let conversationId: String
    let text: String
    let replyToMessageId: String?
    let updatedAtMs: Int64
}

internal struct MessagingFolder: Identifiable, Equatable, Sendable {
    let id: String
    var title: String
    var icon: String?
    var conversationIds: [String]
    var includeContacts: Bool
    var includeBots: Bool
    var includeGroups: Bool
    var includeChannels: Bool
    var excludeMuted: Bool
    var excludeRead: Bool
    var excludeArchived: Bool
}

internal struct MessagingContact: Identifiable, Equatable, Sendable {
    let id: String
    let displayName: String
    let username: String?
    let kind: String
}

internal struct ChatPollOption: Identifiable, Equatable, Sendable {
    let id: String
    let text: String
    let voterCount: Int
    let chosen: Bool
}

internal struct ChatReaction: Equatable, Sendable {
    let reaction: String
    let count: Int
    let chosenByMe: Bool
}

internal struct ChatMessage: Identifiable, Equatable, Sendable {
    let id: String
    let conversationId: String
    let text: String
    let contentType: String
    let mediaFileName: String?
    let mediaBlobId: String?
    let mediaMimeType: String?
    let mediaSizeBytes: Int
    let contactName: String?
    let latitude: Double?
    let longitude: Double?
    let pollQuestion: String?
    let pollOptions: [ChatPollOption]
    let pollMultipleAnswers: Bool
    let isOutgoing: Bool
    let time: String
    let replyToMessageId: String?
    let forwardOrigin: String?
    let reactions: [ChatReaction]
    let deliveryState: String
    let isEdited: Bool
    let isPinned: Bool
}

@MainActor
@Observable
final class MessagingModel {
    private(set) var conversations: [ConversationSummary] = []
    private(set) var contacts: [MessagingContact] = []
    private(set) var folders: [MessagingFolder] = []
    private(set) var draftsByConversation: [String: MessagingDraft] = [:]
    private(set) var messagesByConversation: [String: [ChatMessage]] = [:]
    private(set) var typingActorByConversation: [String: String] = [:]
    private(set) var loading = false
    private(set) var errorMessage: String?

    private let host: MahayanaHost
    private var actorId = ""
    private var displayName = "当前用户"
    private let deviceId = "ios:native"
    private let sessionId = "account-session:ios-native"

    init(host: MahayanaHost) {
        self.host = host
    }

    var currentActorId: String { actorId }

    func refresh() async {
        loading = true
        defer { loading = false }
        do {
            try await ensureIdentity()
            _ = try await execute(command: ["type": "sync", "cursor": NSNull(), "limit": 1000])
            errorMessage = nil
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func createDirect(contact: MessagingContact) async throws -> ConversationSummary? {
        try await ensureIdentity()
        let now = Int64(Date().timeIntervalSince1970 * 1000)
        let id = "direct:\(UUID().uuidString.lowercased())"
        let participants: [[String: Any]] = [
            ["actorId": actorId, "role": "owner", "joinedAtMs": now, "mutedUntilMs": NSNull()],
            ["actorId": contact.id, "role": "member", "joinedAtMs": now, "mutedUntilMs": NSNull()],
        ]
        let conversation = conversationPayload(id: id, kind: .direct, title: contact.displayName, description: "", participants: participants, now: now)
        _ = try await execute(command: ["type": "createConversation", "conversation": conversation])
        return conversations.first(where: { $0.id == id })
    }

    func createConversation(kind: ConversationKind, title: String, description: String = "", participantActorIds: [String] = []) async throws -> ConversationSummary? {
        guard kind == .group || kind == .channel else {
            throw MahayanaHost.HostError.requestFailed("私聊请从联系人列表发起")
        }
        try await ensureIdentity()
        let now = Int64(Date().timeIntervalSince1970 * 1000)
        let id = "\(kind.rawValue):\(UUID().uuidString.lowercased())"
        let uniqueParticipantIds = participantActorIds.filter { !$0.isEmpty && $0 != actorId }.reduce(into: [String]()) { values, id in if !values.contains(id) { values.append(id) } }
        let participants: [[String: Any]] = [["actorId": actorId, "role": "owner", "joinedAtMs": now, "mutedUntilMs": NSNull()]] + uniqueParticipantIds.map { ["actorId": $0, "role": "member", "joinedAtMs": now, "mutedUntilMs": NSNull()] }
        let conversation = conversationPayload(id: id, kind: kind, title: title.trimmingCharacters(in: .whitespacesAndNewlines), description: description, participants: participants, now: now)
        _ = try await execute(command: ["type": "createConversation", "conversation": conversation])
        return conversations.first(where: { $0.id == id })
    }

    func sendText(conversationId: String, text: String, replyToMessageId: String? = nil, silent: Bool = false, scheduledAtMs: Int64? = nil) async throws {
        try await ensureIdentity()
        let value = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !value.isEmpty else { return }
        _ = try await execute(command: [
            "type": "sendMessage",
            "conversationId": conversationId,
            "clientMessageId": "ios:\(UUID().uuidString.lowercased())",
            "content": ["type": "text", "data": ["text": ["text": value, "entities": []]]],
            "replyToMessageId": nullableValue(replyToMessageId),
            "threadRootMessageId": NSNull(),
            "scheduledAtMs": nullableValue(scheduledAtMs),
            "silent": silent,
            "protectedContent": false,
        ])
    }

    func sendContact(conversationId: String, contact: MessagingContact) async throws {
        try await ensureIdentity()
        _ = try await execute(command: [
            "type": "sendMessage", "conversationId": conversationId, "clientMessageId": "ios:\(UUID().uuidString.lowercased())",
            "content": ["type": "contact", "data": ["actorId": contact.id, "displayName": contact.displayName, "phoneNumber": NSNull()]],
            "replyToMessageId": NSNull(), "threadRootMessageId": NSNull(), "scheduledAtMs": NSNull(), "silent": false, "protectedContent": false,
        ])
    }

    func sendLocation(conversationId: String, latitude: Double, longitude: Double, liveUntilMs: Int64? = nil) async throws {
        try await ensureIdentity()
        _ = try await execute(command: [
            "type": "sendMessage", "conversationId": conversationId, "clientMessageId": "ios:\(UUID().uuidString.lowercased())",
            "content": ["type": "location", "data": ["latitude": latitude, "longitude": longitude, "liveUntilMs": nullableValue(liveUntilMs)]],
            "replyToMessageId": NSNull(), "threadRootMessageId": NSNull(), "scheduledAtMs": NSNull(), "silent": false, "protectedContent": false,
        ])
    }

    func votePoll(conversationId: String, messageId: String, optionIds: [String]) async {
        do {
            _ = try await executeAfterIdentity(["type": "votePoll", "conversationId": conversationId, "messageId": messageId, "optionIds": optionIds])
            await refresh()
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func sendPoll(conversationId: String, question: String, options: [String], multipleAnswers: Bool = false) async throws {
        try await ensureIdentity()
        let cleanOptions = options.map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }.filter { !$0.isEmpty }
        guard !question.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty, cleanOptions.count >= 2 else { return }
        let pollOptions = cleanOptions.enumerated().map { index, option in ["id": "option-\(index + 1)", "text": option, "voterCount": 0, "chosen": false, "correct": NSNull()] as [String: Any] }
        _ = try await execute(command: [
            "type": "sendMessage", "conversationId": conversationId, "clientMessageId": "ios:\(UUID().uuidString.lowercased())",
            "content": ["type": "poll", "data": ["question": ["text": question.trimmingCharacters(in: .whitespacesAndNewlines), "entities": []], "options": pollOptions, "anonymous": true, "multipleAnswers": multipleAnswers, "quiz": false]],
            "replyToMessageId": NSNull(), "threadRootMessageId": NSNull(), "scheduledAtMs": NSNull(), "silent": false, "protectedContent": false,
        ])
    }

    func loadBlob(blobId: String, sizeBytes: Int) async throws -> Data {
        guard sizeBytes > 0 else { return Data() }
        var result = Data()
        var offset = 0
        let chunkSize = 1024 * 1024
        while offset < sizeBytes {
            let requested = min(chunkSize, sizeBytes - offset)
            let response = try await host.request(method: "feature.messaging.blob.read", params: ["blobId": blobId, "offset": offset, "length": requested])
            guard let object = response.value as? [String: Any], let encoded = object["dataBase64"] as? String, let chunk = Data(base64Encoded: encoded), !chunk.isEmpty else {
                throw MahayanaHost.HostError.invalidResponse
            }
            result.append(chunk)
            offset += chunk.count
        }
        return result
    }

    func sendVoice(conversationId: String, fileName: String, mimeType: String, data: Data, waveform: [UInt8] = []) async throws {
        guard !data.isEmpty else { throw MahayanaHost.HostError.requestFailed("不能发送空语音") }
        try await ensureIdentity()
        let blobId = "voice-\(UUID().uuidString.lowercased())"
        let createdAt = Int64(Date().timeIntervalSince1970 * 1000)
        _ = try await execute(command: ["type": "beginBlobUpload", "metadata": ["id": blobId, "fileName": fileName, "mimeType": mimeType, "sizeBytes": data.count, "contentHash": NSNull(), "createdAtMs": createdAt]])
        let chunkSize = 512 * 1024
        var offset = 0
        while offset < data.count {
            let end = min(data.count, offset + chunkSize)
            _ = try await execute(command: ["type": "appendBlobChunk", "blobId": blobId, "offset": offset, "dataBase64": data.subdata(in: offset..<end).base64EncodedString()])
            offset = end
        }
        _ = try await execute(command: ["type": "finishBlobUpload", "blobId": blobId])
        let media: [String: Any] = ["id": blobId, "fileName": fileName, "mimeType": mimeType, "sizeBytes": data.count, "remoteUrl": "fabushi-blob://\(blobId)"]
        _ = try await execute(command: [
            "type": "sendMessage", "conversationId": conversationId, "clientMessageId": "ios:\(UUID().uuidString.lowercased())",
            "content": ["type": "voice", "data": ["media": media, "caption": ["text": "", "entities": []], "waveform": waveform]],
            "replyToMessageId": NSNull(), "threadRootMessageId": NSNull(), "scheduledAtMs": NSNull(), "silent": false, "protectedContent": false,
        ])
    }

    func sendAttachment(conversationId: String, fileName: String, mimeType: String, data: Data) async throws {
        guard !data.isEmpty else { throw MahayanaHost.HostError.requestFailed("不能发送空文件") }
        try await ensureIdentity()
        let blobId = "blob-\(UUID().uuidString.lowercased())"
        let createdAt = Int64(Date().timeIntervalSince1970 * 1000)
        _ = try await execute(command: [
            "type": "beginBlobUpload",
            "metadata": ["id": blobId, "fileName": fileName, "mimeType": mimeType, "sizeBytes": data.count, "contentHash": NSNull(), "createdAtMs": createdAt],
        ])
        let chunkSize = 512 * 1024
        var offset = 0
        while offset < data.count {
            let end = min(data.count, offset + chunkSize)
            let chunk = data.subdata(in: offset..<end)
            _ = try await execute(command: ["type": "appendBlobChunk", "blobId": blobId, "offset": offset, "dataBase64": chunk.base64EncodedString()])
            offset = end
        }
        _ = try await execute(command: ["type": "finishBlobUpload", "blobId": blobId])
        let media: [String: Any] = ["id": blobId, "fileName": fileName, "mimeType": mimeType, "sizeBytes": data.count, "remoteUrl": "fabushi-blob://\(blobId)"]
        let caption: [String: Any] = ["text": "", "entities": []]
        let content: [String: Any]
        if mimeType.hasPrefix("image/") {
            content = ["type": "photo", "data": ["media": media, "caption": caption, "spoiler": false]]
        } else if mimeType.hasPrefix("video/") {
            content = ["type": "video", "data": ["media": media, "caption": caption, "spoiler": false, "streaming": true]]
        } else {
            content = ["type": "document", "data": ["media": media, "caption": caption]]
        }
        _ = try await execute(command: [
            "type": "sendMessage", "conversationId": conversationId, "clientMessageId": "ios:\(UUID().uuidString.lowercased())",
            "content": content, "replyToMessageId": NSNull(), "threadRootMessageId": NSNull(), "scheduledAtMs": NSNull(), "silent": false, "protectedContent": false,
        ])
    }

    func setMessagePinned(conversationId: String, messageId: String, pinned: Bool) async {
        try? await executeAfterIdentity(["type": "pinMessage", "conversationId": conversationId, "messageId": messageId, "pinned": pinned])
    }

    func upsertFolder(_ folder: MessagingFolder) async {
        try? await executeAfterIdentity([
            "type": "upsertFolder",
            "folder": [
                "id": folder.id, "title": folder.title, "icon": nullableValue(folder.icon), "conversationIds": folder.conversationIds,
                "includeContacts": folder.includeContacts, "includeBots": folder.includeBots, "includeGroups": folder.includeGroups, "includeChannels": folder.includeChannels,
                "excludeMuted": folder.excludeMuted, "excludeRead": folder.excludeRead, "excludeArchived": folder.excludeArchived,
            ],
        ])
    }

    func deleteFolder(_ folderId: String) async {
        try? await executeAfterIdentity(["type": "deleteFolder", "folderId": folderId])
    }

    func updateConversationInfo(conversationId: String, title: String, description: String) async {
        let cleanDescription = description.trimmingCharacters(in: .whitespacesAndNewlines)
        let nullableDescription: String? = cleanDescription.isEmpty ? nil : cleanDescription
        try? await executeAfterIdentity([
            "type": "updateConversationInfo",
            "conversationId": conversationId,
            "title": title,
            "description": nullableValue(nullableDescription),
        ])
    }

    func setConversationParticipant(conversation: ConversationSummary, actorId targetActorId: String, role: String) async {
        let joinedAt = conversation.participants.first(where: { $0.actorId == targetActorId })?.joinedAtMs ?? Int64(Date().timeIntervalSince1970 * 1000)
        try? await executeAfterIdentity(["type": "setConversationParticipant", "conversationId": conversation.id, "participant": ["actorId": targetActorId, "role": role, "joinedAtMs": joinedAt, "mutedUntilMs": NSNull()]])
    }

    func removeConversationParticipant(conversationId: String, actorId targetActorId: String) async {
        try? await executeAfterIdentity(["type": "removeConversationParticipant", "conversationId": conversationId, "actorId": targetActorId])
    }

    func setMarkedUnread(_ conversationId: String, markedUnread: Bool) async {
        try? await executeAfterIdentity(["type": "setMarkedUnread", "conversationId": conversationId, "markedUnread": markedUnread])
    }

    func setDraft(conversationId: String, text: String, replyToMessageId: String?) async {
        try? await executeAfterIdentity([
            "type": "setDraft", "conversationId": conversationId, "text": text, "replyToMessageId": nullableValue(replyToMessageId),
        ])
    }

    func setPinned(_ conversationId: String, pinned: Bool) async {
        try? await executeAfterIdentity(["type": "pinConversation", "conversationId": conversationId, "pinned": pinned])
    }

    func setArchived(_ conversationId: String, archived: Bool) async {
        try? await executeAfterIdentity(["type": "archiveConversation", "conversationId": conversationId, "archived": archived])
    }

    func setMuted(_ conversationId: String, muted: Bool) async {
        let until: Any
        if muted {
            until = Int64.max / 4
        } else {
            until = NSNull()
        }
        try? await executeAfterIdentity([
            "type": "setConversationNotifications",
            "conversationId": conversationId,
            "settings": ["mutedUntilMs": until, "sound": NSNull(), "showPreview": true, "notifyMentions": true],
        ])
    }

    func markRead(_ conversationId: String) async {
        guard let last = conversations.first(where: { $0.id == conversationId })?.lastMessageId else { return }
        try? await executeAfterIdentity(["type": "markRead", "conversationId": conversationId, "messageId": last])
    }

    func editText(conversationId: String, messageId: String, text: String) async throws {
        let value = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !value.isEmpty else { return }
        try await executeAfterIdentity([
            "type": "editMessage",
            "conversationId": conversationId,
            "messageId": messageId,
            "content": ["type": "text", "data": ["text": ["text": value, "entities": []]]],
        ])
    }

    func deleteMessage(conversationId: String, messageId: String, forEveryone: Bool = true) async {
        try? await executeAfterIdentity(["type": "deleteMessages", "conversationId": conversationId, "messageIds": [messageId], "forEveryone": forEveryone])
    }

    func setReaction(conversationId: String, messageId: String, reaction: String, enabled: Bool) async {
        try? await ensureIdentity()
        try? await execute(command: [
            "type": "setReaction",
            "conversationId": conversationId,
            "messageId": messageId,
            "reaction": ["reaction": reaction, "count": enabled ? 1 : 0, "chosenByMe": enabled, "recentActorIds": enabled ? [actorId] : []],
        ])
    }

    func forwardMessage(sourceConversationId: String, messageId: String, destinationConversationId: String) async {
        try? await executeAfterIdentity([
            "type": "forwardMessage",
            "sourceConversationId": sourceConversationId,
            "messageId": messageId,
            "destinationConversationId": destinationConversationId,
            "clientMessageId": "ios:\(UUID().uuidString.lowercased())",
        ])
    }

    func startTyping(_ conversationId: String) async {
        try? await executeAfterIdentity(["type": "startTyping", "conversationId": conversationId, "action": "typing"])
    }

    func stopTyping(_ conversationId: String) async {
        try? await executeAfterIdentity(["type": "stopTyping", "conversationId": conversationId])
    }

    private func nullableValue<T>(_ value: T?) -> Any {
        guard let value else { return NSNull() }
        return value
    }

    private func executeAfterIdentity(_ command: [String: Any]) async throws {
        try await ensureIdentity()
        _ = try await execute(command: command)
    }

    private func ensureIdentity() async throws {
        guard actorId.isEmpty else { return }
        let auth = try await host.request(method: "feature.auth.status")
        if let object = auth.value as? [String: Any], let user = object["user"] as? [String: Any] {
            displayName = (user["nickname"] as? String) ?? (user["username"] as? String) ?? displayName
        }
        let access = try await host.request(
            method: "feature.messaging.access.issue",
            params: ["deviceId": deviceId, "sessionId": sessionId, "scopes": ["messaging", "calls", "blobsRead", "blobsWrite", "payments", "miniApps"]]
        )
        guard let object = access.value as? [String: Any], let resolvedActor = object["actorId"] as? String, !resolvedActor.isEmpty else {
            throw MahayanaHost.HostError.invalidResponse
        }
        actorId = resolvedActor
        _ = try await execute(command: [
            "type": "upsertProfile",
            "actor": [
                "id": actorId,
                "kind": "human",
                "displayName": displayName,
                "username": NSNull(),
                "avatarUrl": NSNull(),
                "bio": NSNull(),
                "capabilities": ["messages", "groups", "channels", "calls", "payments", "miniApps"],
                "presence": ["status": "online", "lastSeenAtMs": Int64(Date().timeIntervalSince1970 * 1000), "statusText": NSNull()],
                "verified": false,
            ],
        ])
    }

    @discardableResult
    private func execute(command: [String: Any]) async throws -> [[String: Any]] {
        let requestId = "ios-messaging-\(UUID().uuidString.lowercased())"
        let envelope: [String: Any] = [
            "protocolVersion": 2,
            "context": [
                "requestId": requestId,
                "deviceId": deviceId,
                "actorId": actorId,
                "sessionId": sessionId,
                "sentAtMs": Int64(Date().timeIntervalSince1970 * 1000),
            ],
            "command": command,
        ]
        let result = try await host.request(method: "feature.messaging.execute", params: ["requestId": requestId, "envelope": envelope])
        guard let root = result.value as? [String: Any], let envelopes = root["envelopes"] as? [[String: Any]] else {
            throw MahayanaHost.HostError.invalidResponse
        }
        apply(envelopes)
        return envelopes
    }

    private func apply(_ envelopes: [[String: Any]]) {
        for envelope in envelopes {
            guard let event = envelope["event"] as? [String: Any], let type = event["type"] as? String else { continue }
            switch type {
            case "syncBatch":
                let rows = (event["conversations"] as? [[String: Any]] ?? []).compactMap(parseConversation)
                conversations = rows
                contacts = (event["actors"] as? [[String: Any]] ?? []).compactMap(parseContact)
                folders = (event["folders"] as? [[String: Any]] ?? []).compactMap(parseFolder)
                draftsByConversation = Dictionary(uniqueKeysWithValues: (event["drafts"] as? [[String: Any]] ?? []).compactMap(parseDraft).map { ($0.conversationId, $0) })
                let messages = (event["messages"] as? [[String: Any]] ?? []).compactMap(parseMessage)
                messagesByConversation = Dictionary(grouping: messages, by: \.conversationId)
            case "conversationChanged":
                if let raw = event["conversation"] as? [String: Any], let conversation = parseConversation(raw) { upsert(conversation) }
            case "conversationParticipantChanged":
                if let removedActorId = event["removedActorId"] as? String, removedActorId == actorId, let raw = event["conversation"] as? [String: Any], let id = raw["id"] as? String {
                    conversations.removeAll { $0.id == id }; messagesByConversation.removeValue(forKey: id); draftsByConversation.removeValue(forKey: id)
                } else if let raw = event["conversation"] as? [String: Any], let conversation = parseConversation(raw) { upsert(conversation) }
            case "markedUnreadChanged":
                guard let conversationId = event["conversationId"] as? String else { continue }
                if let index = conversations.firstIndex(where: { $0.id == conversationId }) { conversations[index].markedUnread = event["markedUnread"] as? Bool ?? false }
            case "draftChanged":
                if let raw = event["draft"] as? [String: Any], let draft = parseDraft(raw) {
                    if draft.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && draft.replyToMessageId == nil { draftsByConversation.removeValue(forKey: draft.conversationId) }
                    else { draftsByConversation[draft.conversationId] = draft }
                }
            case "folderChanged":
                if let raw = event["folder"] as? [String: Any], let folder = parseFolder(raw) {
                    if let index = folders.firstIndex(where: { $0.id == folder.id }) { folders[index] = folder } else { folders.append(folder) }
                }
            case "folderDeleted":
                if let folderId = event["folderId"] as? String { folders.removeAll { $0.id == folderId } }
            case "messageAdded", "messageChanged":
                if let raw = event["message"] as? [String: Any], let message = parseMessage(raw) {
                    var list = messagesByConversation[message.conversationId] ?? []
                    if let index = list.firstIndex(where: { $0.id == message.id }) { list[index] = message } else { list.append(message) }
                    messagesByConversation[message.conversationId] = list.sorted { $0.time < $1.time }
                }
            case "messagesDeleted":
                guard let id = event["conversationId"] as? String, let ids = event["messageIds"] as? [String] else { continue }
                messagesByConversation[id]?.removeAll { ids.contains($0.id) }
            case "typingChanged":
                guard let conversationId = event["conversationId"] as? String, let typingActorId = event["actorId"] as? String else { continue }
                if typingActorId == actorId || event["action"] is NSNull || event["action"] == nil {
                    typingActorByConversation.removeValue(forKey: conversationId)
                } else {
                    typingActorByConversation[conversationId] = contacts.first(where: { $0.id == typingActorId })?.displayName ?? "对方"
                }
            default:
                break
            }
        }
        hydratePreviews()
    }

    private func upsert(_ conversation: ConversationSummary) {
        if let index = conversations.firstIndex(where: { $0.id == conversation.id }) { conversations[index] = conversation }
        else { conversations.append(conversation) }
    }

    private func hydratePreviews() {
        for index in conversations.indices {
            let conversation = conversations[index]
            if let draft = draftsByConversation[conversation.id] {
                conversations[index].preview = "草稿：\(draft.text)"
            } else if let last = messagesByConversation[conversation.id]?.last {
                conversations[index].preview = last.text
                conversations[index].time = last.time
            }
        }
    }

    private func conversationPayload(id: String, kind: ConversationKind, title: String, description: String, participants: [[String: Any]], now: Int64) -> [String: Any] {
        let cleanDescription = description.trimmingCharacters(in: .whitespacesAndNewlines)
        let nullableDescription: String? = cleanDescription.isEmpty ? nil : cleanDescription
        return [
            "id": id, "kind": kind.rawValue, "title": title,
            "description": nullableValue(nullableDescription),
            "avatarUrl": NSNull(), "participants": participants, "ownerId": actorId,
            "lastMessageId": NSNull(), "lastReadMessageId": NSNull(), "unreadCount": 0, "mentionCount": 0, "pinnedMessageIds": [],
            "notificationSettings": ["mutedUntilMs": NSNull(), "sound": NSNull(), "showPreview": true, "notifyMentions": true],
            "permissions": ["canSendMessages": true, "canSendMedia": true, "canSendPolls": true, "canAddMembers": true, "canPinMessages": true, "canManageTopics": true, "canManageCalls": true],
            "historyVisibility": "allMembers", "topics": [], "folderIds": [], "archived": false, "pinned": false, "markedUnread": false,
            "createdAtMs": now, "updatedAtMs": now,
        ]
    }

    private func parseContact(_ raw: [String: Any]) -> MessagingContact? {
        guard let id = raw["id"] as? String, id != actorId, let name = raw["displayName"] as? String, !name.isEmpty else { return nil }
        return MessagingContact(id: id, displayName: name, username: raw["username"] as? String, kind: raw["kind"] as? String ?? "human")
    }

    private func parseDraft(_ raw: [String: Any]) -> MessagingDraft? {
        guard let conversationId = raw["conversationId"] as? String else { return nil }
        return MessagingDraft(conversationId: conversationId, text: raw["text"] as? String ?? "", replyToMessageId: raw["replyToMessageId"] as? String, updatedAtMs: (raw["updatedAtMs"] as? NSNumber)?.int64Value ?? 0)
    }

    private func parseFolder(_ raw: [String: Any]) -> MessagingFolder? {
        guard let id = raw["id"] as? String, let title = raw["title"] as? String else { return nil }
        return MessagingFolder(
            id: id, title: title, icon: raw["icon"] as? String, conversationIds: raw["conversationIds"] as? [String] ?? [],
            includeContacts: raw["includeContacts"] as? Bool ?? false, includeBots: raw["includeBots"] as? Bool ?? false,
            includeGroups: raw["includeGroups"] as? Bool ?? false, includeChannels: raw["includeChannels"] as? Bool ?? false,
            excludeMuted: raw["excludeMuted"] as? Bool ?? false, excludeRead: raw["excludeRead"] as? Bool ?? false, excludeArchived: raw["excludeArchived"] as? Bool ?? true
        )
    }

    private func parseConversation(_ raw: [String: Any]) -> ConversationSummary? {
        guard let id = raw["id"] as? String, let title = raw["title"] as? String else { return nil }
        let kind = ConversationKind(rawValue: raw["kind"] as? String ?? "direct") ?? .direct
        let participants = (raw["participants"] as? [[String: Any]] ?? []).compactMap { participant -> ConversationParticipant? in
            guard let actorId = participant["actorId"] as? String, let role = participant["role"] as? String else { return nil }
            return ConversationParticipant(actorId: actorId, role: role, joinedAtMs: (participant["joinedAtMs"] as? NSNumber)?.int64Value ?? 0)
        }
        let mutedUntil = (raw["notificationSettings"] as? [String: Any])?["mutedUntilMs"] as? NSNumber
        let updatedAt = (raw["updatedAtMs"] as? NSNumber)?.int64Value ?? 0
        let initial = title.trimmingCharacters(in: .whitespacesAndNewlines).first.map { String($0).uppercased() } ?? "✦"
        return ConversationSummary(
            id: id,
            title: title,
            description: (raw["description"] as? String) ?? "",
            ownerId: raw["ownerId"] as? String,
            participants: participants,
            preview: (raw["description"] as? String) ?? "",
            time: Self.timeLabel(updatedAt),
            badge: initial,
            kind: kind,
            unreadCount: (raw["unreadCount"] as? NSNumber)?.intValue ?? 0,
            isPinned: raw["pinned"] as? Bool ?? false,
            isMuted: (mutedUntil?.int64Value ?? 0) > Int64(Date().timeIntervalSince1970 * 1000),
            isArchived: raw["archived"] as? Bool ?? false,
            lastMessageId: raw["lastMessageId"] as? String,
            pinnedMessageIds: raw["pinnedMessageIds"] as? [String] ?? [],
            folderIds: raw["folderIds"] as? [String] ?? [],
            markedUnread: raw["markedUnread"] as? Bool ?? false
        )
    }

    private func parseMessage(_ raw: [String: Any]) -> ChatMessage? {
        guard let id = raw["id"] as? String,
              let conversationId = raw["conversationId"] as? String,
              let senderId = raw["senderId"] as? String,
              let content = raw["content"] as? [String: Any]
        else { return nil }
        let contentType = content["type"] as? String ?? "unknown"
        let data = content["data"] as? [String: Any] ?? [:]
        let media = data["media"] as? [String: Any]
        let mediaFileName = media?["fileName"] as? String
        let mediaBlobId = media?["id"] as? String
        let mediaMimeType = media?["mimeType"] as? String
        let mediaSizeBytes = (media?["sizeBytes"] as? NSNumber)?.intValue ?? 0
        let contactName = data["displayName"] as? String
        let latitude = (data["latitude"] as? NSNumber)?.doubleValue
        let longitude = (data["longitude"] as? NSNumber)?.doubleValue
        let pollQuestion = (data["question"] as? [String: Any])?["text"] as? String
        let pollOptions = (data["options"] as? [[String: Any]] ?? []).compactMap { option -> ChatPollOption? in
            guard let id = option["id"] as? String, let text = option["text"] as? String else { return nil }
            return ChatPollOption(id: id, text: text, voterCount: (option["voterCount"] as? NSNumber)?.intValue ?? 0, chosen: option["chosen"] as? Bool ?? false)
        }
        let pollMultipleAnswers = data["multipleAnswers"] as? Bool ?? false
        let text: String
        switch contentType {
        case "text": text = ((data["text"] as? [String: Any])?["text"] as? String) ?? ""
        case "photo": text = mediaFileName.map { "🖼 \($0)" } ?? "🖼 图片"
        case "video": text = mediaFileName.map { "🎬 \($0)" } ?? "🎬 视频"
        case "document": text = mediaFileName.map { "📎 \($0)" } ?? "📎 文件"
        case "voice": text = "🎙 语音消息"
        case "audio": text = mediaFileName.map { "🎵 \($0)" } ?? "🎵 音频"
        case "location": text = "📍 位置"
        case "contact": text = contactName.map { "👤 \($0)" } ?? "👤 联系人"
        case "invoice": text = "🧾 账单"
        case "poll": text = pollQuestion.map { "📊 \($0)" } ?? "📊 投票"
        case "miniApp": text = "▣ Mini App"
        default: text = "消息"
        }
        let createdAt = (raw["createdAtMs"] as? NSNumber)?.int64Value ?? 0
        let reactions = (raw["reactions"] as? [[String: Any]] ?? []).compactMap { reaction -> ChatReaction? in
            guard let symbol = reaction["reaction"] as? String else { return nil }
            return ChatReaction(reaction: symbol, count: (reaction["count"] as? NSNumber)?.intValue ?? 0, chosenByMe: reaction["chosenByMe"] as? Bool ?? false)
        }
        let deliveryState: String = {
            if let state = raw["deliveryState"] as? String { return state }
            if let object = raw["deliveryState"] as? [String: Any] {
                if let state = object["state"] as? String { return state }
                return object.keys.first ?? "sent"
            }
            return "sent"
        }()
        return ChatMessage(
            id: id, conversationId: conversationId, text: text, contentType: contentType, mediaFileName: mediaFileName, mediaBlobId: mediaBlobId, mediaMimeType: mediaMimeType, mediaSizeBytes: mediaSizeBytes, contactName: contactName, latitude: latitude, longitude: longitude, pollQuestion: pollQuestion, pollOptions: pollOptions, pollMultipleAnswers: pollMultipleAnswers, isOutgoing: senderId == actorId, time: Self.timeLabel(createdAt),
            replyToMessageId: raw["replyToMessageId"] as? String, forwardOrigin: raw["forwardOrigin"] as? String, reactions: reactions,
            deliveryState: deliveryState, isEdited: raw["editedAtMs"] is NSNumber, isPinned: raw["pinned"] as? Bool ?? false
        )
    }

    private static func timeLabel(_ milliseconds: Int64) -> String {
        guard milliseconds > 0 else { return "" }
        let date = Date(timeIntervalSince1970: Double(milliseconds) / 1000)
        return date.formatted(date: .omitted, time: .shortened)
    }
}
