import Foundation

typealias SandLocalToolPermission = String
typealias SandLocalToolAction = String

let SAND_LOCAL_TOOL_PERMISSIONS: [SandLocalToolPermission] = ["always", "ask", "never"]
let SAND_DEFAULT_LOCAL_TOOL_PERMISSION: SandLocalToolPermission = "ask"
let SAND_LOCAL_TOOL_ACTIONS: [SandLocalToolAction] = [
    "run-command",
    "send-input",
    "read-file",
    "list-directory",
    "write-file",
]
let SAND_LOCAL_TOOL_PERMISSION_RANK: [SandLocalToolPermission: Int] = [
    "never": 0,
    "ask": 1,
    "always": 2,
]

func isSandLocalToolAction(_ value: String?) -> Bool {
    guard let value else { return false }
    return SAND_LOCAL_TOOL_ACTIONS.contains(value)
}

func isSandLocalToolPermission(_ value: String?) -> Bool {
    guard let value else { return false }
    return SAND_LOCAL_TOOL_PERMISSIONS.contains(value)
}

func normalizeSandLocalToolPermission(_ value: String?) -> SandLocalToolPermission {
    isSandLocalToolPermission(value) ? value! : SAND_DEFAULT_LOCAL_TOOL_PERMISSION
}

func resolveSandLocalToolPermission(
    _ choice: SandLocalToolPermission,
    adminCeiling: SandLocalToolPermission? = nil
) -> SandLocalToolPermission {
    guard let adminCeiling,
          let choiceRank = SAND_LOCAL_TOOL_PERMISSION_RANK[choice],
          let ceilingRank = SAND_LOCAL_TOOL_PERMISSION_RANK[adminCeiling] else {
        return choice
    }
    return choiceRank <= ceilingRank ? choice : adminCeiling
}
