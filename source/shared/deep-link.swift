import Foundation

enum FabushiDeepLinkSource: String, Codable, Sendable {
    case customScheme
    case universalLink
}

enum FabushiDeepLinkRoute: Equatable, Sendable {
    case authComplete(attemptId: String, status: String)
    case agent(id: String)
    case section(String)
    case info(topic: String)
    case pluginAdd(id: String)
    case open
}

struct ParsedFabushiDeepLink: Equatable, Sendable {
    let route: FabushiDeepLinkRoute
    let source: FabushiDeepLinkSource
    let canonicalURL: URL
}

enum FabushiDeepLinkParser {
    static let customScheme = "fabushi"
    static let universalHost = "fabushi.app"
    static let maxLength = 2_048

    private static let sectionHosts: Set<String> = [
        "settings", "feedback", "about", "widgets", "onboarding",
    ]
    private static let allowedAuthStatuses: Set<String> = [
        "completed", "cancelled", "failed",
    ]

    static func parse(_ raw: String) -> ParsedFabushiDeepLink? {
        guard isSafeRawCandidate(raw) else { return nil }
        let lower = raw.lowercased()
        let source: FabushiDeepLinkSource
        if lower.hasPrefix("\(customScheme):") {
            source = .customScheme
        } else if lower.hasPrefix("https:") {
            source = .universalLink
        } else {
            return nil
        }

        guard let components = URLComponents(string: raw),
              components.user == nil,
              components.password == nil,
              components.port == nil,
              components.fragment == nil
        else { return nil }

        switch source {
        case .customScheme:
            guard components.scheme?.lowercased() == customScheme,
                  let host = components.host?.lowercased()
            else { return nil }
            return parseCustomScheme(host: host, components: components)
        case .universalLink:
            guard components.scheme?.lowercased() == "https",
                  components.host?.lowercased() == universalHost
            else { return nil }
            return parseUniversalLink(components)
        }
    }

    private static func parseCustomScheme(
        host: String,
        components: URLComponents
    ) -> ParsedFabushiDeepLink? {
        let path = pathComponents(components.path)
        switch host {
        case "auth":
            guard path == ["complete"] else { return nil }
            return parseAuthComplete(components: components, source: .customScheme)
        case "agent":
            guard path.count == 1, isSafeIdentifier(path[0], maxLength: 200) else { return nil }
            return ParsedFabushiDeepLink(
                route: .agent(id: path[0]),
                source: .customScheme,
                canonicalURL: canonicalURL(host: "agent", path: "/\(path[0])", queryItems: [])
            )
        case "app":
            return parseReferenceAppRoute(path: path, components: components, source: .customScheme)
        default:
            guard sectionHosts.contains(host), path.isEmpty, (components.queryItems ?? []).isEmpty else {
                return nil
            }
            return ParsedFabushiDeepLink(
                route: .section(host),
                source: .customScheme,
                canonicalURL: canonicalURL(host: host, path: "", queryItems: [])
            )
        }
    }

    private static func parseUniversalLink(_ components: URLComponents) -> ParsedFabushiDeepLink? {
        let path = pathComponents(components.path)
        guard path.first == "link" else { return nil }
        let rest = Array(path.dropFirst())

        if rest == ["auth", "complete"] {
            return parseAuthComplete(components: components, source: .universalLink)
        }
        if rest.count == 2, rest[0] == "agent", isSafeIdentifier(rest[1], maxLength: 200) {
            return ParsedFabushiDeepLink(
                route: .agent(id: rest[1]),
                source: .universalLink,
                canonicalURL: canonicalURL(host: "agent", path: "/\(rest[1])", queryItems: [])
            )
        }
        if rest.count == 1, sectionHosts.contains(rest[0]), (components.queryItems ?? []).isEmpty {
            return ParsedFabushiDeepLink(
                route: .section(rest[0]),
                source: .universalLink,
                canonicalURL: canonicalURL(host: rest[0], path: "", queryItems: [])
            )
        }
        if rest.first == "v1" {
            return parseReferenceAppRoute(path: rest, components: components, source: .universalLink)
        }
        return nil
    }

