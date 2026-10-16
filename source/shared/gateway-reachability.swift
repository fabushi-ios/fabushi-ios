import Foundation

let CLOUD_AGENT_STORAGE_DISABLED = "CLOUD_AGENT_STORAGE_DISABLED"
let GATEWAY_NO_STORAGE_MESSAGE_MARKER =
    "sand box access blocked by privacy mode (no_storage)"
let GATEWAY_ACCESS_DENIED_MESSAGE_MARKER =
    "sand box access refused by backend access gate (access_denied)"
let SAND_BOX_BLOCKED = "SAND_BOX_BLOCKED"
let SAND_BOX_BLOCK_REASON_KEY = "sandBoxBlockReason"
let GATEWAY_BOX_BLOCKED_PREFIX = "sand box blocked by kill switch: "
private let UNIT_SEPARATOR_NEVER_IN_COPY = "\u{001f}"

struct SandBoxBlockedInfo: Equatable, Sendable {
    let reason: String
    let title: String
    let detail: String
}

func encodeSandBoxBlockedMessage(_ info: SandBoxBlockedInfo) -> String {
    GATEWAY_BOX_BLOCKED_PREFIX +
        [info.reason, info.title, info.detail].joined(separator: UNIT_SEPARATOR_NEVER_IN_COPY)
}

func hasSandBoxBlockedMarker(_ message: String) -> Bool {
    message.contains(GATEWAY_BOX_BLOCKED_PREFIX)
}

let SAND_CLIENT_PAUSE_REASON = "SAND_CLIENT_PAUSE"
let SAND_CLIENT_PAUSE_BLOCKED_MESSAGE = encodeSandBoxBlockedMessage(.init(
    reason: SAND_CLIENT_PAUSE_REASON,
    title: "",
    detail: ""
))

func findSandBoxBlockedMessage(_ error: Error?) -> String? {
    var current: NSError? = error.map { $0 as NSError }
    var seen = Set<ObjectIdentifier>()
    while let node = current {
        let identifier = ObjectIdentifier(node)
        guard seen.insert(identifier).inserted else { break }
        let message = node.localizedDescription
        if let range = message.range(of: GATEWAY_BOX_BLOCKED_PREFIX) {
            return String(message[range.lowerBound...])
        }
        current = node.userInfo[NSUnderlyingErrorKey] as? NSError
    }
    return nil
}
