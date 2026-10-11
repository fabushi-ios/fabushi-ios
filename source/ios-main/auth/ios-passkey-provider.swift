import AuthenticationServices
import Foundation
import UIKit

@MainActor
final class IOSAuthenticationServicesPasskeyProvider:
    NSObject,
    PasskeyProviding,
    ASAuthorizationControllerDelegate,
    ASAuthorizationControllerPresentationContextProviding
{
    enum ProviderError: LocalizedError, Equatable {
        case relyingPartyNotAllowed(String)
        case ceremonyAlreadyInProgress
        case unexpectedCredential

        var errorDescription: String? {
            switch self {
            case .relyingPartyNotAllowed(let relyingPartyID):
                "passkey_relying_party_not_allowed: \(relyingPartyID)"
            case .ceremonyAlreadyInProgress:
                "passkey_ceremony_already_in_progress"
            case .unexpectedCredential:
                "passkey_unexpected_credential"
            }
        }
    }

    typealias NowMilliseconds = @MainActor () -> Int64

    private let allowedRelyingPartyIDs: Set<String>
    private let ceremonyDeadline: IOSPasskeyCeremonyDeadline
    private let nowMilliseconds: NowMilliseconds
    private var continuation: CheckedContinuation<PasskeyAssertion, Error>?
    private var authorizationController: ASAuthorizationController?
    private var activeCeremonyGeneration: UInt64?

    init(
        allowedRelyingPartyIDs: Set<String> = ["fabushi.app"],
        stallNanoseconds: UInt64 = IOSPasskeyStall.stallNanoseconds,
        nowMilliseconds: @escaping NowMilliseconds = {
            Int64(Date().timeIntervalSince1970 * 1_000)
        },
        reportStall: @escaping IOSPasskeyCeremonyDeadline.StallReporter = { _, _ in }
    ) {
        self.allowedRelyingPartyIDs = Set(
            allowedRelyingPartyIDs.map { $0.lowercased() }
        )
        self.nowMilliseconds = nowMilliseconds
        ceremonyDeadline = IOSPasskeyCeremonyDeadline(
            timeoutNanoseconds: stallNanoseconds,
            reportStall: reportStall
        )
        super.init()
    }

    nonisolated static func isAllowedRelyingPartyID(
        _ relyingPartyID: String,
        allowed: Set<String>
    ) -> Bool {
        let candidate = relyingPartyID
            .trimmingCharacters(in: .whitespacesAndNewlines)
            .lowercased()
        guard !candidate.isEmpty,
              candidate.count <= 253,
              candidate.range(
                of: #"^(?=.{1,253}$)(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])$"#,
                options: .regularExpression
              ) != nil
        else { return false }
        return allowed.map { $0.lowercased() }.contains(candidate)
    }

    func assertion(for challenge: PasskeyChallenge) async throws -> PasskeyAssertion {
        guard continuation == nil else {
            throw ProviderError.ceremonyAlreadyInProgress
        }
        guard Self.isAllowedRelyingPartyID(
            challenge.relyingPartyID,
            allowed: allowedRelyingPartyIDs
        ) else {
            throw ProviderError.relyingPartyNotAllowed(challenge.relyingPartyID)
        }

        let provider = ASAuthorizationPlatformPublicKeyCredentialProvider(
            relyingPartyIdentifier: challenge.relyingPartyID.lowercased()
        )
        let request = provider.createCredentialAssertionRequest(
            challenge: challenge.challenge
        )
        request.userVerificationPreference = challenge.userVerificationRequired
            ? .required
            : .preferred

        let sinceMilliseconds = nowMilliseconds()
        return try await withCheckedThrowingContinuation {
            (continuation: CheckedContinuation<PasskeyAssertion, Error>) in
            self.continuation = continuation
            let controller = ASAuthorizationController(
                authorizationRequests: [request]
            )
            authorizationController = controller
            controller.delegate = self
            controller.presentationContextProvider = self

            let generation = ceremonyDeadline.arm(
                method: "credentials.get",
                sinceMilliseconds: sinceMilliseconds
            ) { [weak self, weak controller] generation in
                guard let self, let controller else { return }
                self.settle(
                    .failure(IOSPasskeyStallError.stalled(method: "credentials.get")),
                    controller: controller,
                    generation: generation
                )
            }
            activeCeremonyGeneration = generation
            controller.performRequests()
        }
    }

    func authorizationController(
        controller: ASAuthorizationController,
        didCompleteWithAuthorization authorization: ASAuthorization
    ) {
        guard let credential = authorization.credential
            as? ASAuthorizationPlatformPublicKeyCredentialAssertion
        else {
            settle(
                .failure(ProviderError.unexpectedCredential),
                controller: controller
            )
            return
        }

        settle(
            .success(PasskeyAssertion(
                credentialID: credential.credentialID,
                authenticatorData: credential.rawAuthenticatorData,
                clientDataJSON: credential.rawClientDataJSON,
                signature: credential.signature,
                userID: credential.userID
            )),
            controller: controller
        )
    }

    func authorizationController(
        controller: ASAuthorizationController,
        didCompleteWithError error: Error
    ) {
        settle(.failure(error), controller: controller)
    }

    func presentationAnchor(
        for controller: ASAuthorizationController
    ) -> ASPresentationAnchor {
        let scenes = UIApplication.shared.connectedScenes
            .compactMap { $0 as? UIWindowScene }
        if let keyWindow = scenes.flatMap(\.windows).first(where: \.isKeyWindow) {
            return keyWindow
        }
        if let window = scenes.flatMap(\.windows).first {
            return window
        }
        return ASPresentationAnchor()
    }

    private func settle(
        _ result: Result<PasskeyAssertion, Error>,
        controller: ASAuthorizationController? = nil,
        generation: UInt64? = nil
    ) {
        if let controller, controller !== authorizationController { return }
        if let generation, generation != activeCeremonyGeneration { return }
        guard let continuation else { return }

        if let activeCeremonyGeneration {
            ceremonyDeadline.disarm(generation: activeCeremonyGeneration)
        }
        self.activeCeremonyGeneration = nil
        self.continuation = nil
        authorizationController = nil

        switch result {
        case .success(let assertion):
            continuation.resume(returning: assertion)
        case .failure(let error):
            continuation.resume(throwing: error)
        }
    }
}
