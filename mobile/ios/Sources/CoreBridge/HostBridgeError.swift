import Foundation
import Security

/// Strongly-typed errors surfaced by `HostBridgeClient` across the Rust `mahayana_app_host` C-ABI.
public enum HostBridgeError: LocalizedError, Sendable, Equatable {
    case notInitialized
    case serializationFailed
    case invalidResponse(String)
    case rpcError(code: String, message: String, httpStatus: Int?)
    case rustPanic(String)
    case keychainFailed(OSStatus)

    public var errorDescription: String? {
        switch self {
        case .notInitialized:
            return "HostBridgeClient is not initialized or has already been shut down."
        case .serializationFailed:
            return "Failed to encode JSON-RPC request parameters."
        case .invalidResponse(let raw):
            return "Invalid UTF-8 JSON envelope from Rust host: \(raw)"
        case .rpcError(let code, let message, let status):
            if let status {
                return "[\(code) HTTP \(status)] \(message)"
            }
            return "[\(code)] \(message)"
        case .rustPanic(let message):
            return "Recovered Rust FFI panic: \(message)"
        case .keychainFailed(let status):
            return "iOS Keychain operation failed with OSStatus \(status)."
        }
    }

    public var isUnknownCommand: Bool {
        if case .rpcError(let code, _, _) = self {
            return code == "unknown_command"
        }
        return false
    }
}
