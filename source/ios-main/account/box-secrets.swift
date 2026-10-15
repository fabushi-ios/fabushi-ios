import Foundation
import Security

enum IOSBoxSecretsError: Error, LocalizedError, Equatable {
    case accountRequired
    case validation(String)
    case keychain(OSStatus)
    case invalidStoredPayload
    case superseded
    case quiesced

    var errorDescription: String? {
        switch self {
        case .accountRequired:
            return "Box secrets can only change while an account is signed in."
        case .validation(let message):
            return message
        case .keychain(let status):
            return "Box secrets Keychain operation failed with status \(status)."
        case .invalidStoredPayload:
            return "Stored box secrets are invalid."
        case .superseded:
            return "Box secrets operation was superseded by an account transition."
        case .quiesced:
            return "Box secrets pushes are quiesced."
        }
    }
}

@MainActor
protocol IOSBoxSecretsStoring: AnyObject {
    func readSnapshot(accountSlot: String) async throws -> [String: String]
    func writeSnapshot(accountSlot: String, secrets: [String: String]) async throws
    func deleteSnapshot(accountSlot: String) async throws
}

@MainActor
final class IOSBoxSecretsKeychainStore: IOSBoxSecretsStoring {
    static let defaultService = "com.ombhrum.fabushi.box-secrets.v1"

    private let service: String

    init(service: String = IOSBoxSecretsKeychainStore.defaultService) {
        self.service = service
    }

    private func baseQuery(accountSlot: String) -> [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: accountSlot,
        ]
    }

    func readSnapshot(accountSlot: String) async throws -> [String: String] {
        var query = baseQuery(accountSlot: accountSlot)
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne

        var item: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &item)
        if status == errSecItemNotFound { return [:] }
        guard status == errSecSuccess else { throw IOSBoxSecretsError.keychain(status) }
        guard let data = item as? Data,
              let raw = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else {
            throw IOSBoxSecretsError.invalidStoredPayload
        }

        var result: [String: String] = [:]
        for (key, value) in raw {
            guard let value = value as? String else {
                throw IOSBoxSecretsError.invalidStoredPayload
            }
            result[key] = value
        }
        if let validation = validateBoxSecrets(result) {
            throw IOSBoxSecretsError.validation(validation)
        }
        return result
    }

    func writeSnapshot(accountSlot: String, secrets: [String: String]) async throws {
        if let validation = validateBoxSecrets(secrets) {
            throw IOSBoxSecretsError.validation(validation)
        }
        let data = try JSONSerialization.data(withJSONObject: secrets, options: [.sortedKeys])
        let query = baseQuery(accountSlot: accountSlot)
        let update: [String: Any] = [
            kSecValueData as String: data,
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
        ]
        let updateStatus = SecItemUpdate(query as CFDictionary, update as CFDictionary)
        if updateStatus == errSecSuccess { return }
        guard updateStatus == errSecItemNotFound else {
            throw IOSBoxSecretsError.keychain(updateStatus)
        }

        var insert = query
        insert[kSecValueData as String] = data
        insert[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        let insertStatus = SecItemAdd(insert as CFDictionary, nil)
        guard insertStatus == errSecSuccess else {
            throw IOSBoxSecretsError.keychain(insertStatus)
        }
    }

    func deleteSnapshot(accountSlot: String) async throws {
        let status = SecItemDelete(baseQuery(accountSlot: accountSlot) as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw IOSBoxSecretsError.keychain(status)
        }
    }
}

struct IOSBoxSecretsTelemetryProjection: Equatable, Sendable {
    let level: IOSLifecycleTelemetryLevel
    let metadata: [String: String]
}

@MainActor
final class IOSBoxSecretsController {
    typealias Apply = @MainActor (_ secrets: [String: String]) async throws -> Bool
    typealias Reporter = @MainActor (IOSBoxSecretsTelemetryProjection) -> Void

    private let store: any IOSBoxSecretsStoring
    private let apply: Apply
    private let reporter: Reporter

