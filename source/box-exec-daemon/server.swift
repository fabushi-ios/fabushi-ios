import Foundation

enum IOSBoxExecOperation: Equatable, Sendable {
    case read(path: String, offset: Int?, limit: Int?)
    case shell(command: String, workingDirectory: String?)
    case shellStream(command: String, workingDirectory: String?)
    case backgroundShell(command: String, workingDirectory: String?)
    case writeShellStdin(shellId: Int, data: String, closeStdin: Bool)

    var messageCase: String {
        switch self {
        case .read: "readArgs"
        case .shell: "shellArgs"
        case .shellStream: "shellStreamArgs"
        case .backgroundShell: "backgroundShellSpawnArgs"
        case .writeShellStdin: "writeShellStdinArgs"
        }
    }

    var payload: CoordinatorPayload {
        var object: [String: CoordinatorPayload] = [:]
        switch self {
        case .read(let path, let offset, let limit):
            object["path"] = .string(path)
            if let offset { object["offset"] = .number(Double(offset)) }
            if let limit { object["limit"] = .number(Double(limit)) }
        case .shell(let command, let workingDirectory),
             .shellStream(let command, let workingDirectory),
             .backgroundShell(let command, let workingDirectory):
            object["command"] = .string(command)
            if let workingDirectory { object["workingDirectory"] = .string(workingDirectory) }
        case .writeShellStdin(let shellId, let data, let closeStdin):
            object["shellId"] = .number(Double(shellId))
            object["data"] = .string(data)
            object["closeStdin"] = .bool(closeStdin)
        }
        return .object(object)
    }
}

/// iOS counterpart of the desktop box-exec server.
///
/// iOS never starts a localhost shell daemon. The full desktop execution
/// message vocabulary is preserved, but execution is delegated to the
/// coordinator-owned RemoteRunner transport.
actor IOSBoxExecServerAdapter {
    private let runner: RemoteRunner

    init(runner: RemoteRunner) {
        self.runner = runner
    }

    func execute(_ operation: IOSBoxExecOperation) async throws -> CoordinatorPayload {
        try await runner.dispatch(
            method: "box.exec",
            params: .object([
                "case": .string(operation.messageCase),
                "args": operation.payload,
            ])
        )
    }
}
