import Foundation

@MainActor
final class CoordinatorMainPortClient {
    private let client: CoordinatorControlPortClient

    init(port: CoordinatorPort, autoStart: Bool = true) {
        client = CoordinatorControlPortClient(port: port, autoStart: autoStart)
    }

    var readyObserved: Bool { client.readyObserved }

    func start() {
        client.start()
    }

    func call(method: String, args: CoordinatorPayload = .object([:])) async throws -> CoordinatorPayload {
        guard CoordinatorMainMethodRegistry.contains(method) else {
            throw ControlPortCallError(code: "unknown-main-method", message: "no coordinator-main method named \(method)")
        }
        return try await client.call(method: method, args: args)
    }

    func receive(_ frame: CoordinatorFrame) {
        client.receive(frame)
    }

    func portClosed() {
        client.portClosed()
    }

    func shutdown() {
        client.shutdown()
    }
}
