import CryptoKit
import Foundation

public enum JSONValue: Codable, Sendable, Equatable {
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
        } else if let a = try? container.decode([JSONValue].self) {
            self = .array(a)
        } else {
            self = .object(try container.decode([String: JSONValue].self))
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
        case .array(let a):
            try container.encode(a)
        case .object(let o):
            try container.encode(o)
        }
    }

    public subscript(key: String) -> JSONValue? {
        guard case .object(let map) = self else { return nil }
        return map[key]
    }

    public var stringValue: String? {
        guard case .string(let s) = self else { return nil }
        return s
    }

    public var int64Value: Int64? {
        switch self {
        case .int(let i): return i
        case .double(let d): return Int64(d)
        default: return nil
        }
    }

    public var uint64Value: UInt64? {
        guard let i = int64Value, i >= 0 else { return nil }
        return UInt64(i)
    }

    public var boolValue: Bool? {
        guard case .bool(let b) = self else { return nil }
        return b
    }

    public var arrayValue: [JSONValue]? {
        guard case .array(let a) = self else { return nil }
        return a
    }
}

public struct HTTPResponseEnvelope: Sendable, Equatable {
    public let statusCode: Int
    public let headers: [String: String]
    public let body: Data

    public var bodyText: String {
        String(decoding: body, as: UTF8.self)
    }

    public func decode<T: Decodable>(_ type: T.Type) throws -> T {
        try JSONDecoder().decode(T.self, from: body)
    }

    public func decodeJSON() throws -> JSONValue {
        try JSONDecoder().decode(JSONValue.self, from: body)
    }
}

// MARK: - R1..R4 Wire DTOs (Reconciled with `bhrum/gpui-telegram@b436dcdc`)

public struct RegisterRequestDTO: Codable, Sendable, Equatable {
    public let username: String
    public let password: String
    public let first_name: String
    public let last_name: String?
    public let device_name: String?
    public let auto_welcome: Bool

    public init(
        username: String,
        password: String = "StrongPass#2026_E2E",
        firstName: String = "Test",
        lastName: String? = "User",
        deviceName: String? = "iPhone 16 Pro",
        autoWelcome: Bool = false
    ) {
        self.username = username
        self.password = password
        self.first_name = firstName
        self.last_name = lastName
        self.device_name = deviceName
        self.auto_welcome = autoWelcome
    }
}

public struct LoginRequestDTO: Codable, Sendable, Equatable {
    public let username: String
    public let password: String
    public let device_name: String?

    public init(username: String, password: String, deviceName: String? = "iPhone 16 Pro") {
        self.username = username
        self.password = password
        self.device_name = deviceName
    }
}

public struct AuthResponseDTO: Codable, Sendable, Equatable {
    public let user_id: UInt64
    public let username: String
    public let session_id: String
    public let token: String
    public let first_name: String?
    public let last_name: String?
    public let account_generation: UInt64?
    public let expires_at: String?
}

public struct SessionItemDTO: Codable, Sendable, Equatable {
    public let session_id: String
    public let device_name: String
    public let is_current: Bool
    public let platform: String?
    public let ip_address: String?
}

public struct SessionsListResponseDTO: Codable, Sendable, Equatable {
    public let user_id: UInt64?
    public let sessions: [SessionItemDTO]
}

public struct AlipayStartResponseDTO: Codable, Sendable, Equatable {
    public let attempt_id: String
    public let login_url: String
    public let expires_at_ms: Int64?
}

public struct AlipayPollResponseDTO: Codable, Sendable, Equatable {
    public let status: String
    public let token: String?
}

public struct PhoneSendCodeResponseDTO: Codable, Sendable, Equatable {
    public let ok: Bool
    public let sent: Bool
    public let phone_code_hash: String
    public let expires_at_ms: Int64
    public let resend_after_ms: Int64
    public let debug_code: String?
}

public struct EmailSendCodeResponseDTO: Codable, Sendable, Equatable {
    public let ok: Bool
    public let cooldown_seconds: Int64?
    public let expires_in_seconds: Int64?
    public let expires_at_ms: Int64?
    public let resend_after_ms: Int64?
    public let debug_code: String?
}

public struct QrPairInitResponseDTO: Codable, Sendable, Equatable {
    public let pair_token: String
    public let qr_url: String
    public let qr_payload: String?
    public let expires_at_ms: Int64
}