    private(set) var accountSlot: String?
    private(set) var generation: UInt64 = 0
    private var pushActive = false
    private var pushWaiters: [CheckedContinuation<Void, Never>] = []
    private var quiesced = false

    init(
        store: any IOSBoxSecretsStoring = IOSBoxSecretsKeychainStore(),
        apply: @escaping Apply,
        reporter: @escaping Reporter
    ) {
        self.store = store
        self.apply = apply
        self.reporter = reporter
    }

    @discardableResult
    func adoptAccount(_ slot: String?) -> UInt64 {
        generation = generation == UInt64.max ? 1 : generation + 1
        accountSlot = normalizedSlot(slot)
        return generation
    }

    func listKeys() async throws -> [String] {
        let (slot, expectedGeneration) = try requireActiveAccount()
        let snapshot = try await store.readSnapshot(accountSlot: slot)
        try requireCurrent(slot: slot, generation: expectedGeneration)
        return snapshot.keys.sorted()
    }

    func reveal(key: String) async throws -> String? {
        let (slot, expectedGeneration) = try requireActiveAccount()
        let snapshot = try await store.readSnapshot(accountSlot: slot)
        try requireCurrent(slot: slot, generation: expectedGeneration)
        return snapshot[key]
    }

    func upsert(_ entries: [String: String]) async throws -> Bool {
        let (slot, expectedGeneration) = try requireActiveAccount()
        var snapshot = try await store.readSnapshot(accountSlot: slot)
        try Task.checkCancellation()
        try requireCurrent(slot: slot, generation: expectedGeneration)
        for (key, value) in entries {
            snapshot[key] = value
        }
        if let validation = validateBoxSecrets(snapshot) {
            throw IOSBoxSecretsError.validation(validation)
        }
        try await store.writeSnapshot(accountSlot: slot, secrets: snapshot)
        try Task.checkCancellation()
        try requireCurrent(slot: slot, generation: expectedGeneration)
        return try await push(trigger: "edit")
    }

    func remove(_ keys: [String]) async throws -> Bool {
        let (slot, expectedGeneration) = try requireActiveAccount()
        var snapshot = try await store.readSnapshot(accountSlot: slot)
        try Task.checkCancellation()
        try requireCurrent(slot: slot, generation: expectedGeneration)
        for key in keys {
            snapshot.removeValue(forKey: key)
        }
        try await store.writeSnapshot(accountSlot: slot, secrets: snapshot)
        try Task.checkCancellation()
        try requireCurrent(slot: slot, generation: expectedGeneration)
        return try await push(trigger: "edit")
    }

    @discardableResult
    func push(trigger: String) async throws -> Bool {
        try await withPushLock {
            guard !self.quiesced else { throw IOSBoxSecretsError.quiesced }
            let slot = self.accountSlot
            let expectedGeneration = self.generation
            let secrets: [String: String]
            do {
                secrets = try await slot.map { try await self.store.readSnapshot(accountSlot: $0) } ?? [:]
            } catch {
                self.reportFailure(trigger: trigger, error: error, keyCount: nil)
                throw error
            }

            if let validation = validateBoxSecrets(secrets) {
                let error = IOSBoxSecretsError.validation(validation)
                self.reportFailure(trigger: trigger, error: error, keyCount: secrets.count)
                throw error
            }
            try Task.checkCancellation()
            guard expectedGeneration == self.generation, slot == self.accountSlot else {
                throw IOSBoxSecretsError.superseded
            }

            do {
                let applied = try await self.apply(secrets)
                guard expectedGeneration == self.generation, slot == self.accountSlot else {
                    throw IOSBoxSecretsError.superseded
                }
                self.reporter(.init(
                    level: .info,
                    metadata: [
                        "area": "box_secrets_push",
                        "outcome": "ok",
                        "trigger": String(trigger.prefix(64)),
                        "key_count": String(min(secrets.count, 999)),
                        "scope_present": String(slot != nil),
                        "applied": String(applied),
                    ]
                ))
                return applied
            } catch {
                self.reportFailure(trigger: trigger, error: error, keyCount: secrets.count)
                throw error
            }
        }
    }

