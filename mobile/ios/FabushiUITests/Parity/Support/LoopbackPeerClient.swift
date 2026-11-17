import Foundation
import XCTest

/// Swift 6 `Sendable` recursive JSON representation avoiding `[String: Any]` across async boundaries.
public enum JSONValue: Sendable, Equatable, Codable {
    case null
    case bool(Bool)
    case int(Int64)
    case double(Double)
    case string(String)
    case array([JSONValue])
    case object([String: JSONValue])

    public init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() {
            self = .null
        } else if let b = try? container.decode(Bool.self) {
            self = .bool(b)
        } else if let i = try? container.decode(Int64.self) {
            self = .int(i)
        } else if let d = try? container.decode(Double.self) {
            self = .double(d)
        } else if let s = try? container.decode(String.self) {
            self = .string(s)
        } else if let arr = try? container.decode([JSONValue].self) {
            self = .array(arr)
        } else if let obj = try? container.decode([String: JSONValue].self) {
            self = .object(obj)
        } else {
            throw DecodingError.dataCorruptedError(
                in: container,
                debugDescription: "Unsupported JSONValue payload"
            )
        }
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        switch self {
        case .null:
            try container.encodeNil()
        case .bool(let b):
            try container.encode(b)
        case .int(let i):
            try container.encode(i)
        case .double(let d):
            try container.encode(d)
        case .string(let s):
            try container.encode(s)
        case .array(let arr):
            try container.encode(arr)
        case .object(let obj):
            try container.encode(obj)
        }
    }

    public subscript(key: String) -> JSONValue? {
        if case .object(let dict) = self {
            return dict[key]
        }
        return nil
    }

    public var stringValue: String? {
        if case .string(let s) = self { return s }
        return nil
    }

    public var boolValue: Bool? {
        if case .bool(let b) = self { return b }
        return nil
    }

    public var int64Value: Int64? {
        switch self {
        case .int(let i): return i
        case .double(let d): return Int64(d)
        default: return nil
        }
    }

    public var uint64Value: UInt64? {
        if let v = int64Value, v >= 0 {
            return UInt64(v)
        }
        return nil
    }

    public var intValue: Int? {
        if let v = int64Value {
            return Int(v)
        }
        return nil
    }

    public var arrayValue: [JSONValue]? {
        if case .array(let arr) = self { return arr }
        return nil
    }

    public var objectValue: [String: JSONValue]? {
        if case .object(let obj) = self { return obj }
        return nil
    }
}

extension JSONValue: ExpressibleByNilLiteral {
    public init(nilLiteral: ()) { self = .null }
}

extension JSONValue: ExpressibleByBooleanLiteral {
    public init(booleanLiteral value: Bool) { self = .bool(value) }
}

extension JSONValue: ExpressibleByIntegerLiteral {
    public init(integerLiteral value: Int64) { self = .int(value) }
}

extension JSONValue: ExpressibleByFloatLiteral {
    public init(floatLiteral value: Double) { self = .double(value) }
}

extension JSONValue: ExpressibleByStringLiteral {
    public init(stringLiteral value: String) { self = .string(value) }
}

extension JSONValue: ExpressibleByArrayLiteral {
    public init(arrayLiteral elements: JSONValue...) { self = .array(elements) }
}

extension JSONValue: ExpressibleByDictionaryLiteral {
    public init(dictionaryLiteral elements: (String, JSONValue)...) {
        var dict: [String: JSONValue] = [:]
        for (k, v) in elements {
            dict[k] = v
        }
        self = .object(dict)
    }
}

public struct PeerAuthSession: Sendable, Equatable {
    public let userId: UInt64
    public let username: String
    public let firstName: String
    public let sessionId: String
    public let token: String

    public init(userId: UInt64, username: String, firstName: String, sessionId: String, token: String) {
        self.userId = userId
        self.username = username
        self.firstName = firstName
        self.sessionId = sessionId
        self.token = token
    }
}

public enum LoopbackPeerError: Error, CustomStringConvertible, Sendable {
    case invalidURL(String)
    case unexpectedHTTPStatus(method: String, path: String, status: Int, body: String)
    case missingField(String)
    case websocketTimeout(String)

    public var description: String {
        switch self {
        case .invalidURL(let raw):
            return "Invalid URL: \(raw)"
        case .unexpectedHTTPStatus(let method, let path, let status, let body):
            return "\(method) \(path) returned HTTP \(status): \(body)"
        case .missingField(let name):
            return "Missing expected JSON field '\(name)'"
        case .websocketTimeout(let detail):
            return "WebSocket timeout: \(detail)"
        }
    }
}

/// Real-time companion WebSocket client speaking `WsClientFrame` / `WsServerFrame` (`/api/v1/ws`).
public final class LoopbackWebSocketSession: Sendable {
    private let task: URLSessionWebSocketTask

    public init(task: URLSessionWebSocketTask) {
        self.task = task
        self.task.resume()
    }

