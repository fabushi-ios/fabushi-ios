import Foundation

enum ExternalURLPolicy {
    static let allowedSchemes: Set<String> = ["http", "https", "mailto", "obsidian", "tel"]

    static func parseAllowed(_ value: String) -> String? {
        guard !value.isEmpty,
              var components = URLComponents(string: value),
              let scheme = components.scheme?.lowercased(),
              allowedSchemes.contains(scheme)
        else { return nil }

        if scheme == "http" || scheme == "https" {
            stripForeignWebAuthTokens(&components)
        }

        return components.url?.absoluteString
    }

    private static func decodedParameterName(_ value: String) -> String {
        var result = value
        for _ in 0..<4 {
            guard let decoded = result.removingPercentEncoding, decoded != result else { break }
            result = decoded
        }
        while result.hasPrefix("?") {
            result.removeFirst()
        }
        return result
    }

    private static func isForeignWebAuthTokenPart(_ part: Substring) -> Bool {
        let encodedName = part.split(separator: "=", maxSplits: 1, omittingEmptySubsequences: false).first ?? ""
        let name = decodedParameterName(String(encodedName)).lowercased()
        return name.hasPrefix("tgwebauth") || name == "autologin_token"
    }

    private static func withoutForeignWebAuthTokenParams(_ encoded: String) -> String? {
        var removed = false
        let kept = encoded.split(separator: "&", omittingEmptySubsequences: false).filter { part in
            if isForeignWebAuthTokenPart(part) {
                removed = true
                return false
            }
            return true
        }
        return removed ? kept.map(String.init).joined(separator: "&") : nil
    }

    private static func withoutForeignWebAuthTokenFragment(_ encoded: String) -> String? {
        guard let question = encoded.firstIndex(of: "?") else {
            return withoutForeignWebAuthTokenParams(encoded)
        }
        let routePart = String(encoded[..<question])
        let paramsPart = String(encoded[encoded.index(after: question)...])
        let route = withoutForeignWebAuthTokenParams(routePart)
        let params = withoutForeignWebAuthTokenParams(paramsPart)
        guard route != nil || params != nil else { return nil }
        let nextRoute = route ?? routePart
        let nextParams = params ?? paramsPart
        return nextParams.isEmpty ? nextRoute : "\(nextRoute)?\(nextParams)"
    }

    private static func stripForeignWebAuthTokens(_ components: inout URLComponents) {
        if let query = components.percentEncodedQuery,
           let sanitized = withoutForeignWebAuthTokenParams(query) {
            components.percentEncodedQuery = sanitized.isEmpty ? nil : sanitized
        }
        if let fragment = components.percentEncodedFragment,
           let sanitized = withoutForeignWebAuthTokenFragment(fragment) {
            components.percentEncodedFragment = sanitized.isEmpty ? nil : sanitized
        }
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