    func departAccount() async {
        _ = adoptAccount(nil)
        do {
            _ = try await push(trigger: "account")
        } catch {
            // The account transition must continue even if Host is already down.
        }
    }

    func quiesce() async {
        quiesced = true
        await acquirePush()
        releasePush()
    }

    private func requireActiveAccount() throws -> (String, UInt64) {
        guard let accountSlot else { throw IOSBoxSecretsError.accountRequired }
        return (accountSlot, generation)
    }

    private func requireCurrent(slot: String, generation: UInt64) throws {
        guard generation == self.generation, slot == accountSlot else {
            throw IOSBoxSecretsError.superseded
        }
    }

    private func normalizedSlot(_ slot: String?) -> String? {
        let value = slot?.trimmingCharacters(in: .whitespacesAndNewlines)
        return value?.isEmpty == false ? value : nil
    }

    private func reportFailure(trigger: String, error: Error, keyCount: Int?) {
        let errorClass: String
        switch error {
        case IOSBoxSecretsError.keychain:
            errorClass = "keychain_locked"
        case is IOSBoxSecretsError:
            errorClass = "other"
        default:
            errorClass = "host_unreachable"
        }
        var metadata = [
            "area": "box_secrets_push",
            "outcome": "failed",
            "trigger": String(trigger.prefix(64)),
            "scope_present": String(accountSlot != nil),
            "error_class": errorClass,
        ]
        if let keyCount {
            metadata["key_count"] = String(min(max(keyCount, 0), 999))
        }
        reporter(.init(level: .warn, metadata: metadata))
    }

    private func acquirePush() async {
        if !pushActive {
            pushActive = true
            return
        }
        await withCheckedContinuation { continuation in
            pushWaiters.append(continuation)
        }
    }

    private func releasePush() {
        if pushWaiters.isEmpty {
            pushActive = false
            return
        }
        pushWaiters.removeFirst().resume()
    }

    private func withPushLock<T>(
        _ operation: @MainActor () async throws -> T
    ) async throws -> T {
        await acquirePush()
        defer { releasePush() }
        return try await operation()
    }
}

@MainActor
final class IOSBoxSecretsMainRPCAdapter {
    static let methods: Set<String> = [
        "boxSecretsList",
        "boxSecretsReveal",
        "boxSecretsUpsert",
        "boxSecretsDelete",
    ]

    private let controller: IOSBoxSecretsController

    init(controller: IOSBoxSecretsController) {
        self.controller = controller
    }

    func route(method: String, args: CoordinatorPayload) async throws -> CoordinatorReplyOutcome? {
        guard Self.methods.contains(method) else { return nil }
        let raw = args.foundationValue as? [String: Any] ?? [:]

        switch method {
        case "boxSecretsList":
            return .ok(try CoordinatorPayload.fromFoundation([
                "keys": try await controller.listKeys(),
                "isPersistent": true,
            ]))
        case "boxSecretsReveal":
            guard let key = raw["key"] as? String else {
                return .failed(.init(code: "invalid-params", message: "boxSecretsReveal requires key"))
            }
            let value = try await controller.reveal(key: key)
            return .ok(try CoordinatorPayload.fromFoundation(value ?? NSNull()))
        case "boxSecretsUpsert":
            guard let entries = raw["entries"] as? [String: Any] else {
                return .failed(.init(code: "invalid-params", message: "boxSecretsUpsert requires entries"))
            }
            let strings = entries.reduce(into: [String: String]()) { partial, row in
                if let value = row.value as? String { partial[row.key] = value }
            }
            let synced = try await controller.upsert(strings)
            return .ok(try CoordinatorPayload.fromFoundation(["synced": synced]))
        case "boxSecretsDelete":
            let keys = (raw["keys"] as? [Any] ?? []).compactMap { $0 as? String }
            let synced = try await controller.remove(keys)
            return .ok(try CoordinatorPayload.fromFoundation(["synced": synced]))
        default:
            return nil
        }
    }
}
