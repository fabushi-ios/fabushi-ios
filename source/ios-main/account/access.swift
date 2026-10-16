import Foundation

let IOS_SAND_ACCESS_REQUEST_TIMEOUT_MS = 10_000

enum IOSCursorSandAccessAuthState: Equatable, Sendable {
    case loggingIn
    case loggedOut
    case loggedIn(authId: String?, email: String?)
}

struct IOSCursorSandAccessSnapshot: Equatable, Sendable {
    let identity: String?
    let access: SandAccessState
}

func iosSandAccessStateFromWire(_ state: Int) -> String {
    switch state {
    case 1: return "granted"
    case 2: return "unavailable"
    case 3: return "paymentRequired"
    default: return "unknown"
    }
}

func iosSandAccessBlockReasonFromWire(_ reason: Int) -> String {
    switch reason {
    case 1: return "none"
    case 2: return "teamPrivacyMode"
    case 3: return "teamSetupRequired"
    case 4: return "teamAccessRequired"
    case 5: return "notOffered"
    case 6: return "freeTrialAvailable"
    case 7: return "paywallIndividual"
    case 8: return "paywallTeamMember"
    case 9: return "paywallTeamAdmin"
    default: return "unspecified"
    }
}

func iosSandAccessFromWire(state: Int, blockReason: Int) -> SandAccessState {
    .init(
        state: iosSandAccessStateFromWire(state),
        reason: iosSandAccessBlockReasonFromWire(blockReason)
    )
}

func iosSandAccessFoundationValue(_ value: SandAccessState) -> [String: Any] {
    [
        "state": value.state,
        "reason": value.reason,
    ]
}

/// Native counterpart of Desktop createSandAccessReader/readSandAccessOnce.
///
/// This is a projection/cache fence only. IOSCursorAuthService remains the
/// credential owner and IOSCursorDashboardClient remains the network owner.
@MainActor
final class IOSCursorSandAccessReader {
    typealias AuthStateReader = @MainActor () async throws -> IOSCursorSandAccessAuthState
    typealias AccessReader = @MainActor () async throws -> SandAccessState

    private let getAuthState: AuthStateReader
    private let readAccess: AccessReader
    private var generation: UInt64 = 0
    private var answer: IOSCursorSandAccessSnapshot?

    init(
        getAuthState: @escaping AuthStateReader,
        readAccess: @escaping AccessReader
    ) {
        self.getAuthState = getAuthState
        self.readAccess = readAccess
    }

    func read() async -> SandAccessState {
        generation &+= 1
        if generation == 0 { generation = 1 }
        let attempt = generation
        let result = await readFreshSnapshot()
        guard attempt == generation else {
            return answerFor(identity: result.identity)
        }
        answer = result
        return result.access
    }

    func readFresh() async -> SandAccessState {
        let snapshot = await readFreshSnapshot()
        return snapshot.access
    }

    func readFreshSnapshot() async -> IOSCursorSandAccessSnapshot {
        let status: IOSCursorSandAccessAuthState
        do {
            status = try await getAuthState()
        } catch {
            return .init(identity: nil, access: SAND_ACCESS_UNKNOWN)
        }

        switch status {
        case .loggingIn:
            return .init(identity: nil, access: SAND_ACCESS_CHECKING)
        case .loggedOut:
            return .init(identity: nil, access: SAND_ACCESS_UNKNOWN)
        case .loggedIn(let authId, let email):
            let slot = authId ?? email
            guard let slot, !slot.isEmpty else {
                return .init(identity: nil, access: SAND_ACCESS_CHECKING)
            }
            do {
                return .init(identity: slot, access: try await readAccess())
            } catch {
                return .init(identity: slot, access: SAND_ACCESS_UNKNOWN)
            }
        }
    }

    private func answerFor(identity: String?) -> SandAccessState {
        guard let answer, answer.identity == identity else {
            return SAND_ACCESS_CHECKING
        }
        return answer.access
    }
}
