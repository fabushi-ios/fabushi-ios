import XCTest
@testable import Fabushi

@MainActor
private final class StubPasskeyProvider: PasskeyProviding {
    let assertionValue: PasskeyAssertion

    init(assertionValue: PasskeyAssertion) {
        self.assertionValue = assertionValue
    }

    func assertion(for challenge: PasskeyChallenge) async throws -> PasskeyAssertion {
        assertionValue
    }
}

final class PasskeyParityTests: XCTestCase {
    func testNativeProviderAllowsOnlyConfiguredRelyingParty() {
        XCTAssertTrue(IOSAuthenticationServicesPasskeyProvider.isAllowedRelyingPartyID(
            "fabushi.app",
            allowed: ["fabushi.app"]
        ))
        XCTAssertFalse(IOSAuthenticationServicesPasskeyProvider.isAllowedRelyingPartyID(
            "evil.example",
            allowed: ["fabushi.app"]
        ))
        XCTAssertFalse(IOSAuthenticationServicesPasskeyProvider.isAllowedRelyingPartyID(
            "https://fabushi.app",
            allowed: ["fabushi.app"]
        ))
    }

    @MainActor
    func testCoordinatorSignerSerializesNativeAssertion() async throws {
        let assertion = PasskeyAssertion(
            credentialID: Data([0x01, 0x02]),
            authenticatorData: Data([0x03]),
            clientDataJSON: Data([0x04]),
            signature: Data([0x05, 0x06]),
            userID: Data([0x07])
        )
        let provider = StubPasskeyProvider(assertionValue: assertion)
        let signer = CoordinatorWebAuthnSigner(
            passkeys: CoordinatorPasskeyProvider(provider: provider)
        )

        let result = try await signer.sign(PasskeyChallenge(
            relyingPartyID: "fabushi.app",
            challenge: Data([0x08]),
            userVerificationRequired: true
        ))

        guard case .object(let object) = result else {
            return XCTFail("expected passkey payload")
        }
        XCTAssertEqual(
            object["credentialId"],
            .string(assertion.credentialID.base64EncodedString())
        )
        XCTAssertEqual(
            object["signature"],
            .string(assertion.signature.base64EncodedString())
        )
        XCTAssertEqual(
            object["userId"],
            .string(assertion.userID!.base64EncodedString())
        )
    }
}
