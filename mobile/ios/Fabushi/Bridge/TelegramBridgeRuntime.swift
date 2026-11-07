import Darwin
import Foundation
import Security

// MARK: - Global C-ABI Event Callback
private let cTelegramBridgeEventCallback: TelegramBridgeEventCallback = { eventTypePtr, payloadJsonPtr, userContext in
    guard let eventTypePtr, let payloadJsonPtr, let userContext else { return }
    let eventType = String(cString: eventTypePtr)
    let payloadJson = String(cString: payloadJsonPtr)

    // Safely retrieve unmanaged runtime instance pointer
    let runtime = Unmanaged<TelegramBridgeRuntime>.fromOpaque(userContext).takeUnretainedValue()
    runtime.receivePushEvent(type: eventType, payloadJson: payloadJson)
}

// MARK: - Telegram Bridge Errors
public enum TelegramBridgeError: LocalizedError, Sendable {
    case notInitialized
    case serializationFailed
    case invalidResponse(String)
    case serverError(code: String, message: String)
    case rustPanic(String)
    case keychainAccessFailed(OSStatus)

    public var errorDescription: String? {
        switch self {
        case .notInitialized:
            return "Telegram Rust core runtime is not initialized."
        case .serializationFailed:
            return "Failed to serialize JSON-RPC command parameters."
        case .invalidResponse(let raw):
            return "Invalid or unparseable response from Rust runtime: \(raw)"
        case .serverError(let code, let message):
            return "[\(code)] \(message)"
        case .rustPanic(let message):
            return "Rust runtime panic: \(message)"
        case .keychainAccessFailed(let status):
            return "Failed to access iOS Keychain storage passphrase (status: \(status))."
        }
    }
}

// MARK: - Sendable Pointer Wrapper
private struct SendableHostPointer: @unchecked Sendable {
    let pointer: UnsafeMutableRawPointer
}

// MARK: - Generic Envelopes
private struct BridgeRequestEnvelope<P: Encodable>: Encodable {
    let command: String
    let params: P
}

private struct BridgeResponseEnvelope<T: Decodable>: Decodable {
    let ok: Bool
    let result: T?
    let error: BridgeErrorPayload?
}

private struct BridgeErrorPayload: Decodable {
    let code: String?
    let message: String?
}

// MARK: - TelegramBridgeRuntime
@MainActor
public final class TelegramBridgeRuntime: ObservableObject {
    public static let shared = TelegramBridgeRuntime()

    // Opaque handle to Rust TelegramAppHost
    private var hostHandle: SendableHostPointer?

    // Dedicated background serial queue for offloaded FFI dispatch
    private let bridgeQueue = DispatchQueue(
        label: "com.ombhrum.telegram.bridge-serial",
        qos: .userInitiated
    )

    // Push event streaming engine
    private let eventPipeline = TelegramEventStream()
    public var eventStream: AsyncStream<TelegramEvent> {
        eventPipeline.stream
    }

    @Published public private(set) var isReady: Bool = false
    @Published public private(set) var connectionStatus: String = "Disconnected"

    private init() {}

    deinit {
        if let handle = hostHandle {
            bridgeQueue.sync {
                telegram_app_host_destroy(handle.pointer)
            }
        }
        eventPipeline.finish()
    }

    // MARK: - Lifecycle Management

    public func initialize(appDataDirectory: URL? = nil, isTestMode: Bool = false) throws {
        guard hostHandle == nil else { return }

        let directory = appDataDirectory ?? FileManager.default.urls(
            for: .applicationSupportDirectory,
            in: .userDomainMask
        )[0].appendingPathComponent("TelegramCore", isDirectory: true)

        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)

        let rawPointer: UnsafeMutableRawPointer?
        if isTestMode {
            rawPointer = directory.path.withCString { dirPtr in
                telegram_app_host_create_test(dirPtr)
            }
        } else {
            let passphrase = (try? loadOrCreateStoragePassphrase()) ?? ""
            rawPointer = directory.path.withCString { dirPtr in
                if passphrase.isEmpty {
                    telegram_app_host_create(dirPtr, nil)
                } else {
                    passphrase.withCString { passPtr in
                        telegram_app_host_create(dirPtr, passPtr)
                    }
                }
            }
        }

        guard let rawPointer else {
            let errorMsg = telegram_app_host_last_error().map { String(cString: $0) } ?? "Unknown initialization error"
            throw TelegramBridgeError.serverError(code: "INIT_FAILED", message: errorMsg)
        }

        let handle = SendableHostPointer(pointer: rawPointer)
        self.hostHandle = handle

        // Register push event callback
        telegram_bridge_register_event_callback(
            handle.pointer,
            cTelegramBridgeEventCallback,
            Unmanaged.passUnretained(self).toOpaque()
        )

