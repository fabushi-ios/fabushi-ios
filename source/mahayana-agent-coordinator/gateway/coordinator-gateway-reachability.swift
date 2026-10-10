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

enum GatewayReachability {
    static func classifyBaseURL(_ url: URL) -> GatewayReachabilityKind {
        guard let host = url.host?.lowercased() else { return .unknown }
        if ["localhost", "127.0.0.1", "::1"].contains(host) { return .loopback }
        return .remote
    }

    static func classify(_ error: Error) -> (outcome: String, cause: String) {
        if let error = error as? URLError {
            switch error.code {
            case .notConnectedToInternet, .networkConnectionLost:
                return ("network", error.localizedDescription)
            case .timedOut:
                return ("timeout", error.localizedDescription)
            case .cannotFindHost, .dnsLookupFailed:
                return ("dns", error.localizedDescription)
            case .cannotConnectToHost:
                return ("connect", error.localizedDescription)
            default:
                return ("transport", error.localizedDescription)
            }
        }
        return ("transport", error.localizedDescription)
    }
}
