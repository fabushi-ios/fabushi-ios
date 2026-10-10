import Foundation

let STATSIG_CLIENT_KEY = "client-Bm4HJ0aDjXHQVsoACMREyLNxm5p6zzuzhO50MgtoT5D"
let STATSIG_LOG_EVENT_PROXY_URL = "https://api3.cursor.sh/tev1/v1"
let BOOTSTRAP_CACHE_FILENAME = "sand-statsig-bootstrap.json"

func sandStatsigNetworkUrlAllowed(_ url: String) -> Bool {
    url.contains("/rgstr")
}

func extractStatsigUser(_ config: String) -> [String: Any] {
    guard let data = config.data(using: .utf8),
          let object = try? JSONSerialization.jsonObject(with: data),
          let record = object as? [String: Any],
          let user = record["user"] as? [String: Any] else { return [:] }
    return user
}

func readStatsigBootstrapUserId(_ config: String) -> String? {
    extractStatsigUser(config)["userID"] as? String
}

func createCursorChecksum(
    machineId: String,
    nowMs: Int64 = Int64(Date().timeIntervalSince1970 * 1_000)
) -> String {
    let unixKiloSeconds = nowMs / 1_000_000
    var bytes: [UInt8] = [40,32,24,16,8,0].map { shift in
        UInt8((unixKiloSeconds >> shift) & 255)
    }
    var lastByte: UInt8 = 165
    for index in bytes.indices {
        bytes[index] = (bytes[index] ^ lastByte) &+ UInt8(index)
        lastByte = bytes[index]
    }
    return Data(bytes).base64EncodedString()
        .replacingOccurrences(of: "+", with: "-")
        .replacingOccurrences(of: "/", with: "_")
        .replacingOccurrences(of: "=", with: "") + machineId
}

struct StatsigBootstrapResult: Equatable, Sendable {
    var config: String? = nil
    var retryAfterMs: Int? = nil
}

func fetchStatsigBootstrap(
    backendUrl: String,
    getAccessToken: @Sendable (String) async throws -> String,
    getMachineId: @Sendable () async throws -> String,
    env: [String: String] = ProcessInfo.processInfo.environment,
    timeoutSeconds: TimeInterval = 30,
    session: URLSession = .shared
) async throws -> StatsigBootstrapResult {
    let accessToken: String?
    do { accessToken = try await getAccessToken(backendUrl) }
    catch {
        reportExperimentsDiagnostic(.init(kind: "bootstrap_anonymous", metadata: ["errorClass": errorLogTag(error)]))
        accessToken = nil
    }
    let machineId = try await getMachineId()
    guard let url = URL(string: "aiserver.v1.AnalyticsService/BootstrapStatsig", relativeTo: URL(string: backendUrl))?.absoluteURL else {
        return .init()
    }
    var request = URLRequest(url: url)
    request.httpMethod = "POST"
    request.timeoutInterval = timeoutSeconds
    request.httpBody = Data("{}".utf8)
    var headers = getSandBackendClientHeaders(env)
    headers["content-type"] = "application/json"
    headers["x-cursor-checksum"] = createCursorChecksum(machineId: machineId)
    headers["x-ghost-mode"] = "true"
    headers["x-request-id"] = UUID().uuidString
    if let accessToken { headers["authorization"] = "Bearer \(accessToken)" }
    if env["CURSOR_AGENT_CLI_LOCAL_MODE"] == "true" { headers["local-cli-mode"] = "true" }
    for (name,value) in headers { request.setValue(value, forHTTPHeaderField: name) }

    let (data,response) = try await session.data(for: request)
    guard let http = response as? HTTPURLResponse else { return .init() }
    guard (200..<300).contains(http.statusCode) else {
        let retry = RetryAfter.parseMilliseconds(http.value(forHTTPHeaderField: "retry-after"))
        return .init(retryAfterMs: retry)
    }
    guard let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
          let config = object["config"] as? String else { return .init() }
    return .init(config: config)
}

struct CachedStatsigBootstrap: Codable, Equatable, Sendable {
    let config: String
    let userId: String?
    var fetchedAtMs: Int64? = nil
}

func bootstrapCachePath(_ cacheDir: String) -> String {
    URL(fileURLWithPath: cacheDir).appendingPathComponent(BOOTSTRAP_CACHE_FILENAME).path
}

func loadCachedBootstrap(_ cacheDir: String) -> CachedStatsigBootstrap? {
    do {
        let path = bootstrapCachePath(cacheDir)
        guard FileManager.default.fileExists(atPath: path) else { return nil }
        let data = try Data(contentsOf: URL(fileURLWithPath: path))
        let value = try JSONDecoder().decode(CachedStatsigBootstrap.self, from: data)
        if let fetchedAtMs = value.fetchedAtMs, fetchedAtMs < 0 { return nil }
        return value
    } catch {
        reportExperimentsDiagnostic(.init(kind: "bootstrap_cache_read_failed", metadata: ["errorClass": errorLogTag(error)]))
        return nil
    }
}

func saveCachedBootstrap(_ cacheDir: String, cache: CachedStatsigBootstrap) {
    do {
        let data = try JSONEncoder().encode(cache)
        try writeFileAtomic(targetPath: bootstrapCachePath(cacheDir), data: data)
    } catch {
        reportExperimentsDiagnostic(.init(kind: "bootstrap_cache_write_failed", metadata: ["errorClass": errorLogTag(error)]))
    }
}
