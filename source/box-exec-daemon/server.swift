import Foundation

enum IOSBoxExecOperation: Equatable, Sendable {
    enum TimeoutBehavior: Int, Equatable, Sendable {
        case cancel = 1
        case background = 2
    }

    case read(path: String, offset: Int?, limit: Int?)
    case redactedRead(path: String, offset: Int?, limit: Int?)
    case write(path: String, fileBytes: String, toolCallId: String)
    case shell(command: String, workingDirectory: String?)
    case shellStream(command: String, workingDirectory: String?)
    case shellStreamWithPolicy(
        command: String,
        workingDirectory: String?,
        timeoutMs: Int,
        hardTimeoutMs: Int?,
        timeoutBehavior: TimeoutBehavior,
        closeStdin: Bool
    )
    case backgroundShell(command: String, workingDirectory: String?)
    case writeShellStdin(shellId: Int, data: String, closeStdin: Bool)

    var messageCase: String {
        switch self {
        case .read: "readArgs"
        case .redactedRead: "redactedReadArgs"
        case .write: "writeArgs"
        case .shell: "shellArgs"
        case .shellStream, .shellStreamWithPolicy: "shellStreamArgs"
        case .backgroundShell: "backgroundShellSpawnArgs"
        case .writeShellStdin: "writeShellStdinArgs"
        }
    }

    var payload: CoordinatorPayload {
        var object: [String: CoordinatorPayload] = [:]
        switch self {
        case .read(let path, let offset, let limit),
             .redactedRead(let path, let offset, let limit):
            object["path"] = .string(path)
            if let offset { object["offset"] = .number(Double(offset)) }
            if let limit { object["limit"] = .number(Double(limit)) }
        case .write(let path, let fileBytes, let toolCallId):
            object["path"] = .string(path)
            object["fileBytes"] = .string(fileBytes)
            object["toolCallId"] = .string(toolCallId)
        case .shell(let command, let workingDirectory),
             .shellStream(let command, let workingDirectory),
             .backgroundShell(let command, let workingDirectory):
            object["command"] = .string(command)
            if let workingDirectory { object["workingDirectory"] = .string(workingDirectory) }
        case .shellStreamWithPolicy(
            let command,
            let workingDirectory,
            let timeoutMs,
            let hardTimeoutMs,
            let timeoutBehavior,
            let closeStdin
        ):
            object["command"] = .string(command)
            if let workingDirectory { object["workingDirectory"] = .string(workingDirectory) }
            object["timeout"] = .number(Double(timeoutMs))
            if let hardTimeoutMs { object["hardTimeout"] = .number(Double(hardTimeoutMs)) }
            object["timeoutBehavior"] = .number(Double(timeoutBehavior.rawValue))
            object["closeStdin"] = .bool(closeStdin)
        case .writeShellStdin(let shellId, let data, let closeStdin):
            object["shellId"] = .number(Double(shellId))
            object["data"] = .string(data)
            object["closeStdin"] = .bool(closeStdin)
        }
        return .object(object)
    }
}

/// iOS counterpart of the Desktop box-exec server.
///
/// iOS never starts a localhost shell daemon. Applicable local-exec requests
/// are represented as typed operations and delegated to the single
/// Coordinator-owned RemoteRunner transport. The Rust Host remains the
/// canonical protocol/state-machine owner.
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
