import Foundation

enum IOSWebViewNavigationDecision: Equatable, Sendable {
    case allowInView
    case openExternally
    case handToApp
    case reject
}

enum IOSWebViewPreloadPolicy {
    static func decision(
        for url: URL,
        trustedHosts: Set<String>
    ) -> IOSWebViewNavigationDecision {
        if IOSBrowserPreloadPolicy.isAppCallback(url) {
            return .handToApp
        }
        guard IOSBrowserPreloadPolicy.allowsInAppNavigation(url) else {
            return .reject
        }
        guard let host = url.host?.lowercased() else {
            return .reject
        }
        return trustedHosts.contains(host) || IOSBrowserPreloadPolicy.isAllowlistedIdentityProvider(hostname: host)
            ? .allowInView
            : .openExternally
    }

    static func shouldUseSystemPasskeyFlow(for url: URL) -> Bool {
        IOSBrowserPreloadPolicy.isAllowlistedIdentityProvider(hostname: url.host)
    }
}
