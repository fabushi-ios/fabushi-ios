import Foundation

/// iOS adaptation of Grok's box-exec-daemon boundary.
///
/// This layer must never point back upward into the renderer/preload bridge.
/// Desktop-only execution is delegated through a coordinator-owned transport.
protocol RemoteRunnerTransport: Sendable {
    func dispatch(method: String, params: CoordinatorPayload) async throws -> CoordinatorPayload
}

actor RemoteRunner {
    enum RunnerError: LocalizedError {
        case unsupportedOnDevice(String)

        var errorDescription: String? {
            switch self {
            case .unsupportedOnDevice(let method):
                return "remote_runner_required: \(method)"
            }
        }
    }

    private let transport: any RemoteRunnerTransport

    init(transport: any RemoteRunnerTransport) {
        self.transport = transport
    }

    func dispatch(method: String, params: CoordinatorPayload = .object([:])) async throws -> CoordinatorPayload {
        guard !method.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            throw RunnerError.unsupportedOnDevice("empty-method")
        }
        return try await transport.dispatch(method: method, params: params)
    }
}
