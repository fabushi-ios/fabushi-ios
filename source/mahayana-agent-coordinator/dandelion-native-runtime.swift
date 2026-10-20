import CryptoKit
import Foundation
import Security
import UIKit
import UniformTypeIdentifiers

struct IOSDandelionNativeRuntimeGrant: Equatable, Sendable {
    let token: String
    let accountScope: String
    let scopeGeneration: UInt64
    let appId: String
    let version: String
    let packageDigest: String
}

protocol IOSDandelionNativeSecretStoring: AnyObject {
    func loadOrCreateSecret(accountScope: String, appId: String) throws -> Data
}

final class IOSDandelionNativeKeychainStore: IOSDandelionNativeSecretStoring {
    private let service = "com.ombhrum.fabushi.dandelion-native.v1"

    func loadOrCreateSecret(accountScope: String, appId: String) throws -> Data {
        let account = Self.key(accountScope: accountScope, appId: appId)
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
            return data
        }
        guard status == errSecItemNotFound else {
            throw MahayanaHostRuntime.HostError.requestFailed(
                "无法读取 Dandelion 原生运行时安全存储（\(status)）"
            )
        }
        var bytes = [UInt8](repeating: 0, count: 32)
        let randomStatus = bytes.withUnsafeMutableBytes { buffer in
            guard let baseAddress = buffer.baseAddress else { return errSecParam }
            return SecRandomCopyBytes(kSecRandomDefault, buffer.count, baseAddress)
        }
        guard randomStatus == errSecSuccess else {
            throw MahayanaHostRuntime.HostError.requestFailed(
                "无法生成 Dandelion 原生运行时安全存储密钥"
            )
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
            throw MahayanaHostRuntime.HostError.requestFailed(
                "无法写入 Dandelion 原生运行时安全存储（\(addStatus)）"
            )
        }
        return data
    }

    private static func key(accountScope: String, appId: String) -> String {
        let digest = SHA256.hash(data: Data("\(accountScope)\u{0}\(appId)".utf8))
        return digest.map { String(format: "%02x", $0) }.joined()
    }
}

@MainActor
private final class IOSDandelionDirectoryPicker: NSObject, UIDocumentPickerDelegate {
    private var continuation: CheckedContinuation<URL?, Never>?

    func pickDirectory() async -> URL? {
        guard continuation == nil, let presenter = Self.topViewController() else {
            return nil
        }
        return await withCheckedContinuation { continuation in
            self.continuation = continuation
            let picker = UIDocumentPickerViewController(
                forOpeningContentTypes: [.folder],
                asCopy: false
            )
            picker.allowsMultipleSelection = false
            picker.delegate = self
            presenter.present(picker, animated: true)
        }
    }

    func documentPicker(
        _ controller: UIDocumentPickerViewController,
        didPickDocumentsAt urls: [URL]
    ) {
        settle(urls.first)
    }

    func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) {
        settle(nil)
    }

    private func settle(_ url: URL?) {
        let pending = continuation
        continuation = nil
        pending?.resume(returning: url)
    }

    private static func topViewController() -> UIViewController? {
        let scene = UIApplication.shared.connectedScenes
            .compactMap { $0 as? UIWindowScene }
            .first { $0.activationState == .foregroundActive }
        var controller = scene?.windows.first(where: \.isKeyWindow)?.rootViewController
        while let presented = controller?.presentedViewController {
            controller = presented
        }
        return controller
    }
}

@MainActor
final class IOSDandelionNativeRuntimeBroker {
    typealias PickDirectory = @MainActor () async -> URL?
    typealias ConfirmRuntime = @MainActor (_ appId: String, _ version: String, _ digest: String) async -> Bool

    private let hostSupervisor: MahayanaLocalHostSupervisor
    private let pickDirectory: PickDirectory
    private let confirmRuntime: ConfirmRuntime
    private let beginSecurityScope: (URL) -> Bool
    private let endSecurityScope: (URL) -> Void
    private let secretStore: any IOSDandelionNativeSecretStoring
    private var accountScope: String?
    private var scopeGeneration: UInt64 = 0
    private var runtimeGrants: [String: IOSDandelionNativeRuntimeGrant] = [:]
    private var sourceURLs: [String: URL] = [:]
    private var syncTask: Task<Void, Never>?

    init(
        hostSupervisor: MahayanaLocalHostSupervisor,
        pickDirectory: PickDirectory? = nil,
        confirmRuntime: ConfirmRuntime? = nil,
        beginSecurityScope: @escaping (URL) -> Bool = { $0.startAccessingSecurityScopedResource() },
        endSecurityScope: @escaping (URL) -> Void = { $0.stopAccessingSecurityScopedResource() },
        secretStore: (any IOSDandelionNativeSecretStoring)? = nil
    ) {
        self.hostSupervisor = hostSupervisor
        if let pickDirectory {
            self.pickDirectory = pickDirectory
        } else {
            let picker = IOSDandelionDirectoryPicker()
            self.pickDirectory = { await picker.pickDirectory() }
        }
        self.confirmRuntime = confirmRuntime ?? { appId, version, digest in
            await Self.presentNativeRuntimeConfirmation(
                appId: appId,
                version: version,
                digest: digest
            )
        }
        self.beginSecurityScope = beginSecurityScope
        self.endSecurityScope = endSecurityScope
        self.secretStore = secretStore ?? IOSDandelionNativeKeychainStore()
    }

