import Foundation

/// iOS adaptation of Grok's platform-main account authorizer.
///
/// Authentication, credential validation, and the stable account identity are
/// owned by the Rust Feature Host. CoordinatorAccountRuntime calls this object
/// only after a successful, UI-safe Host auth reply has already been projected
/// to a stable slot. The iOS authorizer therefore does not create a second auth
/// source or read credentials; it only adopts/clears Coordinator-owned settings
/// scope for that settled Host identity.
@MainActor
final class IOSAccountAuthorizer {
    typealias ApplyAccountScope = @MainActor (_ slot: String?) -> Void
    typealias ApplyLocalHumanIdentity = @MainActor (_ slot: String?) -> Void

    private let applyAccountScope: ApplyAccountScope
    private let applyLocalHumanIdentity: ApplyLocalHumanIdentity

    init(
        applyAccountScope: @escaping ApplyAccountScope,
        applyLocalHumanIdentity: @escaping ApplyLocalHumanIdentity = { _ in }
    ) {
        self.applyAccountScope = applyAccountScope
        self.applyLocalHumanIdentity = applyLocalHumanIdentity
    }

    func authorizeSettledHostSlot(
        _ slot: String?,
        previousSlot _: String?
    ) -> CoordinatorAccountRuntime.Authorization {
        applyAccountScope(slot)
        applyLocalHumanIdentity(slot)
        return .ready(slot: slot)
    }
}
