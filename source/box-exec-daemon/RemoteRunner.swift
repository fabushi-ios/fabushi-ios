import Foundation

/// iOS adaptation of Grok's box-exec-daemon boundary.
///
/// This layer must never point back upward into the renderer/preload bridge.
/// Desktop-only execution is delegated through a coordinator-owned transport.
protocol RemoteRunnerTransport: Sendable {
    func dispatch(method: String, params: CoordinatorPayload) async throws -> CoordinatorPayload
}

struct IOSAgentToolsPreparedWrite: Equatable, Sendable {
    let path: String
    let fileBytesBase64: String
    let sizeBytes: Int
    let lineCount: Int
}

enum IOSAgentToolsOutputMaterializer {
    static let mcpTextThresholdBytes = 40_000
    static let maxRemoteWriteBytes = 8 * 1024 * 1024

    static func prepare(
        content: String,
        thresholdBytes: Int = mcpTextThresholdBytes,
        projectDir: String = "/workspace",
        maxSizeBytes: Int = maxRemoteWriteBytes,
        identifier: String = UUID().uuidString.lowercased()
    ) -> IOSAgentToolsPreparedWrite? {
        guard thresholdBytes > 0 else { return nil }
        let original = Data(content.utf8)
        guard original.count > thresholdBytes else { return nil }

        let normalizedProjectDir = NSString(string: projectDir).standardizingPath
        guard normalizedProjectDir == projectDir,
              projectDir == "/workspace" || projectDir.hasPrefix("/workspace/"),
              !identifier.isEmpty,
              identifier.utf8.count <= 128,
              !identifier.contains("/"),
              !identifier.contains("\\"),
              !identifier.contains("\0"),
              maxSizeBytes > 0,
              maxSizeBytes <= maxRemoteWriteBytes
        else {
            return nil
        }

        var bounded = original.count > maxSizeBytes
            ? Data(original.prefix(maxSizeBytes))
            : original
        while !bounded.isEmpty, String(data: bounded, encoding: .utf8) == nil {
            bounded.removeLast()
        }
        guard let text = String(data: bounded, encoding: .utf8) else { return nil }

        let base = projectDir.hasSuffix("/")
            ? String(projectDir.dropLast())
            : projectDir
        return IOSAgentToolsPreparedWrite(
            path: "\(base)/agent-tools/\(identifier).txt",
            fileBytesBase64: bounded.base64EncodedString(),
            sizeBytes: bounded.count,
            lineCount: text.components(separatedBy: "\n").count
        )
    }

    enum SpillOutcome: Equatable, Sendable {
        case inline
        case failed
        case written(path: String, sizeBytes: Int, lineCount: Int)
    }

    /// Materialize large tool output through the canonical coordinator-owned remote executor.
    /// A write is accepted only when the remote confirms the exact fenced path requested.
    static func spill(
        content: String,
        toolCallId: String,
        thresholdBytes: Int = mcpTextThresholdBytes,
        projectDir: String = "/workspace",
        maxSizeBytes: Int = maxRemoteWriteBytes,
        identifier: String = UUID().uuidString.lowercased(),
        dispatch: @MainActor @Sendable (String, CoordinatorPayload) async throws -> CoordinatorPayload
    ) async -> SpillOutcome {
        guard thresholdBytes > 0 else { return .failed }
        guard content.utf8.count > thresholdBytes else { return .inline }
        guard !toolCallId.isEmpty,
              let prepared = prepare(
                  content: content,
                  thresholdBytes: thresholdBytes,
                  projectDir: projectDir,
                  maxSizeBytes: maxSizeBytes,
                  identifier: identifier
              )
        else {
            return .failed
        }

        let response: CoordinatorPayload
        do {
            response = try await dispatch(
                "local-exec.box.exec",
                .object([
                    "case": .string("writeArgs"),
                    "args": .object([
                        "path": .string(prepared.path),
                        "fileBytes": .string(prepared.fileBytesBase64),
                        "toolCallId": .string(toolCallId),
                    ]),
                ])
            )
        } catch {
            return .failed
        }

        guard case .object(let responseObject) = response,
              responseObject["success"] == .bool(true),
              case .string(let writtenPath)? = responseObject["path"],
              writtenPath == prepared.path
        else {
            return .failed
        }

        return .written(
            path: writtenPath,
            sizeBytes: prepared.sizeBytes,
            lineCount: prepared.lineCount
        )
    }
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
