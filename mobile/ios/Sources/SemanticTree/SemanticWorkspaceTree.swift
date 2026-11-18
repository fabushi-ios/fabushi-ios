import CryptoKit
import Foundation

// MARK: - Sub-Nodes (Exact Wire & Hash Mirror of `gpui_telegram_proto::semantic_tree`)

public struct SemanticWindowNode: Codable, Sendable, Equatable {
    public var layout: String
    public var widthPx: Float
    public var heightPx: Float
    public var activePanel: String
    public var isAuthenticated: Bool

    public init(
        layout: String = "",
        widthPx: Float = 0,
        heightPx: Float = 0,
        activePanel: String = "",
        isAuthenticated: Bool = false
    ) {
        self.layout = layout
        self.widthPx = widthPx
        self.heightPx = heightPx
        self.activePanel = activePanel
        self.isAuthenticated = isAuthenticated
    }

    enum CodingKeys: String, CodingKey {
        case layout
        case widthPx = "width_px"
        case heightPx = "height_px"
        case activePanel = "active_panel"
        case isAuthenticated = "is_authenticated"
    }
}

public struct SemanticNavRailNode: Codable, Sendable, Equatable {
    public var activeDestination: String
    public var connected: Bool

    public init(activeDestination: String = "", connected: Bool = false) {
        self.activeDestination = activeDestination
        self.connected = connected
    }

    enum CodingKeys: String, CodingKey {
        case activeDestination = "active_destination"
        case connected
    }
}

public struct SemanticSidebarNode: Codable, Sendable, Equatable {
    public var dialogCount: UInt64
    public var selectedChatId: UInt64?
    public var searchQuery: String

    public init(dialogCount: UInt64 = 0, selectedChatId: UInt64? = nil, searchQuery: String = "") {
        self.dialogCount = dialogCount
        self.selectedChatId = selectedChatId
        self.searchQuery = searchQuery
    }

    enum CodingKeys: String, CodingKey {
        case dialogCount = "dialog_count"
        case selectedChatId = "selected_chat_id"
        case searchQuery = "search_query"
    }
}

public struct SemanticStageNode: Codable, Sendable, Equatable {
    public var activeChatId: UInt64?
    public var chatTitle: String
    public var messageCount: UInt64
    public var isGroupChat: Bool
    public var pinnedMessage: String?

    public init(
        activeChatId: UInt64? = nil,
        chatTitle: String = "",
        messageCount: UInt64 = 0,
        isGroupChat: Bool = false,
        pinnedMessage: String? = nil
    ) {
        self.activeChatId = activeChatId
        self.chatTitle = chatTitle
        self.messageCount = messageCount
        self.isGroupChat = isGroupChat
        self.pinnedMessage = pinnedMessage
    }

    enum CodingKeys: String, CodingKey {
        case activeChatId = "active_chat_id"
        case chatTitle = "chat_title"
        case messageCount = "message_count"
        case isGroupChat = "is_group_chat"
        case pinnedMessage = "pinned_message"
    }
}

public struct SemanticComposerNode: Codable, Sendable, Equatable {
    public var text: String
    public var isEditing: Bool
    public var hasReplyTarget: Bool

    public init(text: String = "", isEditing: Bool = false, hasReplyTarget: Bool = false) {
        self.text = text
        self.isEditing = isEditing
        self.hasReplyTarget = hasReplyTarget
    }

    enum CodingKeys: String, CodingKey {
        case text
        case isEditing = "is_editing"
        case hasReplyTarget = "has_reply_target"
    }
}

public struct SemanticInspectorNode: Codable, Sendable, Equatable {
    public var isOpen: Bool
    public var profileName: String
    public var sharedMediaCount: UInt64

    public init(isOpen: Bool = false, profileName: String = "", sharedMediaCount: UInt64 = 0) {
        self.isOpen = isOpen
        self.profileName = profileName
        self.sharedMediaCount = sharedMediaCount
    }

    enum CodingKeys: String, CodingKey {
        case isOpen = "is_open"
        case profileName = "profile_name"
        case sharedMediaCount = "shared_media_count"
    }
}

public struct SemanticModalNode: Codable, Sendable, Equatable {
    public var modalId: String
    public var title: String

    public init(modalId: String = "", title: String = "") {
        self.modalId = modalId
        self.title = title
    }

    enum CodingKeys: String, CodingKey {
        case modalId = "modal_id"
        case title
    }
}

// MARK: - SemanticWorkspaceTree (Bit-Exact SHA-256 Parity with Rust `gpui_telegram_proto`)