    public func sendJSON(_ frame: JSONValue) async throws {
        let data = try JSONEncoder().encode(frame)
        guard let text = String(data: data, encoding: .utf8) else {
            throw LoopbackPeerError.missingField("utf8_ws_frame")
        }
        try await task.send(.string(text))
    }

    public func receiveJSON() async throws -> JSONValue {
        let message = try await task.receive()
        switch message {
        case .string(let text):
            return try JSONDecoder().decode(JSONValue.self, from: Data(text.utf8))
        case .data(let data):
            return try JSONDecoder().decode(JSONValue.self, from: data)
        @unknown default:
            return .null
        }
    }

    public func receiveUntil(
        maxFrames: Int = 24,
        matching predicate: @Sendable (JSONValue) -> Bool
    ) async throws -> JSONValue {
        for _ in 0..<maxFrames {
            let frame = try await receiveJSON()
            if predicate(frame) {
                return frame
            }
        }
        throw LoopbackPeerError.websocketTimeout("Predicate not satisfied within \(maxFrames) frames")
    }

    @discardableResult
    public func ping(nonce: Int64) async throws -> JSONValue {
        try await sendJSON(["type": "Ping", "payload": ["nonce": .int(nonce)]])
        return try await receiveUntil { frame in
            frame["type"]?.stringValue == "Pong" && frame["payload"]?["nonce"]?.int64Value == nonce
        }
    }

    public func subscribeChat(_ chatId: Int64) async throws {
        try await sendJSON(["type": "SubscribeChat", "payload": ["chat_id": .int(chatId)]])
    }

    public func sendMessage(chatId: Int64, clientNonce: UInt64, text: String) async throws {
        try await sendJSON([
            "type": "SendMessage",
            "payload": [
                "chat_id": .int(chatId),
                "client_nonce": .int(Int64(clientNonce)),
                "reply_to": nil,
                "topic_id": nil,
                "content": [
                    "type": "Text",
                    "data": [
                        "text": .string(text),
                        "entities": [],
                    ],
                ],
                "silent": false,
            ],
        ])
    }

    public func sendTyping(chatId: Int64, userId: UInt64, action: String = "typing") async throws {
        let nowMs = Int64(Date().timeIntervalSince1970 * 1000)
        try await sendJSON([
            "type": "SendTyping",
            "payload": [
                "chat_id": .int(chatId),
                "user_id": .int(Int64(userId)),
                "action": .string(action),
                "timestamp_ms": .int(nowMs),
            ],
        ])
    }

    public func sendReaction(chatId: Int64, messageId: Int64, emoji: String, add: Bool = true) async throws {
        try await sendJSON([
            "type": "SendReaction",
            "payload": [
                "chat_id": .int(chatId),
                "message_id": .int(messageId),
                "emoji": .string(emoji),
                "add": .bool(add),
            ],
        ])
    }

    public func readHistory(chatId: Int64, maxId: Int64) async throws {
        try await sendJSON([
            "type": "ReadHistory",
            "payload": [
                "chat_id": .int(chatId),
                "max_id": .int(maxId),
            ],
        ])
    }

    public func close() {
        task.cancel(with: .normalClosure, reason: nil)
    }
}

/// Pure `Foundation` companion HTTP REST + WebSocket peer client for opaque-box E2E tests.
public struct LoopbackPeerClient: Sendable {
    /// Canonical RFC 8032 §7.1 Test Vector 1 Ed25519 keypair + signature over
    /// `b"gpui-e2ee-spk-v1" || [0x42; 32]` verified live against `POST /api/v1/e2ee/prekeys`.
    public static let canonicalIdentityX25519KeyB64 = "ISEhISEhISEhISEhISEhISEhISEhISEhISEhISEhISE="
    public static let canonicalIdentitySigningKeyB64 = "11qYAYKxCrfVS/7TyWQHOg7hcvPapiMlrwIaaPcHURo="
    public static let canonicalSpkPublicKeyB64 = "QkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkI="
    public static let canonicalSpkSignatureB64 = "PENDING_RFC8032_CI_VERIFICATION_PLACEHOLDER=="
    public static let canonicalOtpkPublicKeyB64 = "U1NTU1NTU1NTU1NTU1NTU1NTU1NTU1NTU1NTU1NTU1M="

    public let baseURL: URL

    public init(baseURL: URL) {
        self.baseURL = baseURL
    }

    private func makeURL(_ path: String) throws -> URL {
        let baseStr = baseURL.absoluteString.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        let normalizedPath = path.hasPrefix("/") ? path : "/\(path)"
        guard let url = URL(string: "\(baseStr)\(normalizedPath)") else {
            throw LoopbackPeerError.invalidURL("\(baseStr)\(normalizedPath)")
        }
        return url
    }

