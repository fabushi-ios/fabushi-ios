import Foundation

let iosMachineIDSecretKey = "cursor-machine-id"

@MainActor
protocol IOSMachineIDSecretStore: AnyObject {
    func readSecret(_ key: String) async throws -> String?
    func writeSecret(_ key: String, value: String) async throws
    func waitForEncryptedStorage() async throws
}

@MainActor
final class IOSMachineIDResolver {
    private let secrets: any IOSMachineIDSecretStore
    private let createID: () -> String

    init(
        secrets: any IOSMachineIDSecretStore,
        createID: @escaping () -> String = { UUID().uuidString.lowercased() }
    ) {
        self.secrets = secrets
        self.createID = createID
    }

    func getOrCreate() async throws -> String {
        if let existing = try await secrets.readSecret(iosMachineIDSecretKey) {
            return existing
        }

        try await secrets.waitForEncryptedStorage()
        if let afterSettle = try await secrets.readSecret(iosMachineIDSecretKey) {
            return afterSettle
        }

        let machineID = createID()
        try await secrets.writeSecret(iosMachineIDSecretKey, value: machineID)
        return machineID
    }
}
