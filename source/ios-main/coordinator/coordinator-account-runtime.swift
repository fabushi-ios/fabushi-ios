import Foundation

/// iOS adaptation of Grok's coordinator account runtime.
///
/// Authentication remains owned by the Rust Feature Host. This runtime observes
/// only successful, UI-safe auth replies and projects them into one stable
/// account slot used by Coordinator-owned settings. It never reads or stores
/// credentials and never invents a second login state.
@MainActor
final class CoordinatorAccountRuntime {
    enum Authorization: Equatable, Sendable {
        case ready(slot: String?)
        case refused(slot: String?, reason: String)
    }

    enum AuthProjection: Equatable, Sendable {
        case loggedOut
        case loggedIn(slot: String)
        case invalidLoggedIn(reason: String)
    }

    typealias Authorize = @MainActor (_ slot: String?, _ previousSlot: String?) async -> Authorization

    private let authorize: Authorize
    private let cleanup: ProductionAccountTransitionCleanup
    private(set) var activeSlot: String?

    init(
        activeSlot: String? = nil,
        cleanup: ProductionAccountTransitionCleanup,
        authorize: @escaping Authorize
    ) {
        self.activeSlot = activeSlot
        self.cleanup = cleanup
        self.authorize = authorize
    }

    func transition(to nextSlot: String?) async -> Authorization {
        let previous = activeSlot
        if previous != nextSlot {
            await cleanup.prepare(previousSlot: previous, nextSlot: nextSlot)
        }
        let result = await authorize(nextSlot, previous)
        if case .ready(let slot) = result {
            activeSlot = slot
        }
        return result
    }

    /// Observe one coordinator reply. Non-auth calls, failed calls, and auth
    /// commands that do not carry settled auth state are deliberately ignored.
    @discardableResult
    func observeAuthReply(
        method: String,
        outcome: CoordinatorReplyOutcome
    ) async -> Authorization? {
        guard method.hasPrefix("feature.auth."),
              case .ok(let payload) = outcome,
              let projection = Self.authProjection(from: payload)
        else { return nil }

        switch projection {
        case .loggedOut:
            return await transition(to: nil)

        case .loggedIn(let slot):
            return await transition(to: slot)

        case .invalidLoggedIn(let reason):
            let previous = activeSlot
            if previous != nil {
                await cleanup.prepare(previousSlot: previous, nextSlot: nil)
            }
            _ = await authorize(nil, previous)
            activeSlot = nil
            return .refused(slot: nil, reason: reason)
        }
    }

    func reset() {
        activeSlot = nil
    }

    static func authProjection(from payload: CoordinatorPayload) -> AuthProjection? {
        guard let root = payload.foundationValue as? [String: Any] else { return nil }
        let auth = (root["auth"] as? [String: Any]) ?? root
        guard let loggedIn = auth["loggedIn"] as? Bool else { return nil }
        guard loggedIn else { return .loggedOut }

        if let slot = stableAccountSlot(auth: auth) {
            return .loggedIn(slot: slot)
        }

        return .invalidLoggedIn(reason: "logged-in auth reply has no stable account slot")
    }

    /// Keep this extractor identical to Rust FeatureHostController's
    /// stable_authenticated_account_id(): same key order, same user-first/root
    /// fallback, and the same String/Number value acceptance. Coordinator-side
    /// settings must never invent a different account identity (for example by
    /// falling back to email when the Host would refuse the account boundary).
    private static func stableAccountSlot(auth: [String: Any]) -> String? {
        let keys = [
            "principalId",
            "principal_id",
            "id",
            "userId",
            "user_id",
            "userNo",
            "user_no",
            "username",
        ]

        if let user = auth["user"] as? [String: Any],
           let slot = stableIdentityComponent(in: user, keys: keys) {
            return slot
        }
        return stableIdentityComponent(in: auth, keys: keys)
    }

    private static func stableIdentityComponent(
        in object: [String: Any],
        keys: [String]
    ) -> String? {
        for key in keys {
            guard let raw = object[key] else { continue }
            if let value = raw as? String {
                let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
                if !trimmed.isEmpty { return trimmed }
                continue
            }
            if let number = raw as? NSNumber,
               CFGetTypeID(number) != CFBooleanGetTypeID() {
                return number.stringValue
            }
        }
        return nil
    }
}
