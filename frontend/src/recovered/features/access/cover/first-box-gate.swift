import Foundation

enum FirstBoxRosterLoadState: String, Equatable, Sendable {
    case loading
    case ready
    case error
}

struct FirstBoxRosterSnapshot: Equatable, Sendable {
    let loadState: FirstBoxRosterLoadState
    let isShowingRestoredRoster: Bool
    let failureCode: String?
    let failureTransportKind: String?
}

struct FirstBoxGateState: Equatable, Sendable {
    let isAwaitingFirstBox: Bool
    let hasReachedBox: Bool

    static let initial = Self(isAwaitingFirstBox: false, hasReachedBox: false)
}

enum FirstBoxGate {
    static func project(
        previous: FirstBoxGateState,
        roster: FirstBoxRosterSnapshot
    ) -> FirstBoxGateState {
        let hasReachedBox = previous.hasReachedBox || roster.loadState == .ready
        let connectivityFailure =
            roster.failureTransportKind == "network"
            || roster.failureTransportKind == "dns"
        let suppressed =
            roster.isShowingRestoredRoster
            || roster.failureCode == ACCESS_BLOCKED_FAILURE_CODE
            || connectivityFailure

        return .init(
            isAwaitingFirstBox:
                roster.loadState != .loading
                && !hasReachedBox
                && !suppressed,
            hasReachedBox: hasReachedBox
        )
    }

    static func reset() -> FirstBoxGateState {
        .initial
    }
}
