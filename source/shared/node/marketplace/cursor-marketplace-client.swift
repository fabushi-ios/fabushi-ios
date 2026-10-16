import Foundation

let CURSOR_MARKETPLACE_REQUEST_TIMEOUT_MS = 12_000

func bestEffortToken(
    _ getAccessToken: @Sendable () async throws -> String?
) async -> String? {
    do {
        guard let token = try await getAccessToken(), !token.isEmpty else { return nil }
        return token
    } catch {
        return nil
    }
}

struct MarketplaceRequestContext: Sendable {
    let baseURL: URL
    let headers: [String: String]
    let timeoutSeconds: TimeInterval
}

func createMarketplaceRequestContext(
    getAccessToken: @Sendable () async throws -> String?,
    getMachineId: (@Sendable () async throws -> String)? = nil,
    env: [String: String] = ProcessInfo.processInfo.environment,
    uuid: @Sendable () -> String = { UUID().uuidString.lowercased() }
) async -> MarketplaceRequestContext? {
    let backend = getConfiguredBackendUrl(env)
    guard let baseURL = URL(string: backend) else { return nil }

    var headers = getSandBackendClientHeaders(env)
    headers["x-ghost-mode"] = "true"
    headers["x-request-id"] = uuid()

    if let getMachineId {
        do {
            headers["x-cursor-checksum"] = createCursorChecksum(machineId: try await getMachineId())
        } catch {
            // Marketplace lookup remains best-effort when device identity is unavailable.
        }
    }

    if let token = await bestEffortToken(getAccessToken) {
        headers["authorization"] = "Bearer \(token)"
    }

    return .init(
        baseURL: baseURL,
        headers: headers,
        timeoutSeconds: TimeInterval(CURSOR_MARKETPLACE_REQUEST_TIMEOUT_MS) / 1_000
    )
}

struct MarketplaceDashboardClient: Sendable {
    let context: MarketplaceRequestContext

    func makeRequest(
        path: String,
        body: Data,
        contentType: String = "application/connect+proto"
    ) -> URLRequest? {
        let relative = path.hasPrefix("/") ? String(path.dropFirst()) : path
        guard let url = URL(string: relative, relativeTo: context.baseURL)?.absoluteURL else { return nil }
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.httpBody = body
        request.timeoutInterval = context.timeoutSeconds
        request.setValue(contentType, forHTTPHeaderField: "content-type")
        for (name, value) in context.headers {
            request.setValue(value, forHTTPHeaderField: name)
        }
        return request
    }

    func send(
        path: String,
        body: Data,
        contentType: String = "application/connect+proto",
        session: URLSession = .shared
    ) async throws -> (Data, HTTPURLResponse) {
        guard let request = makeRequest(path: path, body: body, contentType: contentType) else {
            throw URLError(.badURL)
        }
        let (data, response) = try await session.data(for: request)
        guard let http = response as? HTTPURLResponse else {
            throw URLError(.badServerResponse)
        }
        return (data, http)
    }
}

func createDashboardClient(
    getAccessToken: @Sendable () async throws -> String?,
    getMachineId: (@Sendable () async throws -> String)? = nil,
    env: [String: String] = ProcessInfo.processInfo.environment,
    uuid: @Sendable () -> String = { UUID().uuidString.lowercased() }
) async -> MarketplaceDashboardClient? {
    guard let context = await createMarketplaceRequestContext(
        getAccessToken: getAccessToken,
        getMachineId: getMachineId,
        env: env,
        uuid: uuid
    ) else { return nil }
    return .init(context: context)
}
