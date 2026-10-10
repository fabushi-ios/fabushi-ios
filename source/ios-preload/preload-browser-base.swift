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
