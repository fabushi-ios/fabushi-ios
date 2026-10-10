import Foundation

struct MahayanaHostSettingsSnapshot: Equatable, Sendable {
    let hasSeenOnboarding: Bool?
}

private enum MahayanaHostSettingsTransportError: LocalizedError {
    case invalidResponse

    var errorDescription: String? {
        "Mahayana Host settings transport returned an invalid response"
    }
}

/// Owns the current local Mahayana Host generation.
///
/// A request that observes a native-host integrity failure is never replayed:
/// the failed request settles to the caller, while the supervisor replaces the
/// host generation for subsequent requests. The observed-generation guard
/// prevents a stale failure from replacing a newer recovered Host.
@MainActor
final class MahayanaLocalHostSupervisor {
    typealias HostFactory = @MainActor () throws -> any MahayanaHostRequesting

    private var host: any MahayanaHostRequesting
    private let factory: HostFactory

    private(set) var generation: UInt64 = 1
    private(set) var recoveryCount: UInt64 = 0

    init(
        host: any MahayanaHostRequesting,
        factory: @escaping HostFactory
    ) {
        self.host = host
        self.factory = factory
    }

    static func make(
        appDataDirectory: URL,
        featureHostTest: Bool
    ) throws -> MahayanaLocalHostSupervisor {
        let factory: HostFactory = {
            try MahayanaHostRuntime(
                appDataDirectory: appDataDirectory,
                featureHostTest: featureHostTest
            )
        }
        return MahayanaLocalHostSupervisor(
            host: try factory(),
            factory: factory
        )
    }

    func request(
        method: String,
        params: [String: Any]
    ) async throws -> MahayanaHostJSONResult {
        try await host.request(method: method, params: params)
    }

    func readHostSettings() async throws -> MahayanaHostSettingsSnapshot {
        let response = try await request(method: "getHostSettings", params: [:])
        return try Self.decodeHostSettings(response.value)
    }

    @discardableResult
    func pushHostSettings(
        _ settings: MahayanaHostSettingsSnapshot
    ) async throws -> MahayanaHostSettingsSnapshot {
        let encoded: Any = settings.hasSeenOnboarding.map { $0 as Any } ?? NSNull()
        let response = try await request(
            method: "setHostSettings",
            params: [
                "settings": [
                    "hasSeenOnboarding": encoded,
                ],
            ]
        )
        return try Self.decodeHostSettings(response.value)
    }

    private static func decodeHostSettings(
        _ value: Any
    ) throws -> MahayanaHostSettingsSnapshot {
        guard let object = value as? [String: Any],
              let raw = object["hasSeenOnboarding"]
        else {
            throw MahayanaHostSettingsTransportError.invalidResponse
        }
        if raw is NSNull {
            return .init(hasSeenOnboarding: nil)
        }
        guard let seen = raw as? Bool else {
            throw MahayanaHostSettingsTransportError.invalidResponse
        }
        return .init(hasSeenOnboarding: seen)
    }

    @discardableResult
    func recoverAfterFailure(observedGeneration: UInt64) throws -> Bool {
        guard observedGeneration == generation else {
            return false
        }

        let replacement = try factory()
        host = replacement
        generation = generation == UInt64.max ? 1 : generation + 1
        recoveryCount = recoveryCount == UInt64.max ? 1 : recoveryCount + 1
        return true
    }
}
