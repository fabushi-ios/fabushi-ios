import XCTest
@testable import Fabushi

final class AccessCoverParityTests: XCTestCase {
    func testSandAccessWireProjectionMatchesDesktopContract() {
        XCTAssertEqual(
            AccessCoverModel.project(stateWire: 1, reasonWire: 1),
            .init(state: .granted, reason: .none)
        )
        XCTAssertEqual(
            AccessCoverModel.project(stateWire: 2, reasonWire: 2),
            .init(state: .unavailable, reason: .teamPrivacyMode)
        )
        XCTAssertEqual(
            AccessCoverModel.project(stateWire: 3, reasonWire: 9),
            .init(state: .paymentRequired, reason: .paywallTeamAdmin)
        )
        XCTAssertEqual(
            AccessCoverModel.project(stateWire: 99, reasonWire: 99),
            .unknown
        )
    }

    func testAccessCoverRequiresCanonicalRosterFailureAndAllSuppressorsClear() {
        let visible = AccessCoverGateInput(
            rosterFailureCode: ACCESS_BLOCKED_FAILURE_CODE,
            hasReachedBox: false,
            isShowingRestoredRoster: false,
            isComputerRebuildLocked: false
        )
        XCTAssertTrue(AccessCoverModel.shouldShowCover(visible))

        XCTAssertFalse(AccessCoverModel.shouldShowCover(.init(
            rosterFailureCode: "network",
            hasReachedBox: false,
            isShowingRestoredRoster: false,
            isComputerRebuildLocked: false
        )))
        XCTAssertFalse(AccessCoverModel.shouldShowCover(.init(
            rosterFailureCode: ACCESS_BLOCKED_FAILURE_CODE,
            hasReachedBox: true,
            isShowingRestoredRoster: false,
            isComputerRebuildLocked: false
        )))
        XCTAssertFalse(AccessCoverModel.shouldShowCover(.init(
            rosterFailureCode: ACCESS_BLOCKED_FAILURE_CODE,
            hasReachedBox: false,
            isShowingRestoredRoster: true,
            isComputerRebuildLocked: false
        )))
        XCTAssertFalse(AccessCoverModel.shouldShowCover(.init(
            rosterFailureCode: ACCESS_BLOCKED_FAILURE_CODE,
            hasReachedBox: false,
            isShowingRestoredRoster: false,
            isComputerRebuildLocked: true
        )))
    }

    func testAccessCopyPreservesDesktopReasonSpecificActions() {
        XCTAssertEqual(
            AccessCoverModel.noticeCopy(for: .init(
                state: .paymentRequired,
                reason: .freeTrialAvailable
            ))?.action,
            "Start Trial"
        )
        XCTAssertEqual(
            AccessCoverModel.noticeCopy(for: .init(
                state: .paymentRequired,
                reason: .paywallIndividual
            ))?.action,
            "Upgrade"
        )
        XCTAssertEqual(
            AccessCoverModel.noticeCopy(for: .init(
                state: .unavailable,
                reason: .notOffered
            ))?.action,
            nil
        )
        XCTAssertNil(AccessCoverModel.noticeCopy(for: .checking))
        XCTAssertNil(AccessCoverModel.noticeCopy(for: .unknown))
    }

    func testFirstBoxGateIsStickyAndSuppressesRestoredAccessAndConnectivityFailures() {
        var state = FirstBoxGateState.initial
        state = FirstBoxGate.project(
            previous: state,
            roster: .init(
                loadState: .error,
                isShowingRestoredRoster: false,
                failureCode: nil,
                failureTransportKind: nil
            )
        )
        XCTAssertTrue(state.isAwaitingFirstBox)
        XCTAssertFalse(state.hasReachedBox)

        for suppressed in [
            FirstBoxRosterSnapshot(
                loadState: .error,
                isShowingRestoredRoster: true,
                failureCode: nil,
                failureTransportKind: nil
            ),
            FirstBoxRosterSnapshot(
                loadState: .error,
                isShowingRestoredRoster: false,
                failureCode: ACCESS_BLOCKED_FAILURE_CODE,
                failureTransportKind: nil
            ),
            FirstBoxRosterSnapshot(
                loadState: .error,
                isShowingRestoredRoster: false,
                failureCode: nil,
                failureTransportKind: "network"
            ),
            FirstBoxRosterSnapshot(
                loadState: .error,
                isShowingRestoredRoster: false,
                failureCode: nil,
                failureTransportKind: "dns"
            ),
        ] {
            XCTAssertFalse(
                FirstBoxGate.project(previous: .initial, roster: suppressed).isAwaitingFirstBox
            )
        }

        state = FirstBoxGate.project(
            previous: .initial,
            roster: .init(
                loadState: .ready,
                isShowingRestoredRoster: false,
                failureCode: nil,
                failureTransportKind: nil
            )
        )
        XCTAssertTrue(state.hasReachedBox)
        XCTAssertFalse(state.isAwaitingFirstBox)

        state = FirstBoxGate.project(
            previous: state,
            roster: .init(
                loadState: .error,
                isShowingRestoredRoster: false,
                failureCode: nil,
                failureTransportKind: nil
            )
        )
        XCTAssertTrue(state.hasReachedBox)
        XCTAssertFalse(state.isAwaitingFirstBox)
        XCTAssertEqual(FirstBoxGate.reset(), .initial)
    }
}
