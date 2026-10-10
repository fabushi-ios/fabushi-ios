import Foundation

#if DEBUG
@MainActor
final class IOSDevControls {
    private let main: IOSMainRuntime

    init(main: IOSMainRuntime) {
        self.main = main
    }

    func setGatewayOffline(_ offline: Bool) async throws {
        _ = try await main.dispatch(
            method: "dev.gateway.offline",
            params: ["offline": offline]
        )
    }

    func gatewayOfflineStatus() async throws -> MahayanaCoordinator.JSONResult {
        try await main.dispatch(method: "dev.gatewayOfflineStatus")
    }

    func setNetworkLatency(_ milliseconds: Int) async throws {
        _ = try await main.dispatch(
            method: "dev.setNetworkLatency",
            params: ["ms": milliseconds]
        )
    }

    func networkLatencyStatus() async throws -> MahayanaCoordinator.JSONResult {
        try await main.dispatch(method: "dev.networkLatencyStatus")
    }
}
#endif
