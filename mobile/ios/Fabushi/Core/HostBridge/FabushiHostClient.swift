import Darwin
import Foundation
import os

private let cFabushiHostPushCallback: TelegramBridgeEventCallback = { eventTypePtr, payloadPtr, userCtx in
    guard let eventTypePtr, let payloadPtr, let userCtx else { return }
    let eventType = String(cString: eventTypePtr)
    let payloadJSON = String(cString: payloadPtr)
    let broadcaster = Unmanaged<HostEventBroadcaster>.fromOpaque(userCtx).takeUnretainedValue()
    broadcaster.publish(FabushiHostEventDecoder.decode(type: eventType, payloadJSON: payloadJSON))
}

enum FabushiHostEventDecoder {
    static func decode(type: String, payloadJSON: String) -> HostRealtimeEvent {
        guard let data = payloadJSON.data(using: .utf8),
              let dict = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        else {
            return .raw(type: type, payloadJSON: payloadJSON)
        }

        switch type {
        case "connectionStateChanged":
            let raw = (dict["state"] as? String ?? "").lowercased()
            let mapped: ConnectionStatus
            if raw.contains("connect") && !raw.contains("disconnect") && !raw.contains("reconnect") {
                mapped = .connected
            } else if raw.contains("updat") || raw.contains("sync") {
                mapped = .updating
            } else if raw.contains("offline") || raw.contains("suspend") || raw.contains("disconnect") {
                mapped = .offline
            } else {
                mapped = .connecting
            }
            return .connectionStateChanged(mapped)

        case "newMessage", "messageEdited":
            let id = (dict["id"] as? NSNumber)?.int64Value ?? 0
            let chatId = (dict["chat_id"] as? NSNumber)?.int64Value ?? 0
            let senderId = (dict["sender_id"] as? NSNumber)?.uint64Value ?? 0
            let text = dict["text"] as? String ?? ""
            let ts = (dict["timestamp"] as? NSNumber)?.int64Value
                ?? (dict["timestamp_ms"] as? NSNumber)?.int64Value ?? 0
            return .messagesUpdated(
                chatId: chatId,
                [MessageItemDTO(id: id, chatId: chatId, senderId: senderId, text: text, timestampMs: ts)]
            )

        case "reactionUpdated":
            let cid = (dict["chat_id"] as? NSNumber)?.int64Value ?? 0
            let mid = (dict["msg_id"] as? NSNumber)?.int64Value
                ?? (dict["message_id"] as? NSNumber)?.int64Value ?? 0
            let emoji = dict["emoji"] as? String ?? ""
            return .reactionUpdated(ReactionUpdateDTO(chatId: cid, messageId: mid, emoji: emoji))

        case "vadBargeIn":
            let rms = (dict["rms"] as? NSNumber)?.floatValue ?? 0
            let count = (dict["barge_in_count"] as? NSNumber)?.uint64Value ?? 0
            return .talkBargeIn(TalkEventDTO(rms: rms, bargeInCount: count))

        case "cuaEmergencyTakeover":
            let gen = (dict["generation"] as? NSNumber)?.uint64Value ?? 0
            return .tailnetLeaseApproval(
                ComputerApprovalDTO(nodeId: "local", operation: "emergency_takeover", generation: gen)
            )

        default:
            return .raw(type: type, payloadJSON: payloadJSON)
        }
    }
}

public struct EmptyHostParams: Codable, Sendable, Equatable {
    public init() {}
}

private struct WireRequestEnvelope<P: Encodable & Sendable>: Encodable, Sendable {
    let command: String
    let params: P
}

private struct WireResponseEnvelope<R: Decodable>: Decodable {
    let ok: Bool
    let result: R?
    let error: HostError?
}

/// Concurrent reader / barrier-writer box managing the opaque Rust `TelegramAppHost` pointer.
private final class NativeHostHandleBox: @unchecked Sendable {
    private let queue = DispatchQueue(
        label: "com.ombhrum.fabushi.host-client-pool",
        qos: .userInitiated,
        attributes: .concurrent
    )
    private var rawHost: UnsafeMutableRawPointer?
    private var retainedCtx: UnsafeMutableRawPointer?
    let broadcaster: HostEventBroadcaster

