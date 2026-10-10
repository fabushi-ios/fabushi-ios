import Foundation

let SAND_BOX_SHELL_TOOL_NAME = "Shell"
let SAND_BOX_READ_TOOL_NAME = "Read"
let SAND_BOX_AWAIT_SHELL_TOOL_NAME = "AwaitShell"
let SAND_EXTERNAL_SHELL_TOOL_NAME = "ExternalShell"
let SAND_EXTERNAL_READ_TOOL_NAME = "ExternalRead"
let SAND_EXTERNAL_AWAIT_SHELL_TOOL_NAME = "AwaitExternalShell"
let SAND_DEFAULT_EXTERNAL_MACHINE_ID = "user-computer"

struct SandExternalMachine: Codable, Equatable, Sendable {
    let id: String
    let label: String
}

let SAND_USER_COMPUTER = SandExternalMachine(
    id: "user-computer",
    label: "the user's computer"
)

let SAND_EXTERNAL_MACHINES: [String: SandExternalMachine] = [
    "user-computer": SAND_USER_COMPUTER,
]

func resolveSandExternalMachine(
    _ id: String = SAND_DEFAULT_EXTERNAL_MACHINE_ID
) -> SandExternalMachine? {
    SAND_EXTERNAL_MACHINES[id]
}

struct SandDualSurfaceToolTelemetry: Equatable, Sendable {
    enum Surface: String, Equatable, Sendable {
        case box
        case external
    }

    let toolName: String
    let surface: Surface
}

func sandDualSurfaceToolTelemetry(
    _ toolName: String
) -> SandDualSurfaceToolTelemetry? {
    switch toolName {
    case SAND_BOX_SHELL_TOOL_NAME,
         SAND_BOX_READ_TOOL_NAME,
         SAND_BOX_AWAIT_SHELL_TOOL_NAME:
        return .init(toolName: toolName, surface: .box)
    case SAND_EXTERNAL_SHELL_TOOL_NAME,
         SAND_EXTERNAL_READ_TOOL_NAME,
         SAND_EXTERNAL_AWAIT_SHELL_TOOL_NAME:
        return .init(toolName: toolName, surface: .external)
    default:
        return nil
    }
}