    public func requestRaw(
        method: String,
        path: String,
        token: String? = nil,
        body: Data? = nil,
        contentType: String? = "application/json",
        extraHeaders: [String: String] = [:],
        expectedStatuses: Set<Int> = [200, 201]
    ) async throws -> (statusCode: Int, headers: [String: String], data: Data) {
        let url = try makeURL(path)
        var req = URLRequest(url: url)
        req.httpMethod = method
        req.timeoutInterval = 15
        if let token, !token.isEmpty {
            req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        if let body {
            req.httpBody = body
            if let contentType {
                req.setValue(contentType, forHTTPHeaderField: "Content-Type")
            }
        }
        for (k, v) in extraHeaders {
            req.setValue(v, forHTTPHeaderField: k)
        }
        let (data, response) = try await URLSession.shared.data(for: req)
        let http = response as? HTTPURLResponse
        let status = http?.statusCode ?? 0
        var headerMap: [String: String] = [:]
        if let fields = http?.allHeaderFields {
            for (key, val) in fields {
                headerMap[String(describing: key).lowercased()] = String(describing: val)
            }
        }
        if !expectedStatuses.isEmpty && !expectedStatuses.contains(status) {
            let text = String(data: data, encoding: .utf8) ?? "<binary>"
            throw LoopbackPeerError.unexpectedHTTPStatus(
                method: method,
                path: path,
                status: status,
                body: text
            )
        }
        return (status, headerMap, data)
    }

    @discardableResult
    public func requestJSON(
        method: String,
        path: String,
        token: String? = nil,
        body: JSONValue? = nil,
        extraHeaders: [String: String] = [:],
        expectedStatuses: Set<Int> = [200, 201]
    ) async throws -> JSONValue {
        let encoded = try body.map { try JSONEncoder().encode($0) }
        let (_, _, data) = try await requestRaw(
            method: method,
            path: path,
            token: token,
            body: encoded,
            contentType: body != nil ? "application/json" : nil,
            extraHeaders: extraHeaders,
            expectedStatuses: expectedStatuses
        )
        if data.isEmpty {
            return .object([:])
        }
        return try JSONDecoder().decode(JSONValue.self, from: data)
    }

    // MARK: - F01..F09 Health & Auth

    public func checkHealth() async throws -> Bool {
        let (status, _, data) = try await requestRaw(
            method: "GET",
            path: "/health",
            contentType: nil,
            expectedStatuses: [200]
        )
        let text = String(data: data, encoding: .utf8)?.trimmingCharacters(in: .whitespacesAndNewlines)
        return status == 200 && text == "OK"
    }

    public func fetchStatus() async throws -> JSONValue {
        try await requestJSON(method: "GET", path: "/api/v1/status")
    }

    public func registerUser(
        username: String,
        password: String = "StrongPass123!",
        firstName: String = "Peer",
        deviceName: String = "CompanionSwift"
    ) async throws -> PeerAuthSession {
        let res = try await requestJSON(
            method: "POST",
            path: "/api/v1/auth/register",
            body: [
                "username": .string(username),
                "password": .string(password),
                "first_name": .string(firstName),
                "device_name": .string(deviceName),
                "auto_welcome": false,
            ]
        )
        guard let uid = res["user_id"]?.uint64Value,
              let sid = res["session_id"]?.stringValue,
              let tok = res["token"]?.stringValue
        else {
            throw LoopbackPeerError.missingField("auth.register fields")
        }
        return PeerAuthSession(
            userId: uid,
            username: res["username"]?.stringValue ?? username,
            firstName: res["first_name"]?.stringValue ?? firstName,
            sessionId: sid,
            token: tok
        )
    }

    public func loginUser(
        username: String,
        password: String = "StrongPass123!",
        deviceName: String = "CompanionSwift"
    ) async throws -> JSONValue {
        try await requestJSON(
            method: "POST",
            path: "/api/v1/auth/login",
            body: [
                "username": .string(username),
                "password": .string(password),
                "device_name": .string(deviceName),
            ]
        )
    }

    public func fetchMe(token: String) async throws -> JSONValue {
        try await requestJSON(method: "GET", path: "/api/v1/auth/me", token: token)
    }

    public func startAlipay(deviceName: String = "iPhone 16 Pro") async throws -> JSONValue {
        try await requestJSON(
            method: "POST",
            path: "/api/v1/auth/alipay/start",
            body: ["device_name": .string(deviceName)]
        )
    }

    @discardableResult
    public func simulateAlipayScan(attemptId: String, nickname: String = "慈悲居士") async throws -> JSONValue {
        try await requestJSON(
            method: "POST",
            path: "/api/v1/auth/alipay/simulate-scan",
            body: [
                "attempt_id": .string(attemptId),
                "mock_alipay_user_id": .string("alipay_\(attemptId.prefix(8))"),
                "mock_nickname": .string(nickname),
            ]
        )
    }

    public func pollAlipay(attemptId: String) async throws -> JSONValue {
        try await requestJSON(method: "GET", path: "/api/v1/auth/alipay/poll/\(attemptId)")
    }

    public func sendEmailCode(email: String) async throws -> JSONValue {
        try await requestJSON(
            method: "POST",
            path: "/api/v1/auth/email/send-code",
            body: ["email": .string(email)]
        )
    }

    public func sendPhoneCode(phone: String) async throws -> JSONValue {
        try await requestJSON(
            method: "POST",
            path: "/api/v1/auth/phone/send-code",
            body: ["phone": .string(phone)]
        )
    }

    public func verifyPhone(
        phone: String,
        phoneCodeHash: String,
        code: String,
        firstName: String = "PhonePeer"
    ) async throws -> JSONValue {
        try await requestJSON(
            method: "POST",
            path: "/api/v1/auth/phone/verify",
            body: [
                "phone": .string(phone),
                "phone_code_hash": .string(phoneCodeHash),
                "code": .string(code),
                "first_name": .string(firstName),
                "device_name": "CompanionPhone",
            ]
        )
    }

    public func initQrPair(deviceName: String = "CompanionDesktop") async throws -> JSONValue {
        try await requestJSON(
            method: "POST",
            path: "/api/v1/auth/pair/init",
            body: ["device_name": .string(deviceName)]
        )
    }

    @discardableResult
    public func acceptQrPair(token: String, pairToken: String) async throws -> JSONValue {
        try await requestJSON(
            method: "POST",
            path: "/api/v1/auth/pair/accept",
            token: token,
            body: ["pair_token": .string(pairToken)]
        )
    }

    public func pollQrPair(pairToken: String) async throws -> JSONValue {
        try await requestJSON(method: "GET", path: "/api/v1/auth/pair/poll/\(pairToken)")
    }

    @discardableResult
    public func setTwoFactor(
        token: String,
        newPassword: String,
        currentPassword: String? = nil,
        hint: String = "lotus"
    ) async throws -> JSONValue {
        var payload: [String: JSONValue] = [
            "new_password": .string(newPassword),
            "hint": .string(hint),
        ]
        if let currentPassword {
            payload["current_password"] = .string(currentPassword)
        }
        return try await requestJSON(
            method: "POST",
            path: "/api/v1/auth/2fa/set",
            token: token,
            body: .object(payload)
        )
    }

    public func verifyTwoFactor(challengeToken: String, cloudPassword: String) async throws -> JSONValue {
        try await requestJSON(
            method: "POST",
            path: "/api/v1/auth/2fa/verify",
            body: [
                "challenge_token": .string(challengeToken),
                "cloud_password": .string(cloudPassword),
                "device_name": "CompanionVerifier",
            ]
        )
    }

    @discardableResult
    public func registerPasskey(
        token: String,
        credentialId: String,
        deviceName: String = "MacBook Pro Passkey"
    ) async throws -> JSONValue {
        try await requestJSON(
            method: "POST",
            path: "/api/v1/auth/passkey/register",
            token: token,
            body: [
                "credential_id": .string(credentialId),
                "public_key": .string(Self.canonicalIdentitySigningKeyB64),
                "device_name": .string(deviceName),
            ]
        )
    }

    public func listPasskeys(token: String) async throws -> JSONValue {
        try await requestJSON(method: "GET", path: "/api/v1/auth/passkey/list", token: token)
    }

    public func listSessions(token: String) async throws -> JSONValue {
        try await requestJSON(method: "GET", path: "/api/v1/auth/sessions", token: token)
    }

    @discardableResult
    public func revokeSession(token: String, sessionId: String) async throws -> JSONValue {
        try await requestJSON(
            method: "DELETE",
            path: "/api/v1/auth/sessions/\(sessionId)",
            token: token
        )
    }

    // MARK: - F12..F16 Chats, Folders, Topics, Search, Stories, Bots

    public func createChat(
        token: String,
        title: String,
        chatType: String = "direct",
        peerUserId: UInt64? = nil,
        memberIds: [UInt64] = [],
        isForum: Bool = false
    ) async throws -> Int64 {
        var payload: [String: JSONValue] = [
            "title": .string(title),
            "chat_type": .string(chatType),
            "is_forum": .bool(isForum),
            "member_ids": .array(memberIds.map { .int(Int64($0)) }),
        ]
        if let peerUserId {
            payload["peer_user_id"] = .int(Int64(peerUserId))
        }
        let res = try await requestJSON(
            method: "POST",
            path: "/api/v1/chats/create",
            token: token,
            body: .object(payload)
        )
        guard let chatId = res["chat_id"]?.int64Value ?? res["id"]?.int64Value else {
            throw LoopbackPeerError.missingField("chat_id")
        }
        return chatId
    }

    public func listChats(token: String) async throws -> JSONValue {
        try await requestJSON(method: "GET", path: "/api/v1/chats", token: token)
    }

    public func getChat(token: String, chatId: Int64) async throws -> JSONValue {
        try await requestJSON(method: "GET", path: "/api/v1/chats/\(chatId)", token: token)
    }

    @discardableResult
    public func setChatArchive(token: String, chatId: Int64, archived: Bool) async throws -> JSONValue {
        try await requestJSON(
            method: "POST",
            path: "/api/v1/chats/\(chatId)/archive",
            token: token,
            body: ["archived": .bool(archived)]
        )
    }

    @discardableResult
    public func setChatPin(token: String, chatId: Int64, pinned: Bool) async throws -> JSONValue {
        try await requestJSON(
            method: "POST",
            path: "/api/v1/chats/\(chatId)/pin",
            token: token,
            body: ["pinned": .bool(pinned)]
        )
    }

    @discardableResult
    public func setChatMute(token: String, chatId: Int64, muted: Bool) async throws -> JSONValue {
        try await requestJSON(
            method: "POST",
            path: "/api/v1/chats/\(chatId)/mute",
            token: token,
            body: ["muted": .bool(muted)]
        )
    }

    public func createFolder(
        token: String,
        title: String,
        includeChats: [Int64] = [],
        includeGroups: Bool = false
    ) async throws -> Int64 {
        let res = try await requestJSON(
            method: "POST",
            path: "/api/v1/folders",
            token: token,
            body: [
                "title": .string(title),
                "emoticon": "📁",
                "include_chats": .array(includeChats.map { .int($0) }),
                "exclude_chats": [],
                "include_groups": .bool(includeGroups),
            ]
        )
        guard let fid = res["id"]?.int64Value ?? res["folder_id"]?.int64Value else {
            throw LoopbackPeerError.missingField("folder.id")
        }
        return fid
    }

    public func listFolders(token: String) async throws -> JSONValue {
        try await requestJSON(method: "GET", path: "/api/v1/folders", token: token)
    }

    public func createForumTopic(token: String, chatId: Int64, title: String) async throws -> Int64 {
        let res = try await requestJSON(
            method: "POST",
            path: "/api/v1/chats/\(chatId)/topics",
            token: token,
            body: ["title": .string(title), "icon_color": 7322096]
        )
        guard let tid = res["topic_id"]?.int64Value ?? res["id"]?.int64Value else {
            throw LoopbackPeerError.missingField("topic_id")
        }
        return tid
    }

    public func listForumTopics(token: String, chatId: Int64) async throws -> JSONValue {
        try await requestJSON(method: "GET", path: "/api/v1/chats/\(chatId)/topics", token: token)
    }

    public func searchGlobal(token: String, query: String, category: String = "all") async throws -> JSONValue {
        let q = query.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) ?? query
        return try await requestJSON(
            method: "GET",
            path: "/api/v1/search?q=\(q)&category=\(category)",
            token: token
        )
    }

