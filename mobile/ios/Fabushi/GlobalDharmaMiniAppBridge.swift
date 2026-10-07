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
    nonisolated static let globalDharmaId = "global-dharma"
    static let prayerWheelCapability = "local.prayer-wheel.start"
    static let prayerWheelLifetimeSku = "local-prayer-wheel.lifetime"
    static let prayerWheelLifetimeProductId = "prod.global-dharma.local-prayer-wheel.lifetime"
    static let prayerWheelLifetimeCNYMinor: Int64 = 108_000

    private static let mcpProtocol = "2025-06-18"
    private let bridge: IOSPreloadBridge
    private let session: URLSession

    init(bridge: IOSPreloadBridge, session: URLSession = .shared) {
        self.bridge = bridge
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
            throw MahayanaCoordinator.CoordinatorError.requestFailed("Mini App input must not be blank")
        }
        return try await platform(
            method: "POST",
            path: "/v1/marketplace/plugins/\(pluginId)/route",
            body: ["input": cleanInput]
        )
    }

    func listOfficialMcpTools(pluginId: String) async throws -> [[String: Any]] {
        try Self.requirePluginId(pluginId)
        let token = try await delegatedPluginToken(pluginId: pluginId)
        let endpoint = GlobalDharmaCommerceModel.resolvePlatformBaseURL()
            .appending(path: "/api/mcp/apps/\(pluginId)")
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
            throw MahayanaCoordinator.CoordinatorError.requestFailed(
                "Mini App MCP initialize did not return mcp-session-id"
            )
        }
        try Self.ensureNoMcpError(initialized.body, phase: "initialize")
        defer {
            Task {
                try? await self.mcpDelete(endpoint: endpoint, token: token, sessionId: sessionId)
            }
        }
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
        return ((listed.body?["result"] as? [String: Any])?["tools"] as? [[String: Any]]) ?? []
    }

    func callOfficialMcpTool(
        pluginId: String,
        name: String,
        arguments: [String: Any]
    ) async throws -> [String: Any] {
        try Self.requirePluginId(pluginId)
        guard Self.validToolName(name) else {
            throw MahayanaCoordinator.CoordinatorError.requestFailed("Invalid Mini App MCP tool name")
        }
        let token = try await delegatedPluginToken(pluginId: pluginId)
        let endpoint = GlobalDharmaCommerceModel.resolvePlatformBaseURL()
            .appending(path: "/api/mcp/apps/\(pluginId)")
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
            throw MahayanaCoordinator.CoordinatorError.requestFailed("Mini App MCP initialize did not return mcp-session-id")
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
            throw MahayanaCoordinator.CoordinatorError.requestFailed("Mini App MCP tool \(name) is not advertised by tools/list")
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
            throw MahayanaCoordinator.CoordinatorError.requestFailed("Mini App MCP tools/call did not return result")
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
            throw MahayanaCoordinator.CoordinatorError.requestFailed("Invalid entitlement capability")
        }
        return try await platform(
            method: "GET",
            path: "/v1/plugins/\(pluginId)/entitlements/\(capability)"
        )
    }

    func purchaseLifetimeTest(idempotencyKey: String) async throws -> [String: Any] {
        guard testCommerceEnabled else {
            throw MahayanaCoordinator.CoordinatorError.requestFailed("Production payment rail must use provider checkout; CI test purchase is disabled")
        }
        let cleanKey = idempotencyKey.trimmingCharacters(in: .whitespacesAndNewlines)
        guard (12...160).contains(cleanKey.count), !cleanKey.contains(where: \.isWhitespace) else {
            throw MahayanaCoordinator.CoordinatorError.requestFailed("Invalid purchase idempotency key")
        }
        let intent = try await platform(
            method: "POST",
            path: "/v1/miniapps/\(Self.globalDharmaId)/pay/intents",
            body: [
                "sku": Self.prayerWheelLifetimeSku,
                "rail": "web_provider",
                "idempotencyKey": cleanKey,
            ]
        )
        guard let paymentId = intent["paymentId"] as? String,
              UUID(uuidString: paymentId) != nil,
              intent["sku"] as? String == Self.prayerWheelLifetimeSku,
              Self.int64(intent["amount"]) == Self.prayerWheelLifetimeCNYMinor,
              intent["currency"] as? String == "CNY"
        else {
            throw MahayanaCoordinator.CoordinatorError.requestFailed(
                "Canonical CI payment intent drifted from the governed lifetime contract"
            )
        }
        let checkout = try await platform(
            method: "POST",
            path: "/v1/pay/intents/\(paymentId)/checkout",
            body: [String: Any]()
        )
        guard let payment = checkout["payment"] as? [String: Any],
              payment["paymentId"] as? String == paymentId,
              payment["status"] as? String == "succeeded"
        else {
            throw MahayanaCoordinator.CoordinatorError.requestFailed(
                "Canonical CI checkout did not settle the payment"
            )
        }
        return checkout
    }

    func restorePurchases() async throws -> [String: Any] {
        try await platform(method: "POST", path: "/v1/purchases/restore", body: [String: Any]())
    }

    func validateLifetimeCatalog(_ entitlement: [String: Any]) throws -> (allowed: Bool, reason: String, activeRails: [String]) {
        guard let access = entitlement["access"] as? [String: Any], access["protected"] as? Bool == true else {
            throw MahayanaCoordinator.CoordinatorError.requestFailed("Canonical service did not mark local.prayer-wheel.start protected")
        }
        let options = entitlement["purchaseOptions"] as? [[String: Any]] ?? []
        guard let lifetime = options.first(where: { ($0["sku"] as? String) == Self.prayerWheelLifetimeSku }) else {
            throw MahayanaCoordinator.CoordinatorError.requestFailed("Canonical lifetime SKU is missing")
        }
        let currency = lifetime["currency"] as? String
        let amount = Self.int64(lifetime["amount"])
        guard currency == "CNY", amount == Self.prayerWheelLifetimeCNYMinor else {
            throw MahayanaCoordinator.CoordinatorError.requestFailed("Server lifetime SKU drifted from the governed CNY 1080 contract")
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
            throw MahayanaCoordinator.CoordinatorError.requestFailed("Protected prayer-wheel capability is owned by Global Dharma")
        }
        let response = try await entitlement(pluginId: pluginId, capability: Self.prayerWheelCapability)
        guard let access = response["access"] as? [String: Any], access["protected"] as? Bool == true else {
            throw MahayanaCoordinator.CoordinatorError.requestFailed("Prayer-wheel capability is not marked protected by the canonical entitlement service")
        }
        guard access["allowed"] as? Bool == true else {
            let reason = access["reason"] as? String ?? "not_entitled"
            throw MahayanaCoordinator.CoordinatorError.requestFailed("本地转经轮尚未获得有效权益：\(reason)")
        }
    }

    private func delegatedPluginToken(pluginId: String) async throws -> String {
        try Self.requirePluginId(pluginId)
        // platform.request intentionally redacts bearer credentials. Ask the trusted
        // native Host to consume the Rust-owned account session and mint the exact,
        // five-minute Mini App credential without exposing the account token.
        let result = try await bridge.request(
            method: "feature.miniapp.delegatedToken",
            params: ["pluginId": pluginId]
        )
        guard let response = result.value as? [String: Any] else {
            throw MahayanaCoordinator.CoordinatorError.invalidResponse
        }
        return try Self.delegatedPluginCredential(from: response)
    }

    nonisolated static func delegatedPluginCredential(from response: [String: Any]) throws -> String {
        let token = (response["accessToken"] as? String)?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let tokenType = (response["tokenType"] as? String)?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let expiresIn = int64(response["expiresIn"])
        guard let token,
              token.count >= 24,
              token != "[stored by Mahayana]",
              tokenType == "Bearer",
              (1...300).contains(expiresIn)
        else {
            throw MahayanaCoordinator.CoordinatorError.requestFailed(
                "Fabushi did not issue a bounded delegated Mini App token"
            )
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
        let result = try await bridge.request(method: "platform.request", params: params)
        guard let response = result.value as? [String: Any] else {
            throw MahayanaCoordinator.CoordinatorError.invalidResponse
        }
        guard response["ok"] as? Bool == true else {
            let statusCode = Self.int64(response["statusCode"])
            let data = response["data"] ?? response["bodyText"] ?? ""
            throw MahayanaCoordinator.CoordinatorError.requestFailed("Fabushi platform request failed: \(method) \(path) -> HTTP \(statusCode) \(data)")
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
            throw MahayanaCoordinator.CoordinatorError.requestFailed("Mini App MCP HTTP \(status): \(String(text.prefix(800)))")
        }
        let returnedSession = http.value(forHTTPHeaderField: "Mcp-Session-Id")?.trimmingCharacters(in: .whitespacesAndNewlines).nonEmpty
        if !expectJSON || data.isEmpty { return MCPResponse(body: nil, sessionId: returnedSession ?? sessionId) }
        guard let object = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            throw MahayanaCoordinator.CoordinatorError.requestFailed("Mini App MCP returned non-JSON response")
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
        throw MahayanaCoordinator.CoordinatorError.requestFailed("Mini App MCP \(phase) failed: \(message)")
    }

    private nonisolated static func requirePluginId(_ pluginId: String) throws {
        guard validPluginId(pluginId) else {
            throw MahayanaCoordinator.CoordinatorError.requestFailed("Invalid Mini App id")
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