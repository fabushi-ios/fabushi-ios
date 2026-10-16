import Foundation

typealias OnePasswordProvisioningErrorCode = String

struct OnePasswordProvisioningError: LocalizedError, Equatable, Sendable {
    let code: OnePasswordProvisioningErrorCode
    let message: String

    var errorDescription: String? { message }
}

enum OnePasswordProvisioningAvailability: String, Equatable, Sendable {
    case available
    case unavailable
}

protocol OnePasswordProvisioningSink: Sendable {
    associatedtype Credential: Sendable
    var availability: OnePasswordProvisioningAvailability { get }
    func accept(_ credential: Credential) async throws
}

struct UnavailableOnePasswordProvisioningSink<Credential: Sendable>: OnePasswordProvisioningSink {
    let availability: OnePasswordProvisioningAvailability = .unavailable

    func accept(_ credential: Credential) async throws {
        throw OnePasswordProvisioningError(
            code: "sink-unavailable",
            message: "1Password provisioning is unavailable until a credential consumer is configured."
        )
    }
}
