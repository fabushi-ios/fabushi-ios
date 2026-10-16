import Foundation
import CryptoKit

let DEFAULT_CURSOR_BACKEND_URL = "https://api2.cursor.sh"
let PROD_AUTH_CLIENT_ID = "KbZUR41cY7W6zRSdpSUJ7I7mLYBKOCmB"
let DEV_AUTH_CLIENT_ID = "OzaBXLClY5CAGxNzUhQ2vlknpi07tGuE"
let TOKEN_REFRESH_LEEWAY_MS: Int64 = 5 * 60 * 1_000

struct JwtPayload: Equatable, Sendable {
    var email: String? = nil
    var exp: Double? = nil
    var sub: String? = nil
}

private func decodeBase64Url(_ raw: String) -> Data? {
    var base64 = raw.replacingOccurrences(of: "-", with: "+").replacingOccurrences(of: "_", with: "/")
    let remainder = base64.count % 4
    if remainder != 0 { base64 += String(repeating: "=", count: 4 - remainder) }
    return Data(base64Encoded: base64)
}

func parseJwtPayload(_ token: String) -> JwtPayload? {
    let parts = token.split(separator: ".", omittingEmptySubsequences: false)
    guard parts.count >= 2, !parts[1].isEmpty,
          let data = decodeBase64Url(String(parts[1])),
          let object = try? JSONSerialization.jsonObject(with: data),
          let record = object as? [String: Any] else { return nil }

    if record["email"] != nil && !(record["email"] is String) { return nil }
    if let exp = record["exp"], !(exp is NSNumber) { return nil }
    if record["sub"] != nil && !(record["sub"] is String) { return nil }

    return .init(
        email: record["email"] as? String,
        exp: (record["exp"] as? NSNumber)?.doubleValue,
        sub: record["sub"] as? String
    )
}

func accountCacheScope(_ accessToken: String) -> String {
    let source = parseJwtPayload(accessToken)?.sub ?? accessToken
    let digest = SHA256.hash(data: Data(source.utf8))
    return digest.map { String(format: "%02x", $0) }.joined()
}

func isTokenExpiringSoon(
    _ token: String,
    nowMs: Int64 = Int64(Date().timeIntervalSince1970 * 1_000)
) -> Bool {
    guard let exp = parseJwtPayload(token)?.exp else { return true }
    return Int64(exp * 1_000) - nowMs < TOKEN_REFRESH_LEEWAY_MS
}

func getAccessTokenExpiryMs(_ token: String) -> Int64? {
    guard let exp = parseJwtPayload(token)?.exp, exp.isFinite else { return nil }
    return Int64(exp * 1_000)
}

private func normalizedBackendUrl(_ raw: String) -> String {
    guard let url = URL(string: raw), let scheme = url.scheme, let host = url.host else { return raw }
    var components = URLComponents()
    components.scheme = scheme
    components.host = host
    components.port = url.port
    components.path = url.path.isEmpty ? "/" : url.path
    components.query = url.query
    return components.url?.absoluteString ?? raw
}

func getConfiguredBackendUrl(_ env: [String: String] = ProcessInfo.processInfo.environment) -> String {
    normalizedBackendUrl(env["SAND_BACKEND_URL"] ?? env["CURSOR_API_BASE_URL"] ?? DEFAULT_CURSOR_BACKEND_URL)
}

func getAuthClientId(
    _ backendUrl: String,
    env: [String: String] = ProcessInfo.processInfo.environment
) -> String {
    if let configured = env["SAND_AUTH_CLIENT_ID"], !configured.isEmpty { return configured }
    let hostname = URL(string: backendUrl)?.host?.lowercased() ?? ""
    let isDev = hostname == "localhost"
        || hostname == "127.0.0.1"
        || hostname.hasSuffix(".lclhst.build")
        || hostname == "dev-staging.cursor.sh"
    return isDev ? DEV_AUTH_CLIENT_ID : PROD_AUTH_CLIENT_ID
}

func isDevAuthBackend(_ backendUrl: String, env: [String: String] = ProcessInfo.processInfo.environment) -> Bool {
    getAuthClientId(backendUrl, env: env) != PROD_AUTH_CLIENT_ID
}

func shouldRefreshAccessToken(
    _ backendUrl: String,
    accessToken: String,
    env: [String: String] = ProcessInfo.processInfo.environment,
    nowMs: Int64 = Int64(Date().timeIntervalSince1970 * 1_000)
) -> Bool {
    isTokenExpiringSoon(accessToken, nowMs: nowMs) || isDevAuthBackend(backendUrl, env: env)
}