public struct PasskeyChallengeDTO: Codable, Sendable, Equatable {
    public let challenge: String
    public let rp_id: String
    public let timeout_ms: UInt64
}

public struct PasskeyKeyMaterial: Sendable {
    public let credentialID: String
    public let publicKeyHex: String
    private let rawPrivateKey: Data

    public init(labelPrefix: String = "ios_pk") {
        let key = Curve25519.Signing.PrivateKey()
        self.credentialID = "\(labelPrefix)_\(UUID().uuidString.prefix(10).lowercased())"
        self.publicKeyHex = key.publicKey.rawRepresentation.map { String(format: "%02x", $0) }.joined()
        self.rawPrivateKey = key.rawRepresentation
    }

    public func signChallengeHex(_ challenge: String) throws -> String {
        let key = try Curve25519.Signing.PrivateKey(rawRepresentation: rawPrivateKey)
        let sig = try key.signature(for: Data(challenge.utf8))
        return sig.map { String(format: "%02x", $0) }.joined()
    }
}

public struct MessageEntityDTO: Codable, Sendable, Equatable {
    public let type: String
    public let offset: Int
    public let length: Int
    public let url: String?
    public let language: String?

    public init(type: String, offset: Int, length: Int, url: String? = nil, language: String? = nil) {
        self.type = type
        self.offset = offset
        self.length = length
        self.url = url
        self.language = language
    }
}

public struct CreateChatRequestDTO: Codable, Sendable, Equatable {
    public let title: String
    public let chat_type: String?
    public let peer_user_id: UInt64?
    public let username: String?
    public let about: String?
    public let is_forum: Bool
    public let member_ids: [UInt64]

    public init(
        title: String,
        chatType: String? = nil,
        peerUserId: UInt64? = nil,
        username: String? = nil,
        about: String? = nil,
        isForum: Bool = false,
        memberIds: [UInt64] = []
    ) {
        self.title = title
        self.chat_type = chatType
        self.peer_user_id = peerUserId
        self.username = username
        self.about = about
        self.is_forum = isForum
        self.member_ids = memberIds
    }
}

public struct ChatDetailDTO: Codable, Sendable, Equatable {
    public let chat_id: Int64
    public let title: String
    public let kind: String?
    public let is_group: Bool?
    public let is_channel: Bool?
    public let is_bot: Bool?
    public let is_forum: Bool?
    public let is_archived: Bool?
    public let is_pinned: Bool?
    public let is_muted: Bool?
    public let pts: UInt64?
    public let unread_count: UInt32?
}

public struct SendMessageRequestDTO: Codable, Sendable, Equatable {
    public let text: String?
    public let entities: [MessageEntityDTO]
    public let reply_to: Int64?
    public let topic_id: Int64?
    public let quote_text: String?
    public let client_nonce: UInt64?
    public let media_id: String?
    public let forward_from_chat_id: Int64?
    public let forward_from_msg_id: Int64?

    public init(
        text: String?,
        entities: [MessageEntityDTO] = [],
        replyTo: Int64? = nil,
        topicId: Int64? = nil,
        quoteText: String? = nil,
        clientNonce: UInt64? = nil,
        mediaId: String? = nil,
        forwardFromChatId: Int64? = nil,
        forwardFromMsgId: Int64? = nil
    ) {
        self.text = text
        self.entities = entities
        self.reply_to = replyTo
        self.topic_id = topicId
        self.quote_text = quoteText
        self.client_nonce = clientNonce
        self.media_id = mediaId
        self.forward_from_chat_id = forwardFromChatId
        self.forward_from_msg_id = forwardFromMsgId
    }
}

public struct SendMessageResponseDTO: Codable, Sendable, Equatable {
    public let message_id: Int64
    public let pts: UInt64
    public let chat_id: Int64?
    public let duplicate: Bool?
    public let text: String?
}

public struct MediaUploadResponseDTO: Codable, Sendable, Equatable {
    public let file_id: String
    public let media_id: String?
    public let sha256: String
    public let size_bytes: UInt64
    public let mime_type: String
}

// MARK: - Live WebSocket Multi-Actor Oracle (`URLSessionWebSocketTask`)

