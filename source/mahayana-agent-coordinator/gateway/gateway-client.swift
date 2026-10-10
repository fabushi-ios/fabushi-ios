import Foundation

@MainActor
final class CoordinatorGatewayClient {
    struct Connection: Equatable, Sendable {
        let baseURL: URL
        let headers: [String: String]
    }

    typealias CommandTransport = @MainActor (_ connection: Connection, _ method: String, _ args: CoordinatorPayload) async throws -> CoordinatorPayload

    private let supervisor: CoordinatorHostSupervisor
    private let transport: CommandTransport

    init(supervisor: CoordinatorHostSupervisor, transport: @escaping CommandTransport) {
        self.supervisor = supervisor
        self.transport = transport
    }

    func dispatchCommand(method: String, args: CoordinatorPayload) async throws -> CoordinatorPayload {
        let connection = try await supervisor.ensureConnection()
        do {
            return try await transport(connection, method, args)
        } catch {
            supervisor.invalidateHealthCache()
            throw error
        }
    }
}
