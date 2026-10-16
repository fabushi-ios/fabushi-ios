import Foundation

let SAND_ACCESS_BLOCKED_CAUSES = ["wrong_account_suspected", "seat_revoked", "plan_expired", "no_plan", "unknown"]
let SAND_ONBOARDING_STEP_NAMES = ["meet", "computer-demo", "jobs", "tools", "create", "hand-off"]
let SAND_QUEUED_FLUSH_CAUSE_CODES = [
    "nonceMismatch": "SAND-E0703",
    "capabilityUnavailable": "SAND-E0704",
    "hostRejected": "SAND-E0705",
    "superseded": "SAND-E0706",
    "ackExpired": "SAND-E0707",
]
let SAND_CLIENT_PERSISTENCE_SLICES = [
    "client-meta.account-slot",
    "composer-drafts",
    "host-settings.onboarding",
    "roster.agent-avatars",
    "roster.last-roster",
    "selection.last-agent",
    "send-journal",
    "sidebar.last-sections",
    "transcript.replicas",
    "ui-agent-refs",
    "ui-layout",
    "other",
]
private let SAND_CLIENT_PERSISTENCE_SLICE_SET = Set(SAND_CLIENT_PERSISTENCE_SLICES)

func isSandClientPersistenceSlice(_ value: String?) -> Bool {
    guard let value else { return false }
    return SAND_CLIENT_PERSISTENCE_SLICE_SET.contains(value)
}