public actor OpaqueWebSocketPeer {
    private let session: URLSession
    private let task: URLSessionWebSocketTask

    public init(baseURL: URL, bearerToken: String) throws {
        var urlStr = baseURL.absoluteString.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        if urlStr.hasPrefix("https://") {
            urlStr = "wss://" + urlStr.dropFirst("https://".count)
        } else if urlStr.hasPrefix("http://") {
            urlStr = "ws://" + urlStr.dropFirst("http://".count)
        }
        let encoded = bearerToken.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) ?? bearerToken
        guard let wsURL = URL(string: "\(urlStr)/api/v1/ws?token=\(encoded)") else {
            throw URLError(.badURL)
        }
        let session = URLSession(configuration: .ephemeral)
        let task = session.webSocketTask(with: wsURL)
        self.session = session
        self.task = task
        task.resume()
    }

    public func sendFrame(_ frame: JSONValue) async throws {
        let data = try JSONEncoder().encode(frame)
        let text = String(decoding: data, as: UTF8.self)
        try await task.send(.string(text))
    }

    public func receiveNextFrame() async throws -> JSONValue {
        let msg = try await task.receive()
        switch msg {
        case .string(let s):
            return try JSONDecoder().decode(JSONValue.self, from: Data(s.utf8))
        case .data(let d):
            return try JSONDecoder().decode(JSONValue.self, from: d)
        @unknown default:
            return .null
        }
    }

    public func close() {
        task.cancel(with: .normalClosure, reason: nil)
        session.finishTasksAndInvalidate()
    }
}

// MARK: - Loopback Server HTTP/WS Client

public struct LoopbackServerClient: Sendable {
    public let baseURL: URL

    public init(baseURL: URL) {
        self.baseURL = baseURL
    }

    public static func fromEnvironment() -> LoopbackServerClient {
        let env = ProcessInfo.processInfo.environment
        let raw = env["FABUSHI_SERVER_URL"]
            ?? env["TEST_RUNNER_FABUSHI_SERVER_URL"]
            ?? "http://127.0.0.1:18480"
        return LoopbackServerClient(baseURL: URL(string: raw)!)
    }

    public func buildURL(path: String, queryItems: [URLQueryItem] = []) throws -> URL {
        let baseStr = baseURL.absoluteString.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        let normalizedPath = path.hasPrefix("/") ? path : "/\(path)"
        guard var components = URLComponents(string: baseStr + normalizedPath) else {
            throw URLError(.badURL)
        }
        if !queryItems.isEmpty {
            var existing = components.queryItems ?? []
            existing.append(contentsOf: queryItems)
            components.queryItems = existing
        }
        guard let url = components.url else {
            throw URLError(.badURL)
        }
        return url
    }