    init(
        directory: URL,
        passphrase: String?,
        isTestMode: Bool,
        broadcaster: HostEventBroadcaster
    ) throws {
        self.broadcaster = broadcaster
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        setenv("GPUI_TELEGRAM_CONFIG_DIR", directory.path, 1)

        let created: UnsafeMutableRawPointer?
        if isTestMode {
            created = directory.path.withCString { telegram_app_host_create_test($0) }
        } else {
            created = directory.path.withCString { dirPtr in
                if let passphrase, !passphrase.isEmpty {
                    return passphrase.withCString { telegram_app_host_create(dirPtr, $0) }
                }
                return telegram_app_host_create(dirPtr, nil)
            }
        }

        guard let created else {
            let detail = telegram_app_host_last_error().map { String(cString: $0) }
                ?? "telegram_app_host_create returned NULL"
            throw HostError(code: "init_failed", httpStatus: nil, message: detail, retryAfterSeconds: nil)
        }

        let ctx = Unmanaged.passRetained(broadcaster).toOpaque()
        telegram_bridge_register_event_callback(created, cFabushiHostPushCallback, ctx)
        self.rawHost = created
        self.retainedCtx = ctx
    }

    deinit {
        closeBarrier()
    }

    var isOpen: Bool {
        queue.sync { rawHost != nil }
    }

    func dispatch(_ requestString: String) async throws -> Data {
        try await withCheckedThrowingContinuation { continuation in
            queue.async { [self] in
                guard let ptr = self.rawHost else {
                    continuation.resume(
                        throwing: HostError(code: "not_initialized", message: "FabushiHostClient is closed")
                    )
                    return
                }
                let outPtr = requestString.withCString { telegram_app_host_dispatch(ptr, $0) }
                guard let outPtr else {
                    continuation.resume(
                        throwing: HostError(code: "null_response", message: "Rust FFI returned NULL")
                    )
                    return
                }
                defer { telegram_app_host_free_string(outPtr) }
                let str = String(cString: outPtr)
                continuation.resume(returning: Data(str.utf8))
            }
        }
    }

    func closeBarrier() {
        queue.sync(flags: .barrier) {
            if let ptr = rawHost {
                telegram_bridge_register_event_callback(ptr, nil, nil)
                telegram_app_host_destroy(ptr)
                rawHost = nil
            }
            if let ctx = retainedCtx {
                Unmanaged<HostEventBroadcaster>.fromOpaque(ctx).release()
                retainedCtx = nil
            }
        }
        broadcaster.finishAll()
    }
}

