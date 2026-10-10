import Foundation

@MainActor
final class PreloadPrimaryAPI {
    private let bridge: IOSPreloadBridge

    init(bridge: IOSPreloadBridge) {
        self.bridge = bridge
    }

    func call(method: String, payload: CoordinatorPayload = .object([:])) async throws -> CoordinatorPayload {
        guard case .object = payload, let params = payload.foundationValue as? [String: Any] else {
            throw ControlPortCallError(code: "invalid-params", message: "preload primary payload must be an object")
        }
        let result = try await bridge.request(method: method, params: params)
        return try CoordinatorPayload.fromFoundation(result.value)
    }
}
