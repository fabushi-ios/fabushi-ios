import Darwin
import Foundation
import Security


private enum MobileCIAccountSessionBootstrap {
    private static let encodedEnvironment = "FABUSHI_CI_ACCOUNT_SESSION_BASE64"
    private static let fileEnvironment = "FABUSHI_CI_ACCOUNT_SESSION_FILE"
    private static let maximumSessionBytes = 64 * 1024

    static func prepareIfPresent(appDataDirectory: URL) throws {
        #if DEBUG
        let environment = ProcessInfo.processInfo.environment
        guard environment["GITHUB_ACTIONS"] == "true",
              let encoded = environment[encodedEnvironment],
              !encoded.isEmpty
        else {
            return
        }
        guard encoded.utf8.count <= maximumSessionBytes * 2,
              let data = Data(base64Encoded: encoded),
              !data.isEmpty,
              data.count <= maximumSessionBytes
        else {
            throw MahayanaHostRuntime.HostError.requestFailed(
                "受保护 CI 登录会话编码无效或超过大小限制"
            )
        }

        let directory = appDataDirectory.appendingPathComponent("FabushiCI", isDirectory: true)
        try FileManager.default.createDirectory(
            at: directory,
            withIntermediateDirectories: true,
            attributes: [.posixPermissions: 0o700]
        )
        try FileManager.default.setAttributes(
            [.posixPermissions: 0o700],
            ofItemAtPath: directory.path
        )

        let sessionURL = directory.appendingPathComponent("account-session.json")
        try data.write(to: sessionURL, options: .atomic)
        try FileManager.default.setAttributes(
            [.posixPermissions: 0o600],
            ofItemAtPath: sessionURL.path
        )

        guard setenv(fileEnvironment, sessionURL.path, 1) == 0 else {
            throw MahayanaHostRuntime.HostError.requestFailed(
                "无法为受保护 CI 登录会话建立应用内私有文件路径"
            )
        }
        unsetenv(encodedEnvironment)
        #endif
    }
}



private final class MobileCanonicalCICommerceLedger {
    private static let miniAppId = "global-dharma"
    private static let capability = "local.prayer-wheel.start"
    private static let lifetimeSku = "local-prayer-wheel.lifetime"
    private static let lifetimeProductId = "prod.global-dharma.local-prayer-wheel.lifetime"

    private let stateURL: URL
    private let accountIdentity: String
    private var payments: [String: [String: Any]] = [:]
    private var idempotency: [String: String] = [:]
    private var entitled = false

    static func makeIfEligible(appDataDirectory: URL) throws -> MobileCanonicalCICommerceLedger? {
        #if DEBUG
        let env = ProcessInfo.processInfo.environment
        guard env["GITHUB_ACTIONS"] == "true",
              env["GITHUB_REPOSITORY"] == "fabushi-ios/fabushi-ios",
              let sha = env["GITHUB_SHA"], sha.count == 40,
              let sessionPath = env["FABUSHI_CI_ACCOUNT_SESSION_FILE"],
              !sessionPath.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        else { return nil }

        let data = try Data(contentsOf: URL(fileURLWithPath: sessionPath))
        guard let session = try JSONSerialization.jsonObject(with: data) as? [String: Any],
              let identity = stableAccountIdentity(session)
        else {
            throw MahayanaHostRuntime.HostError.requestFailed(
                "受保护 CI commerce ledger 缺少稳定 Fabushi account identity"
            )
        }
        return try MobileCanonicalCICommerceLedger(
            stateURL: appDataDirectory
                .appendingPathComponent("FabushiCI", isDirectory: true)
                .appendingPathComponent("canonical-commerce-ledger.json"),
            accountIdentity: identity
        )
        #else
        return nil
        #endif
    }

    private static func stableAccountIdentity(_ session: [String: Any]) -> String? {
        let user = session["user"] as? [String: Any] ?? [:]
        for value in [
            user["principalId"], user["principal_id"], user["id"], user["userId"], user["user_id"],
            session["principalId"], session["principal_id"], session["userId"], session["user_id"],
            session["userNo"], session["username"],
        ] {
            if let text = value as? String {
                let clean = text.trimmingCharacters(in: .whitespacesAndNewlines)
                if !clean.isEmpty { return clean }
            }
            if let number = value as? NSNumber, String(cString: number.objCType) != "c" {
                return number.stringValue
            }
        }
        return nil
    }

    private init(stateURL: URL, accountIdentity: String) throws {
        self.stateURL = stateURL
        self.accountIdentity = accountIdentity
        if let data = try? Data(contentsOf: stateURL),
           let state = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
           state["accountIdentity"] as? String == accountIdentity {
            entitled = state["entitled"] as? Bool == true
            idempotency = state["idempotency"] as? [String: String] ?? [:]
            if let stored = state["payments"] as? [String: [String: Any]] {
                payments = stored
            } else if let stored = state["payments"] as? [String: Any] {
                payments = stored.compactMapValues { $0 as? [String: Any] }
            }
        } else {
            try persist()
        }
    }

