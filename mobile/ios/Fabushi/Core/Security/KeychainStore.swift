import Foundation
import Security
import os

/// Protocol contract for session, passcode, and TON wallet secret storage (`PROJECT.md § IC.1`).
public protocol KeychainProvider: Sendable {
    func setData(_ data: Data, forKey key: String) throws
    func getData(forKey key: String) throws -> Data?
    func removeValue(forKey key: String) throws
    func removeAll() throws
}

public extension KeychainProvider {
    func setString(_ value: String, forKey key: String) throws {
        try setData(Data(value.utf8), forKey: key)
    }

    func getString(forKey key: String) throws -> String? {
        guard let data = try getData(forKey: key) else { return nil }
        return String(data: data, encoding: .utf8)
    }

    func setCodable<T: Encodable>(_ value: T, forKey key: String) throws {
        let encoded = try JSONEncoder().encode(value)
        try setData(encoded, forKey: key)
    }

    func getCodable<T: Decodable>(_ type: T.Type, forKey key: String) throws -> T? {
        guard let data = try getData(forKey: key) else { return nil }
        return try JSONDecoder().decode(T.self, from: data)
    }
}

/// Native `Security.framework` Keychain store (`kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly`)
/// with automatic per-sandbox `.isolated_keychain.json` fallback when `FABUSHI_UI_TEST_MODE == "1"`
/// or unsigned Simulator builds return `errSecMissingEntitlement (-34018)`.
public final class KeychainStore: KeychainProvider, Sendable {
    public let serviceNamespace: String
    public let fallbackFileURL: URL
    public let forceIsolatedFallback: Bool
    private let fileLock = OSAllocatedUnfairLock()

    public init(
        serviceNamespace: String = "com.ombhrum.fabushi.keychain.v2",
        appSupportDir: URL? = nil,
        environment: [String: String] = ProcessInfo.processInfo.environment
    ) {
        self.serviceNamespace = serviceNamespace
        let resolvedDir: URL
        if let appSupportDir {
            resolvedDir = appSupportDir
        } else if let envDir = environment["FABUSHI_APP_SUPPORT_DIR"] ?? environment["GPUI_TELEGRAM_CONFIG_DIR"],
                  !envDir.isEmpty {
            resolvedDir = URL(fileURLWithPath: envDir, isDirectory: true)
        } else {
            resolvedDir = FileManager.default.urls(
                for: .applicationSupportDirectory,
                in: .userDomainMask
            )[0].appendingPathComponent("Fabushi", isDirectory: true)
        }
        self.fallbackFileURL = resolvedDir.appendingPathComponent(".isolated_keychain.json", isDirectory: false)
        self.forceIsolatedFallback = (environment["FABUSHI_UI_TEST_MODE"] == "1")
    }

    public func setData(_ data: Data, forKey key: String) throws {
        if forceIsolatedFallback {
            try writeFallback(data, forKey: key)
            return
        }
        let baseQuery: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: serviceNamespace,
            kSecAttrAccount as String: key,
        ]
        let attrs: [String: Any] = [
            kSecValueData as String: data,
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
        ]
        let updateStatus = SecItemUpdate(baseQuery as CFDictionary, attrs as CFDictionary)
        if updateStatus == errSecSuccess {
            return
        }
        if updateStatus == errSecMissingEntitlement {
            try writeFallback(data, forKey: key)
            return
        }
        if updateStatus != errSecItemNotFound {
            throw HostError(
                code: "keychain_error",
                httpStatus: nil,
                message: "SecItemUpdate failed with OSStatus \(updateStatus)",
                retryAfterSeconds: nil
            )
        }

        var addQuery = baseQuery
        addQuery[kSecValueData as String] = data
        addQuery[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        let addStatus = SecItemAdd(addQuery as CFDictionary, nil)
        if addStatus == errSecMissingEntitlement {
            try writeFallback(data, forKey: key)
            return
        }
        guard addStatus == errSecSuccess else {
            throw HostError(
                code: "keychain_error",
                httpStatus: nil,
                message: "SecItemAdd failed with OSStatus \(addStatus)",
                retryAfterSeconds: nil
            )
        }
    }

