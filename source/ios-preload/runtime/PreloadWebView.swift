import Foundation

enum IOSWebSurfaceDecision: Equatable, Sendable {
    case inAppMiniApp(URL)
    case externalBrowser(URL)
    case appCallback(URL)
    case blocked(String)
}

enum PreloadWebSurfacePolicy {
    static func decide(_ url: URL) -> IOSWebSurfaceDecision {
        guard let scheme = url.scheme?.lowercased() else {
            return .blocked("URL has no scheme")
        }
        switch scheme {
        case "fabushi":
            return .appCallback(url)
        case "https":
            if url.host?.hasSuffix("fabushi.app") == true {
                return .inAppMiniApp(url)
            }
            return .externalBrowser(url)
        case "http":
            if ["127.0.0.1", "localhost", "::1"].contains(url.host?.lowercased() ?? "") {
                return .blocked("desktop loopback web surfaces are not exposed on iOS")
            }
            return .externalBrowser(url)
        default:
            return .blocked("unsupported iOS web surface scheme: \(scheme)")
        }
    }
}
