import Foundation

@MainActor
final class PreloadDevControlsAPI {
    private let primary: PreloadPrimaryAPI

    init(primary: PreloadPrimaryAPI) {
        self.primary = primary
    }

    func gatewayOfflineStatus() async throws -> CoordinatorPayload {
        try await invoke(method: "gatewayOfflineStatus")
    }

    func setGatewayOffline(_ induced: Bool) async throws -> CoordinatorPayload {
        try await invoke(
            method: "setGatewayOffline",
            payload: .object(["induced": .bool(induced)])
        )
    }

    func networkLatencyStatus() async throws -> CoordinatorPayload {
        try await invoke(method: "networkLatencyStatus")
    }

    func setNetworkLatency(_ milliseconds: Int) async throws -> CoordinatorPayload {
        try await invoke(
            method: "setNetworkLatency",
            payload: .object(["ms": .number(Double(milliseconds))])
        )
    }

    func invoke(method: String, payload: CoordinatorPayload = .object([:])) async throws -> CoordinatorPayload {
        switch IOSDevControlsContract.disposition(for: method) {
        case .local:
            return try await primary.call(method: "dev.\(method)", payload: payload)
        case .remoteRunner:
            return try await primary.call(method: "remote-runner.dev.\(method)", payload: payload)
        case .unavailable(let reason):
            throw ControlPortCallError(code: "unsupported-ios-dev-control", message: reason)
        }
    }
}
