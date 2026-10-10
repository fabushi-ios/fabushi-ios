import Foundation

let SAND_SUMMARIZATION_MODEL_ID = "gemini-2.5-flash"
let SAND_COMPUTER_USE_SUBAGENT_MODEL_ID = "claude-opus-4-8"

struct SandAgentModelParameter: Codable, Equatable, Sendable {
    let id: String
    let value: String
}

struct SandAgentModelSelection: Codable, Equatable, Sendable {
    let modelId: String
    let maxMode: Bool
    let parameters: [SandAgentModelParameter]
}

let SAND_COMPUTER_USE_MODEL_SELECTION = SandAgentModelSelection(
    modelId: SAND_COMPUTER_USE_SUBAGENT_MODEL_ID,
    maxMode: false,
    parameters: [
        .init(id: "thinking", value: "false"),
        .init(id: "effort", value: "low"),
    ]
)

func isSandAgentModelSelection(_ value: Any) -> Bool {
    guard let record = value as? [String: Any],
          let modelId = record["modelId"] as? String,
          !modelId.isEmpty,
          record["maxMode"] is Bool,
          let parameters = record["parameters"] as? [[String: Any]]
    else {
        return false
    }
    return parameters.allSatisfy { parameter in
        guard let id = parameter["id"] as? String,
              !id.isEmpty,
              parameter["value"] is String
        else {
            return false
        }
        return true
    }
}

func resolveComputerUseModelSelection(
    storedModel: SandAgentModelSelection?,
    overrideModel: SandAgentModelSelection?
) -> SandAgentModelSelection? {
    overrideModel ?? storedModel
}
