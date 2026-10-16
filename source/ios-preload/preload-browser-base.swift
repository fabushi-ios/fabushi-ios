import Foundation

enum IOSBrowserPreloadPolicy {
    static let identityProviderHostnameSuffixes = [
        ".okta.com",
        ".okta-emea.com",
        ".oktapreview.com",
        ".duosecurity.com",
        ".login.microsoftonline.com",
        ".onelogin.com",
        ".auth0.com",
        ".pingidentity.com",
        ".rippling.com",
    ]

    static func isAllowlistedIdentityProvider(hostname: String?) -> Bool {
        guard let hostname else { return false }
        let host = hostname.lowercased()
        return identityProviderHostnameSuffixes.contains { suffix in
            host == String(suffix.dropFirst()) || host.hasSuffix(suffix)
        }
    }

    static func allowsLocalNetworkCompatibility(for url: URL) -> Bool {
        url.scheme?.lowercased() == "https"
            && isAllowlistedIdentityProvider(hostname: url.host)
    }

    static func allowsInAppNavigation(_ url: URL) -> Bool {
        guard let scheme = url.scheme?.lowercased() else { return false }
        return scheme == "https" || scheme == "http"
    }

    static func isAppCallback(_ url: URL) -> Bool {
        url.scheme?.lowercased() == "fabushi"
    }

    static let usesSystemPasskeyPresentation = true
}


enum IOSBrowserLocalNetworkCompatibility: Equatable, Sendable {
    case systemBrowserOwnedIdentityProvider
    case notRequired
}

struct IOSBrowserAuthPresentationPlan: Equatable, Sendable {
    let originalOrigin: String
    let localNetworkCompatibility: IOSBrowserLocalNetworkCompatibility
    let usesSystemAuthenticationSession: Bool
    let usesSystemDialogLifecycle: Bool
    let usesSystemPasskeyPresentation: Bool
    let callbackReturnsToApp: Bool
}

extension IOSBrowserPreloadPolicy {
    static func originString(for url: URL) -> String? {
        guard let scheme = url.scheme?.lowercased(),
              let host = url.host?.lowercased()
        else { return nil }
        let port = url.port.map { ":\($0)" } ?? ""
        return "\(scheme)://\(host)\(port)"
    }

    /// Native replacement for the Desktop browser preload contract.
    ///
    /// ASWebAuthenticationSession owns the browser permission/dialog stack,
    /// popup dismissal and enterprise-IdP navigation. Fabushi therefore does
    /// not inject a fake local-network permission grant into arbitrary pages.
    /// The allowlist is retained to classify IdPs that required the Desktop
    /// embedded-webview compatibility shim.
    static func authPresentationPlan(for url: URL) -> IOSBrowserAuthPresentationPlan? {
        guard allowsInAppNavigation(url),
              url.user == nil,
              url.password == nil,
              let origin = originString(for: url)
        else { return nil }

        return IOSBrowserAuthPresentationPlan(
            originalOrigin: origin,
            localNetworkCompatibility: isAllowlistedIdentityProvider(hostname: url.host)
                ? .systemBrowserOwnedIdentityProvider
                : .notRequired,
            usesSystemAuthenticationSession: true,
            usesSystemDialogLifecycle: true,
            usesSystemPasskeyPresentation: true,
            callbackReturnsToApp: true
        )
    }

    static func isNativeOriginReturn(_ url: URL) -> Bool {
        guard isAppCallback(url),
              url.user == nil,
              url.password == nil,
              url.port == nil,
              url.host?.lowercased() == "auth"
        else { return false }
        return url.pathComponents.filter { $0 != "/" && !$0.isEmpty } == ["complete"]
    }
}