public struct SemanticWorkspaceTree: Codable, Sendable, Equatable {
    public var timestampMs: Int64
    public var windowLayout: String
    public var selectedChatId: UInt64?
    public var composerText: String
    public var activeModal: String?
    public var isCallActive: Bool
    public var window: SemanticWindowNode
    public var navRail: SemanticNavRailNode
    public var sidebar: SemanticSidebarNode
    public var stage: SemanticStageNode
    public var composer: SemanticComposerNode
    public var inspector: SemanticInspectorNode
    public var activeModalNode: SemanticModalNode?
    public var stateHash: String

    public init(
        timestampMs: Int64 = Int64(Date().timeIntervalSince1970 * 1000),
        windowLayout: String = "CompactTab",
        selectedChatId: UInt64? = nil,
        composerText: String = "",
        activeModal: String? = nil,
        isCallActive: Bool = false,
        window: SemanticWindowNode = SemanticWindowNode(),
        navRail: SemanticNavRailNode = SemanticNavRailNode(),
        sidebar: SemanticSidebarNode = SemanticSidebarNode(),
        stage: SemanticStageNode = SemanticStageNode(),
        composer: SemanticComposerNode = SemanticComposerNode(),
        inspector: SemanticInspectorNode = SemanticInspectorNode(),
        activeModalNode: SemanticModalNode? = nil,
        stateHash: String = ""
    ) {
        self.timestampMs = timestampMs
        self.windowLayout = windowLayout
        self.selectedChatId = selectedChatId
        self.composerText = composerText
        self.activeModal = activeModal
        self.isCallActive = isCallActive
        self.window = window
        self.navRail = navRail
        self.sidebar = sidebar
        self.stage = stage
        self.composer = composer
        self.inspector = inspector
        self.activeModalNode = activeModalNode
        self.stateHash = stateHash
        if self.stateHash.isEmpty {
            self.recomputeStateHash()
        }
    }

    enum CodingKeys: String, CodingKey {
        case timestampMs = "timestamp_ms"
        case windowLayout = "window_layout"
        case selectedChatId = "selected_chat_id"
        case composerText = "composer_text"
        case activeModal = "active_modal"
        case isCallActive = "is_call_active"
        case window
        case navRail = "nav_rail"
        case sidebar
        case stage
        case composer
        case inspector
        case activeModalNode = "active_modal_node"
        case stateHash = "state_hash"
    }

    private static func hashStr(_ hasher: inout SHA256, _ s: String) {
        let utf8 = Array(s.utf8)
        var lenLE = UInt64(utf8.count).littleEndian
        withUnsafeBytes(of: &lenLE) { hasher.update(bufferPointer: $0) }
        hasher.update(data: utf8)
    }

    private static func hashOptStr(_ hasher: inout SHA256, _ s: String?) {
        switch s {
        case .none:
            hasher.update(data: [0])
        case .some(let v):
            hasher.update(data: [1])
            hashStr(&hasher, v)
        }
    }

    private static func hashOptU64(_ hasher: inout SHA256, _ v: UInt64?) {
        switch v {
        case .none:
            hasher.update(data: [0])
        case .some(let n):
            hasher.update(data: [1])
            var le = n.littleEndian
            withUnsafeBytes(of: &le) { hasher.update(bufferPointer: $0) }
        }
    }

    private static func hashU64(_ hasher: inout SHA256, _ v: UInt64) {
        var le = v.littleEndian
        withUnsafeBytes(of: &le) { hasher.update(bufferPointer: $0) }
    }

    private static func hashF32Bits(_ hasher: inout SHA256, _ v: Float) {
        var le = v.bitPattern.littleEndian
        withUnsafeBytes(of: &le) { hasher.update(bufferPointer: $0) }
    }

    private static func finalizeHex(_ hasher: SHA256) -> String {
        hasher.finalize().map { String(format: "%02x", $0) }.joined()
    }

    public static func computeHashParts(
        layout: String,
        chatId: UInt64?,
        text: String,
        modal: String?,
        call: Bool
    ) -> String {
        var hasher = SHA256()
        hashStr(&hasher, layout)
        hashOptU64(&hasher, chatId)
        hashStr(&hasher, text)
        hashOptStr(&hasher, modal)
        hasher.update(data: [call ? 1 : 0])
        return finalizeHex(hasher)
    }

