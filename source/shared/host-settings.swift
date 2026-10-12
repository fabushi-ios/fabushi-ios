import Foundation

struct SandNotificationConfig: Codable, Equatable, Sendable {
    let isEnabled: Bool
    let allowedApps: [String]
    let minIntervalMs: Int
    let maxPerWindow: Int
    let windowMs: Int
}

let SAND_DISABLED_NOTIFICATION_CONFIG = SandNotificationConfig(
    isEnabled: false,
    allowedApps: [],
    minIntervalMs: 5_000,
    maxPerWindow: 10,
    windowMs: 5 * 60_000
)