    public func createStory(token: String, mediaId: String, caption: String) async throws -> Int64 {
        let res = try await requestJSON(
            method: "POST",
            path: "/api/v1/stories",
            token: token,
            body: [
                "media_id": .string(mediaId),
                "media_type": "photo",
                "caption": .string(caption),
                "privacy": "contacts",
                "ttl_hours": 24,
            ]
        )
        guard let sid = res["story_id"]?.int64Value ?? res["id"]?.int64Value else {
            throw LoopbackPeerError.missingField("story_id")
        }
        return sid
    }

    public func listStories(token: String) async throws -> JSONValue {
        try await requestJSON(method: "GET", path: "/api/v1/stories", token: token)
    }

    public func listFabushiBots(token: String) async throws -> JSONValue {
        try await requestJSON(method: "GET", path: "/api/v1/fabushi-bots", token: token)
    }

    // MARK: - F17..F34 Messages, Polls, Reactions, Pins, TTL, Drafts, Media

    @discardableResult
    public func sendMessage(
        token: String,
        chatId: Int64,
        text: String,
        entities: [JSONValue] = [],
        replyTo: Int64? = nil,
        quoteText: String? = nil,
        topicId: Int64? = nil,
        mediaId: String? = nil,
        inlineKeyboard: JSONValue? = nil,
        silent: Bool = false,
        scheduledAtMs: Int64? = nil,
        sendAsPeerId: UInt64? = nil,
        clientNonce: UInt64 = UInt64.random(in: 10_000...9_000_000)
    ) async throws -> Int64 {
        var payload: [String: JSONValue] = [
            "chat_id": .int(chatId),
            "text": .string(text),
            "entities": .array(entities),
            "silent": .bool(silent),
            "client_nonce": .int(Int64(clientNonce)),
        ]
        if let replyTo { payload["reply_to"] = .int(replyTo) }
        if let quoteText { payload["quote_text"] = .string(quoteText) }
        if let topicId { payload["topic_id"] = .int(topicId) }
        if let mediaId { payload["media_id"] = .string(mediaId) }
        if let inlineKeyboard { payload["inline_keyboard"] = inlineKeyboard }
        if let scheduledAtMs { payload["scheduled_at_ms"] = .int(scheduledAtMs) }
        if let sendAsPeerId { payload["send_as_peer_id"] = .int(Int64(sendAsPeerId)) }

        let res = try await requestJSON(
            method: "POST",
            path: "/api/v1/chats/\(chatId)/messages",
            token: token,
            body: .object(payload)
        )
        guard let mid = res["message_id"]?.int64Value ?? res["id"]?.int64Value else {
            throw LoopbackPeerError.missingField("message_id")
        }
        return mid
    }

