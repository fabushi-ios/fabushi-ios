//
//  ConversationStageModels.swift
//  Fabushi
//
//  Production models for Telegram 1:1 and Group Chat Stages.
//  Conforms to Swift 6 strict concurrency and MTProto entity representations.
//

import Foundation
import CoreGraphics

// MARK: - Delivery Status
public enum TelegramDeliveryStatus: String, Sendable, Codable, Equatable, Hashable, CaseIterable {
    case pending
    case sent
    case read
    case failed
}

// MARK: - Message Reactions
public struct TelegramReaction: Hashable, Sendable, Codable, Identifiable {
    public var id: String { emoji }
    public let emoji: String
    public var count: Int
    public var chosenByMe: Bool

    public init(emoji: String, count: Int = 1, chosenByMe: Bool = false) {
        self.emoji = emoji
        self.count = count
        self.chosenByMe = chosenByMe
    }
}

// MARK: - Message Entity Representation (UTF-16 Offset/Length)
public enum MessageEntityType: String, Codable, Sendable, Equatable, Hashable, CaseIterable {
    case bold
    case italic
    case underline
    case strikethrough
    case code
    case pre
    case textUrl
    case url
    case mention
    case spoiler
}

public struct MessageEntity: Codable, Sendable, Equatable, Hashable, Identifiable {
    public var id: String { "\(type.rawValue)_\(offset)_\(length)_\(url ?? "")" }
    public let type: MessageEntityType
    public let offset: Int // UTF-16 code units
    public let length: Int // UTF-16 code units
    public let url: String?
    public let language: String?

    public init(
        type: MessageEntityType,
        offset: Int,
        length: Int,
        url: String? = nil,
        language: String? = nil
    ) {
        self.type = type
        self.offset = offset
        self.length = length
        self.url = url
        self.language = language
    }
}

// MARK: - Cluster Positioning & Corner Geometry
public enum MessageClusterPosition: String, Sendable, Equatable, Hashable, CaseIterable {
    case single
    case top
    case middle
    case bottom
}

public struct BubbleCornerRadii: Sendable, Equatable {
    public let topLeft: CGFloat
    public let topRight: CGFloat
    public let bottomLeft: CGFloat
    public let bottomRight: CGFloat
    public let hasTail: Bool

    public init(
        topLeft: CGFloat,
        topRight: CGFloat,
        bottomLeft: CGFloat,
        bottomRight: CGFloat,
        hasTail: Bool
    ) {
        self.topLeft = topLeft
        self.topRight = topRight
        self.bottomLeft = bottomLeft
        self.bottomRight = bottomRight
        self.hasTail = hasTail
    }

    public static let standardSingle = BubbleCornerRadii(
        topLeft: 16,
        topRight: 16,
        bottomLeft: 16,
        bottomRight: 16,
        hasTail: true
    )
}

// MARK: - Primary Telegram Stage Message
public struct TelegramStageMessage: Identifiable, Hashable, Sendable, Codable {
    public let id: Int64
    public let chatId: Int64
    public let senderId: Int64
    public var senderName: String?
    public var text: String
    public let timestamp: Int64 // Epoch milliseconds
    public var replyTo: Int64?
    public var clientNonce: UInt64?
    public var status: TelegramDeliveryStatus
    public var isPinned: Bool
    public var mediaType: String?
    public var reactions: [TelegramReaction]
    public var isOutgoing: Bool
    public var isEdited: Bool
    public var entities: [MessageEntity]
    public var ttlSeconds: Int?
    public var expiresAtMs: Int64?

    public init(
        id: Int64,
        chatId: Int64,
        senderId: Int64,
        senderName: String? = nil,
        text: String,
        timestamp: Int64,
        replyTo: Int64? = nil,
        clientNonce: UInt64? = nil,
        status: TelegramDeliveryStatus = .sent,
        isPinned: Bool = false,
        mediaType: String? = nil,
        reactions: [TelegramReaction] = [],
        isOutgoing: Bool = false,
        isEdited: Bool = false,
        entities: [MessageEntity] = [],
        ttlSeconds: Int? = nil,
        expiresAtMs: Int64? = nil
    ) {
        self.id = id
        self.chatId = chatId
        self.senderId = senderId
        self.senderName = senderName
        self.text = text
        self.timestamp = timestamp
        self.replyTo = replyTo
        self.clientNonce = clientNonce
        self.status = status
        self.isPinned = isPinned
        self.mediaType = mediaType
        self.reactions = reactions
        self.isOutgoing = isOutgoing
        self.isEdited = isEdited
        self.entities = entities
        self.ttlSeconds = ttlSeconds
        self.expiresAtMs = expiresAtMs
    }
}