    public func request(
        method: String,
        path: String,
        bearerToken: String? = nil,
        extraHeaders: [String: String] = [:],
        queryItems: [URLQueryItem] = [],
        body: Data? = nil,
        contentType: String? = "application/json",
        timeout: TimeInterval = 15.0
    ) async throws -> HTTPResponseEnvelope {
        let url = try buildURL(path: path, queryItems: queryItems)
        var urlRequest = URLRequest(url: url, timeoutInterval: timeout)
        urlRequest.httpMethod = method
        if let token = bearerToken {
            urlRequest.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        if let body = body {
            if let ct = contentType {
                urlRequest.setValue(ct, forHTTPHeaderField: "Content-Type")
            }
            urlRequest.httpBody = body
        }
        for (k, v) in extraHeaders {
            urlRequest.setValue(v, forHTTPHeaderField: k)
        }

        let session = URLSession(configuration: .ephemeral)
        defer { session.finishTasksAndInvalidate() }

        let (data, response) = try await session.data(for: urlRequest)
        guard let http = response as? HTTPURLResponse else {
            throw URLError(.badServerResponse)
        }
        var stringHeaders: [String: String] = [:]
        for (k, v) in http.allHeaderFields {
            stringHeaders[String(describing: k).lowercased()] = String(describing: v)
        }
        return HTTPResponseEnvelope(statusCode: http.statusCode, headers: stringHeaders, body: data)
    }

    public func requestJSON(
        method: String,
        path: String,
        bearerToken: String? = nil,
        extraHeaders: [String: String] = [:],
        queryItems: [URLQueryItem] = [],
        payload: JSONValue? = nil
    ) async throws -> HTTPResponseEnvelope {
        let bodyData = try payload.map { try JSONEncoder().encode($0) }
        return try await request(
            method: method,
            path: path,
            bearerToken: bearerToken,
            extraHeaders: extraHeaders,
            queryItems: queryItems,
            body: bodyData
        )
    }

    // MARK: - Auth & Sessions

    public func registerUser(_ req: RegisterRequestDTO) async throws -> AuthResponseDTO {
        let body = try JSONEncoder().encode(req)
        let resp = try await request(method: "POST", path: "/api/v1/auth/register", body: body)
        guard (200..<300).contains(resp.statusCode) else {
            throw NSError(
                domain: "LoopbackServerClient",
                code: resp.statusCode,
                userInfo: [NSLocalizedDescriptionKey: "register failed (\(resp.statusCode)): \(resp.bodyText)"]
            )
        }
        return try resp.decode(AuthResponseDTO.self)
    }

    public func registerRandomUser(
        prefix: String = "actor",
        password: String = "StrongPass#2026_E2E",
        firstName: String = "Alice",
        autoWelcome: Bool = false
    ) async throws -> AuthResponseDTO {
        let suffix = UUID().uuidString.prefix(8).lowercased()
        return try await registerUser(
            RegisterRequestDTO(
                username: "\(prefix)_\(suffix)",
                password: password,
                firstName: firstName,
                autoWelcome: autoWelcome
            )
        )
    }

    public func loginUser(_ req: LoginRequestDTO) async throws -> HTTPResponseEnvelope {
        let body = try JSONEncoder().encode(req)
        return try await request(method: "POST", path: "/api/v1/auth/login", body: body)
    }

    public func fetchMe(token: String) async throws -> HTTPResponseEnvelope {
        try await request(method: "GET", path: "/api/v1/auth/me", bearerToken: token)
    }

    public func listSessions(token: String) async throws -> SessionsListResponseDTO {
        let resp = try await request(method: "GET", path: "/api/v1/auth/sessions", bearerToken: token)
        return try resp.decode(SessionsListResponseDTO.self)
    }

    public func revokeSession(token: String, sessionId: String) async throws -> HTTPResponseEnvelope {
        try await request(method: "DELETE", path: "/api/v1/auth/sessions/\(sessionId)", bearerToken: token)
    }

    public func revokeOtherSessions(token: String) async throws -> HTTPResponseEnvelope {
        try await request(method: "DELETE", path: "/api/v1/auth/sessions/others", bearerToken: token)
    }

    public func enableTwoFactor(
        token: String,
        cloudPassword: String,
        hint: String? = nil,
        recoveryEmail: String? = nil
    ) async throws -> HTTPResponseEnvelope {
        var map: [String: JSONValue] = ["new_password": .string(cloudPassword)]
        if let hint = hint { map["hint"] = .string(hint) }
        if let email = recoveryEmail { map["recovery_email"] = .string(email) }
        return try await requestJSON(
            method: "POST",
            path: "/api/v1/auth/2fa/set",
            bearerToken: token,
            payload: .object(map)
        )
    }

    public func disableTwoFactor(token: String, currentPassword: String) async throws -> HTTPResponseEnvelope {
        try await requestJSON(
            method: "DELETE",
            path: "/api/v1/auth/2fa/disable",
            bearerToken: token,
            payload: .object(["current_password": .string(currentPassword)])
        )
    }

    public func sendPhoneCode(phone: String) async throws -> PhoneSendCodeResponseDTO {
        let resp = try await requestJSON(
            method: "POST",
            path: "/api/v1/auth/phone/send-code",
            payload: .object(["phone": .string(phone)])
        )
        return try resp.decode(PhoneSendCodeResponseDTO.self)
    }

    public func sendEmailCode(email: String) async throws -> EmailSendCodeResponseDTO {
        let resp = try await requestJSON(
            method: "POST",
            path: "/api/v1/auth/email/send-code",
            payload: .object(["email": .string(email)])
        )
        return try resp.decode(EmailSendCodeResponseDTO.self)
    }

    public func startAlipayOAuth() async throws -> AlipayStartResponseDTO {
        let resp = try await requestJSON(
            method: "POST",
            path: "/api/v1/auth/alipay/start",
            payload: .object([:])
        )
        return try resp.decode(AlipayStartResponseDTO.self)
    }

    public func simulateAlipayScan(attemptId: String) async throws -> HTTPResponseEnvelope {
        try await requestJSON(
            method: "POST",
            path: "/api/v1/auth/alipay/simulate-scan",
            payload: .object(["attempt_id": .string(attemptId)])
        )
    }

    public func initQrPairing(deviceName: String = "iPad Companion") async throws -> QrPairInitResponseDTO {
        let resp = try await requestJSON(
            method: "POST",
            path: "/api/v1/auth/pair/init",
            payload: .object(["device_name": .string(deviceName)])
        )
        return try resp.decode(QrPairInitResponseDTO.self)
    }

    public func acceptQrPairing(token: String, pairToken: String) async throws -> HTTPResponseEnvelope {
        try await requestJSON(
            method: "POST",
            path: "/api/v1/auth/pair/accept",
            bearerToken: token,
            payload: .object(["pair_token": .string(pairToken)])
        )
    }

    public func registerPasskey(token: String, material: PasskeyKeyMaterial, deviceName: String = "iPhone 16 Pro") async throws -> HTTPResponseEnvelope {
        try await requestJSON(
            method: "POST",
            path: "/api/v1/auth/passkey/register",
            bearerToken: token,
            payload: .object([
                "credential_id": .string(material.credentialID),
                "public_key": .string(material.publicKeyHex),
                "device_name": .string(deviceName),
            ])
        )
    }

    // MARK: - Chats, Folders, Stories, Invites & Messaging

    public func createChat(token: String, _ req: CreateChatRequestDTO) async throws -> ChatDetailDTO {
        let body = try JSONEncoder().encode(req)
        let resp = try await request(method: "POST", path: "/api/v1/chats", bearerToken: token, body: body)
        guard (200..<300).contains(resp.statusCode) else {
            throw NSError(
                domain: "LoopbackServerClient",
                code: resp.statusCode,
                userInfo: [NSLocalizedDescriptionKey: "createChat failed (\(resp.statusCode)): \(resp.bodyText)"]
            )
        }
        return try resp.decode(ChatDetailDTO.self)
    }

    public func listChats(token: String) async throws -> JSONValue {
        let resp = try await request(method: "GET", path: "/api/v1/chats", bearerToken: token)
        return try resp.decodeJSON()
    }

    public func sendMessage(
        token: String,
        chatId: Int64,
        _ req: SendMessageRequestDTO
    ) async throws -> SendMessageResponseDTO {
        let body = try JSONEncoder().encode(req)
        let resp = try await request(
            method: "POST",
            path: "/api/v1/chats/\(chatId)/messages",
            bearerToken: token,
            body: body
        )
        guard (200..<300).contains(resp.statusCode) else {
            throw NSError(
                domain: "LoopbackServerClient",
                code: resp.statusCode,
                userInfo: [NSLocalizedDescriptionKey: "sendMessage failed (\(resp.statusCode)): \(resp.bodyText)"]
            )
        }
        return try resp.decode(SendMessageResponseDTO.self)
    }

    public func listMessages(token: String, chatId: Int64, limit: Int = 50) async throws -> JSONValue {
        let resp = try await request(
            method: "GET",
            path: "/api/v1/chats/\(chatId)/messages",
            bearerToken: token,
            queryItems: [URLQueryItem(name: "limit", value: String(limit))]
        )
        return try resp.decodeJSON()
    }

    public func editMessage(token: String, chatId: Int64, messageId: Int64, text: String) async throws -> HTTPResponseEnvelope {
        try await requestJSON(
            method: "PUT",
            path: "/api/v1/chats/\(chatId)/messages/\(messageId)",
            bearerToken: token,
            payload: .object(["text": .string(text)])
        )
    }

    public func deleteMessage(token: String, chatId: Int64, messageId: Int64, revoke: Bool = true) async throws -> HTTPResponseEnvelope {
        try await request(
            method: "DELETE",
            path: "/api/v1/chats/\(chatId)/messages/\(messageId)",
            bearerToken: token,
            queryItems: [URLQueryItem(name: "revoke", value: revoke ? "true" : "false")]
        )
    }

    public func setDraft(token: String, chatId: Int64, text: String) async throws -> HTTPResponseEnvelope {
        try await requestJSON(
            method: "PUT",
            path: "/api/v1/chats/\(chatId)/draft",
            bearerToken: token,
            payload: .object(["text": .string(text)])
        )
    }

    public func createFolder(token: String, title: String, includeChats: [Int64], emoticon: String = "📁") async throws -> JSONValue {
        let resp = try await requestJSON(
            method: "POST",
            path: "/api/v1/folders",
            bearerToken: token,
            payload: .object([
                "title": .string(title),
                "emoticon": .string(emoticon),
                "include_chats": .array(includeChats.map { .int($0) }),
            ])
        )
        return try resp.decodeJSON()
    }

    public func archiveChat(token: String, chatId: Int64, archived: Bool = true) async throws -> HTTPResponseEnvelope {
        try await requestJSON(
            method: "POST",
            path: "/api/v1/chats/\(chatId)/archive",
            bearerToken: token,
            payload: .object(["archived": .bool(archived)])
        )
    }

    public func postStory(token: String, caption: String, mediaURL: String = "https://example.invalid/story.jpg") async throws -> JSONValue {
        let resp = try await requestJSON(
            method: "POST",
            path: "/api/v1/stories",
            bearerToken: token,
            payload: .object([
                "media_url": .string(mediaURL),
                "media_type": .string("photo"),
                "caption": .string(caption),
            ])
        )
        return try resp.decodeJSON()
    }

    public func createInvite(token: String, chatId: Int64, usageLimit: Int? = nil) async throws -> JSONValue {
        var map: [String: JSONValue] = [:]
        if let limit = usageLimit {
            map["usage_limit"] = .int(Int64(limit))
        }
        let resp = try await requestJSON(
            method: "POST",
            path: "/api/v1/chats/\(chatId)/invites",
            bearerToken: token,
            payload: .object(map)
        )
        return try resp.decodeJSON()
    }

    public func pinMessage(token: String, chatId: Int64, messageId: Int64, silent: Bool = false) async throws -> HTTPResponseEnvelope {
        try await requestJSON(
            method: "POST",
            path: "/api/v1/chats/\(chatId)/pins",
            bearerToken: token,
            payload: .object([
                "message_id": .int(messageId),
                "silent": .bool(silent),
            ])
        )
    }

    public func setChatTTL(token: String, chatId: Int64, ttlSeconds: Int64) async throws -> HTTPResponseEnvelope {
        try await requestJSON(
            method: "POST",
            path: "/api/v1/chats/\(chatId)/ttl",
            bearerToken: token,
            payload: .object(["ttl_seconds": .int(ttlSeconds)])
        )
    }

    public func reactToMessage(token: String, chatId: Int64, messageId: Int64, emoji: String, action: String = "toggle") async throws -> HTTPResponseEnvelope {
        try await requestJSON(
            method: "POST",
            path: "/api/v1/chats/\(chatId)/messages/\(messageId)/reactions",
            bearerToken: token,
            payload: .object([
                "emoji": .string(emoji),
                "action": .string(action),
            ])
        )
    }

    public func createPoll(
        token: String,
        chatId: Int64,
        question: String,
        options: [String],
        isAnonymous: Bool = false,
        allowsMultiple: Bool = false,
        isQuiz: Bool = false,
        correctOptionId: Int? = nil,
        solution: String? = nil
    ) async throws -> JSONValue {
        var map: [String: JSONValue] = [
            "question": .string(question),
            "options": .array(options.map { .string($0) }),
            "is_anonymous": .bool(isAnonymous),
            "allows_multiple_answers": .bool(allowsMultiple),
            "is_quiz": .bool(isQuiz),
        ]
        if let cid = correctOptionId { map["correct_option_id"] = .int(Int64(cid)) }
        if let sol = solution { map["solution"] = .string(sol) }
        let resp = try await requestJSON(
            method: "POST",
            path: "/api/v1/chats/\(chatId)/polls",
            bearerToken: token,
            payload: .object(map)
        )
        return try resp.decodeJSON()
    }

    public func uploadMediaBase64(
        token: String,
        data: Data,
        filename: String,
        mimeType: String,
        isVoice: Bool = false,
        isVideoNote: Bool = false
    ) async throws -> MediaUploadResponseDTO {
        let resp = try await requestJSON(
            method: "POST",
            path: "/api/v1/media/upload",
            bearerToken: token,
            queryItems: [
                URLQueryItem(name: "is_voice", value: isVoice ? "true" : "false"),
                URLQueryItem(name: "is_video_note", value: isVideoNote ? "true" : "false"),
            ],
            payload: .object([
                "data_base64": .string(data.base64EncodedString()),
                "filename": .string(filename),
                "mime_type": .string(mimeType),
            ])
        )
        return try resp.decode(MediaUploadResponseDTO.self)
    }

    public func connectWebSocket(token: String) throws -> OpaqueWebSocketPeer {
        try OpaqueWebSocketPeer(baseURL: baseURL, bearerToken: token)
    }
}
