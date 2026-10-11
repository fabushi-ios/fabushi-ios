import Foundation

struct IOSBrowserAuthSessionToken: Equatable, Sendable {
    let generation: UInt64
    let attemptID: String
    let accountScope: String
    let originalOrigin: String
    let localNetworkCompatibility: IOSBrowserLocalNetworkCompatibility
}

@MainActor
final class IOSBrowserAuthSessionFence {
    private(set) var generation: UInt64 = 0
    private(set) var activeToken: IOSBrowserAuthSessionToken?

    func begin(
        attemptID: String,
        accountScope: String,
        url: URL
    ) -> IOSBrowserAuthSessionToken? {
        guard !attemptID.isEmpty,
              !accountScope.isEmpty,
              let plan = IOSWebViewPreloadEntrypoint.browserAuthPlan(for: url)
        else { return nil }
        generation = generation == UInt64.max ? 1 : generation + 1
        let token = IOSBrowserAuthSessionToken(
            generation: generation,
            attemptID: attemptID,
            accountScope: accountScope,
            originalOrigin: plan.originalOrigin,
            localNetworkCompatibility: plan.localNetworkCompatibility
        )
        activeToken = token
        return token
    }

    func cancel() {
        generation = generation == UInt64.max ? 1 : generation + 1
        activeToken = nil
    }

    func accepts(
        _ token: IOSBrowserAuthSessionToken,
        currentAttemptID: String?,
        currentAccountScope: String,
        callbackURL: URL? = nil
    ) -> Bool {
        guard activeToken == token,
              currentAttemptID == token.attemptID,
              currentAccountScope == token.accountScope
        else { return false }
        if let callbackURL {
            return IOSBrowserPreloadPolicy.isNativeOriginReturn(callbackURL)
        }
        return true
    }

    func settleCallback(
        _ token: IOSBrowserAuthSessionToken,
        currentAttemptID: String?,
        currentAccountScope: String,
        callbackURL: URL
    ) -> Bool {
        guard accepts(
            token,
            currentAttemptID: currentAttemptID,
            currentAccountScope: currentAccountScope,
            callbackURL: callbackURL
        ) else { return false }
        activeToken = nil
        return true
    }
}

enum IOSWebViewPreloadEntrypoint {
    static func navigationDecision(
        for url: URL,
        trustedHosts: Set<String>
    ) -> IOSWebViewNavigationDecision {
        IOSWebViewPreloadPolicy.decision(for: url, trustedHosts: trustedHosts)
    }

    static func browserAuthPlan(for url: URL) -> IOSBrowserAuthPresentationPlan? {
        IOSBrowserPreloadPolicy.authPresentationPlan(for: url)
    }
}
