import Foundation
import LocalAuthentication

public enum BiometricHardwareKind: String, Codable, Sendable, Equatable {
    case none
    case faceID
    case touchID
    case opticID
}

/// Protocol contract for Face ID / Touch ID evaluation (`PROJECT.md § IC.1`).
public protocol BiometricsProvider: Sendable {
    func availableBiometricKind() -> BiometricHardwareKind
    func authenticate(reason: String) async -> Bool
}

/// Wraps `LAContext` `.deviceOwnerAuthenticationWithBiometrics` and honors
/// `FABUSHI_MOCK_BIOMETRICS_RESULT=success|fail` when `FABUSHI_UI_TEST_MODE == "1"`.
public final class BiometricsAuthenticator: BiometricsProvider, Sendable {
    private let environment: [String: String]

    public init(environment: [String: String] = ProcessInfo.processInfo.environment) {
        self.environment = environment
    }

    public func availableBiometricKind() -> BiometricHardwareKind {
        if environment["FABUSHI_UI_TEST_MODE"] == "1",
           let mock = environment["FABUSHI_MOCK_BIOMETRICS_RESULT"]?.lowercased(),
           mock == "success" || mock == "fail" {
            return .faceID
        }
        let ctx = LAContext()
        var error: NSError?
        guard ctx.canEvaluatePolicy(.deviceOwnerAuthenticationWithBiometrics, error: &error) else {
            return .none
        }
        switch ctx.biometryType {
        case .faceID:
            return .faceID
        case .touchID:
            return .touchID
        case .opticID:
            return .opticID
        case .none:
            return .none
        @unknown default:
            return .none
        }
    }

    public func authenticate(reason: String) async -> Bool {
        if environment["FABUSHI_UI_TEST_MODE"] == "1",
           let mock = environment["FABUSHI_MOCK_BIOMETRICS_RESULT"]?.lowercased() {
            return mock == "success"
        }
        let ctx = LAContext()
        var error: NSError?
        guard ctx.canEvaluatePolicy(.deviceOwnerAuthenticationWithBiometrics, error: &error) else {
            return false
        }
        do {
            return try await ctx.evaluatePolicy(
                .deviceOwnerAuthenticationWithBiometrics,
                localizedReason: reason.isEmpty ? "Unlock Fabushi" : reason
            )
        } catch {
            return false
        }
    }
}
