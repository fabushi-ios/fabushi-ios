import Foundation

@MainActor
final class CoordinatorMainLegs {
    private var current: CoordinatorMainPortClient?

    func adopt(_ client: CoordinatorMainPortClient) {
        current?.shutdown()
        current = client
    }

    func revoke() {
        current?.shutdown()
        current = nil
    }

    func call(_ method: String, args: CoordinatorPayload = .object([:])) async throws -> CoordinatorPayload {
        guard let current else {
            throw ControlPortCallError(code: "main-port-unavailable", message: "coordinator main-data port is not connected")
        }
        return try await current.call(method: method, args: args)
    }
}
