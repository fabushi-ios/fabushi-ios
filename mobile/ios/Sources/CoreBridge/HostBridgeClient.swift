import Darwin
import Foundation
import os

private let cHostBridgePushCallback: TelegramBridgeEventCallback = { eventTypePtr, payloadPtr, userCtx in
    guard let eventTypePtr, let payloadPtr, let userCtx else { return }
    let eventType = String(cString: eventTypePtr)
    let payloadJson = String(cString: payloadPtr)
    let broadcaster = Unmanaged<HostEventBroadcaster>.fromOpaque(userCtx).takeUnretainedValue()
    broadcaster.publishRaw(eventType: eventType, payloadJson: payloadJson)
}

private struct HostRequestEnvelope<P: Encodable & Sendable>: Encodable, Sendable {
    let command: String
    let params: P
}

private struct HostResponseEnvelope<T: Decodable>: Decodable {
    let ok: Bool
    let result: T?
    let error: HostErrorWirePayload?
}

private struct HostErrorWirePayload: Decodable {
    let code: String?
    let message: String?
    let httpStatus: Int?

    enum CodingKeys: String, CodingKey {
        case code
        case message
        case httpStatus = "http_status"
    }
}

public struct EmptyRpcParams: Codable, Sendable, Equatable {
    public init() {}
}

/// Concurrent reader / barrier-writer box managing the opaque Rust `TelegramAppHost` pointer
/// and retained `HostEventBroadcaster` context with zero use-after-free under parallel FFI calls.
private final class HostRuntimeHandleBox: @unchecked Sendable {
    private let queue = DispatchQueue(
        label: "com.ombhrum.fabushi.host-ffi-pool",
        qos: .userInitiated,
        attributes: .concurrent
    )
    private var rawHost: UnsafeMutableRawPointer?
    private var retainedBroadcasterCtx: UnsafeMutableRawPointer?
    let broadcaster: HostEventBroadcaster

    init(
        directory: URL,
        passphrase: String?,
        isTestMode: Bool,
        broadcaster: HostEventBroadcaster
    ) throws {
        self.broadcaster = broadcaster
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)

        let created: UnsafeMutableRawPointer?
        if isTestMode {
            created = directory.path.withCString { dirPtr in
                telegram_app_host_create_test(dirPtr)
            }
        } else {
            created = directory.path.withCString { dirPtr in
                if let passphrase, !passphrase.isEmpty {
                    return passphrase.withCString { passPtr in
                        telegram_app_host_create(dirPtr, passPtr)
                    }
                } else {
                    return telegram_app_host_create(dirPtr, nil)
                }
            }
        }

        guard let created else {
            let reason = telegram_app_host_last_error().map { String(cString: $0) }
                ?? "telegram_app_host_create returned NULL"
            throw HostBridgeError.rpcError(code: "init_failed", message: reason, httpStatus: nil)
        }

        let ctx = Unmanaged.passRetained(broadcaster).toOpaque()
        telegram_bridge_register_event_callback(created, cHostBridgePushCallback, ctx)
        self.rawHost = created
        self.retainedBroadcasterCtx = ctx
    }

    deinit {
        closeBarrier()
    }

    var isOpen: Bool {
        queue.sync { rawHost != nil }
    }

    func dispatchAsync(_ requestJson: String) async throws -> String {
        try await withCheckedThrowingContinuation { continuation in
            queue.async { [self] in
                guard let ptr = self.rawHost else {
                    continuation.resume(throwing: HostBridgeError.notInitialized)
                    return
                }
                let outPtr = requestJson.withCString { reqCStr in
                    telegram_app_host_dispatch(ptr, reqCStr)
                }
                guard let outPtr else {
                    continuation.resume(
                        throwing: HostBridgeError.invalidResponse("NULL response pointer")
                    )
                    return
                }
                defer { telegram_app_host_free_string(outPtr) }
                let str = String(cString: outPtr)
                continuation.resume(returning: str)
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
            if let ctx = retainedBroadcasterCtx {
                Unmanaged<HostEventBroadcaster>.fromOpaque(ctx).release()
                retainedBroadcasterCtx = nil
            }
        }
        broadcaster.finishAll()
    }
}

/// Thread-safe 128-bit sliding replay window backed by Rust `telegram_replay_window_*` C-ABI.
public final class HostSlidingReplayWindow: @unchecked Sendable {
    private let lock = NSLock()
    private var handle: UnsafeMutableRawPointer?

    public init() throws {
        guard let ptr = telegram_replay_window_create() else {
            throw HostBridgeError.rpcError(
                code: "replay_window_init_failed",
                message: "telegram_replay_window_create returned NULL",
                httpStatus: nil
            )
        }
        self.handle = ptr
    }

    deinit {
        lock.lock()
        if let ptr = handle {
            telegram_replay_window_destroy(ptr)
            handle = nil
        }
        lock.unlock()
    }

    public func checkAndAdd(sequence: UInt64) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        guard let ptr = handle else { return false }
        return telegram_replay_window_check_and_add(ptr, sequence)
    }
}