    public func getData(forKey key: String) throws -> Data? {
        if forceIsolatedFallback {
            return try readFallback(forKey: key)
        }
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: serviceNamespace,
            kSecAttrAccount as String: key,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        var out: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &out)
        if status == errSecItemNotFound {
            return try readFallback(forKey: key)
        }
        if status == errSecMissingEntitlement {
            return try readFallback(forKey: key)
        }
        guard status == errSecSuccess else {
            throw HostError(
                code: "keychain_error",
                httpStatus: nil,
                message: "SecItemCopyMatching failed with OSStatus \(status)",
                retryAfterSeconds: nil
            )
        }
        return out as? Data
    }

    public func removeValue(forKey key: String) throws {
        try deleteFallback(forKey: key)
        guard !forceIsolatedFallback else { return }
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: serviceNamespace,
            kSecAttrAccount as String: key,
        ]
        let status = SecItemDelete(query as CFDictionary)
        if status != errSecSuccess && status != errSecItemNotFound && status != errSecMissingEntitlement {
            throw HostError(
                code: "keychain_error",
                httpStatus: nil,
                message: "SecItemDelete failed with OSStatus \(status)",
                retryAfterSeconds: nil
            )
        }
    }

    public func removeAll() throws {
        fileLock.withLock {
            try? FileManager.default.removeItem(at: fallbackFileURL)
        }
        guard !forceIsolatedFallback else { return }
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: serviceNamespace,
        ]
        let status = SecItemDelete(query as CFDictionary)
        if status != errSecSuccess && status != errSecItemNotFound && status != errSecMissingEntitlement {
            throw HostError(
                code: "keychain_error",
                httpStatus: nil,
                message: "SecItemDelete(all) failed with OSStatus \(status)",
                retryAfterSeconds: nil
            )
        }
    }

    public func loadOrCreateStoragePassphrase() throws -> String {
        let key = "sqlite_storage_passphrase_v2"
        if let existing = try getData(forKey: key), existing.count == 32 {
            return existing.base64EncodedString()
        }
        var bytes = [UInt8](repeating: 0, count: 32)
        let rc = bytes.withUnsafeMutableBytes { raw in
            guard let base = raw.baseAddress else { return errSecParam }
            return SecRandomCopyBytes(kSecRandomDefault, raw.count, base)
        }
        guard rc == errSecSuccess else {
            throw HostError(code: "rng_failed", httpStatus: nil, message: "SecRandomCopyBytes failed", retryAfterSeconds: nil)
        }
        let data = Data(bytes)
        try setData(data, forKey: key)
        return data.base64EncodedString()
    }

    // MARK: - Sandbox Fallback Store

    private func loadFallbackMapUnlocked() -> [String: String] {
        guard let raw = try? Data(contentsOf: fallbackFileURL),
              let decoded = try? JSONDecoder().decode([String: [String: String]].self, from: raw)
        else {
            return [:]
        }
        return decoded[serviceNamespace] ?? [:]
    }

    private func saveFallbackMapUnlocked(_ namespaceMap: [String: String]) throws {
        let parent = fallbackFileURL.deletingLastPathComponent()
        try FileManager.default.createDirectory(at: parent, withIntermediateDirectories: true)
        var root: [String: [String: String]] = [:]
        if let raw = try? Data(contentsOf: fallbackFileURL),
           let existing = try? JSONDecoder().decode([String: [String: String]].self, from: raw) {
            root = existing
        }
        root[serviceNamespace] = namespaceMap
        let encoded = try JSONEncoder().encode(root)
        try encoded.write(to: fallbackFileURL, options: .atomic)
    }

    private func writeFallback(_ data: Data, forKey key: String) throws {
        try fileLock.withLock {
            var map = loadFallbackMapUnlocked()
            map[key] = data.base64EncodedString()
            try saveFallbackMapUnlocked(map)
        }
    }

    private func readFallback(forKey key: String) throws -> Data? {
        fileLock.withLock {
            let map = loadFallbackMapUnlocked()
            guard let b64 = map[key] else { return nil }
            return Data(base64Encoded: b64)
        }
    }

    private func deleteFallback(forKey key: String) throws {
        try fileLock.withLock {
            var map = loadFallbackMapUnlocked()
            if map.removeValue(forKey: key) != nil {
                try saveFallbackMapUnlocked(map)
            }
        }
    }
}
