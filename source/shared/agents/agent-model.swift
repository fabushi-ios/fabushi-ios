import Foundation

let SAND_DEFAULT_MODEL_ID = "grok-4.5"

let SAND_DEFAULT_MODEL_SELECTION = SandAgentModelSelection(
    modelId: SAND_DEFAULT_MODEL_ID,
    maxMode: true,
    parameters: [
        .init(id: "effort", value: "high"),
        .init(id: "fast", value: "true"),
    ]
)
