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
protocol IOSCursorCredentialStoring: IOSMachineIDSecretStore {
    func deleteSecret(_ key: String) async throws
}

@MainActor
final class IOSCursorCredentialStore: IOSCursorCredentialStoring {
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
    typealias StatusObserver = @MainActor (IOSCursorAuthStatus) -> Void

    private struct PendingLogin {
        let uuid: String
        let verifier: String
    }

    private let store: any IOSCursorCredentialStoring
    private let session: URLSession
    private let backendURL: URL
    private let websiteURL: URL
    private let machineIDResolver: IOSMachineIDResolver
    private let authTelemetry: IOSAuthTelemetryRelay
    private let sessionSettlementShipper: IOSCursorSessionSettlementShipper
    private var pending: [String: PendingLogin] = [:]
    private var refreshFailureCount = 0
    private var refreshDegradedSinceMs: Int64?
    private var statusObserver: StatusObserver?
    private var statusObserverGeneration: UInt64 = 0
    private var credentialsRevoked = false
    private var signoutSettled = false
    private var keychainUnavailableSettled = false

    init(
        store: any IOSCursorCredentialStoring = IOSCursorCredentialStore(),
        session: URLSession = .shared,
        backendURL: URL = URL(string: getConfiguredBackendUrl())!,
        websiteURL: URL = URL(string: "https://cursor.com")!,
        authTelemetry: IOSAuthTelemetryRelay = .init(),
        structuredLogRequestExecutor: IOSCursorStructuredLogBackend.RequestExecutor? = nil
    ) {
        self.store = store
        self.session = session
        self.backendURL = backendURL
        self.websiteURL = websiteURL
        let machineIDResolver = IOSMachineIDResolver(secrets: store)
        self.machineIDResolver = machineIDResolver
        self.authTelemetry = authTelemetry
        self.sessionSettlementShipper = IOSCursorSessionSettlementShipper(
            backendURL: backendURL,
            getMachineID: {
                try await machineIDResolver.getOrCreate()
            },
            session: session,
            requestExecutor: structuredLogRequestExecutor,
            reportFailure: { operation, error in
                authTelemetry.report(.init(
                    stream: .session,
                    level: .warn,
                    metadata: [
                        "phase": "settlement_ship_failed",
                        "operation": operation,
                        "error_type": String(reflecting: type(of: error)),
                    ]
                ))
            }
        )
    }

    func setStatusObserver(_ observer: StatusObserver?) {
        statusObserverGeneration = statusObserverGeneration == UInt64.max
            ? 1
            : statusObserverGeneration + 1
        statusObserver = observer
        guard let observer else { return }
        let generation = statusObserverGeneration
        Task { @MainActor [weak self] in
            guard let self,
                  generation == self.statusObserverGeneration,
                  self.statusObserver != nil
            else { return }
            observer(await self.status())
        }
    }

    private func publishStatus(_ status: IOSCursorAuthStatus) {
        statusObserver?(status)
    }

    func status() async -> IOSCursorAuthStatus {
        guard !credentialsRevoked else {
            return .init(loggedIn: false)
        }
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
        authTelemetry.report(iosCursorSigninTelemetry(.loginStarted))
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
        let data: Data
        let response: URLResponse
        do {
            (data, response) = try await session.data(for: request)
        } catch {
            authTelemetry.report(iosCursorSigninTelemetry(.loginFailed(cause: "error")))
            throw error
        }
        guard let http = response as? HTTPURLResponse else {
            throw IOSCursorAuthError.invalidLoginResponse
        }
        if http.statusCode == 404 { return nil }
        guard (200..<300).contains(http.statusCode) else {
            authTelemetry.report(iosCursorSigninTelemetry(.loginFailed(cause: "error")))
            throw IOSCursorAuthError.loginRejected(http.statusCode)
        }
        guard let object = try JSONSerialization.jsonObject(with: data) as? [String: Any],
              let accessToken = object["accessToken"] as? String,
              !accessToken.isEmpty,
              let refreshToken = object["refreshToken"] as? String,
              !refreshToken.isEmpty
        else {
            authTelemetry.report(iosCursorSigninTelemetry(.loginFailed(cause: "error")))
            throw IOSCursorAuthError.invalidLoginResponse
        }
        try await store.writeSecret(IOS_CURSOR_ACCESS_TOKEN_SECRET_KEY, value: accessToken)
        try await store.writeSecret(IOS_CURSOR_REFRESH_TOKEN_SECRET_KEY, value: refreshToken)
        credentialsRevoked = false
        signoutSettled = false
        keychainUnavailableSettled = false
        pending.removeValue(forKey: attemptId)
        let settled = await status()
        publishStatus(settled)
        authTelemetry.report(iosCursorSigninTelemetry(.loginCompleted))
        return settled
    }

