import Foundation
import XCTest

public struct SemanticWindowNodeDTO: Codable, Sendable, Equatable {
    public var layout: String
    public var width_px: Float
    public var height_px: Float
    public var active_panel: String
    public var is_authenticated: Bool
}

public struct SemanticNavRailNodeDTO: Codable, Sendable, Equatable {
    public var active_destination: String
    public var connected: Bool
}

public struct SemanticSidebarNodeDTO: Codable, Sendable, Equatable {
    public var dialog_count: Int
    public var selected_chat_id: UInt64?
    public var search_query: String
}

public struct SemanticStageNodeDTO: Codable, Sendable, Equatable {
    public var active_chat_id: UInt64?
    public var chat_title: String
    public var message_count: Int
    public var is_group_chat: Bool
    public var pinned_message: String?
}

public struct SemanticComposerNodeDTO: Codable, Sendable, Equatable {
    public var text: String
    public var is_editing: Bool
    public var has_reply_target: Bool
}

public struct SemanticInspectorNodeDTO: Codable, Sendable, Equatable {
    public var is_open: Bool
    public var profile_name: String
    public var shared_media_count: Int
}

public struct SemanticModalNodeDTO: Codable, Sendable, Equatable {
    public var modal_id: String
    public var title: String
}

public struct SemanticWorkspaceTreeDTO: Codable, Sendable, Equatable {
    public var timestamp_ms: Int64
    public var window_layout: String
    public var selected_chat_id: UInt64?
    public var composer_text: String
    public var active_modal: String?
    public var is_call_active: Bool
    public var window: SemanticWindowNodeDTO
    public var nav_rail: SemanticNavRailNodeDTO
    public var sidebar: SemanticSidebarNodeDTO
    public var stage: SemanticStageNodeDTO
    public var composer: SemanticComposerNodeDTO
    public var inspector: SemanticInspectorNodeDTO
    public var active_modal_node: SemanticModalNodeDTO?
    public var state_hash: String

    public var isValidHexHash: Bool {
        state_hash.count == 64 && state_hash.allSatisfy { $0.isHexDigit && !$0.isUppercase }
    }
}

public enum SemanticActionExecutionStatus: String, Codable, Sendable, Equatable {
    case Succeeded
    case Failed
    case Rejected
}

public struct SemanticActionReceiptDTO: Codable, Sendable, Equatable {
    public var action_id: String
    public var action_type: String
    public var success: Bool
    public var status: SemanticActionExecutionStatus
    public var error: String?
    public var timestamp_ms: Int64
    public var mutation_summary: JSONValue
    public var resulting_state_hash: String

    public var isValidResultingHash: Bool {
        resulting_state_hash.count == 64 && resulting_state_hash.allSatisfy { $0.isHexDigit && !$0.isUppercase }
    }
}

public enum SemanticTreeSnapshotReader: Sendable {
    @MainActor
    public static func readSnapshot(
        from app: XCUIApplication,
        fallbackFileURL: URL? = nil,
        timeout: TimeInterval = 5.0
    ) throws -> SemanticWorkspaceTreeDTO {
        let exportTrigger = app.buttons[FabushiAID.Root.exportSemanticTree]
        if exportTrigger.exists && exportTrigger.isHittable {
            exportTrigger.tap()
        }

        let probe = app.descendants(matching: .any)[FabushiAID.Root.semanticTree]
        if probe.waitForExistence(timeout: timeout),
           let rawJSON = probe.value as? String,
           !rawJSON.isEmpty,
           let data = rawJSON.data(using: .utf8)
        {
            return try JSONDecoder().decode(SemanticWorkspaceTreeDTO.self, from: data)
        }

        if let fileURL = fallbackFileURL,
           FileManager.default.fileExists(atPath: fileURL.path)
        {
            let data = try Data(contentsOf: fileURL)
            return try JSONDecoder().decode(SemanticWorkspaceTreeDTO.self, from: data)
        }

        throw NSError(
            domain: "SemanticTreeSnapshotReader",
            code: 404,
            userInfo: [
                NSLocalizedDescriptionKey: "SemanticWorkspaceTree JSON not found via accessibility element or export path"
            ]
        )
    }

    public static func decodeSnapshotData(_ data: Data) throws -> SemanticWorkspaceTreeDTO {
        try JSONDecoder().decode(SemanticWorkspaceTreeDTO.self, from: data)
    }

    public static func decodeActionReceiptData(_ data: Data) throws -> SemanticActionReceiptDTO {
        try JSONDecoder().decode(SemanticActionReceiptDTO.self, from: data)
    }
}
