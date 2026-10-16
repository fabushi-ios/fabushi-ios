import Foundation

let SAND_LOCAL_TOOL_ASK_TTL_MS: Int64 = 10 * 60 * 1_000
let SAND_LOCAL_TOOLS_DISABLED_MESSAGE = "Local tools are turned off. Use a remote runner or change the local execution permission before retrying."
let SAND_LOCAL_TOOLS_DENIED_MESSAGE = "The user declined this action on their computer. Do not retry it without a new user request."
let SAND_LOCAL_TOOLS_ASK_EXPIRED_MESSAGE = "The local-computer request expired without approval, so nothing ran."
let SAND_LOCAL_TOOLS_ASK_UNAVAILABLE_MESSAGE = "This conversation has no surface on which to ask for local-computer permission."
let SAND_LOCAL_TOOLS_UNDESCRIBABLE_MESSAGE = "The request could not be safely described for user approval, so it did not run."
let SAND_LOCAL_TOOLS_UNAPPROVED_MESSAGE = "The local-computer action was not approved, so nothing ran."
let SAND_LOCAL_TOOLS_ABANDONED_MESSAGE = "This exact local-computer action was already left unapproved for the task and will not be retried."
let SAND_LOCAL_TOOLS_STALE_TASK_MESSAGE = "The task moved on before local-computer approval arrived; stale work will not run."
let SAND_LOCAL_TOOLS_PREPARATORY_MESSAGE = "Preparatory local-computer access was skipped; approval applies to the user-visible action itself."
let SAND_LOCAL_TOOLS_TARGET_TOO_LARGE_MESSAGE = "The action is too large to present safely for approval; split it into smaller steps."

struct SandLocalToolRequest: Equatable, Sendable {
    let action: SandLocalToolAction
    let target: String
    var resourcePath: String? = nil
    var attachToResourcePath: String? = nil
    var outlivesScope = false
    var description: String? = nil
}

struct SandLocalToolApproval: Equatable, Sendable {
    let action: SandLocalToolAction
    let target: String
    var resourcePath: String? = nil
}

struct SandLocalToolScope: Equatable, Sendable {
    let agentId: String
    var toolCallId: String? = nil
    var action: SandLocalToolAction? = nil
    var directionEpoch: Int? = nil
}

struct SandLocalToolDecision: Equatable, Sendable {
    let allowed: Bool
    let reason: String
    var approvalId: String? = nil
}

protocol SandLocalToolGate: Sendable {
    func authorize(
        scope: SandLocalToolScope?,
        request: SandLocalToolRequest
    ) async -> SandLocalToolDecision
}

struct SandLocalToolPermissionDeniedError: Error, Equatable, Sendable {
    let reason: String
}

func authorizeLocalToolAction(
    gate: any SandLocalToolGate,
    scope: SandLocalToolScope?,
    request: SandLocalToolRequest
) async throws -> String? {
    let decision = await gate.authorize(scope: scope, request: request)
    guard decision.allowed else {
        throw SandLocalToolPermissionDeniedError(reason: decision.reason)
    }
    return decision.approvalId
}

private func normalizeLocalToolSeparators(_ path: String) -> String {
    var normalized = path.replacingOccurrences(of: "\\", with: "/")
    while normalized.count > 1 && normalized.hasSuffix("/") {
        normalized.removeLast()
    }
    return normalized
}

func normalizeLocalToolResourcePath(_ path: String?) -> String? {
    guard let path, !path.isEmpty else { return nil }
    return path.replacingOccurrences(of: "\\", with: "/")
}

func sandTerminalFilePath(_ terminalsFolder: String, shellId: String) -> String? {
    guard !shellId.isEmpty else { return nil }
    return "\(normalizeLocalToolSeparators(terminalsFolder))/\(shellId).txt"
}

func isTerminalFile(_ path: String, terminalsFolder: String) -> Bool {
    let folder = normalizeLocalToolSeparators(terminalsFolder)
    guard !folder.isEmpty else { return false }
    let normalized = normalizeLocalToolSeparators(path)
    let prefix = "\(folder)/"
    guard normalized.hasPrefix(prefix) else { return false }
    let remainder = String(normalized.dropFirst(prefix.count))
    return !remainder.isEmpty && !remainder.contains("/") && remainder.hasSuffix(".txt")
}

struct SandLocalExecMessageValue: Equatable, Sendable {
    var command: String? = nil
    var path: String? = nil
    var chars: String? = nil
    var isBackground = false
}

struct SandLocalExecMessage: Equatable, Sendable {
    let caseName: String?
    let value: SandLocalExecMessageValue
}

struct LocalExecServerMessage: Equatable, Sendable {
    let message: SandLocalExecMessage
}

func describeLocalExec(
    _ serverMessage: LocalExecServerMessage,
    terminalsFolder: String
) -> SandLocalToolRequest? {
    let message = serverMessage.message
    switch message.caseName {
    case "shellStreamArgs", "backgroundShellSpawnArgs":
        guard let command = message.value.command else { return nil }
        return SandLocalToolRequest(
            action: "run-command",
            target: command,
            resourcePath: terminalsFolder,
            outlivesScope: message.caseName == "backgroundShellSpawnArgs" || message.value.isBackground
        )
    case "forceBackgroundShellArgs":
        return SandLocalToolRequest(
            action: "run-command",
            target: "a command already running",
            attachToResourcePath: terminalsFolder,
            outlivesScope: true
        )
    case "writeShellStdinArgs":
        guard let chars = message.value.chars else { return nil }
        return SandLocalToolRequest(action: "send-input", target: chars)
    case "readArgs", "redactedReadArgs":
        guard let path = message.value.path else { return nil }
        return SandLocalToolRequest(
            action: "read-file",
            target: path,
            attachToResourcePath: isTerminalFile(path, terminalsFolder: terminalsFolder)
                ? terminalsFolder
                : nil
        )
    case "lsArgs":
        guard let path = message.value.path else { return nil }
        return SandLocalToolRequest(action: "list-directory", target: path)
    default:
        return nil
    }
}

func localToolApprovalCovers(
    _ approval: SandLocalToolApproval,
    request: SandLocalToolRequest
) -> Bool {
    if approval.action == request.action && approval.target == request.target {
        return true
    }
    let owned = normalizeLocalToolResourcePath(approval.resourcePath)
    let wanted = normalizeLocalToolResourcePath(request.attachToResourcePath)
    return owned != nil && owned == wanted
}