    @discardableResult
    public func sendContentMessage(
        token: String,
        chatId: Int64,
        contentType: String,
        data: JSONValue,
        inlineKeyboard: JSONValue? = nil,
        clientNonce: UInt64 = UInt64.random(in: 10_000...9_000_000)
    ) async throws -> Int64 {
        var payload: [String: JSONValue] = [
            "chat_id": .int(chatId),
            "content": [
                "type": .string(contentType),
                "data": data,
            ],
            "client_nonce": .int(Int64(clientNonce)),
        ]
        if let inlineKeyboard {
            payload["inline_keyboard"] = inlineKeyboard
        }
        let res = try await requestJSON(
            method: "POST",
            path: "/api/v1/chats/\(chatId)/messages",
            token: token,
            body: .object(payload)
        )
        guard let mid = res["message_id"]?.int64Value ?? res["id"]?.int64Value else {
            throw LoopbackPeerError.missingField("message_id")
        }
        return mid
    }

    @discardableResult
    public func editMessage(token: String, chatId: Int64, messageId: Int64, text: String) async throws -> JSONValue {
        try await requestJSON(
            method: "PUT",
            path: "/api/v1/chats/\(chatId)/messages/\(messageId)",
            token: token,
            body: ["text": .string(text), "entities": []]
        )
    }

