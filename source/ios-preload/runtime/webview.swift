import Foundation

enum IOSWebViewPreloadEntrypoint {
    static func navigationDecision(
        for url: URL,
        trustedHosts: Set<String>
    ) -> IOSWebViewNavigationDecision {
        IOSWebViewPreloadPolicy.decision(for: url, trustedHosts: trustedHosts)
    }
}
