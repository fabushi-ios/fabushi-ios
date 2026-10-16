import Foundation

let SAND_SUBAGENT_ID_PREFIX = "sand-subagent-"

func isSandSubagentId(_ id: String) -> Bool {
    id.hasPrefix(SAND_SUBAGENT_ID_PREFIX)
}