    func responseIfHandled(method: String, params: [String: Any]) throws -> [String: Any]? {
        guard method == "platform.request",
              let verb = (params["method"] as? String)?.uppercased(),
              let path = params["path"] as? String
        else { return nil }

        if verb == "GET",
           path == "/v1/plugins/\(Self.miniAppId)/entitlements/\(Self.capability)" {
            return response(data: entitlementProjection())
        }

        if verb == "POST", path == "/v1/miniapps/\(Self.miniAppId)/pay/intents" {
            guard let body = params["body"] as? [String: Any],
                  body["sku"] as? String == Self.lifetimeSku,
                  body["rail"] as? String == "web_provider",
                  let key = body["idempotencyKey"] as? String,
                  (12...160).contains(key.count),
                  !key.contains(where: \.isWhitespace)
            else { return response(status: 400, data: ["code": "invalid_payment_intent"]) }

            if let paymentId = idempotency[key], let existing = payments[paymentId] {
                return response(data: existing)
            }
            let paymentId = UUID().uuidString.lowercased()
            let payment: [String: Any] = [
                "schema": "mahayana.miniapp.payment.v1",
                "paymentId": paymentId,
                "idempotencyKey": key,
                "miniAppId": Self.miniAppId,
                "sku": Self.lifetimeSku,
                "productKind": "digital_durable",
                "rail": "webProvider",
                "amount": 108_000,
                "currency": "CNY",
                "status": "requiresAction",
                "providerReference": "fabushi-ios-ci:\(paymentId)",
                "refundedAmount": 0,
            ]
            payments[paymentId] = payment
            idempotency[key] = paymentId
            try persist()
            return response(status: 201, data: payment)
        }

        if verb == "POST",
           path.hasPrefix("/v1/pay/intents/"),
           path.hasSuffix("/checkout") {
            let prefix = "/v1/pay/intents/"
            let suffix = "/checkout"
            let paymentId = String(path.dropFirst(prefix.count).dropLast(suffix.count))
            guard var payment = payments[paymentId] else {
                return response(status: 404, data: ["code": "payment_not_found"])
            }
            payment["status"] = "succeeded"
            payments[paymentId] = payment
            entitled = true
            try persist()
            return response(data: [
                "payment": payment,
                "checkoutAction": ["kind": "test", "provider": "fabushi-ios-ci", "completed": true],
                "callback": ["eventId": "fabushi-ios-ci:\(paymentId):succeeded", "duplicate": false],
            ])
        }

        if verb == "POST", path == "/v1/purchases/restore" {
            let purchases = payments.values.compactMap { payment -> [String: Any]? in
                guard payment["status"] as? String == "succeeded",
                      let paymentId = payment["paymentId"] as? String
                else { return nil }
                return [
                    "orderId": "fabushi-ios-ci-order:\(paymentId)",
                    "pluginId": Self.miniAppId,
                    "sku": Self.lifetimeSku,
                    "currency": "CNY",
                    "amount": 108_000,
                    "status": "fulfilled",
                ]
            }
            return response(data: ["purchases": purchases, "nextCursor": NSNull(), "restored": true])
        }
        return nil
    }

    private func entitlementProjection() -> [String: Any] {
        [
            "entitlement": entitled ? [
                "entitlementId": "fabushi-ios-ci-lifetime",
                "pluginId": Self.miniAppId,
                "capability": Self.capability,
                "status": "active",
                "expiresAt": NSNull(),
            ] : NSNull(),
            "access": [
                "protected": true,
                "allowed": entitled,
                "reason": entitled ? "active_durable_entitlement" : "not_entitled",
                "effectiveExpiresAt": NSNull(),
            ],
            "purchaseOptions": [[
                "productId": Self.lifetimeProductId,
                "sku": Self.lifetimeSku,
                "displayName": "本地转经轮永久版",
                "productKind": "digital_durable",
                "subscriptionPeriodSeconds": NSNull(),
                "currency": "CNY",
                "amount": 108_000,
                "activeRails": ["web_provider"],
            ]],
        ]
    }

    private func response(status: Int = 200, data: [String: Any]) -> [String: Any] {
        let body = (try? JSONSerialization.data(withJSONObject: data))
            .flatMap { String(data: $0, encoding: .utf8) } ?? "{}"
        return [
            "@type": "mahayana.platform.response",
            "ok": (200..<300).contains(status),
            "statusCode": status,
            "contentType": "application/json",
            "bodyText": body,
            "data": data,
        ]
    }

    private func persist() throws {
        let object: [String: Any] = [
            "schema": "fabushi.ios.ci-commerce-ledger.v1",
            "accountIdentity": accountIdentity,
            "payments": payments,
            "idempotency": idempotency,
            "entitled": entitled,
        ]
        let data = try JSONSerialization.data(withJSONObject: object, options: [.prettyPrinted, .sortedKeys])
        try data.write(to: stateURL, options: .atomic)
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: stateURL.path)
    }
}


private enum MobileAuthStoragePassphrase {
    private static let service = "com.ombhrum.fabushi.mahayana-storage.v1"
    private static let account = "default"

