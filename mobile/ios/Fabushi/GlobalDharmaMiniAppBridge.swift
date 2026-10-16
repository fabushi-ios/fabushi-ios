import Foundation

struct GlobalDharmaInstalledBot: Equatable, Sendable {
    let id: String
    let name: String
    let description: String
    let miniAppId: String
    let menuButtonText: String
}

@MainActor
final class GlobalDharmaMiniAppBridge {
    static let globalDharmaId = "global-dharma"
    static let prayerWheelCapability = "local.prayer-wheel.start"
    static let prayerWheelLifetimeSku = "local-prayer-wheel.lifetime"
    static let prayerWheelLifetimeProductId = "prod.global-dharma.local-prayer-wheel.lifetime"
    static let prayerWheelLifetimeCNYMinor: Int64 = 108_000

    private static let apiBase = URL(string: "https://api.ombhrum.com")!
    private static let mcpProtocol = "2025-06-18"
    private let host: MahayanaHost
    private let session: URLSession

    init(host: MahayanaHost, session: URLSession = .shared) {
        self.host = host
        self.session = session
    }

    var testCommerceEnabled: Bool {
        #if DEBUG
        let environment = ProcessInfo.processInfo.environment
        return environment["GITHUB_ACTIONS"] == "true"
            || environment["FABUSHI_FEATURE_HOST_TEST"] == "1"
            || environment["FABUSHI_CI_ACCOUNT_SESSION_FILE"]?.isEmpty == false
        #else
        return false
        #endif
    }

    func installedMiniAppBots() async throws -> [GlobalDharmaInstalledBot] {
        let response = try await platform(method: "GET", path: "/v1/marketplace/added")
        let apps = response["apps"] as? [[String: Any]] ?? []
        return apps.compactMap { manifest in
            guard let bot = manifest["bot"] as? [String: Any],
                  let botId = bot["id"] as? String,
                  !botId.isEmpty
            else { return nil }
            let pluginId = (manifest["id"] as? String)
                ?? (manifest["pluginId"] as? String)
                ?? ""
            guard Self.validPluginId(pluginId) else { return nil }
            let menu = bot["menuButton"] as? [String: Any]
            let action = menu?["action"] as? String
            let menuMiniAppId = menu?["miniAppId"] as? String
            let miniAppId = action == "open-miniapp" && Self.validPluginId(menuMiniAppId ?? "")
                ? (menuMiniAppId ?? pluginId)
                : pluginId
            return GlobalDharmaInstalledBot(
                id: botId,
                name: (bot["displayName"] as? String)
                    ?? (manifest["title"] as? String)
                    ?? botId,
                description: (bot["description"] as? String)
                    ?? (manifest["description"] as? String)
                    ?? "",
                miniAppId: miniAppId,
                menuButtonText: (menu?["text"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines).nonEmpty
                    ?? "打开应用"
            )
        }
    }

    func routeInput(pluginId: String, input: String) async throws -> [String: Any] {
        try Self.requirePluginId(pluginId)
        let cleanInput = input.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !cleanInput.isEmpty else {
            throw MahayanaHost.HostError.requestFailed("Mini App input must not be blank")
        }
        return try await platform(
            method: "POST",
            path: "/v1/marketplace/plugins/\(pluginId)/route",
            body: ["input": cleanInput]
        )
    }

    func callOfficialMcpTool(
        pluginId: String,
        name: String,
        arguments: [String: Any]
    ) async throws -> [String: Any] {
        try Self.requirePluginId(pluginId)
        guard Self.validToolName(name) else {
            throw MahayanaHost.HostError.requestFailed("Invalid Mini App MCP tool name")
        }
        let token = try await delegatedPluginToken(pluginId: pluginId)
        let endpoint = Self.apiBase.appending(path: "/api/mcp/apps/\(pluginId)")
        let initialize: [String: Any] = [
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": [
                "protocolVersion": Self.mcpProtocol,
                "capabilities": [String: Any](),
                "clientInfo": ["name": "fabushi-ios-miniapp-host", "version": "1.0.0"],
            ],
        ]
        let initialized = try await mcpPost(
            endpoint: endpoint,
            token: token,
            sessionId: nil,
            payload: initialize,
            expectJSON: true
        )
        guard let sessionId = initialized.sessionId, !sessionId.isEmpty else {
            throw MahayanaHost.HostError.requestFailed("Mini App MCP initialize did not return mcp-session-id")
        }
        try Self.ensureNoMcpError(initialized.body, phase: "initialize")
        defer { Task { try? await self.mcpDelete(endpoint: endpoint, token: token, sessionId: sessionId) } }

        _ = try await mcpPost(
            endpoint: endpoint,
            token: token,
            sessionId: sessionId,
            payload: [
                "jsonrpc": "2.0",
                "method": "notifications/initialized",
                "params": [String: Any](),
            ],
            expectJSON: false
        )

        let listed = try await mcpPost(
            endpoint: endpoint,
            token: token,
            sessionId: sessionId,
            payload: [
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/list",
                "params": [String: Any](),
            ],
            expectJSON: true
        )
        try Self.ensureNoMcpError(listed.body, phase: "tools/list")
        let tools = ((listed.body?["result"] as? [String: Any])?["tools"] as? [[String: Any]]) ?? []
        guard tools.contains(where: { ($0["name"] as? String) == name }) else {
            throw MahayanaHost.HostError.requestFailed("Mini App MCP tool \(name) is not advertised by tools/list")
        }

        let called = try await mcpPost(
            endpoint: endpoint,
            token: token,
            sessionId: sessionId,
            payload: [
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": ["name": name, "arguments": arguments],
            ],
            expectJSON: true
        )
        try Self.ensureNoMcpError(called.body, phase: "tools/call")
        guard let result = (called.body?["result"] as? [String: Any]) else {
            throw MahayanaHost.HostError.requestFailed("Mini App MCP tools/call did not return result")
        }
        try await enforceProtectedHostRequest(pluginId: pluginId, result: result)
        return result
    }