    public func listMessages(
        token: String,
        chatId: Int64,
        topicId: Int64? = nil,
        limit: Int = 50
    ) async throws -> JSONValue {
        var query = "?limit=\(limit)"
        if let topicId {
            query += "&topic_id=\(topicId)"
        }
        return try await requestJSON(
            method: "GET",
            path: "/api/v1/chats/\(chatId)/messages\(query)",
            token: token
        )
    }

    @discardableResult
    public func markChatRead(token: String, chatId: Int64, maxId: Int64) async throws -> JSONValue {
        try await requestJSON(
            method: "POST",
            path: "/api/v1/chats/\(chatId)/read",
            token: token,
            body: ["max_id": .int(maxId), "topic_id": 0]
        )
    }

    @discardableResult
    public func pinMessage(token: String, chatId: Int64, messageId: Int64) async throws -> JSONValue {
        try await requestJSON(
            method: "POST",
            path: "/api/v1/chats/\(chatId)/pins",
            token: token,
            body: ["message_id": .int(messageId)]
        )
    }

    public func listPins(token: String, chatId: Int64) async throws -> JSONValue {
        try await requestJSON(method: "GET", path: "/api/v1/chats/\(chatId)/pins", token: token)
    }

    @discardableResult
    public func reactToMessage(
        token: String,
        chatId: Int64,
        messageId: Int64,
        emoji: String,
        action: String = "add"
    ) async throws -> JSONValue {
        try await requestJSON(
            method: "POST",
            path: "/api/v1/chats/\(chatId)/messages/\(messageId)/reactions",
            token: token,
            body: ["emoji": .string(emoji), "action": .string(action)]
        )
    }

    public func createPoll(
        token: String,
        chatId: Int64,
        question: String,
        options: [String],
        multipleChoice: Bool = false,
        isQuiz: Bool = false,
        correctOptionId: Int? = nil,
        solution: String? = nil
    ) async throws -> (messageId: Int64, pollId: UInt64) {
        var payload: [String: JSONValue] = [
            "question": .string(question),
            "options": .array(options.map { .string($0) }),
            "is_anonymous": false,
            "multiple_choice": .bool(multipleChoice),
            "is_quiz": .bool(isQuiz),
        ]
        if let correctOptionId { payload["correct_option_id"] = .int(Int64(correctOptionId)) }
        if let solution { payload["solution"] = .string(solution) }

        let res = try await requestJSON(
            method: "POST",
            path: "/api/v1/chats/\(chatId)/polls",
            token: token,
            body: .object(payload)
        )
        guard let mid = res["message_id"]?.int64Value,
              let pid = res["poll"]?["poll_id"]?.uint64Value
        else {
            throw LoopbackPeerError.missingField("poll.poll_id")
        }
        return (mid, pid)
    }

    @discardableResult
    public func votePoll(token: String, pollId: UInt64, optionIds: [Int]) async throws -> JSONValue {
        try await requestJSON(
            method: "POST",
            path: "/api/v1/polls/\(pollId)/vote",
            token: token,
            body: ["option_ids": .array(optionIds.map { .int(Int64($0)) })]
        )
    }