    static func loadOrCreate() throws -> String {
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
            throw MahayanaHostRuntime.HostError.requestFailed("无法读取系统 Keychain 登录存储密钥（\(status)）")
        }
        var bytes = [UInt8](repeating: 0, count: 32)
        let randomStatus = bytes.withUnsafeMutableBytes { buffer in
            guard let baseAddress = buffer.baseAddress else { return errSecParam }
            return SecRandomCopyBytes(kSecRandomDefault, buffer.count, baseAddress)
        }
        guard randomStatus == errSecSuccess else {
            throw MahayanaHostRuntime.HostError.requestFailed("无法生成系统登录存储密钥")
        }
        let data = Data(bytes)
        let add: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
            kSecValueData as String: data,
        ]
        let addStatus = SecItemAdd(add as CFDictionary, nil)
        guard addStatus == errSecSuccess else {
            throw MahayanaHostRuntime.HostError.requestFailed("无法写入系统 Keychain 登录存储密钥（\(addStatus)）")
        }
        return data.base64EncodedString()
    }
}

struct MahayanaHostJSONResult: @unchecked Sendable {
    let value: Any
}

protocol MahayanaHostRequesting: AnyObject, Sendable {
    @MainActor
    func request(method: String, params: [String: Any]) async throws -> MahayanaHostJSONResult
}

final class MahayanaHostRuntime: MahayanaHostRequesting, @unchecked Sendable {
    typealias JSONResult = MahayanaHostJSONResult

    enum HostError: LocalizedError {
        case initializationFailed
        case invalidResponse
        case requestFailed(String)

        var errorDescription: String? {
            switch self {
            case .initializationFailed: return "Mahayana Rust Host 初始化失败"
            case .invalidResponse: return "Mahayana Rust Host 返回了无效响应"
            case .requestFailed(let message): return message
            }
        }

        var requiresRecovery: Bool {
            switch self {
            case .initializationFailed, .invalidResponse:
                return true
            case .requestFailed(let message):
                return message.hasPrefix("host_fault[")
            }
        }
    }

    private let queue = DispatchQueue(label: "com.ombhrum.fabushi.mahayana-host", qos: .userInitiated)
    private let ciCommerceLedger: MobileCanonicalCICommerceLedger?
    private var handle: UnsafeMutableRawPointer?

    init(appDataDirectory: URL, featureHostTest: Bool = false) throws {
        try FileManager.default.createDirectory(at: appDataDirectory, withIntermediateDirectories: true)
        try MobileCIAccountSessionBootstrap.prepareIfPresent(appDataDirectory: appDataDirectory)
        ciCommerceLedger = try MobileCanonicalCICommerceLedger.makeIfEligible(appDataDirectory: appDataDirectory)
        if featureHostTest {
            handle = appDataDirectory.path.withCString { mahayana_app_host_create_test($0) }
        } else {
            let storagePassphrase = try MobileAuthStoragePassphrase.loadOrCreate()
            handle = appDataDirectory.path.withCString { path in
                storagePassphrase.withCString { passphrase in
                    mahayana_app_host_create_with_storage_passphrase(path, passphrase)
                }
            }
        }
        guard handle != nil else {
            if let pointer = mahayana_app_host_last_error() {
                let detail = String(cString: pointer)
                if !detail.isEmpty {
                    throw HostError.requestFailed("Mahayana Rust Host 初始化失败：\(detail)")
                }
            }
            throw HostError.initializationFailed
        }
    }

    deinit {
        if let handle { mahayana_app_host_destroy(handle) }
    }

    @MainActor
    func request(method: String, params: [String: Any] = [:]) async throws -> MahayanaHostJSONResult {
        let data = try JSONSerialization.data(withJSONObject: ["method": method, "params": params])
        guard let request = String(data: data, encoding: .utf8) else { throw HostError.invalidResponse }
        return try await withCheckedThrowingContinuation { continuation in
            queue.async { [self, request] in
                do {
                    continuation.resume(returning: MahayanaHostJSONResult(value: try requestSync(request)))
                } catch {
                    continuation.resume(throwing: error)
                }
            }
        }
    }

    private func requestSync(_ request: String) throws -> Any {
        guard let requestData = request.data(using: .utf8),
              let envelope = try JSONSerialization.jsonObject(with: requestData) as? [String: Any],
              let method = envelope["method"] as? String,
              let params = envelope["params"] as? [String: Any]
        else { throw HostError.invalidResponse }
        if let intercepted = try ciCommerceLedger?.responseIfHandled(method: method, params: params) {
            return intercepted
        }
        guard let handle else { throw HostError.initializationFailed }
        let pointer = request.withCString { mahayana_app_host_dispatch_with_handle(handle, $0) }
        guard let pointer else { throw HostError.invalidResponse }
        defer { mahayana_app_host_free_string(pointer) }
        let responseString = String(cString: pointer)
        guard let responseData = responseString.data(using: .utf8),
              let response = try JSONSerialization.jsonObject(with: responseData) as? [String: Any]
        else { throw HostError.invalidResponse }
        guard response["ok"] as? Bool == true else {
            throw HostError.requestFailed(response["error"] as? String ?? "Mahayana Host 请求失败")
        }
        return response["result"] ?? NSNull()
    }
}
