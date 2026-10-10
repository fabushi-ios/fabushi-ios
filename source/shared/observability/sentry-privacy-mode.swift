import Foundation

enum SandPrivacyMode: Int, Equatable, Sendable {
    case unspecified = 0
    case noStorage = 1
    case noTraining = 2
    case usageDataTrainingAllowed = 3
    case usageCodebaseTrainingAllowed = 4
}

enum SandSentryPrivacyTier: String, Equatable, Sendable {
    case full
    case scrubbed
    case fatalMetadata = "fatal-metadata"
}

func sandSentryPrivacyTierForMode(_ mode: SandPrivacyMode?) -> SandSentryPrivacyTier {
    switch mode {
    case .usageDataTrainingAllowed, .usageCodebaseTrainingAllowed:
        return .full
    case .noTraining:
        return .scrubbed
    default:
        return .fatalMetadata
    }
}
