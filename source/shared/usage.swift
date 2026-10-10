import Foundation

enum SupportedDashboardAction: String, Sendable {
    case requestLimitIncrease
}

struct DashboardActionRequest: Equatable, Sendable {
    let action: SupportedDashboardAction
    let args: [String: String]
}

struct UsageActionResult: Equatable, Sendable {
    let ok: Bool
    let message: String?
}

struct SandWeeklyOnDemandUsage: Equatable, Sendable {
    let usedCents: Int
    let limitCents: Int
}

struct SandWeeklyUsage: Equatable, Sendable {
    let percentUsed: Double
    let nextResetMilliseconds: Int64?
    let hasNonZeroIncludedLimit: Bool
    let onDemand: SandWeeklyOnDemandUsage?
}

struct SandUsageOnDemand: Equatable, Sendable {
    let usedCents: Int
    let limitCents: Int?
    let resetTimestampMilliseconds: Int64?
}

enum SandUsageUpgradeAction: Equatable, Sendable {
    case openURL(String)
    case dashboard(action: SupportedDashboardAction, args: [String: String], successMessage: String?)
}

struct SandUsageUpgradeCTA: Equatable, Sendable {
    let label: String
    let disabled: Bool
    let action: SandUsageUpgradeAction
}

struct SandUsageSummary: Equatable, Sendable {
    let isEnterprise: Bool
    let sandUsagePercent: Double?
    let sandUsageResetTimestampMilliseconds: Int64?
    let hasAvailableUsage: Bool
    let isSandTrial: Bool
    let hasEndedSandTrial: Bool
    let hasNonZeroIncludedLimit: Bool
    let canCancelSandTrial: Bool
    let onDemand: SandUsageOnDemand?
    let upgradeCTA: SandUsageUpgradeCTA?
}

enum UsageContract {
    static let supportedDashboardActions: Set<SupportedDashboardAction> = [.requestLimitIncrease]
}
