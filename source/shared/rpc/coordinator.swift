import Foundation

struct CoordinatorTranscriptEntry: Equatable, Sendable {
    let id: String
    let kind: String
    let payload: CoordinatorPayload
}

struct CoordinatorTranscriptWindowRequest: Equatable, Sendable {
    let id: String
    let beforeSequence: Int?
    let limit: Int?
}

struct CoordinatorTranscriptWindowResponse: Equatable, Sendable {
    let entries: [CoordinatorTranscriptEntry]
    let nextBeforeSequence: Int?
    let threadCounts: [String: Int]
}

enum CoordinatorMethodRegistry {
    static let methods: Set<String> = [
        "getAgentTranscriptWindow", "getAgentThread", "getAgentTranscriptTail", "openAgentTail",
        "sendPrompt", "interruptAgent", "promptAcceptanceStatus", "respondToWidget", "resolveAutoReviewApproval",
        "resolveLocalToolPermission", "dismissWidget", "submitSecret", "reactToMessage",
        "listAgents", "countAgents", "searchAgents", "searchMedia", "createAgent", "createGroup",
        "setGroupMembers", "updateAgent", "deleteAgents", "duplicateAgent", "kickstartAgent",
        "requestDiskSaverAudit", "broadcastToAgents", "getCloudAgentInfo", "getListenerIntegrations",
        "getListenerConnectUrl", "setAgentUnread", "setAgentHiddenFromSidebar",
        "setAgentNotificationsEnabled", "setAgentNotifyOnUpdates", "setAgentAvatarBytes",
        "getAgentAvatar", "getAgentWorkflows", "createAgentWorkflow", "updateAgentWorkflow",
        "setAgentWorkflowEnabled", "deleteAgentWorkflow", "runAgentWorkflowNow",
        "importAgentWorkflowText", "importAgentWorkflowUrl", "portAgentLocalSkills",
        "getConversationOutline", "skillsCatalog", "syncPluginSkills", "getPluginSyncStatus",
        "listRoutedMcpTools", "executeRoutedMcpTool", "getSkillPublishTargets", "publishSkill",
        "resyncPublishedSkill", "unpublishSkill", "getSubagents", "getAsyncTasks",
        "getForeverBoxStatus", "ensureForeverBox", "handBackForeverBox", "startTeachRecording",
        "stopTeachRecording", "getTeachRecordingStatus", "getTrays", "dismissTray", "clearTrays",
        "getAgentChannels", "connectChannel", "disconnectChannel", "refreshChannel",
        "getBoxSecretsStatus", "getAgentAutomations", "listAllAutomations", "isAgentNetworkEnabled",
        "isGlobalSearchEnabled", "isEgressTunnelAvailable", "getSharingState", "createRoomFromAgent",
        "createRoomInvite", "joinSharedRoom", "respondToRoomJoinRequest", "createSharedRoom",
        "addOwnAgentToSharedRoom", "removeOwnAgentFromSharedRoom", "setSharedRoomTyping",
        "leaveSharedRoom", "setAgentAutomationEnabled", "createAgentAutomation",
        "updateAgentAutomation", "deleteAgentAutomation", "runAgentAutomationNow"
    ]

    static func contains(_ method: String) -> Bool {
        methods.contains(method)
    }
}
