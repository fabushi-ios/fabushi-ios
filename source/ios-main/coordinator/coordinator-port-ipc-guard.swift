import Foundation

struct CoordinatorPortAccessContext: Equatable, Sendable {
    let requestedSceneID: String
    let trustedSceneID: String?
    let isForeground: Bool
}

enum CoordinatorPortAccessError: LocalizedError, Equatable {
    case untrustedRequester

    var errorDescription: String? {
        "Coordinator access is only available from the active Fabushi scene."
    }
}

enum CoordinatorPortAccessGuard {
    static func isTrusted(_ context: CoordinatorPortAccessContext) -> Bool {
        context.isForeground
            && !context.requestedSceneID.isEmpty
            && context.trustedSceneID == context.requestedSceneID
    }

    static func requireTrusted(_ context: CoordinatorPortAccessContext) throws {
        guard isTrusted(context) else { throw CoordinatorPortAccessError.untrustedRequester }
    }
}
