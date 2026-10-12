import Foundation

struct PasskeyChallenge: Equatable, Sendable {
    let relyingPartyID: String
    let challenge: Data
    let userVerificationRequired: Bool
}

struct PasskeyAssertion: Equatable, Sendable {
    let credentialID: Data
    let authenticatorData: Data
    let clientDataJSON: Data
    let signature: Data
    let userID: Data?
}

@MainActor
protocol PasskeyProviding: AnyObject {
    func assertion(for challenge: PasskeyChallenge) async throws -> PasskeyAssertion
}

/// Adapter seam for AuthenticationServices. UI presentation context stays in the
/// iOS platform layer while Coordinator only sees typed passkey data.
@MainActor
final class CoordinatorPasskeyProvider {
    private weak var provider: (any PasskeyProviding)?

    init(provider: any PasskeyProviding) {
        self.provider = provider
    }

    func assertion(for challenge: PasskeyChallenge) async throws -> PasskeyAssertion {
        guard let provider else {
            throw ControlPortCallError(code: "passkey-unavailable", message: "Passkey provider is unavailable")
        }
        return try await provider.assertion(for: challenge)
    }
}
