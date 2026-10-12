import CryptoKit
import Foundation
import Security

struct LocalMiniAppSigningDescriptor: Equatable, Sendable {
    let keyId: String
    let publicKeyBase64URL: String
}

enum LocalMiniAppSigningError: LocalizedError {
    case invalidAccountScope
    case invalidStoredKey
    case keychain(OSStatus)

    var errorDescription: String? {
        switch self {
        case .invalidAccountScope:
            return "Local Mini App signing requires an authenticated account scope."
        case .invalidStoredKey:
            return "The local Mini App signing key is invalid."
        case let .keychain(status):
            return "Keychain operation failed (\(status))."
        }
    }
}

/// Native-only local-author signer. The raw Ed25519 private key never crosses
/// this adapter's API boundary and is persisted only as a ThisDeviceOnly
/// Keychain generic-password item. WebView, Agent, MCP, Rust and feature payloads
/// receive only the public descriptor and signatures.
actor LocalMiniAppSigningAuthority {
    private let service: String

    init(service: String = "com.ombhrum.fabushi.local-miniapp-author.ed25519.v1") {
        self.service = service
    }

    func descriptor(accountScope: String) throws -> LocalMiniAppSigningDescriptor {
        let key = try loadOrCreateKey(accountScope: accountScope)
        return Self.descriptor(for: key)
    }

    func sign(_ payload: Data, accountScope: String) throws -> Data {
        let key = try loadOrCreateKey(accountScope: accountScope)
        return try key.signature(for: payload)
    }

    func deleteKeyForTesting(accountScope: String) throws {
        let account = try keychainAccount(accountScope)
        let status = SecItemDelete([
            kSecClass: kSecClassGenericPassword,
            kSecAttrService: service,
            kSecAttrAccount: account,
        ] as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw LocalMiniAppSigningError.keychain(status)
        }
    }

    private func loadOrCreateKey(accountScope: String) throws -> Curve25519.Signing.PrivateKey {
        let account = try keychainAccount(accountScope)
        let query: [CFString: Any] = [
            kSecClass: kSecClassGenericPassword,
            kSecAttrService: service,
            kSecAttrAccount: account,
            kSecReturnData: true,
            kSecMatchLimit: kSecMatchLimitOne,
        ]
        var result: CFTypeRef?
        let readStatus = SecItemCopyMatching(query as CFDictionary, &result)
        if readStatus == errSecSuccess {
            guard let data = result as? Data,
                  let key = try? Curve25519.Signing.PrivateKey(rawRepresentation: data)
            else {
                throw LocalMiniAppSigningError.invalidStoredKey
            }
            return key
        }
        guard readStatus == errSecItemNotFound else {
            throw LocalMiniAppSigningError.keychain(readStatus)
        }

        let key = Curve25519.Signing.PrivateKey()
        let addStatus = SecItemAdd([
            kSecClass: kSecClassGenericPassword,
            kSecAttrService: service,
            kSecAttrAccount: account,
            kSecAttrAccessible: kSecAttrAccessibleWhenUnlockedThisDeviceOnly,
            kSecValueData: key.rawRepresentation,
        ] as CFDictionary, nil)
        if addStatus == errSecDuplicateItem {
            return try loadExistingKey(account: account)
        }
        guard addStatus == errSecSuccess else {
            throw LocalMiniAppSigningError.keychain(addStatus)
        }
        return key
    }

    private func loadExistingKey(account: String) throws -> Curve25519.Signing.PrivateKey {
        let query: [CFString: Any] = [
            kSecClass: kSecClassGenericPassword,
            kSecAttrService: service,
            kSecAttrAccount: account,
            kSecReturnData: true,
            kSecMatchLimit: kSecMatchLimitOne,
        ]
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        guard status == errSecSuccess,
              let data = result as? Data,
              let key = try? Curve25519.Signing.PrivateKey(rawRepresentation: data)
        else {
            if status != errSecSuccess {
                throw LocalMiniAppSigningError.keychain(status)
            }
            throw LocalMiniAppSigningError.invalidStoredKey
        }
        return key
    }

    private func keychainAccount(_ accountScope: String) throws -> String {
        let scope = accountScope.trimmingCharacters(in: .whitespacesAndNewlines)
        guard scope.count == 64,
              scope.allSatisfy({ $0.isHexDigit && !$0.isUppercase })
        else {
            throw LocalMiniAppSigningError.invalidAccountScope
        }
        return "account.\(scope)"
    }

    private static func descriptor(
        for key: Curve25519.Signing.PrivateKey
    ) -> LocalMiniAppSigningDescriptor {
        let publicKey = key.publicKey.rawRepresentation
        let fingerprint = SHA256.hash(data: publicKey)
            .map { String(format: "%02x", $0) }
            .joined()
        return LocalMiniAppSigningDescriptor(
            keyId: "local-ed25519-\(fingerprint.prefix(24))",
            publicKeyBase64URL: publicKey.base64URLEncodedString()
        )
    }
}

private extension Data {
    func base64URLEncodedString() -> String {
        base64EncodedString()
            .replacingOccurrences(of: "+", with: "-")
            .replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "=", with: "")
    }
}
