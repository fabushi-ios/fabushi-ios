import Foundation

let SAND_DEFAULT_MODEL_CONFIG_NAME = "sand_default_model"
let SAND_AUTOMATIONS_MODEL_CONFIG_NAME = "sand_automations_model"
let SAND_MODEL_FILTER_CONFIG_NAME = "sand_model_filter"
let MAX_MODEL_ID_LENGTH = 128
let MAX_PARAMETERS = 16
let MAX_PARAMETER_VALUE_LENGTH = 64
let ROUTED_MODEL_IDS: Set<String> = ["default","premium","auto-low","auto-medium","auto-high","auto-smart"]

enum ModelConfigRejection: String, Equatable, Sendable {
    case identityUnhydrated = "identity_unhydrated"
    case malformed
    case invalidModelId = "invalid_model_id"
    case parametersOutOfBounds = "parameters_out_of_bounds"
    case duplicateParameter = "duplicate_parameter"
    case routedModelParameters = "routed_model_parameters"
}

struct SandModelConfigResolution: Equatable, Sendable {
    var selection: SandAgentModelSelection? = nil
    var rejection: ModelConfigRejection? = nil
}

func resolveSandDefaultModelConfig(
    raw: Any,
    hasHydratedStatsigUserId: Bool
) -> SandModelConfigResolution {
    guard hasHydratedStatsigUserId else { return .init(rejection: .identityUnhydrated) }
    guard let record = raw as? [String: Any],
          let modelId = record["modelId"] as? String,
          let maxMode = record["maxMode"] as? Bool,
          let rawParameters = record["parameters"] as? [[String: Any]] else {
        return .init(rejection: .malformed)
    }

    var parameters: [SandAgentModelParameter] = []
    for item in rawParameters {
        guard let id = item["id"] as? String, !id.isEmpty,
              let value = item["value"] as? String else { return .init(rejection: .malformed) }
        parameters.append(.init(id: id, value: value))
    }

    if modelId.isEmpty { return .init() }
    if modelId.count > MAX_MODEL_ID_LENGTH || modelId.contains(where: { $0.isWhitespace || $0.asciiValue.map { $0 < 32 || $0 == 127 } == true }) {
        return .init(rejection: .invalidModelId)
    }
    if parameters.count > MAX_PARAMETERS || parameters.contains(where: { $0.value.count > MAX_PARAMETER_VALUE_LENGTH }) {
        return .init(rejection: .parametersOutOfBounds)
    }
    var ids = Set<String>()
    for parameter in parameters {
        guard ids.insert(parameter.id).inserted else { return .init(rejection: .duplicateParameter) }
    }
    if ROUTED_MODEL_IDS.contains(modelId) && (maxMode || !parameters.isEmpty) {
        return .init(rejection: .routedModelParameters)
    }
    return .init(selection: .init(modelId: modelId, maxMode: maxMode, parameters: parameters))
}