/// Primary Swift 6 actor wrapping `libmahayana_app_host.a` across all 29 canonical domain namespaces.
public actor HostBridgeClient {
    public static let shared = HostBridgeClient()

    private var box: HostRuntimeHandleBox?
    public nonisolated let events = HostEventBroadcaster()
    public let keychain: KeychainSessionStore

    public init(keychain: KeychainSessionStore = KeychainSessionStore()) {
        self.keychain = keychain
    }

    public nonisolated static var protoMaxPacketSize: Int {
        Int(telegram_proto_max_packet_size())
    }

    public nonisolated var eventStream: AsyncStream<HostRealtimeEvent> {
        events.makeStream()
    }

    public var isInitialized: Bool {
        box?.isOpen ?? false
    }

    public func initialize(
        appDataDirectory: URL? = nil,
        isTestMode: Bool = false,
        explicitPassphrase: String? = nil
    ) throws {
        if let existing = box, existing.isOpen {
            return
        }
        let dir = appDataDirectory ?? FileManager.default.urls(
            for: .applicationSupportDirectory,
            in: .userDomainMask
        )[0].appendingPathComponent("FabushiHostCore", isDirectory: true)

        let passphrase: String?
        if isTestMode {
            passphrase = explicitPassphrase
        } else if let explicitPassphrase {
            passphrase = explicitPassphrase
        } else {
            passphrase = try? keychain.loadOrCreateStoragePassphrase()
        }

        self.box = try HostRuntimeHandleBox(
            directory: dir,
            passphrase: passphrase,
            isTestMode: isTestMode,
            broadcaster: events
        )
    }

    public func shutdown() {
        box?.closeBarrier()
        box = nil
    }

    // MARK: - Generic Typed JSON-RPC Dispatch

    public func call<Params: Encodable & Sendable, Response: Decodable & Sendable>(
        _ command: String,
        params: Params
    ) async throws -> Response {
        guard let activeBox = box, activeBox.isOpen else {
            throw HostBridgeError.notInitialized
        }
        let reqEnvelope = HostRequestEnvelope(command: command, params: params)
        let encoded: Data
        do {
            encoded = try JSONEncoder().encode(reqEnvelope)
        } catch {
            throw HostBridgeError.serializationFailed
        }
        guard let reqJson = String(data: encoded, encoding: .utf8) else {
            throw HostBridgeError.serializationFailed
        }

        let rawReply = try await activeBox.dispatchAsync(reqJson)
        guard let replyData = rawReply.data(using: .utf8) else {
            throw HostBridgeError.invalidResponse(rawReply)
        }

        let decoded: HostResponseEnvelope<Response>
        do {
            decoded = try JSONDecoder().decode(HostResponseEnvelope<Response>.self, from: replyData)
        } catch {
            throw HostBridgeError.invalidResponse(rawReply)
        }

        if decoded.ok, let result = decoded.result {
            return result
        }

        let code = decoded.error?.code ?? "internal_error"
        let msg = decoded.error?.message ?? "Unknown host RPC error"
        if code == "HOST_PANIC" {
            throw HostBridgeError.rustPanic(msg)
        }
        throw HostBridgeError.rpcError(
            code: code,
            message: msg,
            httpStatus: decoded.error?.httpStatus
        )
    }

    public func call<Response: Decodable & Sendable>(
        _ command: String
    ) async throws -> Response {
        try await call(command, params: EmptyRpcParams())
    }

    /// Raw JSON dictionary convenience for dynamic/exploratory payloads.
    public func callRawJson(
        _ command: String,
        paramsJsonObject: [String: Any] = [:]
    ) async throws -> String {
        guard let activeBox = box, activeBox.isOpen else {
            throw HostBridgeError.notInitialized
        }
        let body: [String: Any] = [
            "command": command,
            "params": paramsJsonObject,
        ]
        guard JSONSerialization.isValidJSONObject(body),
              let data = try? JSONSerialization.data(withJSONObject: body),
              let reqStr = String(data: data, encoding: .utf8)
        else {
            throw HostBridgeError.serializationFailed
        }
        return try await activeBox.dispatchAsync(reqStr)
    }

    // MARK: - High-Frequency Typed Foundation Helpers

    public func ping(nonce: UInt64 = 1) async throws -> HostPingResponse {
        struct NonceParams: Codable, Sendable { let nonce: UInt64 }
        return try await call("config.ping", params: NonceParams(nonce: nonce))
    }

    public func getServerConfig() async throws -> HostServerConfigResponse {
        try await call("config.get_server_config")
    }

    @discardableResult
    public func setServerURL(_ serverURL: String, locale: String? = nil) async throws -> HostServerConfigResponse {
        struct Params: Codable, Sendable {
            let server_url: String
            let locale: String?
        }
        return try await call(
            "config.set_server_url",
            params: Params(server_url: serverURL, locale: locale)
        )
    }

    public func pingHealth() async throws -> HostHealthResponse {
        try await call("config.ping_health")
    }

    @discardableResult
    public func enterBackground() async throws -> HostLifecycleStatusResponse {
        try await call("config.lifecycle_background")
    }

    @discardableResult
    public func resumeForeground() async throws -> HostLifecycleStatusResponse {
        try await call("config.lifecycle_foreground")
    }

    public func semanticSnapshot() async throws -> SemanticWorkspaceTree {
        try await call("semantic.snapshot")
    }

    public func semanticHash() async throws -> String {
        struct HashResp: Decodable, Sendable { let state_hash: String }
        let res: HashResp = try await call("semantic.hash")
        return res.state_hash
    }

    public func semanticAct(_ action: SemanticAction) async throws -> SemanticActionReceipt {
        try await call("semantic.act", params: action)
    }
}
