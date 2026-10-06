import CryptoKit
import Foundation
import Security

let IOS_CURSOR_ACCESS_TOKEN_SECRET_KEY = "cursor-access-token"
let IOS_CURSOR_REFRESH_TOKEN_SECRET_KEY = "cursor-refresh-token"
let IOS_CURSOR_AUTH_REDIRECT_TARGET = "sand"

struct IOSCursorAuthStatus: Equatable, Sendable {
    let loggedIn: Bool
    var authId: String? = nil
    var email: String? = nil
    var expiresAtMs: Int64? = nil
}

struct IOSCursorLoginStart: Equatable, Sendable {
    let attemptId: String
    let loginURL: URL
}

enum IOSCursorAuthError: Error, LocalizedError, Equatable {
    case signInRequired
    case invalidLoginResponse
    case loginAttemptNotFound
    case loginRejected(Int)
    case tokenRefreshFailed(Int)
    case keychain(OSStatus)
    case secureRandom(OSStatus)

    var errorDescription: String? {
        switch self {
        case .signInRequired:
            "Sign in to the MCP backend before managing connector accounts."
        case .invalidLoginResponse:
            "The MCP backend returned an invalid authentication response."
        case .loginAttemptNotFound:
            "The MCP backend sign-in attempt is no longer active."
        case .loginRejected(let status):
            "MCP backend sign-in failed with HTTP \(status)."
        case .tokenRefreshFailed(let status):
            "MCP backend session refresh failed with HTTP \(status)."
        case .keychain(let status):
            "Keychain operation failed with status \(status)."
        case .secureRandom(let status):
            "Secure random generation failed with status \(status)."
        }
    }
}

@MainActor
final class IOSCursorCredentialStore: IOSMachineIDSecretStore {
    private let service: String

    init(service: String = "com.ombhrum.fabushi.cursor") {
        self.service = service
    }

    private func baseQuery(_ key: String) -> [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: key,
        ]
    }

    func readSecret(_ key: String) async throws -> String? {
        var query = baseQuery(key)
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne
        var item: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &item)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess else { throw IOSCursorAuthError.keychain(status) }
        guard let data = item as? Data,
              let value = String(data: data, encoding: .utf8) else {
            throw IOSCursorAuthError.invalidLoginResponse
        }
        return value
    }

    func writeSecret(_ key: String, value: String) async throws {
        let query = baseQuery(key)
        let data = Data(value.utf8)
        let update: [String: Any] = [
            kSecValueData as String: data,
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
        ]
        let updateStatus = SecItemUpdate(query as CFDictionary, update as CFDictionary)
        if updateStatus == errSecSuccess { return }
        guard updateStatus == errSecItemNotFound else {
            throw IOSCursorAuthError.keychain(updateStatus)
        }
        var insert = query
        insert[kSecValueData as String] = data
        insert[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        let insertStatus = SecItemAdd(insert as CFDictionary, nil)
        guard insertStatus == errSecSuccess else {
            throw IOSCursorAuthError.keychain(insertStatus)
        }
    }

    func deleteSecret(_ key: String) async throws {
        let status = SecItemDelete(baseQuery(key) as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw IOSCursorAuthError.keychain(status)
        }
    }

    func waitForEncryptedStorage() async throws {}
}

@MainActor
final class IOSCursorAuthService {
    private struct PendingLogin {
        let uuid: String
        let verifier: String
    }

    private let store: IOSCursorCredentialStore
    private let session: URLSession
    private let backendURL: URL
    private let websiteURL: URL
    private let machineIDResolver: IOSMachineIDResolver
    private var pending: [String: PendingLogin] = [:]

    init(
        store: IOSCursorCredentialStore = .init(),
        session: URLSession = .shared,
        backendURL: URL = URL(string: getConfiguredBackendUrl())!,
        websiteURL: URL = URL(string: "https://cursor.com")!
    ) {
        self.store = store
        self.session = session
        self.backendURL = backendURL
        self.websiteURL = websiteURL
        self.machineIDResolver = IOSMachineIDResolver(secrets: store)
    }

    func status() async -> IOSCursorAuthStatus {
        guard let token = try? await store.readSecret(IOS_CURSOR_ACCESS_TOKEN_SECRET_KEY),
              !token.isEmpty,
              let refreshToken = try? await store.readSecret(IOS_CURSOR_REFRESH_TOKEN_SECRET_KEY),
              !refreshToken.isEmpty
        else {
            return .init(loggedIn: false)
        }
        let payload = parseJwtPayload(token)
        return .init(
            loggedIn: true,
            authId: payload?.sub,
            email: payload?.email,
            expiresAtMs: getAccessTokenExpiryMs(token)
        )
    }

    func beginLogin() throws -> IOSCursorLoginStart {
        let verifierBytes = try secureRandomBytes(count: 32)
        let verifier = base64URL(verifierBytes)
        let challenge = base64URL(Data(SHA256.hash(data: Data(verifier.utf8))))
        let uuid = UUID().uuidString.lowercased()
        let attemptId = UUID().uuidString.lowercased()
        var components = URLComponents(
            url: websiteURL.appendingPathComponent("loginDeepControl"),
            resolvingAgainstBaseURL: false
        )
        components?.queryItems = [
            .init(name: "challenge", value: challenge),
            .init(name: "uuid", value: uuid),
            .init(name: "mode", value: "login"),
            .init(name: "redirectTarget", value: IOS_CURSOR_AUTH_REDIRECT_TARGET),
        ]
        guard let loginURL = components?.url else {
            throw IOSCursorAuthError.invalidLoginResponse
        }
        pending[attemptId] = .init(uuid: uuid, verifier: verifier)
        return .init(attemptId: attemptId, loginURL: loginURL)
    }

