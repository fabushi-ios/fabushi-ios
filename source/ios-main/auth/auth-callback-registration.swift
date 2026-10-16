import Foundation

enum IOSAuthCallbackRegistrationError: Error, Equatable {
    case missingURLScheme(String)
}

struct IOSAuthCallbackRegistration: Equatable, Sendable {
    let redirectTarget: String
    let protocolScheme: String
    let registered: Bool
}

/// iOS counterpart of Grok's Electron auth-callback registration.
///
/// iOS URL handlers are declared statically in the signed app Info.plist rather
/// than mutated at runtime. The production composition root validates that the
/// signed bundle still declares the same scheme used by the shared deep-link
/// parser and browser-auth callback contract.
enum IOSAuthCallbackRegistrar {
    static let redirectTarget = FabushiDeepLinkParser.customScheme
    static let protocolScheme = FabushiDeepLinkParser.customScheme

    static func inspect(infoDictionary: [String: Any]) -> IOSAuthCallbackRegistration {
        let declaredSchemes = Set(
            ((infoDictionary["CFBundleURLTypes"] as? [[String: Any]]) ?? [])
                .flatMap { ($0["CFBundleURLSchemes"] as? [String]) ?? [] }
                .map { $0.trimmingCharacters(in: .whitespacesAndNewlines).lowercased() }
                .filter { !$0.isEmpty }
        )
        return IOSAuthCallbackRegistration(
            redirectTarget: redirectTarget,
            protocolScheme: protocolScheme,
            registered: declaredSchemes.contains(protocolScheme)
        )
    }

    static func requireShippingRegistration(
        infoDictionary: [String: Any]
    ) throws -> IOSAuthCallbackRegistration {
        let registration = inspect(infoDictionary: infoDictionary)
        guard registration.registered else {
            throw IOSAuthCallbackRegistrationError.missingURLScheme(protocolScheme)
        }
        return registration
    }

    static func requireShippingRegistration(
        bundle: Bundle = .main
    ) throws -> IOSAuthCallbackRegistration {
        try requireShippingRegistration(infoDictionary: bundle.infoDictionary ?? [:])
    }
}
