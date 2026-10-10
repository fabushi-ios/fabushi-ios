import Foundation

let AUTH_WATCH_POLL_INTERVAL_MS = 5_000
let AUTH_WATCH_TIMEOUT_MS = 15 * 60 * 1_000
let AUTH_WATCH_POLL_TIMEOUT_MS = 30_000

func authWatchKey(_ serverId: String, _ accountKey: String) -> String {
    "\(serverId)::\(accountKey)"
}

struct McpAuthWatchReference: Equatable, Sendable {
    let serverId: String
    let accountKey: String
    var serverName: String? = nil
    var serverIdentifier: String? = nil
    var requestingAgentId: String? = nil
}
