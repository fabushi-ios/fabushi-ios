import Foundation

enum ExternalURLPolicy {
    static let allowedSchemes: Set<String> = ["http", "https", "mailto", "obsidian", "tel"]

    static func parseAllowed(_ value: String) -> String? {
        guard !value.isEmpty,
              let components = URLComponents(string: value),
              let scheme = components.scheme?.lowercased(),
              allowedSchemes.contains(scheme),
              let url = components.url
        else { return nil }
        return url.absoluteString
    }

    static func parseServerAcceptedAuthExternalURL(
        _ value: String,
        expectedOrigin: String
    ) -> String? {
        guard let url = URLComponents(string: value.trimmingCharacters(in: .whitespacesAndNewlines)),
              let expected = URLComponents(string: expectedOrigin.trimmingCharacters(in: .whitespacesAndNewlines)),
              let scheme = url.scheme?.lowercased(),
              let expectedScheme = expected.scheme?.lowercased(),
              ["http", "https"].contains(scheme),
              ["http", "https"].contains(expectedScheme),
              url.user == nil,
              url.password == nil,
              expected.user == nil,
              expected.password == nil,
              let host = url.host?.lowercased(),
              let expectedHost = expected.host?.lowercased(),
              host == expectedHost,
              scheme == expectedScheme,
              effectivePort(url, scheme: scheme) == effectivePort(expected, scheme: expectedScheme),
              let accepted = url.url
        else { return nil }
        return accepted.absoluteString
    }

    private static func effectivePort(_ components: URLComponents, scheme: String) -> Int? {
        components.port ?? (scheme == "https" ? 443 : scheme == "http" ? 80 : nil)
    }

    static func isHTTP(_ value: String) -> Bool {
        guard let scheme = URLComponents(string: value)?.scheme?.lowercased() else { return false }
        return scheme == "http" || scheme == "https"
    }
}
