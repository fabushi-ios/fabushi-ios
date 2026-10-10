import Foundation

let SAND_MODEL_EXPERIMENT_NAME = "sand_model_selection"
let SAND_MODEL_EXPERIMENT_OPUS_MEDIUM_SELECTION = SandAgentModelSelection(
    modelId: "claude-opus-4-8",
    maxMode: true,
    parameters: [
        .init(id: "thinking", value: "true"),
        .init(id: "context", value: "1m"),
        .init(id: "effort", value: "medium"),
        .init(id: "fast", value: "false"),
    ]
)

struct SandModelExperimentState: Equatable, Sendable {
    enum Arm: String, Equatable, Sendable { case control, treatment }
    let active: Bool
    let arm: Arm
}

func readSandModelExperimentEnvOverride(
    _ env: [String: String] = ProcessInfo.processInfo.environment
) -> SandModelExperimentState? {
    let raw = env["SAND_MODEL_EXPERIMENT_OVERRIDE"]?.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
    if raw == "control" { return .init(active: true, arm: .control) }
    if raw == "treatment" || raw == "test" { return .init(active: true, arm: .treatment) }
    return nil
}

func resolveSandModelExperimentState(
    envOverride: SandModelExperimentState? = nil,
    groupName: String?,
    enabled: Bool
) -> SandModelExperimentState? {
    if let envOverride { return envOverride }
    guard groupName != nil else { return nil }
    return .init(active: true, arm: enabled ? .treatment : .control)
}
