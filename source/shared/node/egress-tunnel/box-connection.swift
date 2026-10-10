import Foundation

let EGRESS_TUNNEL_WS_PORT = 8790

struct BoxConnectionInfo: Equatable, Sendable {
    let baseUrl: String
    let token: String?
    let headers: [String: String]?
    let hasVncProxy: Bool

    init(
        baseUrl: String,
        token: String? = nil,
        headers: [String: String]? = nil,
        hasVncProxy: Bool = false
    ) {
        self.baseUrl = baseUrl
        self.token = token
        self.headers = headers
        self.hasVncProxy = hasVncProxy
    }
}

struct EgressTunnelConfig: Equatable, Sendable {
    let url: String
    let bearer: String
    let headers: [String: String]?
    let allowPrivateTargets: Bool
}

func deriveEgressTunnelWsUrl(_ baseUrl: String, podProxied: Bool) -> String? {
    guard var components = URLComponents(string: baseUrl),
          let scheme = components.scheme,
          var host = components.host else { return nil }

    if podProxied {
        var labels = host.split(separator: ".").map(String.init)
        guard !labels.isEmpty else { return nil }
        let first = labels[0]
        guard let regex = try? NSRegularExpression(pattern: #"-\d+$"#),
              regex.firstMatch(
                in: first,
                range: NSRange(first.startIndex..., in: first)
              ) != nil else {
            return nil
        }
        labels[0] = regex.stringByReplacingMatches(
            in: first,
            range: NSRange(first.startIndex..., in: first),
            withTemplate: "-\(EGRESS_TUNNEL_WS_PORT)"
        )
        host = labels.joined(separator: ".")
        components.host = host
    } else {
        components.port = EGRESS_TUNNEL_WS_PORT
    }

    components.scheme = (scheme == "https" || scheme == "wss") ? "wss" : "ws"
    components.path = "/"
    components.query = nil
    components.fragment = nil
    return components.url?.absoluteString
}

func boxConnectionToEgressConfig(_ info: BoxConnectionInfo) -> EgressTunnelConfig? {
    guard let token = info.token, !token.isEmpty,
          let url = deriveEgressTunnelWsUrl(info.baseUrl, podProxied: info.hasVncProxy) else {
        return nil
    }
    return .init(
        url: url,
        bearer: token,
        headers: info.headers,
        allowPrivateTargets: false
    )
}
