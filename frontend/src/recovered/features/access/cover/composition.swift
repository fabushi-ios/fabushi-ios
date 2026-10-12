import Foundation

struct AccessCoverCompositionState: Equatable, Sendable {
    let access: AccessCoverSandAccess
    let rosterFailureCode: String?
    let hasReachedBox: Bool
    let isShowingRestoredRoster: Bool
    let isComputerRebuildLocked: Bool
    let isLoading: Bool
    let isError: Bool
    let isVisible: Bool
}

enum AccessCoverComposition {
    static func project(
        access: AccessCoverSandAccess,
        roster: AccessRosterSnapshot,
        firstBox: FirstBoxGateState,
        isComputerRebuildLocked: Bool
    ) -> AccessCoverCompositionState {
        let isLoading =
            access.state == .checking
            || roster.loadState == .loading
            || roster.isFetching
        let isError =
            roster.loadState == .error
            || access.state == .unknown
        let gate = AccessCoverGateInput(
            rosterFailureCode: roster.failure?.code,
            hasReachedBox: firstBox.hasReachedBox,
            isShowingRestoredRoster: roster.isShowingRestoredRoster,
            isComputerRebuildLocked: isComputerRebuildLocked
        )
        return .init(
            access: access,
            rosterFailureCode: roster.failure?.code,
            hasReachedBox: firstBox.hasReachedBox,
            isShowingRestoredRoster: roster.isShowingRestoredRoster,
            isComputerRebuildLocked: isComputerRebuildLocked,
            isLoading: isLoading,
            isError: isError,
            isVisible: AccessCoverModel.shouldShowCover(gate)
        )
    }
}
