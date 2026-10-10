import Foundation

/// iOS-side account departure cleanup.
///
/// The Rust Feature Host settles its own account boundary before an auth reply is
/// returned (session reset, account-scoped bots/groups/messages/remote-device
/// state, connector state, and provider warmup). This adapter therefore owns
/// only Coordinator-side state that lives outside that Rust boundary. Keeping
/// this responsibility narrow prevents a second account truth from forming in
/// Swift while still fencing account-scoped settings before the next slot is
/// adopted.
@MainActor
final class ProductionAccountTransitionCleanup {
    struct Dependencies {
        let clearAccountScope: @MainActor () -> Void
        let didClearAccountScope: @MainActor (_ previousSlot: String, _ nextSlot: String?) -> Void
    }

    private let dependencies: Dependencies

    init(dependencies: Dependencies) {
        self.dependencies = dependencies
    }

    func prepare(previousSlot: String?, nextSlot: String?) async {
        guard let previousSlot else { return }
        dependencies.clearAccountScope()
        dependencies.didClearAccountScope(previousSlot, nextSlot)
    }
}
