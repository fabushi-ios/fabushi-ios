import Foundation

enum IOSDevControlsGateError: LocalizedError, Equatable, Sendable {
    case disabled
    case invalidParams(String)
    case unsupportedMethod(String)

    var errorDescription: String? {
        switch self {
        case .disabled:
            "iOS developer controls are disabled outside a debug build"
        case .invalidParams(let detail):
            "Invalid iOS developer-control parameters: \(detail)"
        case .unsupportedMethod(let method):
            "Unsupported iOS developer-control method: \(method)"
        }
    }
}

struct IOSDevControlsGate: Equatable, Sendable {
    let enabled: Bool

    static func live() -> IOSDevControlsGate {
        #if DEBUG
        return IOSDevControlsGate(enabled: true)
        #else
        return IOSDevControlsGate(enabled: false)
        #endif
    }

    func requireEnabled() throws {
        guard enabled else {
            throw IOSDevControlsGateError.disabled
        }
    }
}

/// iOS-main implementation injected into MahayanaCoordinator. Dev requests are
/// handled before Host dispatch; non-dev requests cross the offline/latency
/// guard before the real production Host request path.
@MainActor
final class IOSNativeDevControlAdapter: CoordinatorDevControlAdapting {
    private let gate: IOSDevControlsGate
    private let gatewayOffline: IOSDevGatewayOfflineControl
    private let networkLatency: IOSDevNetworkLatency

    init(
        gate: IOSDevControlsGate,
        gatewayOffline: IOSDevGatewayOfflineControl = IOSDevGatewayOfflineControl(),
        networkLatency: IOSDevNetworkLatency = IOSDevNetworkLatency()
    ) {
        self.gate = gate
        self.gatewayOffline = gatewayOffline
        self.networkLatency = networkLatency
    }

    var isEnabled: Bool {
        gate.enabled
    }

    func route(
        method: String,
        params: [String: Any]
    ) async throws -> CoordinatorDevControlRouting {
        guard method.hasPrefix("dev.") else {
            return .notHandled
        }
        try gate.requireEnabled()

        switch method {
        case "dev.gateway.offline", "dev.setGatewayOffline":
            guard let induced = (params["induced"] as? Bool) ?? (params["offline"] as? Bool) else {
                throw IOSDevControlsGateError.invalidParams("setGatewayOffline requires induced/offline Bool")
            }
            return .handled(["induced": gatewayOffline.apply(induced)])

        case "dev.gatewayOfflineStatus":
            return .handled(["induced": gatewayOffline.isInduced])

        case "dev.network.latency", "dev.setNetworkLatency":
            guard let milliseconds = Self.integer(params["ms"]) else {
                throw IOSDevControlsGateError.invalidParams("setNetworkLatency requires numeric ms")
            }
            return .handled(["ms": networkLatency.setMilliseconds(milliseconds)])

        case "dev.networkLatencyStatus":
            return .handled(["ms": networkLatency.currentMilliseconds])

        default:
            throw IOSDevControlsGateError.unsupportedMethod(method)
        }
    }

    func beforeProductionRequest() async throws {
        guard gate.enabled else { return }
        try gatewayOffline.requireOnline()
        try await networkLatency.applyBeforeProductionRequest()
    }

    func coordinatorDidLaunch() {
        guard gate.enabled else { return }
        _ = gatewayOffline.reapplyAfterCoordinatorLaunch()
    }

    private static func integer(_ value: Any?) -> Int? {
        guard !(value is Bool) else { return nil }
        if let value = value as? Int {
            return value
        }
        if let value = value as? NSNumber {
            let double = value.doubleValue
            guard double.isFinite else { return nil }
            return Int(double)
        }
        if let value = value as? Double, value.isFinite {
            return Int(value)
        }
        return nil
    }
}
