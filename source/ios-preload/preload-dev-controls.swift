import Foundation

#if DEBUG
@MainActor
final class IOSDevControlsPreload {
    private let bridge: IOSPreloadBridge

    init(bridge: IOSPreloadBridge) {
        self.bridge = bridge
    }

    func setGatewayOffline(_ offline: Bool) async throws {
        _ = try await bridge.request(
            method: "dev.gateway.offline",
            params: ["offline": offline]
        )
    }

    func setNetworkLatency(_ milliseconds: Int) async throws {
        _ = try await bridge.request(
            method: "dev.setNetworkLatency",
            params: ["ms": milliseconds]
        )
    }

    func gatewayOfflineStatus() async throws -> IOSPreloadBridge.JSONResult {
        try await bridge.request(method: "dev.gatewayOfflineStatus")
    }

    func networkLatencyStatus() async throws -> IOSPreloadBridge.JSONResult {
        try await bridge.request(method: "dev.networkLatencyStatus")
    }
}
#endif
