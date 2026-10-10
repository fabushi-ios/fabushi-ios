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
    private var handle: UnsafeMutableRawPointer?

    init(appDataDirectory: URL, featureHostTest: Bool = false) throws {
        try FileManager.default.createDirectory(at: appDataDirectory, withIntermediateDirectories: true)
        try MobileCIAccountSessionBootstrap.prepareIfPresent(appDataDirectory: appDataDirectory)
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
