import Foundation

struct SandBoxHandoff: Codable, Equatable, Sendable {
    let requestId: String
}

enum SandBoxHandBackDecision: Equatable, Sendable {
    case none
    case resume(
        requestId: String,
        trigger: String,
        resolution: Resolution
    )

    enum Resolution: String, Equatable, Sendable {
        case dismissed
        case handedBack = "handed_back"
    }
}

func decideBoxHandBack(
    _ handoff: SandBoxHandoff?,
    trigger: String
) -> SandBoxHandBackDecision {
    guard let handoff else { return .none }
    return .resume(
        requestId: handoff.requestId,
        trigger: trigger,
        resolution: trigger == "dismissed" ? .dismissed : .handedBack
    )
}