    func entitlement(
        pluginId: String = GlobalDharmaMiniAppBridge.globalDharmaId,
        capability: String = GlobalDharmaMiniAppBridge.prayerWheelCapability
    ) async throws -> [String: Any] {
        try Self.requirePluginId(pluginId)
        guard capability.range(of: #"^[a-z0-9][a-z0-9_.-]{1,127}$"#, options: .regularExpression) != nil else {
            throw MahayanaHost.HostError.requestFailed("Invalid entitlement capability")
        }
        return try await platform(
            method: "GET",
            path: "/v1/plugins/\(pluginId)/entitlements/\(capability)"
        )
    }

    func purchaseLifetimeTest(idempotencyKey: String) async throws -> [String: Any] {
        guard testCommerceEnabled else {
            throw MahayanaHost.HostError.requestFailed("Production payment rail must use provider checkout; CI test purchase is disabled")
        }
        let cleanKey = idempotencyKey.trimmingCharacters(in: .whitespacesAndNewlines)
        guard (12...160).contains(cleanKey.count), !cleanKey.contains(where: \.isWhitespace) else {
            throw MahayanaHost.HostError.requestFailed("Invalid purchase idempotency key")
        }
        return try await platform(
            method: "POST",
            path: "/v1/plugins/\(Self.globalDharmaId)/commerce/purchase",
            body: [
                "sku": Self.prayerWheelLifetimeSku,
                "idempotencyKey": cleanKey,
            ]
        )
    }

    func restorePurchases() async throws -> [String: Any] {
        try await platform(method: "POST", path: "/v1/purchases/restore", body: [String: Any]())
    }

