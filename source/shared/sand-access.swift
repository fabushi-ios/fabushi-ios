import Foundation

struct SandAccessState: Equatable, Sendable {
    let state: String
    let reason: String
}

let SAND_ACCESS_CHECKING = SandAccessState(state: "checking", reason: "unspecified")
let SAND_ACCESS_UNKNOWN = SandAccessState(state: "unknown", reason: "unspecified")
let SAND_ACCESS_BLOCK_REASONS: Set<String> = [
    "unspecified",
    "none",
    "teamPrivacyMode",
    "teamSetupRequired",
    "teamAccessRequired",
    "notOffered",
    "freeTrialAvailable",
    "paywallIndividual",
    "paywallTeamMember",
    "paywallTeamAdmin",
]

func isSandAccessBlockReason(_ value: String?) -> Bool {
    guard let value else { return false }
    return SAND_ACCESS_BLOCK_REASONS.contains(value)
}
