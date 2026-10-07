import Foundation

let ACCESS_BLOCKED_FAILURE_CODE = "sand-access-blocked"
let ACCESS_ONBOARDING_URL = URL(string: "https://fabushi.ombhrum.com/")!

enum AccessCoverSandAccessState: String, Equatable, Sendable {
    case checking
    case granted
    case unavailable
    case paymentRequired
    case unknown
}

enum AccessCoverSandAccessBlockReason: String, Equatable, Sendable {
    case none
    case teamPrivacyMode
    case teamSetupRequired
    case teamAccessRequired
    case notOffered
    case freeTrialAvailable
    case paywallIndividual
    case paywallTeamMember
    case paywallTeamAdmin
    case unspecified
}

struct AccessCoverSandAccess: Equatable, Sendable {
    let state: AccessCoverSandAccessState
    let reason: AccessCoverSandAccessBlockReason

    static let checking = Self(state: .checking, reason: .unspecified)
    static let unknown = Self(state: .unknown, reason: .unspecified)
}

struct AccessCoverCopy: Equatable, Sendable {
    let title: String
    let body: String
    let action: String?
}

struct AccessCoverGateInput: Equatable, Sendable {
    let rosterFailureCode: String?
    let hasReachedBox: Bool
    let isShowingRestoredRoster: Bool
    let isComputerRebuildLocked: Bool
}

enum AccessCoverModel {
    static func project(stateWire: Int, reasonWire: Int) -> AccessCoverSandAccess {
        let state: AccessCoverSandAccessState
        switch stateWire {
        case 1: state = .granted
        case 2: state = .unavailable
        case 3: state = .paymentRequired
        default: state = .unknown
        }

        let reason: AccessCoverSandAccessBlockReason
        switch reasonWire {
        case 1: reason = .none
        case 2: reason = .teamPrivacyMode
        case 3: reason = .teamSetupRequired
        case 4: reason = .teamAccessRequired
        case 5: reason = .notOffered
        case 6: reason = .freeTrialAvailable
        case 7: reason = .paywallIndividual
        case 8: reason = .paywallTeamMember
        case 9: reason = .paywallTeamAdmin
        default: reason = .unspecified
        }

        return .init(state: state, reason: reason)
    }

    static func noticeCopy(for access: AccessCoverSandAccess) -> AccessCoverCopy? {
        if access.state == .checking || access.state == .unknown || access.state == .granted {
            return nil
        }

        switch access.reason {
        case .teamPrivacyMode:
            return .init(
                title: "Your team's privacy mode blocks Fabushi",
                body: "Fabushi cannot run under the team's legacy privacy mode. Ask a team admin to change that policy.",
                action: "See Details"
            )
        case .teamSetupRequired:
            return .init(
                title: "Your team has not set up Fabushi yet",
                body: "A team admin must finish setup before members can send messages.",
                action: "See Details"
            )
        case .teamAccessRequired:
            return .init(
                title: "Your team has not granted this account Fabushi access",
                body: "A team admin can grant access from the team's settings.",
                action: "Request Access"
            )
        case .notOffered:
            return .init(
                title: "Fabushi is not available for this account",
                body: "There is no setup or purchase path available for this account.",
                action: nil
            )
        case .freeTrialAvailable:
            return .init(
                title: "Start a Fabushi trial to send messages",
                body: "This account can start a trial now.",
                action: "Start Trial"
            )
        case .paywallIndividual:
            return .init(
                title: "Fabushi requires an eligible plan",
                body: "Upgrade this account before sending messages.",
                action: "Upgrade"
            )
        case .paywallTeamMember:
            return .init(
                title: "Fabushi requires an eligible team seat",
                body: "Ask a team admin to move this account to an eligible seat.",
                action: "Request Access"
            )
        case .paywallTeamAdmin:
            return .init(
                title: "Fabushi requires an eligible team seat",
                body: "Move this account to an eligible seat before sending messages.",
                action: "Manage Seats"
            )
        case .none, .unspecified:
            break
        }

        if access.state == .unavailable {
            return .init(
                title: "Fabushi is not available for this account",
                body: "Sending stays disabled until this account is granted access.",
                action: "Check Access"
            )
        }
        if access.state == .paymentRequired {
            return .init(
                title: "Fabushi is not included in this plan",
                body: "Sending stays disabled until the account has access.",
                action: "Check Access"
            )
        }
        return nil
    }

    static func coverCopy(for access: AccessCoverSandAccess) -> AccessCoverCopy {
        noticeCopy(for: access) ?? .init(
            title: "Fabushi is not available on this account yet",
            body: "Check what this account needs on the web.",
            action: "Check Access"
        )
    }

    static func shouldShowCover(_ input: AccessCoverGateInput) -> Bool {
        input.rosterFailureCode == ACCESS_BLOCKED_FAILURE_CODE
            && !input.hasReachedBox
            && !input.isShowingRestoredRoster
            && !input.isComputerRebuildLocked
    }
}