    private static func parseReferenceAppRoute(
        path: [String],
        components: URLComponents,
        source: FabushiDeepLinkSource
    ) -> ParsedFabushiDeepLink? {
        switch path {
        case ["v1", "plugin", "add"]:
            guard let query = exactQuery(components, allowed: ["id": nil]),
                  let pluginID = query["id"],
                  isNativeMarketplacePluginID(pluginID)
            else { return nil }
            return .init(
                route: .pluginAdd(id: pluginID),
                source: source,
                canonicalURL: canonicalURL(
                    host: "app",
                    path: "/v1/plugin/add",
                    queryItems: [URLQueryItem(name: "id", value: pluginID)]
                )
            )
        case ["v1", "open"]:
            guard exactQuery(components, allowed: [:]) != nil else { return nil }
            return .init(
                route: .open,
                source: source,
                canonicalURL: canonicalURL(host: "app", path: "/v1/open", queryItems: [])
            )
        case ["v1", "info"]:
            guard let query = exactQuery(components, allowed: ["topic": "deep-links"]),
                  query["topic"] == "deep-links"
            else { return nil }
            return .init(
                route: .info(topic: "deep-links"),
                source: source,
                canonicalURL: canonicalURL(
                    host: "app",
                    path: "/v1/info",
                    queryItems: [URLQueryItem(name: "topic", value: "deep-links")]
                )
            )
        default:
            return nil
        }
    }

    private static func isNativeMarketplacePluginID(_ value: String) -> Bool {
        if FabushiDesktopPolicy.isDeepLinkPluginID(value) {
            return true
        }
        guard value.count <= 64 else { return false }
        return value.range(
            of: #"^[a-z0-9]+(?:-[a-z0-9]+)*$"#,
            options: .regularExpression
        ) != nil
    }

    private static func parseAuthComplete(
        components: URLComponents,
        source: FabushiDeepLinkSource
    ) -> ParsedFabushiDeepLink? {
        let allowedNames: Set<String> = ["attemptId", "status"]
        var values: [String: String] = [:]
        for item in components.queryItems ?? [] {
            guard allowedNames.contains(item.name),
                  values[item.name] == nil,
                  let value = item.value
            else { return nil }
            values[item.name] = value
        }

        guard let attemptId = values["attemptId"],
              isSafeIdentifier(attemptId, minLength: 8, maxLength: 96)
        else { return nil }

        let status = (values["status"] ?? "completed").lowercased()
        guard allowedAuthStatuses.contains(status) else { return nil }
        return ParsedFabushiDeepLink(
            route: .authComplete(attemptId: attemptId, status: status),
            source: source,
            canonicalURL: canonicalURL(
                host: "auth",
                path: "/complete",
                queryItems: [
                    URLQueryItem(name: "attemptId", value: attemptId),
                    URLQueryItem(name: "status", value: status),
                ]
            )
        )
    }

    private static func exactQuery(
        _ components: URLComponents,
        allowed: [String: String?]
    ) -> [String: String]? {
        let items = components.queryItems ?? []
        guard items.count == allowed.count else { return nil }
        var result: [String: String] = [:]
        for item in items {
            guard allowed.keys.contains(item.name),
                  result[item.name] == nil,
                  let value = item.value
            else { return nil }
            if let required = allowed[item.name] ?? nil, required != value { return nil }
            result[item.name] = value
        }
        return result
    }

    private static func canonicalURL(
        host: String,
        path: String,
        queryItems: [URLQueryItem]
    ) -> URL {
        var components = URLComponents()
        components.scheme = customScheme
        components.host = host
        components.path = path
        components.queryItems = queryItems.isEmpty ? nil : queryItems
        precondition(components.url != nil)
        return components.url!
    }

    private static func pathComponents(_ path: String) -> [String] {
        path.split(separator: "/", omittingEmptySubsequences: true).map(String.init)
    }

    private static func isSafeIdentifier(
        _ value: String,
        minLength: Int = 1,
        maxLength: Int
    ) -> Bool {
        guard value.count >= minLength, value.count <= maxLength else { return false }
        return value.range(of: "^[A-Za-z0-9._-]+$", options: .regularExpression) != nil
    }

    private static func isSafeRawCandidate(_ raw: String) -> Bool {
        guard !raw.isEmpty,
              raw.utf8.count <= maxLength,
              !raw.contains("#"),
              !raw.contains("\\"),
              raw.utf8.allSatisfy({ $0 >= 33 && $0 <= 126 }),
              hasValidPercentEncoding(raw)
        else { return false }

        let queryStart = raw.firstIndex(of: "?") ?? raw.endIndex
        let pathSection = raw[..<queryStart]
        guard !pathSection.contains("%"),
              !pathSection.contains("/../"),
              !pathSection.contains("/./"),
              !pathSection.hasSuffix("/.."),
              !pathSection.hasSuffix("/.")
        else { return false }
        return true
    }

    private static func hasValidPercentEncoding(_ raw: String) -> Bool {
        let bytes = Array(raw.utf8)
        var index = 0
        while index < bytes.count {
            if bytes[index] == 37 {
                guard index + 2 < bytes.count,
                      isHex(bytes[index + 1]),
                      isHex(bytes[index + 2])
                else { return false }
                index += 3
            } else {
                index += 1
            }
        }
        return true
    }

    private static func isHex(_ byte: UInt8) -> Bool {
        (48...57).contains(byte) || (65...70).contains(byte) || (97...102).contains(byte)
    }
}