        isReady = true
        connectionStatus = "Connected"
    }

    public func shutdown() {
        isReady = false
        connectionStatus = "Disconnected"
        bridgeQueue.sync {
            if let handle = self.hostHandle {
                telegram_app_host_destroy(handle.pointer)
                self.hostHandle = nil
            }
        }
    }

    // MARK: - Command Dispatch

    private func dispatchRawJson(_ requestString: String) async throws -> String {
        guard let handle = hostHandle else {
            throw TelegramBridgeError.notInitialized
        }

        return try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<String, Error>) in
            bridgeQueue.async {
                let responsePtr = requestString.withCString { cStr in
                    telegram_app_host_dispatch(handle.pointer, cStr)
                }
                guard let responsePtr else {
                    continuation.resume(throwing: TelegramBridgeError.invalidResponse("Null pointer returned by dispatch"))
                    return
                }
                defer { telegram_app_host_free_string(responsePtr) }
                let responseString = String(cString: responsePtr)
                continuation.resume(returning: responseString)
            }
        }
    }

    public func dispatch<T: Decodable, R: Encodable>(command: String, params: R) async throws -> T {
        let envelope = BridgeRequestEnvelope(command: command, params: params)
        let encoder = JSONEncoder()
        guard let requestData = try? encoder.encode(envelope),
              let requestString = String(data: requestData, encoding: .utf8)
        else {
            throw TelegramBridgeError.serializationFailed
        }

        let rawResponse = try await dispatchRawJson(requestString)

        guard let responseData = rawResponse.data(using: .utf8) else {
            throw TelegramBridgeError.invalidResponse(rawResponse)
        }

        let decoder = JSONDecoder()
        let resultEnvelope: BridgeResponseEnvelope<T>
        do {
            resultEnvelope = try decoder.decode(BridgeResponseEnvelope<T>.self, from: responseData)
        } catch {
            throw TelegramBridgeError.invalidResponse(rawResponse)
        }

        if resultEnvelope.ok, let result = resultEnvelope.result {
            return result
        }

        let code = resultEnvelope.error?.code ?? "UNKNOWN_ERROR"
        let message = resultEnvelope.error?.message ?? "Operation failed"
        if code == "HOST_PANIC" {
            throw TelegramBridgeError.rustPanic(message)
        }
        throw TelegramBridgeError.serverError(code: code, message: message)
    }

    public func dispatch<T: Decodable>(command: String) async throws -> T {
        return try await dispatch(command: command, params: [String: String]())
    }

    @discardableResult
    public func dispatch(command: String) async throws -> [String: Any] {
        let envelope = BridgeRequestEnvelope(command: command, params: [String: String]())
        let encoder = JSONEncoder()
        guard let requestData = try? encoder.encode(envelope),
              let requestString = String(data: requestData, encoding: .utf8)
        else {
            throw TelegramBridgeError.serializationFailed
        }

        let rawResponse = try await dispatchRawJson(requestString)

        guard let responseData = rawResponse.data(using: .utf8),
              let json = try? JSONSerialization.jsonObject(with: responseData) as? [String: Any]
        else {
            throw TelegramBridgeError.invalidResponse(rawResponse)
        }

        guard let ok = json["ok"] as? Bool else {
            throw TelegramBridgeError.invalidResponse(rawResponse)
        }

        if ok {
            return (json["result"] as? [String: Any]) ?? [:]
        }

        let errDict = json["error"] as? [String: Any]
        let code = errDict?["code"] as? String ?? "UNKNOWN_ERROR"
        let message = errDict?["message"] as? String ?? "Operation failed"
        if code == "HOST_PANIC" {
            throw TelegramBridgeError.rustPanic(message)
        }
        throw TelegramBridgeError.serverError(code: code, message: message)
    }

    // MARK: - Push Event Reception

    nonisolated func receivePushEvent(type: String, payloadJson: String) {
        eventPipeline.parseAndYield(type: type, json: payloadJson)
    }

    // MARK: - Secure Keychain Storage Passphrase

    private func loadOrCreateStoragePassphrase() throws -> String {
        let service = "com.ombhrum.fabushi.telegram-storage.v1"
        let account = "default"

        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]

        var item: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &item)
        if status == errSecSuccess, let data = item as? Data, data.count == 32 {
            return data.base64EncodedString()
        }

        guard status == errSecItemNotFound else {
            throw TelegramBridgeError.keychainAccessFailed(status)
        }

        var bytes = [UInt8](repeating: 0, count: 32)
        let randomStatus = bytes.withUnsafeMutableBytes { buf in
            guard let baseAddress = buf.baseAddress else { return errSecParam }
            return SecRandomCopyBytes(kSecRandomDefault, buf.count, baseAddress)
        }
        guard randomStatus == errSecSuccess else {
            throw TelegramBridgeError.keychainAccessFailed(randomStatus)
        }

        let data = Data(bytes)
        let addQuery: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
            kSecValueData as String: data,
        ]

        let addStatus = SecItemAdd(addQuery as CFDictionary, nil)
        guard addStatus == errSecSuccess else {
            throw TelegramBridgeError.keychainAccessFailed(addStatus)
        }

        return data.base64EncodedString()
    }
}
