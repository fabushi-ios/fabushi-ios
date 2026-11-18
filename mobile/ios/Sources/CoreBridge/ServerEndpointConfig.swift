import Foundation

/// Canonical production and configurable endpoint configuration matching `gpui_telegram_client::ClientConfig`.
public struct ServerEndpointConfig: Codable, Sendable, Equatable {
    /// Live production cloud backend (`http://34.46.77.133:8080`).
    public static let defaultProductionURL = "http://34.46.77.133:8080"

    public var serverURL: String
    public var language: String

    public init(
        serverURL: String = Self.defaultProductionURL,
        language: String = "en-US"
    ) {
        self.serverURL = Self.normalize(serverURL)
        self.language = language
    }

    public static func resolve(
        environment: [String: String] = ProcessInfo.processInfo.environment,
        persistedURL: String? = nil
    ) -> ServerEndpointConfig {
        let envOverride = ["FABUSHI_SERVER_URL", "GPUI_TELEGRAM_SERVER_URL"]
            .compactMap { environment[$0]?.trimmingCharacters(in: .whitespacesAndNewlines) }
            .first(where: { !$0.isEmpty })

        let url = envOverride ?? persistedURL ?? defaultProductionURL
        let lang = environment["GPUI_TELEGRAM_LOCALE"]?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return ServerEndpointConfig(
            serverURL: url,
            language: (lang?.isEmpty == false ? lang! : "en-US")
        )
    }

    public static func normalize(_ raw: String) -> String {
        var trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.isEmpty {
            return defaultProductionURL
        }
        if !trimmed.contains("://") {
            trimmed = "http://\(trimmed)"
        }
        while trimmed.hasSuffix("/") {
            trimmed.removeLast()
        }
        return trimmed
    }

    public var webSocketURL: String {
        if serverURL.hasPrefix("https://") {
            return "wss://" + serverURL.dropFirst("https://".count) + "/ws"
        }
        if serverURL.hasPrefix("http://") {
            return "ws://" + serverURL.dropFirst("http://".count) + "/ws"
        }
        return "\(serverURL)/ws"
    }
}

public struct HostPingResponse: Codable, Sendable, Equatable {
    public let pong: Bool
    public let nonce: UInt64
    public let timestamp: Int64
    public let version: String?
    public let serverUrl: String?
    public let isTestMode: Bool?

    enum CodingKeys: String, CodingKey {
        case pong
        case nonce
        case timestamp
        case version
        case serverUrl = "server_url"
        case isTestMode = "is_test_mode"
    }
}

public struct HostHealthResponse: Codable, Sendable, Equatable {
    public let healthy: Bool
    public let latencyMs: Double
    public let serverUrl: String
    public let error: String?

    enum CodingKeys: String, CodingKey {
        case healthy
        case latencyMs = "latency_ms"
        case serverUrl = "server_url"
        case error
    }
}

public struct HostServerConfigResponse: Codable, Sendable, Equatable {
    public let serverUrl: String
    public let locale: String
    public let simulateOffline: Bool?
    public let deviceId: String?
    public let version: String?

    enum CodingKeys: String, CodingKey {
        case serverUrl = "server_url"
        case locale
        case simulateOffline = "simulate_offline"
        case deviceId = "device_id"
        case version
    }
}

public struct HostLifecycleStatusResponse: Codable, Sendable, Equatable {
    public let backgrounded: Bool
    public let walCheckpointed: Bool?
    public let wsDisconnected: Bool?
    public let reconnected: Bool?
    public let hasSession: Bool?

    enum CodingKeys: String, CodingKey {
        case backgrounded
        case walCheckpointed = "wal_checkpointed"
        case wsDisconnected = "ws_disconnected"
        case reconnected
        case hasSession = "has_session"
    }
}