/// Canonical Swift 6 actor implementing `FabushiHostClientProtocol` across all 29 domain namespaces.
public actor FabushiHostClient: FabushiHostClientProtocol {
    public static let defaultProductionServerURL = "http://34.46.77.133:8080"
    public static let shared = FabushiHostClient()

    public nonisolated let broadcaster = HostEventBroadcaster()
    public nonisolated var eventStream: AsyncStream<HostRealtimeEvent> {
        broadcaster.makeStream()
    }

    public nonisolated static var protoMaxPacketSize: Int {
        Int(telegram_proto_max_packet_size())
    }

    private var handleBox: NativeHostHandleBox?
    private var keychain: KeychainProvider?
    private var biometrics: BiometricsProvider?
    private var currentBootstrap: RuntimeBootstrapState?
    private let environment: [String: String]

    public init(environment: [String: String] = ProcessInfo.processInfo.environment) {
        self.environment = environment
    }

    public var isInitialized: Bool {
        handleBox?.isOpen ?? false
    }

    public func bootstrapState() -> RuntimeBootstrapState? {
        currentBootstrap
    }

    @discardableResult
    public func initialize(
        appSupportDir: URL? = nil,
        defaultServerURL: String = FabushiHostClient.defaultProductionServerURL,
        keychainBridge: KeychainProvider = KeychainStore(),
        biometricsBridge: BiometricsProvider = BiometricsAuthenticator()
    ) async throws -> RuntimeBootstrapState {
        let resolvedDir: URL
        if let appSupportDir {
            resolvedDir = appSupportDir
        } else if let envPath = environment["FABUSHI_APP_SUPPORT_DIR"]
            ?? environment["GPUI_TELEGRAM_CONFIG_DIR"]
            ?? environment["GPUI_TELEGRAM_STORAGE_DIR"],
            !envPath.isEmpty {
            resolvedDir = URL(fileURLWithPath: envPath, isDirectory: true)
        } else {
            resolvedDir = FileManager.default.urls(
                for: .applicationSupportDirectory,
                in: .userDomainMask
            )[0].appendingPathComponent("Fabushi", isDirectory: true)
        }

        let resolvedServerURL = Self.resolveServerURL(
            environment: environment,
            fallback: defaultServerURL
        )

        self.keychain = keychainBridge
        self.biometrics = biometricsBridge

        if handleBox?.isOpen != true {
            let isTest = (environment["FABUSHI_UI_TEST_MODE"] == "1")
            let passphrase: String?
            if let ks = keychainBridge as? KeychainStore, !isTest {
                passphrase = try? ks.loadOrCreateStoragePassphrase()
            } else {
                passphrase = nil
            }
            self.handleBox = try NativeHostHandleBox(
                directory: resolvedDir,
                passphrase: passphrase,
                isTestMode: isTest,
                broadcaster: broadcaster
            )
        }

        // Apply configured server URL
        struct SetServerParams: Codable, Sendable { let server_url: String }
        struct ServerCfgResp: Decodable, Sendable {
            let server_url: String
            let version: String?
        }
        let serverResp: ServerCfgResp = try await call(
            "config.set_server_url",
            params: SetServerParams(server_url: resolvedServerURL)
        )

        // Apply optional network fault mode (`online` | `offline` | `drop_ws`)
        if let fault = environment["FABUSHI_NETWORK_FAULT_MODE"]?.lowercased() {
            struct OfflineParams: Codable, Sendable { let offline: Bool }
            struct OfflineResp: Decodable, Sendable { let simulate_offline: Bool }
            if fault == "offline" {
                let _: OfflineResp = try await call("config.set_simulate_offline", params: OfflineParams(offline: true))
            } else if fault == "drop_ws" {
                struct EmptyResp: Decodable, Sendable {}
                let _: EmptyResp? = try? await call("sync.disconnect_ws")
            } else if fault == "online" {
                let _: OfflineResp = try await call("config.set_simulate_offline", params: OfflineParams(offline: false))
            }
        }

        // Hydrate pre-authenticated sessions (`FABUSHI_PREAUTH_SESSION_JSON` / `FABUSHI_PREAUTH_ACCOUNTS_JSON` / `session.json`)
        var hydratedAccounts: [AccountSessionSummary] = []
        if let rawAccounts = environment["FABUSHI_PREAUTH_ACCOUNTS_JSON"],
           let data = rawAccounts.data(using: .utf8),
           let parsed = try? JSONDecoder().decode([AccountSessionSummary].self, from: data) {
            hydratedAccounts.append(contentsOf: parsed)
        }
        if let rawPreauth = environment["FABUSHI_PREAUTH_SESSION_JSON"],
           let data = rawPreauth.data(using: .utf8),
           let single = try? JSONDecoder().decode(AccountSessionSummary.self, from: data) {
            if !hydratedAccounts.contains(where: { $0.userId == single.userId }) {
                hydratedAccounts.insert(single, at: 0)
            }
        }
        let sessionFile = resolvedDir.appendingPathComponent("session.json")
        if hydratedAccounts.isEmpty,
           let fileData = try? Data(contentsOf: sessionFile),
           let fromDisk = try? JSONDecoder().decode(AccountSessionSummary.self, from: fileData) {
            hydratedAccounts.append(fromDisk)
        }

        struct RestoreSessionParams: Codable, Sendable {
            let user_id: UInt64
            let username: String
            let token: String
            let session_id: String
            let server_url: String
        }
        struct RestoreResp: Decodable, Sendable { let authenticated: Bool }

        for account in hydratedAccounts.reversed() {
            _ = try? await call(
                "auth.restore_session",
                params: RestoreSessionParams(
                    user_id: account.userId,
                    username: account.username,
                    token: account.token,
                    session_id: account.sessionId,
                    server_url: account.serverURL.isEmpty ? resolvedServerURL : account.serverURL
                )
            ) as RestoreResp
        }

        // Query authoritative session status from Rust host
        struct SessionStatusResp: Decodable, Sendable {
            let authenticated: Bool
            let user_id: UInt64?
            let username: String?
            let server_url: String
        }
        let status: SessionStatusResp = try await call("auth.get_session_status")
        let activeAccount = hydratedAccounts.first
        let connStatus: ConnectionStatus = (environment["FABUSHI_NETWORK_FAULT_MODE"]?.lowercased() == "offline")
            ? .offline
            : (status.authenticated ? .connected : .connecting)

        let state = RuntimeBootstrapState(
            serverURL: serverResp.server_url,
            appSupportPath: resolvedDir.path,
            isAuthenticated: status.authenticated,
            activeAccount: activeAccount,
            accounts: hydratedAccounts,
            connectionStatus: connStatus,
            version: serverResp.version ?? "1.3.0"
        )
        self.currentBootstrap = state
        return state
    }

    public func shutdown() {
        handleBox?.closeBarrier()
        handleBox = nil
        currentBootstrap = nil
    }

    public func handleScenePhaseChange(_ phase: AppLifecyclePhase) async {
        guard handleBox?.isOpen == true else { return }
        struct LifecycleResp: Decodable, Sendable { let backgrounded: Bool }
        switch phase {
        case .background:
            _ = try? await call("config.lifecycle_background") as LifecycleResp
        case .active:
            _ = try? await call("config.lifecycle_foreground") as LifecycleResp
        case .inactive:
            break
        }
    }

    public func dispatchRaw(method: String, payloadJSON: Data) async throws -> Data {
        guard let activeBox = handleBox, activeBox.isOpen else {
            throw HostError(code: "not_initialized", message: "FabushiHostClient not initialized")
        }
        let paramsObj = (try? JSONSerialization.jsonObject(with: payloadJSON)) ?? [String: Any]()
        let envelope: [String: Any] = [
            "command": method,
            "params": paramsObj,
        ]
        let reqData = try JSONSerialization.data(withJSONObject: envelope)
        let reqString = String(decoding: reqData, as: UTF8.self)
        return try await activeBox.dispatch(reqString)
    }

    public func call<Params: Encodable & Sendable, Response: Decodable & Sendable>(
        _ method: String,
        params: Params
    ) async throws -> Response {
        guard let activeBox = handleBox, activeBox.isOpen else {
            throw HostError(code: "not_initialized", message: "FabushiHostClient not initialized")
        }
        let reqData = try JSONEncoder().encode(WireRequestEnvelope(command: method, params: params))
        let reqString = String(decoding: reqData, as: UTF8.self)
        let replyData = try await activeBox.dispatch(reqString)
        let decoded = try JSONDecoder().decode(WireResponseEnvelope<Response>.self, from: replyData)
        if decoded.ok, let result = decoded.result {
            return result
        }
        if let err = decoded.error {
            throw err
        }
        throw HostError(code: "internal_error", message: "Malformed error envelope from Rust host")
    }

    public func call<Response: Decodable & Sendable>(_ method: String) async throws -> Response {
        try await call(method, params: EmptyHostParams())
    }

    public static func resolveServerURL(
        environment: [String: String] = ProcessInfo.processInfo.environment,
        fallback: String = defaultProductionServerURL
    ) -> String {
        let candidate = environment["FABUSHI_SERVER_URL"]
            ?? environment["GPUI_TELEGRAM_SERVER_URL"]
            ?? fallback
        var trimmed = candidate.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.isEmpty {
            return defaultProductionServerURL
        }
        if !trimmed.contains("://") {
            trimmed = "http://\(trimmed)"
        }
        while trimmed.hasSuffix("/") {
            trimmed.removeLast()
        }
        return trimmed
    }
}