    func cancelLogin(attemptId: String) {
        if pending.removeValue(forKey: attemptId) != nil {
            authTelemetry.report(iosCursorSigninTelemetry(.signedOut(cause: "forced")))
        }
    }

    func logout() async throws {
        try await clearCredentials(
            sessionCause: .userAction,
            signinCause: "user_logout"
        )
    }

    func getMachineID() async throws -> String {
        try await machineIDResolver.getOrCreate()
    }

    func getValidAccessToken(backendURL requestedBackendURL: String? = nil) async throws -> String {
        guard !credentialsRevoked else {
            throw IOSCursorAuthError.signInRequired
        }
        let resolved = requestedBackendURL.flatMap(URL.init(string:)) ?? backendURL
        let access: String?
        do {
            access = try await store.readSecret(IOS_CURSOR_ACCESS_TOKEN_SECRET_KEY)
        } catch {
            authTelemetry.report(iosCursorSessionTelemetry(.keychainUnavailable))
            throw error
        }

        let refresh: String?
        do {
            refresh = try await store.readSecret(IOS_CURSOR_REFRESH_TOKEN_SECRET_KEY)
        } catch {
            authTelemetry.report(iosCursorSessionTelemetry(.keychainUnavailable))
            if let access, !access.isEmpty {
                await shipKeychainUnavailableOnce(accessToken: access)
            }
            throw error
        }
        guard let access, !access.isEmpty, let refresh, !refresh.isEmpty else {
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
        let data: Data
        let response: URLResponse
        do {
            (data, response) = try await session.data(for: request)
        } catch {
            let nsError = error as NSError
            noteRefreshFailure(.network(String(nsError.code)))
            throw error
        }
        guard let http = response as? HTTPURLResponse else {
            noteRefreshFailure(.badPayload)
            throw IOSCursorAuthError.invalidLoginResponse
        }
        guard (200..<300).contains(http.statusCode) else {
            noteRefreshFailure(.httpStatus(http.statusCode))
            try? await clearCredentials(
                sessionCause: .sessionRevoked,
                signinCause: "session_expired"
            )
            throw IOSCursorAuthError.tokenRefreshFailed(http.statusCode)
        }
        guard let object = try JSONSerialization.jsonObject(with: data) as? [String: Any],
              object["shouldLogout"] as? Bool != true,
              let nextAccess = object["access_token"] as? String,
              !nextAccess.isEmpty
        else {
            noteRefreshFailure(.badPayload)
            try? await clearCredentials(
                sessionCause: .unparseable,
                signinCause: "session_expired"
            )
            throw IOSCursorAuthError.invalidLoginResponse
        }
        let nextRefresh: String
        if let rotated = object["refresh_token"] as? String, !rotated.isEmpty {
            nextRefresh = rotated
        } else {
            nextRefresh = refresh
        }
        do {
            try await store.writeSecret(IOS_CURSOR_ACCESS_TOKEN_SECRET_KEY, value: nextAccess)
            try await store.writeSecret(IOS_CURSOR_REFRESH_TOKEN_SECRET_KEY, value: nextRefresh)
        } catch {
            authTelemetry.report(iosCursorSessionTelemetry(.keychainUnavailable))
            await shipKeychainUnavailableOnce(accessToken: nextAccess)
            throw error
        }
        credentialsRevoked = false
        signoutSettled = false
        keychainUnavailableSettled = false
        if nextRefresh != refresh {
            authTelemetry.report(iosCursorSessionTelemetry(.rotationRescued))
        }
        noteRefreshRecovered()
        publishStatus(await status())
        return nextAccess
    }

    private func noteRefreshFailure(_ failure: IOSCursorSessionRefreshFailure) {
        refreshFailureCount = min(10_000, refreshFailureCount + 1)
        if refreshDegradedSinceMs == nil {
            refreshDegradedSinceMs = Int64(Date().timeIntervalSince1970 * 1_000)
        }
        authTelemetry.report(iosCursorSessionTelemetry(.refreshFailed(failure)))
    }

    private func noteRefreshRecovered() {
        guard refreshFailureCount > 0 else { return }
        let now = Int64(Date().timeIntervalSince1970 * 1_000)
        let degraded = max(0, now - (refreshDegradedSinceMs ?? now))
        authTelemetry.report(iosCursorSessionTelemetry(.refreshRecovered(
            consecutiveFailures: refreshFailureCount,
            degradedMs: Int(min(Int64(Int.max), degraded))
        )))
        refreshFailureCount = 0
        refreshDegradedSinceMs = nil
    }

    private func clearCredentials(
        sessionCause: IOSCursorSessionSignoutCause,
        signinCause: String
    ) async throws {
        pending.removeAll()

        // Freeze the departing account credential before revocation/deletion.
        // This is the only token the terminal settlement is allowed to use.
        let departingAccessToken = await readDepartingSessionToken()
        credentialsRevoked = true

        var deletionErrors: [Error] = []
        for key in [
            IOS_CURSOR_ACCESS_TOKEN_SECRET_KEY,
            IOS_CURSOR_REFRESH_TOKEN_SECRET_KEY,
        ] {
            do {
                try await store.deleteSecret(key)
            } catch {
                deletionErrors.append(error)
            }
        }
        let durable = deletionErrors.isEmpty

        let localProjection = iosCursorSessionTelemetry(.signedOut(
            cause: sessionCause,
            durable: durable
        ))
        authTelemetry.report(localProjection)
        authTelemetry.report(iosCursorSigninTelemetry(.signedOut(
            cause: durable ? signinCause : "retained_after_failed_logout"
        )))
        publishStatus(.init(loggedIn: false))

        if let departingAccessToken, !signoutSettled {
            signoutSettled = true
            await sessionSettlementShipper.ship(.signedOut(
                cause: sessionCause,
                durable: durable,
                accessToken: departingAccessToken
            ))
        }

        if !durable {
            authTelemetry.report(iosCursorSessionTelemetry(.keychainUnavailable))
            if let departingAccessToken {
                await shipKeychainUnavailableOnce(accessToken: departingAccessToken)
            }
            // Both deletes were attempted and terminal settlement was emitted
            // with the frozen token before surfacing the real Keychain failure.
            throw deletionErrors[0]
        }
    }

    private func readDepartingSessionToken() async -> String? {
        do {
            guard let access = try await store.readSecret(IOS_CURSOR_ACCESS_TOKEN_SECRET_KEY),
                  !access.isEmpty,
                  let refresh = try await store.readSecret(IOS_CURSOR_REFRESH_TOKEN_SECRET_KEY),
                  !refresh.isEmpty
            else { return nil }
            return access
        } catch {
            authTelemetry.report(.init(
                stream: .session,
                level: .warn,
                metadata: [
                    "phase": "session_settlement_read_failed",
                    "error_type": String(reflecting: type(of: error)),
                ]
            ))
            return nil
        }
    }

    private func shipKeychainUnavailableOnce(accessToken: String) async {
        guard !keychainUnavailableSettled else { return }
        keychainUnavailableSettled = true
        await sessionSettlementShipper.ship(.keychainUnavailable(
            accessToken: accessToken
        ))
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


/// Reconciles the authoritative Cursor team ceiling into the single local
/// settings owner. The only retained mutable state is an async generation
/// fence; the permission itself remains owned by SandSettingsStore.
@MainActor
final class IOSCursorLocalToolPermissionCeilingSynchronizer {
    typealias FetchCeiling = @MainActor () async throws -> SandLocalToolPermission?
    typealias SyncHost = @MainActor (SandLocalToolPermission) async throws -> Void
    typealias ReportFailure = @MainActor (_ area: String, _ leg: String, _ error: Error) -> Void

    private let settingsStore: SandSettingsStore
    private let fetchCeiling: FetchCeiling
    private let syncHostProjection: SyncHost
    private let reportFailure: ReportFailure
    private var generation: UInt64 = 0
    private var syncTask: Task<Void, Never>?

    init(
        settingsStore: SandSettingsStore,
        fetchCeiling: @escaping FetchCeiling,
        syncHost: @escaping SyncHost,
        reportFailure: @escaping ReportFailure
    ) {
        self.settingsStore = settingsStore
        self.fetchCeiling = fetchCeiling
        self.syncHostProjection = syncHost
        self.reportFailure = reportFailure
    }

    func consume(_ status: IOSCursorAuthStatus) {
        generation = generation == UInt64.max ? 1 : generation + 1
        let sequence = generation
        syncTask?.cancel()

        guard status.loggedIn else {
            let previous = settingsStore.getResolvedLocalToolPermission()
            settingsStore.setLocalToolPermissionCeiling(nil)
            let effective = settingsStore.getResolvedLocalToolPermission()
            guard effective != previous else {
                syncTask = nil
                return
            }
            syncTask = Task { @MainActor [weak self] in
                await self?.projectToHost(effective, sequence: sequence)
            }
            return
        }

        syncTask = Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                let ceiling = try await self.fetchCeiling()
                guard self.isCurrent(sequence) else { return }
                await self.apply(ceiling, sequence: sequence)
            } catch is CancellationError {
                return
            } catch {
                guard self.isCurrent(sequence) else { return }
                self.reportFailure("cursor-profile", "local-tool-ceiling-fetch", error)
                let failClosed = self.settingsStore.getLocalToolPermissionCeiling() ?? "never"
                await self.apply(failClosed, sequence: sequence)
            }
        }
    }

    func waitForIdleForTesting() async {
        await syncTask?.value
    }

    private func isCurrent(_ sequence: UInt64) -> Bool {
        sequence == generation
    }

    private func apply(
        _ ceiling: SandLocalToolPermission?,
        sequence: UInt64
    ) async {
        guard isCurrent(sequence) else { return }
        let previous = settingsStore.getResolvedLocalToolPermission()
        settingsStore.setLocalToolPermissionCeiling(ceiling)
        let effective = settingsStore.getResolvedLocalToolPermission()
        guard effective != previous else { return }
        await projectToHost(effective, sequence: sequence)
    }

    private func projectToHost(
        _ effective: SandLocalToolPermission,
        sequence: UInt64
    ) async {
        guard isCurrent(sequence) else { return }
        do {
            try await syncHostProjection(effective)
        } catch is CancellationError {
            return
        } catch {
            guard sequence == generation else { return }
            reportFailure("host-settings", "local-tool-ceiling", error)
            // If the normal projection failed, make one best-effort restrictive
            // projection through the same Host owner. The canonical store is
            // not duplicated or rolled back.
            do {
                try await syncHostProjection("never")
            } catch is CancellationError {
                return
            } catch {
                guard sequence == generation else { return }
                reportFailure("host-settings", "local-tool-ceiling-fail-closed", error)
            }
        }
    }
}