    func pollLogin(attemptId: String) async throws -> IOSCursorAuthStatus? {
        guard let login = pending[attemptId] else {
            throw IOSCursorAuthError.loginAttemptNotFound
        }
        var components = URLComponents(
            url: backendURL.appendingPathComponent("auth/poll"),
            resolvingAgainstBaseURL: false
        )
        components?.queryItems = [
            .init(name: "uuid", value: login.uuid),
            .init(name: "verifier", value: login.verifier),
        ]
        guard let url = components?.url else {
            throw IOSCursorAuthError.invalidLoginResponse
        }
        var request = URLRequest(url: url)
        request.timeoutInterval = 15
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.setValue("true", forHTTPHeaderField: "local-cli-mode")
        let (data, response) = try await session.data(for: request)
        guard let http = response as? HTTPURLResponse else {
            throw IOSCursorAuthError.invalidLoginResponse
        }
        if http.statusCode == 404 { return nil }
        guard (200..<300).contains(http.statusCode) else {
            throw IOSCursorAuthError.loginRejected(http.statusCode)
        }
        guard let object = try JSONSerialization.jsonObject(with: data) as? [String: Any],
              let accessToken = object["accessToken"] as? String,
              !accessToken.isEmpty,
              let refreshToken = object["refreshToken"] as? String,
              !refreshToken.isEmpty
        else {
            throw IOSCursorAuthError.invalidLoginResponse
        }
        try await store.writeSecret(IOS_CURSOR_ACCESS_TOKEN_SECRET_KEY, value: accessToken)
        try await store.writeSecret(IOS_CURSOR_REFRESH_TOKEN_SECRET_KEY, value: refreshToken)
        pending.removeValue(forKey: attemptId)
        return await status()
    }

    func cancelLogin(attemptId: String) {
        pending.removeValue(forKey: attemptId)
    }

    func logout() async throws {
        pending.removeAll()
        try await store.deleteSecret(IOS_CURSOR_ACCESS_TOKEN_SECRET_KEY)
        try await store.deleteSecret(IOS_CURSOR_REFRESH_TOKEN_SECRET_KEY)
    }

    func getMachineID() async throws -> String {
        try await machineIDResolver.getOrCreate()
    }

    func getValidAccessToken(backendURL requestedBackendURL: String? = nil) async throws -> String {
        let resolved = requestedBackendURL.flatMap(URL.init(string:)) ?? backendURL
        guard let access = try await store.readSecret(IOS_CURSOR_ACCESS_TOKEN_SECRET_KEY),
              !access.isEmpty,
              let refresh = try await store.readSecret(IOS_CURSOR_REFRESH_TOKEN_SECRET_KEY),
              !refresh.isEmpty
        else {
            throw IOSCursorAuthError.signInRequired
        }
        if !shouldRefreshAccessToken(resolved.absoluteString, accessToken: access) {
            return access
        }

        var request = URLRequest(url: resolved.appendingPathComponent("oauth/token"))
        request.httpMethod = "POST"
        request.timeoutInterval = 20
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONSerialization.data(withJSONObject: [
            "client_id": getAuthClientId(resolved.absoluteString),
            "grant_type": "refresh_token",
            "refresh_token": refresh,
        ])
        let (data, response) = try await session.data(for: request)
        guard let http = response as? HTTPURLResponse else {
            throw IOSCursorAuthError.invalidLoginResponse
        }
        guard (200..<300).contains(http.statusCode) else {
            try? await logout()
            throw IOSCursorAuthError.tokenRefreshFailed(http.statusCode)
        }
        guard let object = try JSONSerialization.jsonObject(with: data) as? [String: Any],
              object["shouldLogout"] as? Bool != true,
              let nextAccess = object["access_token"] as? String,
              !nextAccess.isEmpty
        else {
            try? await logout()
            throw IOSCursorAuthError.invalidLoginResponse
        }
        let nextRefresh: String
        if let rotated = object["refresh_token"] as? String, !rotated.isEmpty {
            nextRefresh = rotated
        } else {
            nextRefresh = refresh
        }
        try await store.writeSecret(IOS_CURSOR_ACCESS_TOKEN_SECRET_KEY, value: nextAccess)
        try await store.writeSecret(IOS_CURSOR_REFRESH_TOKEN_SECRET_KEY, value: nextRefresh)
        return nextAccess
    }

    private func secureRandomBytes(count: Int) throws -> Data {
        var bytes = [UInt8](repeating: 0, count: count)
        let status = bytes.withUnsafeMutableBytes { buffer in
            SecRandomCopyBytes(kSecRandomDefault, count, buffer.baseAddress!)
        }
        guard status == errSecSuccess else {
            throw IOSCursorAuthError.secureRandom(status)
        }
        return Data(bytes)
    }

    private func base64URL(_ data: Data) -> String {
        data.base64EncodedString()
            .replacingOccurrences(of: "+", with: "-")
            .replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "=", with: "")
    }
}
