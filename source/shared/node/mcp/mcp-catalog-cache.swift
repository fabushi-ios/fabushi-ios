import Foundation

let CATALOG_CACHE_TTL_MS = 30_000

func isLoopbackHostname(_ hostname: String) -> Bool {
    let normalized = hostname.lowercased()
    return normalized == "localhost" || normalized == "127.0.0.1"
}

func validateAuthorizationUrl(
    _ authUrl: String,
    serverUrl: String? = nil
) -> String? {
    guard let parsed = URL(string: authUrl.trimmingCharacters(in: .whitespacesAndNewlines)),
          let scheme = parsed.scheme?.lowercased(),
          let host = parsed.host else { return nil }
    if scheme == "https" { return parsed.absoluteString }
    if scheme == "http", isLoopbackHostname(host),
       let serverUrl,
       let server = URL(string: serverUrl),
       let serverHost = server.host,
       isLoopbackHostname(serverHost) {
        return parsed.absoluteString
    }
    return nil
}