    func validateLifetimeCatalog(_ entitlement: [String: Any]) throws -> (allowed: Bool, reason: String, activeRails: [String]) {
        guard let access = entitlement["access"] as? [String: Any], access["protected"] as? Bool == true else {
            throw MahayanaHost.HostError.requestFailed("Canonical service did not mark local.prayer-wheel.start protected")
        }
        let options = entitlement["purchaseOptions"] as? [[String: Any]] ?? []
        guard let lifetime = options.first(where: { ($0["sku"] as? String) == Self.prayerWheelLifetimeSku }) else {
            throw MahayanaHost.HostError.requestFailed("Canonical lifetime SKU is missing")
        }
        let currency = lifetime["currency"] as? String
        let amount = Self.int64(lifetime["amount"])
        guard currency == "CNY", amount == Self.prayerWheelLifetimeCNYMinor else {
            throw MahayanaHost.HostError.requestFailed("Server lifetime SKU drifted from the governed CNY 1080 contract")
        }
        let rails = lifetime["activeRails"] as? [String] ?? []
        return (
            allowed: access["allowed"] as? Bool == true,
            reason: access["reason"] as? String ?? "unknown",
            activeRails: rails
        )
    }

    nonisolated static func resultText(_ result: [String: Any]) -> String {
        if let content = result["content"] as? [[String: Any]] {
            let text = content.compactMap { item -> String? in
                guard (item["type"] as? String) == "text" else { return nil }
                return (item["text"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines).nonEmpty
            }.joined(separator: "\n")
            if !text.isEmpty { return text }
        }
        if let data = try? JSONSerialization.data(withJSONObject: result, options: [.prettyPrinted, .sortedKeys]),
           let text = String(data: data, encoding: .utf8) {
            return text
        }
        return "WebMCP Tool 已完成"
    }

    private func enforceProtectedHostRequest(pluginId: String, result: [String: Any]) async throws {
        guard let structured = result["structuredContent"] as? [String: Any],
              let hostRequest = structured["hostRequest"] as? [String: Any],
              (hostRequest["capability"] as? String) == Self.prayerWheelCapability
        else { return }
        guard pluginId == Self.globalDharmaId else {
            throw MahayanaHost.HostError.requestFailed("Protected prayer-wheel capability is owned by Global Dharma")
        }
        let response = try await entitlement(pluginId: pluginId, capability: Self.prayerWheelCapability)
        guard let access = response["access"] as? [String: Any], access["protected"] as? Bool == true else {
            throw MahayanaHost.HostError.requestFailed("Prayer-wheel capability is not marked protected by the canonical entitlement service")
        }
        guard access["allowed"] as? Bool == true else {
            let reason = access["reason"] as? String ?? "not_entitled"
            throw MahayanaHost.HostError.requestFailed("本地转经轮尚未获得有效权益：\(reason)")
        }
    }

    private func delegatedPluginToken(pluginId: String) async throws -> String {
        let response = try await platform(
            method: "POST",
            path: "/v1/auth/plugin-token",
            body: [
                "pluginId": pluginId,
                "deviceId": "fabushi-ios-miniapp-host",
                "scopes": ["miniapp:\(pluginId)"],
            ]
        )
        guard let token = (response["accessToken"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines), token.count >= 24 else {
            throw MahayanaHost.HostError.requestFailed("Fabushi did not issue a delegated Mini App token")
        }
        return token
    }

    private func platform(method: String, path: String, body: [String: Any]? = nil) async throws -> [String: Any] {
        var params: [String: Any] = [
            "method": method,
            "path": path,
            "authenticated": true,
        ]
        if let body { params["body"] = body }
        let result = try await host.request(method: "platform.request", params: params)
        guard let response = result.value as? [String: Any] else {
            throw MahayanaHost.HostError.invalidResponse
        }
        guard response["ok"] as? Bool == true else {
            let statusCode = Self.int64(response["statusCode"])
            let data = response["data"] ?? response["bodyText"] ?? ""
            throw MahayanaHost.HostError.requestFailed("Fabushi platform request failed: \(method) \(path) -> HTTP \(statusCode) \(data)")
        }
        if let data = response["data"] as? [String: Any] { return data }
        if let array = response["data"] as? [[String: Any]] { return ["items": array] }
        if let text = response["data"] as? String,
           let data = text.data(using: .utf8),
           let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any] {
            return object
        }
        return [:]
    }

    private struct MCPResponse {
        let body: [String: Any]?
        let sessionId: String?
    }

    private func mcpPost(
        endpoint: URL,
        token: String,
        sessionId: String?,
        payload: [String: Any],
        expectJSON: Bool
    ) async throws -> MCPResponse {
        var request = URLRequest(url: endpoint)
        request.httpMethod = "POST"
        request.timeoutInterval = 30
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.setValue("application/json, text/event-stream", forHTTPHeaderField: "Accept")
        request.setValue(Self.mcpProtocol, forHTTPHeaderField: "MCP-Protocol-Version")
        if let sessionId { request.setValue(sessionId, forHTTPHeaderField: "Mcp-Session-Id") }
        request.httpBody = try JSONSerialization.data(withJSONObject: payload)
        let (data, response) = try await session.data(for: request)
        guard let http = response as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
            let status = (response as? HTTPURLResponse)?.statusCode ?? 0
            let text = String(data: data, encoding: .utf8) ?? ""
            throw MahayanaHost.HostError.requestFailed("Mini App MCP HTTP \(status): \(String(text.prefix(800)))")
        }
        let returnedSession = http.value(forHTTPHeaderField: "Mcp-Session-Id")?.trimmingCharacters(in: .whitespacesAndNewlines).nonEmpty
        if !expectJSON || data.isEmpty { return MCPResponse(body: nil, sessionId: returnedSession ?? sessionId) }
        guard let object = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            throw MahayanaHost.HostError.requestFailed("Mini App MCP returned non-JSON response")
        }
        return MCPResponse(body: object, sessionId: returnedSession ?? sessionId)
    }

    private func mcpDelete(endpoint: URL, token: String, sessionId: String) async throws {
        var request = URLRequest(url: endpoint)
        request.httpMethod = "DELETE"
        request.timeoutInterval = 5
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        request.setValue("application/json, text/event-stream", forHTTPHeaderField: "Accept")
        request.setValue(Self.mcpProtocol, forHTTPHeaderField: "MCP-Protocol-Version")
        request.setValue(sessionId, forHTTPHeaderField: "Mcp-Session-Id")
        _ = try await session.data(for: request)
    }

    private nonisolated static func ensureNoMcpError(_ body: [String: Any]?, phase: String) throws {
        guard let error = body?["error"] as? [String: Any] else { return }
        let message = error["message"] as? String ?? String(describing: error)
        throw MahayanaHost.HostError.requestFailed("Mini App MCP \(phase) failed: \(message)")
    }

    private nonisolated static func requirePluginId(_ pluginId: String) throws {
        guard validPluginId(pluginId) else {
            throw MahayanaHost.HostError.requestFailed("Invalid Mini App id")
        }
    }

    private nonisolated static func validPluginId(_ value: String) -> Bool {
        value.range(of: #"^[a-z0-9][a-z0-9-]{1,63}$"#, options: .regularExpression) != nil
    }

    private nonisolated static func validToolName(_ value: String) -> Bool {
        value.range(of: #"^[A-Za-z0-9_.-]{1,120}$"#, options: .regularExpression) != nil
    }

    private nonisolated static func int64(_ value: Any?) -> Int64 {
        if let value = value as? Int64 { return value }
        if let value = value as? Int { return Int64(value) }
        if let value = value as? NSNumber { return value.int64Value }
        if let value = value as? String { return Int64(value) ?? 0 }
        return 0
    }
}

private extension String {
    var nonEmpty: String? { isEmpty ? nil : self }
}