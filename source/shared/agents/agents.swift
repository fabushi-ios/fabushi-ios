import Foundation

enum SandAgentPurpose: String, CaseIterable, Codable, Sendable {
    case diskSaver = "disk-saver"
    case pluginAuth = "plugin-auth"
}

let SAND_AGENT_PURPOSES = SandAgentPurpose.allCases.map(\.rawValue)

func isSandAgentPurpose(_ value: Any) -> Bool {
    guard let value = value as? String else { return false }
    return SandAgentPurpose(rawValue: value) != nil
}

func sanitizeTemplateId(_ value: Any) -> String? {
    guard let value = value as? String,
          value.range(of: #"^[a-z0-9-]{1,64}$"#, options: .regularExpression) != nil
    else {
        return nil
    }
    return value
}

struct SandAgentActivity: Codable, Equatable, Sendable {
    let kind: String?
    let tool: String?
    let detail: String?
    let target: String?
    let callId: String?

    init(
        kind: String? = nil,
        tool: String? = nil,
        detail: String? = nil,
        target: String? = nil,
        callId: String? = nil
    ) {
        self.kind = kind
        self.tool = tool
        self.detail = detail
        self.target = target
        self.callId = callId
    }
}

func areAgentActivitiesEqual(
    _ left: SandAgentActivity?,
    _ right: SandAgentActivity?
) -> Bool {
    left == right
}

let SAND_DEFAULT_AGENT_NAME = "New Bot"
let LEGACY_SAND_DEFAULT_AGENT_NAME = "New Agent"

func isSandDefaultAgentName(_ name: String) -> Bool {
    let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
    return trimmed == SAND_DEFAULT_AGENT_NAME
        || trimmed == LEGACY_SAND_DEFAULT_AGENT_NAME
}

let GROUP_MAX_MEMBERS = 6
let MAX_AGENTS_PER_USER = 50
let SAND_AGENT_LIMIT_MESSAGE = "\(MAX_AGENTS_PER_USER) is the maximum"

struct SandAgentLimitError: LocalizedError, Equatable, Sendable {
    var errorDescription: String? { SAND_AGENT_LIMIT_MESSAGE }
}

func isSandAgentLimitError(_ error: Error) -> Bool {
    error.localizedDescription == SAND_AGENT_LIMIT_MESSAGE
}
