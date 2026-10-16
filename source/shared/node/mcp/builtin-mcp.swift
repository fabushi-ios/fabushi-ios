import Foundation

let BOX_COMPUTER_SERVER_NAME = "cursor-box-computer"
let BUILTIN_MCP_SERVER_NAMES: Set<String> = [BOX_COMPUTER_SERVER_NAME]
let DEFAULT_BOX_CONTAINER_NAME = "cursor-box-shared"
let LOCAL_DOCKER_HOST_SENTINEL = "local"

struct BoxComputerRuntime: Equatable, Sendable {
    let execution: String
    let remoteRunnerMcpUrl: String?
}

func resolveBoxComputerRuntime(
    boxMcpActive: Bool,
    remoteRunnerMcpUrl: String?
) -> BoxComputerRuntime {
    let normalized = remoteRunnerMcpUrl?.trimmingCharacters(in: .whitespacesAndNewlines)
    return .init(
        execution: "remote-runner",
        remoteRunnerMcpUrl: (normalized?.isEmpty == false) ? normalized : nil
    )
}

func getBuiltinMcpServers(
    _ runtime: BoxComputerRuntime
) -> [String: McpServerConfig] {
    guard let url = runtime.remoteRunnerMcpUrl else { return [:] }
    return [
        BOX_COMPUTER_SERVER_NAME: .http(
            url: url,
            headers: ["x-fabushi-capability": "remote-runner-computer"]
        )
    ]
}
