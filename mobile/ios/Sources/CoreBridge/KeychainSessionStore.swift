import Foundation
import Security

/// Cloud session credential record persisted inside iOS Keychain (`kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly`).
public struct StoredCloudSession: Codable, Sendable, Equatable, Identifiable {
    public var id: UInt64 { userId }
    public let userId: UInt64
    public let username: String
    public let token: String
    public let sessionId: String
    public let serverURL: String
    public let createdAtMs: Int64

    public init(
        userId: UInt64,
        username: String,
        token: String,
        sessionId: String,
        serverURL: String,
        createdAtMs: Int64 = Int64(Date().timeIntervalSince1970 * 1000)
    ) {
        self.userId = userId
        self.username = username
        self.token = token
        self.sessionId = sessionId
        self.serverURL = ServerEndpointConfig.normalize(serverURL)
        self.createdAtMs = createdAtMs
    }
}

private struct StoredSessionIndex: Codable, Sendable {
    var activeUserId: UInt64?
    var sessions: [StoredCloudSession]
}

/// Multi-account (up to 6 accounts) iOS Keychain session & SQLite passphrase store.
public struct KeychainSessionStore: Sendable {
    public static let maxAccounts = 6

    public let serviceNamespace: String

    public init(serviceNamespace: String = "com.ombhrum.fabushi.core-session.v1") {
        self.serviceNamespace = serviceNamespace
    }

    private var sessionsAccountKey: String { "cloud_sessions_index" }
    private var passphraseAccountKey: String { "sqlite_storage_passphrase" }

    // MARK: - Multi-Account Cloud Sessions

    public func saveSession(_ session: StoredCloudSession, makeActive: Bool = true) throws {
        var index = (try? readIndex()) ?? StoredSessionIndex(activeUserId: nil, sessions: [])
        if let existingIdx = index.sessions.firstIndex(where: { $0.userId == session.userId }) {
            index.sessions[existingIdx] = session
        } else {
            if index.sessions.count >= Self.maxAccounts {
                index.sessions.removeFirst()
            }
            index.sessions.append(session)
        }
        if makeActive || index.activeUserId == nil {
            index.activeUserId = session.userId
        }
        try writeIndex(index)
    }

    public func listSessions() throws -> [StoredCloudSession] {
        try readIndex()?.sessions ?? []
    }

    public func activeSession() throws -> StoredCloudSession? {
        guard let index = try readIndex() else { return nil }
        if let activeId = index.activeUserId,
           let matched = index.sessions.first(where: { $0.userId == activeId }) {
            return matched
        }
        return index.sessions.first
    }

    @discardableResult
    public func switchActiveAccount(userId: UInt64) throws -> StoredCloudSession? {
        guard var index = try readIndex(),
              let target = index.sessions.first(where: { $0.userId == userId })
        else {
            return nil
        }
        index.activeUserId = userId
        try writeIndex(index)
        return target
    }

    public func removeSession(userId: UInt64) throws {
        guard var index = try readIndex() else { return }
        index.sessions.removeAll { $0.userId == userId }
        if index.activeUserId == userId {
            index.activeUserId = index.sessions.first?.userId
        }
        if index.sessions.isEmpty {
            try deleteItem(account: sessionsAccountKey)
        } else {
            try writeIndex(index)
        }
    }

    public func clearAllSessions() throws {
        try deleteItem(account: sessionsAccountKey)
    }

    // MARK: - SQLite Encryption Passphrase

    public func loadOrCreateStoragePassphrase() throws -> String {
        if let existing = try readData(account: passphraseAccountKey), existing.count == 32 {
            return existing.base64EncodedString()
        }
        var bytes = [UInt8](repeating: 0, count: 32)
        let status = bytes.withUnsafeMutableBytes { rawBuf in
            guard let base = rawBuf.baseAddress else { return errSecParam }
            return SecRandomCopyBytes(kSecRandomDefault, rawBuf.count, base)
        }
        guard status == errSecSuccess else {
            throw HostBridgeError.keychainFailed(status)
        }
        let data = Data(bytes)
        try writeData(data, account: passphraseAccountKey)
        return data.base64EncodedString()
    }

    public func deleteStoragePassphrase() throws {
        try deleteItem(account: passphraseAccountKey)
    }

    // MARK: - Raw Keychain Helpers

    private func readIndex() throws -> StoredSessionIndex? {
        guard let data = try readData(account: sessionsAccountKey) else { return nil }
        return try JSONDecoder().decode(StoredSessionIndex.self, from: data)
    }

    private func writeIndex(_ index: StoredSessionIndex) throws {
        let data = try JSONEncoder().encode(index)
        try writeData(data, account: sessionsAccountKey)
    }

    private func readData(account: String) throws -> Data? {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: serviceNamespace,
            kSecAttrAccount as String: account,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        if status == errSecItemNotFound {
            return nil
        }
        guard status == errSecSuccess else {
            throw HostBridgeError.keychainFailed(status)
        }
        return result as? Data
    }

    private func writeData(_ data: Data, account: String) throws {
        let baseQuery: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: serviceNamespace,
            kSecAttrAccount as String: account,
        ]
        let attributesToUpdate: [String: Any] = [
            kSecValueData as String: data,
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
        ]
        let updateStatus = SecItemUpdate(baseQuery as CFDictionary, attributesToUpdate as CFDictionary)
        if updateStatus == errSecSuccess {
            return
        }
        if updateStatus != errSecItemNotFound {
            throw HostBridgeError.keychainFailed(updateStatus)
        }

        var addQuery = baseQuery
        addQuery[kSecValueData as String] = data
        addQuery[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        let addStatus = SecItemAdd(addQuery as CFDictionary, nil)
        guard addStatus == errSecSuccess else {
            throw HostBridgeError.keychainFailed(addStatus)
        }
    }

    private func deleteItem(account: String) throws {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: serviceNamespace,
            kSecAttrAccount as String: account,
        ]
        let status = SecItemDelete(query as CFDictionary)
        if status != errSecSuccess && status != errSecItemNotFound {
            throw HostBridgeError.keychainFailed(status)
        }
    }
}