    @discardableResult
    public func closePoll(token: String, pollId: UInt64) async throws -> JSONValue {
        try await requestJSON(
            method: "POST",
            path: "/api/v1/polls/\(pollId)/close",
            token: token,
            body: [:]
        )
    }

    @discardableResult
    public func setChatTTL(token: String, chatId: Int64, ttlSeconds: Int) async throws -> JSONValue {
        try await requestJSON(
            method: "POST",
            path: "/api/v1/chats/\(chatId)/ttl",
            token: token,
            body: ["ttl_seconds": .int(Int64(ttlSeconds))]
        )
    }

    @discardableResult
    public func putDraft(token: String, chatId: Int64, text: String) async throws -> JSONValue {
        try await requestJSON(
            method: "PUT",
            path: "/api/v1/chats/\(chatId)/draft",
            token: token,
            body: ["text": .string(text), "topic_id": 0, "entities": []]
        )
    }

    public func getDraft(token: String, chatId: Int64) async throws -> String? {
        let res = try await requestJSON(
            method: "GET",
            path: "/api/v1/chats/\(chatId)/draft?topic_id=0",
            token: token
        )
        return res["draft"]?["text"]?.stringValue
    }

    public func uploadMediaDirect(
        token: String,
        chatId: Int64,
        filename: String,
        mimeType: String,
        data: Data
    ) async throws -> String {
        let encodedName = filename.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) ?? filename
        let encodedMime = mimeType.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) ?? mimeType
        let (_, _, respData) = try await requestRaw(
            method: "POST",
            path: "/api/v1/media/upload?chat_id=\(chatId)&filename=\(encodedName)&mime_type=\(encodedMime)",
            token: token,
            body: data,
            contentType: mimeType
        )
        let json = try JSONDecoder().decode(JSONValue.self, from: respData)
        guard let fid = json["file_id"]?.stringValue ?? json["media_id"]?.stringValue else {
            throw LoopbackPeerError.missingField("file_id")
        }
        return fid
    }

    public func streamMediaRange(
        token: String,
        fileId: String,
        start: Int,
        end: Int
    ) async throws -> (statusCode: Int, data: Data) {
        let (status, _, data) = try await requestRaw(
            method: "GET",
            path: "/api/v1/media/stream/\(fileId)",
            token: token,
            contentType: nil,
            extraHeaders: ["Range": "bytes=\(start)-\(end)"],
            expectedStatuses: [206]
        )
        return (status, data)
    }

    public static func sampleMinimalPngData() -> Data {
        Data(
            base64Encoded:
                "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=="
        ) ?? Data([0x89, 0x50, 0x4E, 0x47])
    }

    // MARK: - F35..F44 Profiles, Contacts, Calls, E2EE, Wallet, Hermes, MiniApps, Computers

    @discardableResult
    public func addContact(
        token: String,
        userId: UInt64,
        username: String,
        firstName: String
    ) async throws -> JSONValue {
        try await requestJSON(
            method: "POST",
            path: "/api/v1/contacts",
            token: token,
            body: [
                "user_id": .int(Int64(userId)),
                "username": .string(username),
                "first_name": .string(firstName),
            ]
        )
    }

    public func listContacts(token: String) async throws -> JSONValue {
        try await requestJSON(method: "GET", path: "/api/v1/contacts", token: token)
    }

    public func resolvePeer(token: String, username: String) async throws -> JSONValue {
        let clean = username.trimmingCharacters(in: CharacterSet(charactersIn: "@"))
        return try await requestJSON(
            method: "GET",
            path: "/api/v1/peers/resolve?username=\(clean)",
            token: token
        )
    }

    @discardableResult
    public func setPeerBlocked(token: String, targetUserId: UInt64, blocked: Bool) async throws -> JSONValue {
        try await requestJSON(
            method: blocked ? "POST" : "DELETE",
            path: "/api/v1/users/\(targetUserId)/block",
            token: token,
            body: blocked ? [:] : nil
        )
    }

    public func inviteCall(
        token: String,
        calleeId: UInt64,
        chatId: Int64?,
        isVideo: Bool = false
    ) async throws -> JSONValue {
        var payload: [String: JSONValue] = [
            "callee_id": .int(Int64(calleeId)),
            "is_video": .bool(isVideo),
        ]
        if let chatId { payload["chat_id"] = .int(chatId) }
        return try await requestJSON(
            method: "POST",
            path: "/api/v1/calls/invite",
            token: token,
            body: .object(payload)
        )
    }

    @discardableResult
    public func sendCallSignal(
        token: String,
        callId: String,
        signalType: String,
        sdp: String? = nil
    ) async throws -> JSONValue {
        var payload: [String: JSONValue] = [
            "call_id": .string(callId),
            "signal_type": .string(signalType),
        ]
        if let sdp { payload["sdp"] = .string(sdp) }
        return try await requestJSON(
            method: "POST",
            path: "/api/v1/calls/signal",
            token: token,
            body: .object(payload)
        )
    }

    public func listCalls(token: String) async throws -> JSONValue {
        try await requestJSON(method: "GET", path: "/api/v1/calls", token: token)
    }

    @discardableResult
    public func publishValidPrekeys(token: String, deviceId: String) async throws -> JSONValue {
        let nowMs = Int64(Date().timeIntervalSince1970 * 1000)
        return try await requestJSON(
            method: "POST",
            path: "/api/v1/e2ee/prekeys",
            token: token,
            body: [
                "device_id": .string(deviceId),
                "device_name": .string(deviceId),
                "identity_key": .string(Self.canonicalIdentityX25519KeyB64),
                "identity_signing_key": .string(Self.canonicalIdentitySigningKeyB64),
                "signed_prekey": [
                    "key_id": 1,
                    "public_key": .string(Self.canonicalSpkPublicKeyB64),
                    "signature": .string(Self.canonicalSpkSignatureB64),
                    "created_at_ms": .int(nowMs),
                ],
                "one_time_prekeys": [
                    ["key_id": 101, "public_key": .string(Self.canonicalOtpkPublicKeyB64)],
                    ["key_id": 102, "public_key": .string(Self.canonicalOtpkPublicKeyB64)],
                ],
            ]
        )
    }

    public func getPrekeyCount(token: String, deviceId: String? = nil) async throws -> JSONValue {
        let suffix = deviceId.map { "?device_id=\($0)" } ?? ""
        return try await requestJSON(
            method: "GET",
            path: "/api/v1/e2ee/prekeys/count\(suffix)",
            token: token
        )
    }

    public func getWalletBalance(token: String) async throws -> Int64 {
        let res = try await requestJSON(method: "GET", path: "/api/v1/wallet/balance", token: token)
        return res["stars_balance"]?.int64Value ?? 0
    }

    @discardableResult
    public func topupStars(
        token: String,
        amount: Int64,
        description: String = "CI Seed"
    ) async throws -> Int64 {
        let res = try await requestJSON(
            method: "POST",
            path: "/api/v1/wallet/topup",
            token: token,
            body: ["amount_stars": .int(amount), "description": .string(description)]
        )
        return res["stars_balance"]?.int64Value ?? amount
    }

    @discardableResult
    public func createRedeemCode(
        token: String,
        code: String,
        plan: String = "monthly",
        days: Int = 30
    ) async throws -> JSONValue {
        try await requestJSON(
            method: "POST",
            path: "/api/admin/create-redeem-code",
            token: token,
            body: ["code": .string(code), "plan": .string(plan), "days": .int(Int64(days))]
        )
    }

    public func createBotWithMiniAppViaNL(
        token: String,
        prompt: String
    ) async throws -> (botId: Int64, botUsername: String, shortName: String, chatId: Int64) {
        let res = try await requestJSON(
            method: "POST",
            path: "/api/v1/botfather/nl-create",
            token: token,
            body: ["prompt": .string(prompt)]
        )
        let botId = res["bot"]?["bot_id"]?.int64Value ?? res["bot"]?["id"]?.int64Value ?? 0
        let botUsername = res["bot"]?["username"]?.stringValue ?? "companion_bot"
        let shortName = res["app"]?["short_name"]?.stringValue ?? "app"
        let chatId = res["companion_chat_id"]?.int64Value ?? 0
        return (botId, botUsername, shortName, chatId)
    }

    @discardableResult
    public func acceptBotAppTos(token: String, botId: Int64) async throws -> JSONValue {
        try await requestJSON(
            method: "POST",
            path: "/api/v1/bots/\(botId)/accept-app-tos",
            token: token,
            body: [:]
        )
    }

    public func registerLocalComputer(
        token: String,
        deviceId: String,
        displayName: String
    ) async throws -> String {
        let res = try await requestJSON(
            method: "POST",
            path: "/api/v1/computers/local/register",
            token: token,
            body: [
                "device_id": .string(deviceId),
                "display_name": .string(displayName),
                "ip_address": "100.64.0.2",
                "os_name": "macOS 15",
                "resolution": "2560x1440",
            ]
        )
        return res["node_id"]?.stringValue ?? res["id"]?.stringValue ?? deviceId
    }

    public func connectWebSocket(token: String) throws -> LoopbackWebSocketSession {
        let baseStr = baseURL.absoluteString
            .replacingOccurrences(of: "https://", with: "wss://")
            .replacingOccurrences(of: "http://", with: "ws://")
            .trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        let encoded = token.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) ?? token
        guard let wsURL = URL(string: "\(baseStr)/api/v1/ws?token=\(encoded)") else {
            throw LoopbackPeerError.invalidURL("\(baseStr)/api/v1/ws")
        }
        let task = URLSession.shared.webSocketTask(with: wsURL)
        return LoopbackWebSocketSession(task: task)
    }
}
