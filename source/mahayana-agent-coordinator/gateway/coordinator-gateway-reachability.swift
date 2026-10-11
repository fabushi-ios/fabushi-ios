import Foundation

enum GatewayReachabilityKind: String, Equatable, Sendable {
    case loopback
    case remote
    case unknown
}

struct GatewayReachabilityReport: Equatable, Sendable {
    let outcome: String
    let latencyMilliseconds: Int
    let baseURLKind: GatewayReachabilityKind
    let httpStatus: Int?
    let causeSummary: String?
}

enum GatewayReachabilityOutcome: Equatable, Sendable {
    case reachable
    case noStorage
    case boxBlocked
    case accessDenied
    case refused
    case dns
    case timeout
    case network
    case http(Int)

    var retryable: Bool {
        switch self {
        case .refused, .dns, .timeout, .network:
            return true
        case .http(let status):
            return (500...599).contains(status)
        default:
            return false
        }
    }

    var transportKind: String {
        switch self {
        case .reachable: return "reachable"
        case .noStorage: return "no_storage"
        case .boxBlocked: return "box_blocked"
        case .accessDenied: return "access_denied"
        case .refused: return "refused"
        case .dns: return "dns"
        case .timeout: return "timeout"
        case .network: return "network"
        case .http(let status):
            return (500...599).contains(status) ? "http_5xx" : "network"
        }
    }
}

enum GatewayReachability {
    static func classifyBaseURL(_ url: URL) -> GatewayReachabilityKind {
        guard let host = url.host?.lowercased() else { return .unknown }
        if ["localhost", "127.0.0.1", "::1"].contains(host) { return .loopback }
        return .remote
    }

    static func outcome(forHTTPStatus status: Int) -> GatewayReachabilityOutcome? {
        if status >= 500 { return .http(status) }
        if status == 401 || status == 403 { return .accessDenied }
        return nil
    }

    static func classifyOutcome(_ error: Error) -> GatewayReachabilityOutcome? {
        guard let error = error as? URLError else { return nil }
        switch error.code {
        case .notConnectedToInternet, .networkConnectionLost:
            return .network
        case .timedOut:
            return .timeout
        case .cannotFindHost, .dnsLookupFailed:
            return .dns
        case .cannotConnectToHost:
            return .refused
        default:
            return .network
        }
    }

    static func isRetryableTransportKind(_ transportKind: String) -> Bool {
        ["refused", "dns", "timeout", "network", "http_5xx"]
            .contains(transportKind)
    }

    static func classify(_ error: Error) -> (outcome: String, cause: String) {
        if let outcome = classifyOutcome(error) {
            return (outcome.transportKind, error.localizedDescription)
        }
        return ("transport", error.localizedDescription)
    }
}
