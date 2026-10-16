import Foundation

@MainActor
final class CoordinatorWebAuthnSigner {
    private let passkeys: CoordinatorPasskeyProvider

    init(passkeys: CoordinatorPasskeyProvider) {
        self.passkeys = passkeys
    }

    func sign(_ challenge: PasskeyChallenge) async throws -> CoordinatorPayload {
        let assertion = try await passkeys.assertion(for: challenge)
        return .object([
            "credentialId": .string(assertion.credentialID.base64EncodedString()),
            "authenticatorData": .string(assertion.authenticatorData.base64EncodedString()),
            "clientDataJSON": .string(assertion.clientDataJSON.base64EncodedString()),
            "signature": .string(assertion.signature.base64EncodedString()),
            "userId": assertion.userID.map { .string($0.base64EncodedString()) } ?? .null,
        ])
    }
}