    func updateAccountScope(_ rawScope: String?) {
        let normalized = rawScope?.trimmingCharacters(in: .whitespacesAndNewlines)
        let next = normalized?.isEmpty == false ? normalized : nil
        guard next != accountScope else { return }
        scopeGeneration = scopeGeneration == UInt64.max ? 1 : scopeGeneration + 1
        syncTask?.cancel()
        syncTask = nil
        releaseAllSources()
        runtimeGrants.removeAll()
        accountScope = next
        let generation = scopeGeneration
        guard let next else {
            syncTask = Task { @MainActor [weak self] in
                try? await self?.hostSupervisor.dandelionNativeRequest(
                    method: "clearAccount",
                    params: [:]
                )
            }
            return
        }
        syncTask = Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                _ = try await self.syncCanonicalIdentity(
                    expectedScope: next,
                    expectedGeneration: generation
                )
            } catch {
                // Fail closed. A later explicit native action revalidates identity.
            }
        }
    }

    func prepareAccountTransition() async {
        scopeGeneration = scopeGeneration == UInt64.max ? 1 : scopeGeneration + 1
        syncTask?.cancel()
        syncTask = nil
        releaseAllSources()
        runtimeGrants.removeAll()
        accountScope = nil
        _ = try? await hostSupervisor.dandelionNativeRequest(
            method: "clearAccount",
            params: [:]
        )
    }

    func suspend() {
        scopeGeneration = scopeGeneration == UInt64.max ? 1 : scopeGeneration + 1
        syncTask?.cancel()
        syncTask = nil
        releaseAllSources()
        runtimeGrants.removeAll()
        Task { @MainActor [weak self] in
            _ = try? await self?.hostSupervisor.dandelionNativeRequest(
                method: "revokeAll",
                params: [:]
            )
        }
    }

    func stop() {
        scopeGeneration = scopeGeneration == UInt64.max ? 1 : scopeGeneration + 1
        syncTask?.cancel()
        syncTask = nil
        releaseAllSources()
        runtimeGrants.removeAll()
        accountScope = nil
        Task { @MainActor [weak self] in
            _ = try? await self?.hostSupervisor.dandelionNativeRequest(
                method: "clearAccount",
                params: [:]
            )
        }
    }

    func requestRuntimeGrant(
        appId rawAppId: String,
        version rawVersion: String,
        packageDigest rawDigest: String
    ) async throws -> IOSDandelionNativeRuntimeGrant? {
        guard let scope = accountScope else {
            throw MahayanaHostRuntime.HostError.requestFailed(
                "Dandelion 原生运行时没有当前账号作用域"
            )
        }
        let appId = try Self.requiredToken(rawAppId, label: "appId")
        let version = try Self.requiredToken(rawVersion, label: "version")
        let digest = rawDigest.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard digest.count == 64, digest.allSatisfy(\.isHexDigit) else {
            throw MahayanaHostRuntime.HostError.requestFailed(
                "Dandelion 原生运行时包摘要无效"
            )
        }
        let localGeneration = scopeGeneration
        _ = try await syncCanonicalIdentity(
            expectedScope: scope,
            expectedGeneration: localGeneration
        )
        guard await confirmRuntime(appId, version, digest),
              accountScope == scope,
              scopeGeneration == localGeneration
        else {
            return nil
        }
        let grant = IOSDandelionNativeRuntimeGrant(
            token: UUID().uuidString.lowercased(),
            accountScope: scope,
            scopeGeneration: localGeneration,
            appId: appId,
            version: version,
            packageDigest: digest
        )
        runtimeGrants[grant.token] = grant
        return grant
    }

    func registerSelectedSource(
        runtimeGrant: IOSDandelionNativeRuntimeGrant
    ) async throws -> String? {
        guard accepts(runtimeGrant), let scope = accountScope else {
            throw MahayanaHostRuntime.HostError.requestFailed(
                "Dandelion 原生运行时授权已失效"
            )
        }
        let localGeneration = scopeGeneration
        _ = try await syncCanonicalIdentity(
            expectedScope: scope,
            expectedGeneration: localGeneration
        )
        guard let url = await pickDirectory() else { return nil }
        guard accepts(runtimeGrant),
              accountScope == scope,
              scopeGeneration == localGeneration,
              beginSecurityScope(url)
        else {
            return nil
        }

        let grantId = UUID().uuidString.lowercased()
        do {
            _ = try await hostSupervisor.dandelionNativeRequest(
                method: "registerSource",
                params: [
                    "grantId": grantId,
                    "path": url.path,
                ]
            )
            guard accepts(runtimeGrant),
                  accountScope == scope,
                  scopeGeneration == localGeneration
            else {
                _ = try? await hostSupervisor.dandelionNativeRequest(
                    method: "revokeSource",
                    params: ["grantId": grantId]
                )
                endSecurityScope(url)
                throw CancellationError()
            }
            sourceURLs[grantId] = url
            return grantId
        } catch {
            endSecurityScope(url)
            throw error
        }
    }

    func probeSource(
        runtimeGrant: IOSDandelionNativeRuntimeGrant,
        grantId: String,
        relativePath: String,
        maxBytes: Int
    ) async throws -> [String: Any] {
        guard accepts(runtimeGrant),
              sourceURLs[grantId] != nil,
              (1...(8 * 1024 * 1024)).contains(maxBytes)
        else {
            throw MahayanaHostRuntime.HostError.requestFailed(
                "Dandelion 原生源授权或读取边界无效"
            )
        }
        let result = try await hostSupervisor.dandelionNativeRequest(
            method: "probeFile",
            params: [
                "grantId": grantId,
                "relativePath": relativePath,
                "maxBytes": maxBytes,
            ]
        )
        guard accepts(runtimeGrant),
              let object = result.value as? [String: Any]
        else {
            throw CancellationError()
        }
        return object
    }

    func revokeSource(_ grantId: String) async {
        if let url = sourceURLs.removeValue(forKey: grantId) {
            endSecurityScope(url)
        }
        _ = try? await hostSupervisor.dandelionNativeRequest(
            method: "revokeSource",
            params: ["grantId": grantId]
        )
    }

    func runtimeSecret(
        for runtimeGrant: IOSDandelionNativeRuntimeGrant
    ) throws -> Data {
        guard accepts(runtimeGrant) else {
            throw MahayanaHostRuntime.HostError.requestFailed(
                "Dandelion 原生运行时授权已失效"
            )
        }
        return try secretStore.loadOrCreateSecret(
            accountScope: runtimeGrant.accountScope,
            appId: runtimeGrant.appId
        )
    }

    private func accepts(_ grant: IOSDandelionNativeRuntimeGrant) -> Bool {
        runtimeGrants[grant.token] == grant
            && accountScope == grant.accountScope
            && scopeGeneration == grant.scopeGeneration
    }

    @discardableResult
    private func syncCanonicalIdentity(
        expectedScope: String,
        expectedGeneration: UInt64
    ) async throws -> [String: Any] {
        let identity = try await hostSupervisor.request(
            method: "feature.plugin.accountIdentity",
            params: [:]
        )
        guard !Task.isCancelled,
              accountScope == expectedScope,
              scopeGeneration == expectedGeneration,
              let object = identity.value as? [String: Any],
              object["protocol"] as? String == "fabushi.plugin-account-identity.v1",
              let scope = object["accountScope"] as? String,
              let generation = object["accountGeneration"] as? String,
              scope == expectedScope,
              scope.count == 64,
              generation.count == 64,
              scope.allSatisfy(\.isHexDigit),
              generation.allSatisfy(\.isHexDigit)
        else {
            throw MahayanaHostRuntime.HostError.requestFailed(
                "Dandelion 原生运行时账号租约无效或已过期"
            )
        }
        let result = try await hostSupervisor.dandelionNativeRequest(
            method: "syncAccount",
            params: [
                "accountScope": scope,
                "accountGeneration": generation,
            ]
        )
        guard !Task.isCancelled,
              accountScope == expectedScope,
              scopeGeneration == expectedGeneration,
              result.value is [String: Any]
        else {
            throw CancellationError()
        }
        return object
    }

    private func releaseAllSources() {
        let urls = sourceURLs.values
        sourceURLs.removeAll()
        for url in urls {
            endSecurityScope(url)
        }
    }

    private static func requiredToken(_ raw: String, label: String) throws -> String {
        let value = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !value.isEmpty, value.utf8.count <= 128 else {
            throw MahayanaHostRuntime.HostError.requestFailed(
                "Dandelion 原生运行时 \(label) 无效"
            )
        }
        return value
    }

    private static func presentNativeRuntimeConfirmation(
        appId: String,
        version: String,
        digest: String
    ) async -> Bool {
        guard let presenter = topViewController() else { return false }
        return await withCheckedContinuation { continuation in
            let alert = UIAlertController(
                title: "允许小程序原生运行时？",
                message: "\(appId) \(version)\n\(digest)\n\n仅在你信任该已签名版本时允许。",
                preferredStyle: .alert
            )
            alert.addAction(UIAlertAction(title: "取消", style: .cancel) { _ in
                continuation.resume(returning: false)
            })
            alert.addAction(UIAlertAction(title: "允许", style: .default) { _ in
                continuation.resume(returning: true)
            })
            presenter.present(alert, animated: true)
        }
    }

    private static func topViewController() -> UIViewController? {
        let scene = UIApplication.shared.connectedScenes
            .compactMap { $0 as? UIWindowScene }
            .first { $0.activationState == .foregroundActive }
        var controller = scene?.windows.first(where: \.isKeyWindow)?.rootViewController
        while let presented = controller?.presentedViewController {
            controller = presented
        }
        return controller
    }
}