    public func computeFullStateHash() -> String {
        var hasher = SHA256()
        Self.hashStr(&hasher, windowLayout)
        Self.hashOptU64(&hasher, selectedChatId)
        Self.hashStr(&hasher, composerText)
        Self.hashOptStr(&hasher, activeModal)
        hasher.update(data: [isCallActive ? 1 : 0])

        // Window node
        Self.hashStr(&hasher, window.layout)
        Self.hashF32Bits(&hasher, window.widthPx)
        Self.hashF32Bits(&hasher, window.heightPx)
        Self.hashStr(&hasher, window.activePanel)
        hasher.update(data: [window.isAuthenticated ? 1 : 0])

        // Nav rail node
        Self.hashStr(&hasher, navRail.activeDestination)
        hasher.update(data: [navRail.connected ? 1 : 0])

        // Sidebar node
        Self.hashU64(&hasher, sidebar.dialogCount)
        Self.hashOptU64(&hasher, sidebar.selectedChatId)
        Self.hashStr(&hasher, sidebar.searchQuery)

        // Stage node
        Self.hashOptU64(&hasher, stage.activeChatId)
        Self.hashStr(&hasher, stage.chatTitle)
        Self.hashU64(&hasher, stage.messageCount)
        hasher.update(data: [stage.isGroupChat ? 1 : 0])
        Self.hashOptStr(&hasher, stage.pinnedMessage)

        // Composer node
        Self.hashStr(&hasher, composer.text)
        hasher.update(data: [composer.isEditing ? 1 : 0])
        hasher.update(data: [composer.hasReplyTarget ? 1 : 0])

        // Inspector node
        hasher.update(data: [inspector.isOpen ? 1 : 0])
        Self.hashStr(&hasher, inspector.profileName)
        Self.hashU64(&hasher, inspector.sharedMediaCount)

        // Active modal node
        switch activeModalNode {
        case .none:
            hasher.update(data: [0])
        case .some(let m):
            hasher.update(data: [1])
            Self.hashStr(&hasher, m.modalId)
            Self.hashStr(&hasher, m.title)
        }

        return Self.finalizeHex(hasher)
    }

    public mutating func recomputeStateHash() {
        stateHash = computeFullStateHash()
    }
}

// MARK: - SemanticAction (`#[serde(tag = "action", content = "params")]`) & Receipt

public enum SemanticAction: Codable, Sendable, Equatable {
    case selectChat(chatId: UInt64)
    case composeText(text: String)
    case sendMessage
    case openModal(modalId: String)
    case closeModal
    case toggleCall(peerId: UInt64, isVideo: Bool)

    enum CodingKeys: String, CodingKey {
        case action
        case params
    }

    private struct SelectChatParams: Codable { let chat_id: UInt64 }
    private struct ComposeTextParams: Codable { let text: String }
    private struct OpenModalParams: Codable { let modal_id: String }
    private struct ToggleCallParams: Codable {
        let peer_id: UInt64
        let is_video: Bool
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let tag = try container.decode(String.self, forKey: .action)
        switch tag {
        case "SelectChat":
            let p = try container.decode(SelectChatParams.self, forKey: .params)
            self = .selectChat(chatId: p.chat_id)
        case "ComposeText":
            let p = try container.decode(ComposeTextParams.self, forKey: .params)
            self = .composeText(text: p.text)
        case "SendMessage":
            self = .sendMessage
        case "OpenModal":
            let p = try container.decode(OpenModalParams.self, forKey: .params)
            self = .openModal(modalId: p.modal_id)
        case "CloseModal":
            self = .closeModal
        case "ToggleCall":
            let p = try container.decode(ToggleCallParams.self, forKey: .params)
            self = .toggleCall(peerId: p.peer_id, isVideo: p.is_video)
        default:
            throw DecodingError.dataCorruptedError(
                forKey: .action,
                in: container,
                debugDescription: "Unknown SemanticAction tag: \(tag)"
            )
        }
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .selectChat(let chatId):
            try container.encode("SelectChat", forKey: .action)
            try container.encode(SelectChatParams(chat_id: chatId), forKey: .params)
        case .composeText(let text):
            try container.encode("ComposeText", forKey: .action)
            try container.encode(ComposeTextParams(text: text), forKey: .params)
        case .sendMessage:
            try container.encode("SendMessage", forKey: .action)
        case .openModal(let modalId):
            try container.encode("OpenModal", forKey: .action)
            try container.encode(OpenModalParams(modal_id: modalId), forKey: .params)
        case .closeModal:
            try container.encode("CloseModal", forKey: .action)
        case .toggleCall(let peerId, let isVideo):
            try container.encode("ToggleCall", forKey: .action)
            try container.encode(ToggleCallParams(peer_id: peerId, is_video: isVideo), forKey: .params)
        }
    }
}

public enum ActionExecutionStatus: String, Codable, Sendable, Equatable {
    case succeeded = "Succeeded"
    case failed = "Failed"
    case rejected = "Rejected"
}

public struct SemanticActionReceipt: Codable, Sendable, Equatable {
    public let actionId: String
    public let actionType: String
    public let success: Bool
    public let status: ActionExecutionStatus
    public let error: String?
    public let timestampMs: Int64
    public let resultingStateHash: String

    enum CodingKeys: String, CodingKey {
        case actionId = "action_id"
        case actionType = "action_type"
        case success
        case status
        case error
        case timestampMs = "timestamp_ms"
        case resultingStateHash = "resulting_state_hash"
    }
}
