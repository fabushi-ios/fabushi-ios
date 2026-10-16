import Foundation

enum SandPrReviewDestination: String, Codable, Equatable, Sendable {
    case github
    case graphite
    case reviewCursor
}

struct SandPrReviewPreferences: Equatable, Sendable {
    let user: SandPrReviewDestination?
    let team: SandPrReviewDestination?
}

let NO_SAND_PR_REVIEW_PREFERENCES = SandPrReviewPreferences(user: nil, team: nil)
