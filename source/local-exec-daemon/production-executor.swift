import Foundation

protocol IOSRemoteExecutionProviding: Sendable {
    func dispatch(
        method: String,
        params: CoordinatorPayload
    ) async throws -> CoordinatorPayload
}

enum IOSRemoteExecutionProviderError: LocalizedError, Equatable, Sendable {
    case unavailable

    var errorDescription: String? {
        "Remote execution provider is not configured for this shipping build."
    }
}

struct IOSUnavailableRemoteExecutionProvider: IOSRemoteExecutionProviding {
    func dispatch(
        method: String,
        params: CoordinatorPayload
    ) async throws -> CoordinatorPayload {
        throw IOSRemoteExecutionProviderError.unavailable
    }
}

struct IOSClosureRemoteExecutionProvider: @unchecked Sendable, IOSRemoteExecutionProviding {
    typealias Dispatch = @MainActor (
        _ method: String,
        _ params: CoordinatorPayload
    ) async throws -> CoordinatorPayload

    private let dispatchImpl: Dispatch

    init(dispatch: @escaping Dispatch) {
        dispatchImpl = dispatch
    }

    func dispatch(
        method: String,
        params: CoordinatorPayload
    ) async throws -> CoordinatorPayload {
        try await dispatchImpl(method, params)
    }
}


enum IOSRemoteExecDaemonError: LocalizedError, Equatable, Sendable {
    case unsupported(String)
    case invalidRequest(String)
    case invalidResponse(String)
    case remote(String)

    var errorDescription: String? {
        switch self {
        case .unsupported(let method): "Remote exec daemon does not support \(method)."
        case .invalidRequest(let detail): "Remote exec request is invalid: \(detail)"
        case .invalidResponse(let detail): "Remote exec response is invalid: \(detail)"
        case .remote(let detail): "Remote exec failed: \(detail)"
        }
    }
}

enum IOSRemoteExecDaemonWire {
    private enum WireValue {
        case varint(UInt64)
        case bytes(Data)
    }

    private struct Reader {
        private let bytes: [UInt8]
        private var index = 0

        init(_ data: Data) { bytes = Array(data) }

        mutating func fields() throws -> [(Int, WireValue)] {
            var result: [(Int, WireValue)] = []
            while index < bytes.count {
                let key = try varint()
                let field = Int(key >> 3)
                guard field > 0, field <= 536_870_911 else {
                    throw IOSRemoteExecDaemonError.invalidResponse("invalid protobuf field number")
                }
                switch Int(key & 7) {
                case 0:
                    result.append((field, .varint(try varint())))
                case 1:
                    try skip(8)
                case 2:
                    let count = try length()
                    guard count <= bytes.count - index else {
                        throw IOSRemoteExecDaemonError.invalidResponse("truncated protobuf bytes")
                    }
                    result.append((field, .bytes(Data(bytes[index..<(index + count)]))))
                    index += count
                case 5:
                    try skip(4)
                default:
                    throw IOSRemoteExecDaemonError.invalidResponse("unsupported protobuf wire type")
                }
            }
            return result
        }

        private mutating func varint() throws -> UInt64 {
            var value: UInt64 = 0
            for byteIndex in 0..<10 {
                guard index < bytes.count else {
                    throw IOSRemoteExecDaemonError.invalidResponse("invalid protobuf varint")
                }
                let byte = bytes[index]
                index += 1
                if byteIndex == 9, byte > 0x01 {
                    throw IOSRemoteExecDaemonError.invalidResponse("protobuf varint overflow")
                }
                value |= UInt64(byte & 0x7f) << (byteIndex * 7)
                if byte & 0x80 == 0 { return value }
            }
            throw IOSRemoteExecDaemonError.invalidResponse("invalid protobuf varint")
        }

        private mutating func length() throws -> Int {
            let value = try varint()
            guard value <= UInt64(Int.max) else {
                throw IOSRemoteExecDaemonError.invalidResponse("protobuf length overflow")
            }
            return Int(value)
        }

        private mutating func skip(_ count: Int) throws {
            guard count <= bytes.count - index else {
                throw IOSRemoteExecDaemonError.invalidResponse("truncated fixed protobuf field")
            }
            index += count
        }
    }

    struct LsNode: Equatable {
        let absPath: String
        let childrenDirs: [LsNode]
        let childrenFiles: [String]
        let childrenWereProcessed: Bool
        let fullSubtreeExtensionCounts: [String: Int]
        let numFiles: Int

        var foundationValue: [String: Any] {
            [
                "absPath": absPath,
                "childrenDirs": childrenDirs.map(\.foundationValue),
                "childrenFiles": childrenFiles.map { ["name": $0] },
                "childrenWereProcessed": childrenWereProcessed,
                "fullSubtreeExtensionCounts": fullSubtreeExtensionCounts,
                "numFiles": numFiles,
            ]
        }
    }

    struct HookAdditionalContext: Equatable {
        let hookEventName: String
        let content: String

        var coordinatorPayload: CoordinatorPayload {
            .object([
                "hookEventName": .string(hookEventName),
                "content": .string(content),
            ])
        }
    }

    struct ExecutionMetadata: Equatable {
        var id: UInt32?
        var execID: String?
        var localExecutionTimeMs: Int?
        var hookAdditionalContexts: [HookAdditionalContext] = []

        var hasForwardableMetadata: Bool {
            localExecutionTimeMs != nil || !hookAdditionalContexts.isEmpty
        }

        var coordinatorPayload: CoordinatorPayload {
            var fields: [String: CoordinatorPayload] = [:]
            if let id { fields["id"] = .number(Double(id)) }
            if let execID { fields["execId"] = .string(execID) }
            if let localExecutionTimeMs {
                fields["localExecutionTimeMs"] = .number(Double(localExecutionTimeMs))
            }
            if !hookAdditionalContexts.isEmpty {
                fields["hookAdditionalContexts"] = .array(hookAdditionalContexts.map(\.coordinatorPayload))
            }
            return .object(fields)
        }
    }

    enum Event: Equatable {
        case stdout(String)
        case stderr(String)
        case exit(code: Int, cwd: String, aborted: Bool)
        case read([String: AnyHashable])
        case write([String: AnyHashable])
        case delete([String: AnyHashable])
        case fetch([String: AnyHashable])
        case shellResult([String: AnyHashable])
        case backgroundShellSpawn([String: AnyHashable])
        case writeShellStdin([String: AnyHashable])
        case forceBackgroundShellResult(status: AnyHashable, shellResult: [String: AnyHashable]?)
        case shellBackgrounded([String: AnyHashable])
        case shellSandboxUnsupported([String: AnyHashable])
        case grep(String)
        case lsSuccess(LsNode)
        case lsTimeout(LsNode)
        case lsError(path: String, error: String)
        case lsRejected(path: String, reason: String)
        case shellRejected(command: String, workingDirectory: String, reason: String)
        case shellPermissionDenied(command: String, workingDirectory: String, error: String, isReadonly: Bool)
        case heartbeat(UInt32)
        case streamClose(UInt32)
        case remoteFailure(id: UInt32, message: String, stackTrace: String?, errorCode: String?)
        case remoteError(String)
        case ignore
    }

    private static func sandboxPolicy(_ payload: CoordinatorPayload) throws -> Data {
        guard case .object(let object) = payload else {
            throw IOSRemoteExecDaemonError.invalidRequest("requestedSandboxPolicy must be an object")
        }
        let allowed = Set([
            "type",
            "networkAccess",
            "additionalReadwritePaths",
            "additionalReadonlyPaths",
            "debugOutputDir",
            "disableTmpWrite",
            "allowlistEscalated",
            "enableSharedBuildCache",
            "networkPolicy",
            "networkPolicyStrict",
            "captureDenies",
            "skipStatsigDefaults",
            "readBoundary",
            "additionalReadPaths",
        ])
        let unknown = Set(object.keys).subtracting(allowed)
        guard unknown.isEmpty else {
            throw IOSRemoteExecDaemonError.invalidRequest(
                "requestedSandboxPolicy contains unsupported fields: \(unknown.sorted().joined(separator: ", "))"
            )
        }

        var result = Data()
        if let type = try enumValue(
            object["type"],
            field: "requestedSandboxPolicy.type",
            names: [
                "UNSPECIFIED": 0,
                "TYPE_UNSPECIFIED": 0,
                "INSECURE_NONE": 1,
                "TYPE_INSECURE_NONE": 1,
                "WORKSPACE_READWRITE": 2,
                "TYPE_WORKSPACE_READWRITE": 2,
                "WORKSPACE_READONLY": 3,
                "TYPE_WORKSPACE_READONLY": 3,
            ],
            maximum: 3
        ), type != 0 {
            varint(1, type, into: &result)
        }
        if let value = try boolValue(object["networkAccess"], field: "requestedSandboxPolicy.networkAccess") {
            varint(2, value ? 1 : 0, into: &result)
        }
        for value in try stringArray(object["additionalReadwritePaths"], field: "requestedSandboxPolicy.additionalReadwritePaths") {
            bytes(3, Data(value.utf8), into: &result)
        }
        for value in try stringArray(object["additionalReadonlyPaths"], field: "requestedSandboxPolicy.additionalReadonlyPaths") {
            bytes(4, Data(value.utf8), into: &result)
        }
        if let value = try stringValue(object["debugOutputDir"], field: "requestedSandboxPolicy.debugOutputDir") {
            bytes(5, Data(value.utf8), into: &result)
        }
        if let value = try boolValue(object["disableTmpWrite"], field: "requestedSandboxPolicy.disableTmpWrite") {
            varint(7, value ? 1 : 0, into: &result)
        }
        if let value = try boolValue(object["allowlistEscalated"], field: "requestedSandboxPolicy.allowlistEscalated") {
            varint(8, value ? 1 : 0, into: &result)
        }
        if let value = try boolValue(object["enableSharedBuildCache"], field: "requestedSandboxPolicy.enableSharedBuildCache") {
            varint(9, value ? 1 : 0, into: &result)
        }
        if let value = object["networkPolicy"], value != .null {
            bytes(10, try networkPolicy(value), into: &result)
        }
        if let value = try boolValue(object["networkPolicyStrict"], field: "requestedSandboxPolicy.networkPolicyStrict") {
            varint(11, value ? 1 : 0, into: &result)
        }
        if let value = try boolValue(object["captureDenies"], field: "requestedSandboxPolicy.captureDenies") {
            varint(12, value ? 1 : 0, into: &result)
        }
        if let value = try boolValue(object["skipStatsigDefaults"], field: "requestedSandboxPolicy.skipStatsigDefaults") {
            varint(13, value ? 1 : 0, into: &result)
        }
        if let boundary = try enumValue(
            object["readBoundary"],
            field: "requestedSandboxPolicy.readBoundary",
            names: [
                "UNSPECIFIED": 0,
                "READ_BOUNDARY_MODE_UNSPECIFIED": 0,
                "SYSTEM": 1,
                "READ_BOUNDARY_MODE_SYSTEM": 1,
                "WORKSPACE": 2,
                "READ_BOUNDARY_MODE_WORKSPACE": 2,
                "CUSTOM": 3,
                "READ_BOUNDARY_MODE_CUSTOM": 3,
            ],
            maximum: 3
        ), boundary != 0 {
            varint(14, boundary, into: &result)
        }
        for value in try stringArray(object["additionalReadPaths"], field: "requestedSandboxPolicy.additionalReadPaths") {
            bytes(15, Data(value.utf8), into: &result)
        }
        return result
    }

    private static func networkPolicy(_ payload: CoordinatorPayload) throws -> Data {
        guard case .object(let object) = payload else {
            throw IOSRemoteExecDaemonError.invalidRequest("requestedSandboxPolicy.networkPolicy must be an object")
        }
        let allowed = Set(["version", "defaultAction", "deny", "allow", "logging"])
        let unknown = Set(object.keys).subtracting(allowed)
        guard unknown.isEmpty else {
            throw IOSRemoteExecDaemonError.invalidRequest(
                "requestedSandboxPolicy.networkPolicy contains unsupported fields: \(unknown.sorted().joined(separator: ", "))"
            )
        }

        var result = Data()
        if let version = try unsignedInteger(
            object["version"],
            field: "requestedSandboxPolicy.networkPolicy.version",
            maximum: UInt64(UInt32.max)
        ) {
            varint(1, version, into: &result)
        }
        if let action = try enumValue(
            object["defaultAction"],
            field: "requestedSandboxPolicy.networkPolicy.defaultAction",
            names: [
                "UNSPECIFIED": 0,
                "DEFAULT_ACTION_UNSPECIFIED": 0,
                "ALLOW": 1,
                "DEFAULT_ACTION_ALLOW": 1,
                "DENY": 2,
                "DEFAULT_ACTION_DENY": 2,
            ],
            maximum: 2
        ) {
            varint(2, action, into: &result)
        }
        for value in try stringArray(object["deny"], field: "requestedSandboxPolicy.networkPolicy.deny") {
            bytes(3, Data(value.utf8), into: &result)
        }
        for value in try stringArray(object["allow"], field: "requestedSandboxPolicy.networkPolicy.allow") {
            bytes(4, Data(value.utf8), into: &result)
        }
        if let logging = object["logging"], logging != .null {
            bytes(5, try networkPolicyLogging(logging), into: &result)
        }
        return result
    }

    private static func networkPolicyLogging(_ payload: CoordinatorPayload) throws -> Data {
        guard case .object(let object) = payload else {
            throw IOSRemoteExecDaemonError.invalidRequest(
                "requestedSandboxPolicy.networkPolicy.logging must be an object"
            )
        }
        let allowed = Set(["decisionLogPath", "logFormat"])
        let unknown = Set(object.keys).subtracting(allowed)
        guard unknown.isEmpty else {
            throw IOSRemoteExecDaemonError.invalidRequest(
                "requestedSandboxPolicy.networkPolicy.logging contains unsupported fields: \(unknown.sorted().joined(separator: ", "))"
            )
        }

        var result = Data()
        if let value = try stringValue(
            object["decisionLogPath"],
            field: "requestedSandboxPolicy.networkPolicy.logging.decisionLogPath"
        ) {
            bytes(1, Data(value.utf8), into: &result)
        }
        if let value = try stringValue(
            object["logFormat"],
            field: "requestedSandboxPolicy.networkPolicy.logging.logFormat"
        ) {
            bytes(2, Data(value.utf8), into: &result)
        }
        return result
    }

    private static func boolValue(_ payload: CoordinatorPayload?, field: String) throws -> Bool? {
        guard let payload, payload != .null else { return nil }
        guard case .bool(let value) = payload else {
            throw IOSRemoteExecDaemonError.invalidRequest("\(field) must be a boolean")
        }
        return value
    }

    private static func stringValue(_ payload: CoordinatorPayload?, field: String) throws -> String? {
        guard let payload, payload != .null else { return nil }
        guard case .string(let value) = payload else {
            throw IOSRemoteExecDaemonError.invalidRequest("\(field) must be a string")
        }
        return value
    }

    private static func stringArray(_ payload: CoordinatorPayload?, field: String) throws -> [String] {
        guard let payload, payload != .null else { return [] }
        guard case .array(let values) = payload else {
            throw IOSRemoteExecDaemonError.invalidRequest("\(field) must be an array of strings")
        }
        return try values.map { value in
            guard case .string(let value) = value else {
                throw IOSRemoteExecDaemonError.invalidRequest("\(field) must be an array of strings")
            }
            return value
        }
    }

    private static func unsignedInteger(
        _ payload: CoordinatorPayload?,
        field: String,
        maximum: UInt64
    ) throws -> UInt64? {
        guard let payload, payload != .null else { return nil }
        guard case .number(let value) = payload,
              value.isFinite,
              value.rounded(.towardZero) == value,
              value >= 0,
              value <= Double(maximum)
        else {
            throw IOSRemoteExecDaemonError.invalidRequest("\(field) must be a bounded unsigned integer")
        }
        return UInt64(value)
    }

    private static func int32Value(
        _ payload: CoordinatorPayload?,
        field: String
    ) throws -> Int32? {
        guard let payload, payload != .null else { return nil }
        guard case .number(let value) = payload,
              value.isFinite,
              value.rounded(.towardZero) == value,
              value >= Double(Int32.min),
              value <= Double(Int32.max)
        else {
            throw IOSRemoteExecDaemonError.invalidRequest("\(field) must be a bounded int32")
        }
        return Int32(value)
    }

    private static func enumValue(
        _ payload: CoordinatorPayload?,
        field: String,
        names: [String: UInt64],
        maximum: UInt64
    ) throws -> UInt64? {
        guard let payload, payload != .null else { return nil }
        switch payload {
        case .number:
            return try unsignedInteger(payload, field: field, maximum: maximum)
        case .string(let value):
            guard let raw = names[value] else {
                throw IOSRemoteExecDaemonError.invalidRequest("\(field) has an unsupported enum value")
            }
            return raw
        default:
            throw IOSRemoteExecDaemonError.invalidRequest("\(field) must be an enum name or number")
        }
    }

    static func shellRequest(
        id: UInt32,
        execID: String,
        command: String,
        workingDirectory: String,
        toolCallID: String,
        timeout: CoordinatorPayload? = nil,
        isBackground: CoordinatorPayload? = nil,
        timeoutBehavior: CoordinatorPayload? = nil,
        hardTimeout: CoordinatorPayload? = nil,
        requestedSandboxPolicy: CoordinatorPayload? = nil,
        adminCommandDenylist: CoordinatorPayload? = nil,
        closeStdin: CoordinatorPayload? = nil,
        messageField: UInt64 = 14
    ) throws -> Data {
        guard !command.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            throw IOSRemoteExecDaemonError.invalidRequest("shell command is empty")
        }
        guard messageField == 2 || messageField == 14 else {
            throw IOSRemoteExecDaemonError.invalidRequest("shell request field must be shellArgs or shellStreamArgs")
        }
        var shell = Data()
        bytes(1, Data(command.utf8), into: &shell)
        if !workingDirectory.isEmpty { bytes(2, Data(workingDirectory.utf8), into: &shell) }
        if let value = try int32Value(timeout, field: "timeout"), value != 0 {
            varint(3, UInt64(bitPattern: Int64(value)), into: &shell)
        }
        if !toolCallID.isEmpty { bytes(4, Data(toolCallID.utf8), into: &shell) }
        if let value = try boolValue(isBackground, field: "isBackground"), value {
            varint(11, 1, into: &shell)
        }
        if let value = try enumValue(
            timeoutBehavior,
            field: "timeoutBehavior",
            names: [
                "UNSPECIFIED": 0,
                "TIMEOUT_BEHAVIOR_UNSPECIFIED": 0,
                "CANCEL": 1,
                "TIMEOUT_BEHAVIOR_CANCEL": 1,
                "BACKGROUND": 2,
                "TIMEOUT_BEHAVIOR_BACKGROUND": 2,
            ],
            maximum: 2
        ), value != 0 {
            varint(13, value, into: &shell)
        }
        if let value = try int32Value(hardTimeout, field: "hardTimeout"), value != 0 {
            varint(14, UInt64(bitPattern: Int64(value)), into: &shell)
        }
        if let requestedSandboxPolicy, requestedSandboxPolicy != .null {
            bytes(9, try sandboxPolicy(requestedSandboxPolicy), into: &shell)
        }
        if let value = try boolValue(closeStdin, field: "closeStdin"), value {
            varint(17, 1, into: &shell)
        }
        for rule in try stringArray(adminCommandDenylist, field: "adminCommandDenylist") {
            bytes(22, Data(rule.utf8), into: &shell)
        }
        var root = Data()
        varint(1, UInt64(id), into: &root)
        if !execID.isEmpty { bytes(15, Data(execID.utf8), into: &root) }
        bytes(messageField, shell, into: &root)
        return root
    }

    static func backgroundShellSpawnRequest(
        id: UInt32,
        execID: String,
        command: String,
        workingDirectory: String,
        toolCallID: String,
        sandboxPolicy: CoordinatorPayload? = nil,
        enableWriteShellStdinTool: CoordinatorPayload? = nil,
        description: CoordinatorPayload? = nil,
        skipApproval: CoordinatorPayload? = nil,
        conversationID: CoordinatorPayload? = nil,
        adminCommandDenylist: CoordinatorPayload? = nil
    ) throws -> Data {
        guard !command.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
              command.utf8.count <= 256 * 1024
        else {
            throw IOSRemoteExecDaemonError.invalidRequest("background shell command is invalid")
        }
        var args = Data()
        bytes(1, Data(command.utf8), into: &args)
        if !workingDirectory.isEmpty { bytes(2, Data(workingDirectory.utf8), into: &args) }
        if !toolCallID.isEmpty { bytes(3, Data(toolCallID.utf8), into: &args) }
        if let sandboxPolicy, sandboxPolicy != .null {
            bytes(5, try Self.sandboxPolicy(sandboxPolicy), into: &args)
        }
        if let value = try boolValue(enableWriteShellStdinTool, field: "enableWriteShellStdinTool"), value {
            varint(6, 1, into: &args)
        }
        if let value = try stringValue(description, field: "description"), !value.isEmpty {
            bytes(7, Data(value.utf8), into: &args)
        }
        if let value = try boolValue(skipApproval, field: "skipApproval"), value {
            varint(12, 1, into: &args)
        }
        if let value = try stringValue(conversationID, field: "conversationId"), !value.isEmpty {
            bytes(13, Data(value.utf8), into: &args)
        }
        for rule in try stringArray(adminCommandDenylist, field: "adminCommandDenylist") {
            bytes(14, Data(rule.utf8), into: &args)
        }
        var root = Data()
        varint(1, UInt64(id), into: &root)
        if !execID.isEmpty { bytes(15, Data(execID.utf8), into: &root) }
        bytes(16, args, into: &root)
        return root
    }

    static func writeShellStdinRequest(
        id: UInt32,
        execID: String,
        shellID: Int,
        chars: String
    ) throws -> Data {
        guard shellID >= 0, UInt64(shellID) <= UInt64(UInt32.max) else {
            throw IOSRemoteExecDaemonError.invalidRequest("shellId must fit uint32")
        }
        guard chars.utf8.count <= 256 * 1024 else {
            throw IOSRemoteExecDaemonError.invalidRequest("stdin chars exceed bounded request size")
        }
        var args = Data()
        varint(1, UInt64(shellID), into: &args)
        bytes(2, Data(chars.utf8), into: &args)

        var root = Data()
        varint(1, UInt64(id), into: &root)
        if !execID.isEmpty { bytes(15, Data(execID.utf8), into: &root) }
        bytes(23, args, into: &root)
        return root
    }

    static func forceBackgroundShellRequest(
        id: UInt32,
        execID: String,
        toolCallID: String
    ) throws -> Data {
        var args = Data()
        if !toolCallID.isEmpty { bytes(1, Data(toolCallID.utf8), into: &args) }

        var root = Data()
        varint(1, UInt64(id), into: &root)
        if !execID.isEmpty { bytes(15, Data(execID.utf8), into: &root) }
        bytes(30, args, into: &root)
        return root
    }

    static func writeRequest(
        id: UInt32,
        execID: String,
        path: String,
        fileBytes: Data,
        toolCallID: String,
        returnFileContentAfterWrite: Bool = false,
        encodingHint: String? = nil
    ) throws -> Data {
        guard !path.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            throw IOSRemoteExecDaemonError.invalidRequest("write path is empty")
        }
        if let encodingHint, !encodingHint.isEmpty {
            guard encodingHint.utf8.count <= 128,
                  !encodingHint.contains("\0"),
                  !encodingHint.contains("\r"),
                  !encodingHint.contains("\n")
            else {
                throw IOSRemoteExecDaemonError.invalidRequest("write encodingHint is invalid")
            }
        }

        var write = Data()
        bytes(1, Data(path.utf8), into: &write)
        if !toolCallID.isEmpty { bytes(3, Data(toolCallID.utf8), into: &write) }
        if returnFileContentAfterWrite { varint(4, 1, into: &write) }
        bytes(5, fileBytes, into: &write)
        if let encodingHint, !encodingHint.isEmpty {
            bytes(6, Data(encodingHint.utf8), into: &write)
        }

        var root = Data()
        varint(1, UInt64(id), into: &root)
        if !execID.isEmpty { bytes(15, Data(execID.utf8), into: &root) }
        bytes(3, write, into: &root)
        return root
    }


    static func deleteRequest(
        id: UInt32,
        execID: String,
        path: String,
        toolCallID: String
    ) throws -> Data {
        guard !path.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            throw IOSRemoteExecDaemonError.invalidRequest("delete path is empty")
        }
        var delete = Data()
        bytes(1, Data(path.utf8), into: &delete)
        if !toolCallID.isEmpty { bytes(2, Data(toolCallID.utf8), into: &delete) }

        var root = Data()
        varint(1, UInt64(id), into: &root)
        if !execID.isEmpty { bytes(15, Data(execID.utf8), into: &root) }
        bytes(4, delete, into: &root)
        return root
    }

    static func fetchRequest(
        id: UInt32,
        execID: String,
        url: String,
        toolCallID: String
    ) throws -> Data {
        guard url.utf8.count <= 16 * 1024,
              let parsed = URL(string: url),
              let scheme = parsed.scheme?.lowercased(),
              (scheme == "http" || scheme == "https"),
              parsed.host != nil
        else {
            throw IOSRemoteExecDaemonError.invalidRequest("fetch url must be a bounded HTTP(S) URL")
        }
        var fetch = Data()
        bytes(1, Data(url.utf8), into: &fetch)
        if !toolCallID.isEmpty { bytes(2, Data(toolCallID.utf8), into: &fetch) }

        var root = Data()
        varint(1, UInt64(id), into: &root)
        if !execID.isEmpty { bytes(15, Data(execID.utf8), into: &root) }
        bytes(20, fetch, into: &root)
        return root
    }

    static func grepRequest(
        id: UInt32,
        execID: String,
        pattern: String,
        path: String?,
        glob: String?,
        outputMode: String?,
        contextBefore: Int?,
        contextAfter: Int?,
        context: Int?,
        caseInsensitive: Bool?,
        type: String?,
        headLimit: Int?,
        multiline: Bool?,
        sort: String?,
        sortAscending: Bool?,
        toolCallID: String,
        sandboxPolicy: CoordinatorPayload?,
        offset: Int?
    ) throws -> Data {
        guard !pattern.isEmpty,
              pattern.utf8.count <= 64 * 1024,
              !pattern.contains("\0")
        else {
            throw IOSRemoteExecDaemonError.invalidRequest("grep pattern is invalid")
        }
        func validateText(_ value: String?, field: String) throws {
            guard let value else { return }
            guard value.utf8.count <= 4_096,
                  !value.contains("\0"),
                  !value.contains("\r"),
                  !value.contains("\n")
            else {
                throw IOSRemoteExecDaemonError.invalidRequest("\(field) is invalid")
            }
        }
        for (value, field) in [
            (glob, "grep glob"),
            (outputMode, "grep outputMode"),
            (type, "grep type"),
            (sort, "grep sort"),
        ] {
            try validateText(value, field: field)
        }
        for (value, field) in [
            (contextBefore, "grep contextBefore"),
            (contextAfter, "grep contextAfter"),
            (context, "grep context"),
            (headLimit, "grep headLimit"),
            (offset, "grep offset"),
        ] {
            if let value, !(0...Int(Int32.max)).contains(value) {
                throw IOSRemoteExecDaemonError.invalidRequest("\(field) must be a non-negative Int32")
            }
        }

        var grep = Data()
        bytes(1, Data(pattern.utf8), into: &grep)
        if let path, !path.isEmpty { bytes(2, Data(path.utf8), into: &grep) }
        if let glob, !glob.isEmpty { bytes(3, Data(glob.utf8), into: &grep) }
        if let outputMode, !outputMode.isEmpty { bytes(4, Data(outputMode.utf8), into: &grep) }
        if let contextBefore { varint(5, UInt64(contextBefore), into: &grep) }
        if let contextAfter { varint(6, UInt64(contextAfter), into: &grep) }
        if let context { varint(7, UInt64(context), into: &grep) }
        if let caseInsensitive { varint(8, caseInsensitive ? 1 : 0, into: &grep) }
        if let type, !type.isEmpty { bytes(9, Data(type.utf8), into: &grep) }
        if let headLimit { varint(10, UInt64(headLimit), into: &grep) }
        if let multiline { varint(11, multiline ? 1 : 0, into: &grep) }
        if let sort, !sort.isEmpty { bytes(12, Data(sort.utf8), into: &grep) }
        if let sortAscending { varint(13, sortAscending ? 1 : 0, into: &grep) }
        if !toolCallID.isEmpty { bytes(14, Data(toolCallID.utf8), into: &grep) }
        if let sandboxPolicy {
            bytes(15, try Self.sandboxPolicy(sandboxPolicy), into: &grep)
        }
        if let offset { varint(16, UInt64(offset), into: &grep) }

        var root = Data()
        varint(1, UInt64(id), into: &root)
        if !execID.isEmpty { bytes(15, Data(execID.utf8), into: &root) }
        bytes(5, grep, into: &root)
        return root
    }

    static func lsRequest(
        id: UInt32,
        execID: String,
        path: String,
        ignore: [String],
        toolCallID: String,
        sandboxPolicy: CoordinatorPayload?,
        timeoutMs: Int?
    ) throws -> Data {
        guard !path.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            throw IOSRemoteExecDaemonError.invalidRequest("ls path is empty")
        }
        if let timeoutMs, timeoutMs < 0 {
            throw IOSRemoteExecDaemonError.invalidRequest("ls timeoutMs must be non-negative")
        }

        var ls = Data()
        bytes(1, Data(path.utf8), into: &ls)
        for pattern in ignore {
            guard pattern.utf8.count <= 4_096,
                  !pattern.contains("\0"),
                  !pattern.contains("\r"),
                  !pattern.contains("\n")
            else {
                throw IOSRemoteExecDaemonError.invalidRequest("ls ignore pattern is invalid")
            }
            bytes(2, Data(pattern.utf8), into: &ls)
        }
        if !toolCallID.isEmpty { bytes(3, Data(toolCallID.utf8), into: &ls) }
        if let sandboxPolicy, sandboxPolicy != .null {
            bytes(4, try Self.sandboxPolicy(sandboxPolicy), into: &ls)
        }
        if let timeoutMs {
            guard timeoutMs <= Int(UInt32.max) else {
                throw IOSRemoteExecDaemonError.invalidRequest("ls timeoutMs exceeds uint32")
            }
            varint(5, UInt64(timeoutMs), into: &ls)
        }

        var root = Data()
        varint(1, UInt64(id), into: &root)
        if !execID.isEmpty { bytes(15, Data(execID.utf8), into: &root) }
        bytes(8, ls, into: &root)
        return root
    }

    static func readRequest(
        id: UInt32,
        execID: String,
        path: String,
        toolCallID: String,
        offset: Int?,
        limit: Int?,
        encodingHint: String?,
        redacted: Bool
    ) throws -> Data {
        guard !path.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            throw IOSRemoteExecDaemonError.invalidRequest("read path is empty")
        }
        if let offset, offset < 0 {
            throw IOSRemoteExecDaemonError.invalidRequest("read offset must be non-negative")
        }
        if let limit, limit < 0 {
            throw IOSRemoteExecDaemonError.invalidRequest("read limit must be non-negative")
        }
        var read = Data()
        bytes(1, Data(path.utf8), into: &read)
        if !toolCallID.isEmpty { bytes(2, Data(toolCallID.utf8), into: &read) }
        if let offset { varint(4, UInt64(offset), into: &read) }
        if let limit { varint(5, UInt64(limit), into: &read) }
        if let encodingHint, !encodingHint.isEmpty { bytes(6, Data(encodingHint.utf8), into: &read) }

        var root = Data()
        varint(1, UInt64(id), into: &root)
        if !execID.isEmpty { bytes(15, Data(execID.utf8), into: &root) }
        bytes(redacted ? 29 : 7, read, into: &root)
        return root
    }

    static func appendingExecutionContext(
        to message: Data,
        execID: String,
        spanContext: CoordinatorPayload?,
        acceptHookAdditionalContexts: Bool?
    ) throws -> Data {
        guard !execID.isEmpty,
              execID.utf8.count <= 256,
              !execID.contains("\0"),
              !execID.contains("\r"),
              !execID.contains("\n")
        else {
            throw IOSRemoteExecDaemonError.invalidRequest("execution context execId is invalid")
        }

        var result = message
        if let existingExecID = try firstString(15, in: message) {
            guard existingExecID == execID else {
                throw IOSRemoteExecDaemonError.invalidRequest("execution context execId conflicts with request")
            }
        } else {
            bytes(15, Data(execID.utf8), into: &result)
        }

        if let spanContext, spanContext != .null {
            bytes(19, try encodeSpanContext(spanContext), into: &result)
        }
        if let acceptHookAdditionalContexts {
            varint(55, acceptHookAdditionalContexts ? 1 : 0, into: &result)
        }
        return result
    }

    private static func encodeSpanContext(_ payload: CoordinatorPayload) throws -> Data {
        guard case .object(let object) = payload else {
            throw IOSRemoteExecDaemonError.invalidRequest("spanContext must be an object")
        }
        let allowed = Set(["traceId", "spanId", "traceFlags", "traceState"])
        let unknown = Set(object.keys).subtracting(allowed)
        guard unknown.isEmpty else {
            throw IOSRemoteExecDaemonError.invalidRequest(
                "spanContext contains unsupported fields: \(unknown.sorted().joined(separator: ", "))"
            )
        }
        guard let traceID = try stringValue(object["traceId"], field: "spanContext.traceId"),
              !traceID.isEmpty,
              let spanID = try stringValue(object["spanId"], field: "spanContext.spanId"),
              !spanID.isEmpty
        else {
            throw IOSRemoteExecDaemonError.invalidRequest("spanContext requires non-empty traceId and spanId")
        }

        var result = Data()
        bytes(1, Data(traceID.utf8), into: &result)
        bytes(2, Data(spanID.utf8), into: &result)
        if let traceFlags = try unsignedInteger(
            object["traceFlags"],
            field: "spanContext.traceFlags",
            maximum: UInt64(UInt32.max)
        ) {
            varint(3, traceFlags, into: &result)
        }
        if let traceState = try stringValue(object["traceState"], field: "spanContext.traceState") {
            bytes(4, Data(traceState.utf8), into: &result)
        }
        return result
    }

    static func decodeExecutionMetadata(_ data: Data) throws -> ExecutionMetadata? {
        var root = Reader(data)
        var clientMessage: Data?
        for (field, value) in try root.fields() {
            switch (field, value) {
            case (1, .bytes(let nested)):
                clientMessage = nested
            case (2, .bytes):
                clientMessage = nil
            default:
                break
            }
        }
        guard let clientMessage else { return nil }

        var reader = Reader(clientMessage)
        var metadata = ExecutionMetadata()
        var sawMetadata = false
        for (field, value) in try reader.fields() {
            switch (field, value) {
            case (1, .varint(let raw)):
                guard raw <= UInt64(UInt32.max) else {
                    throw IOSRemoteExecDaemonError.invalidResponse("execution metadata id overflow")
                }
                metadata.id = UInt32(raw)
                sawMetadata = true
            case (15, .bytes(let raw)):
                guard let value = String(data: raw, encoding: .utf8) else {
                    throw IOSRemoteExecDaemonError.invalidResponse("execution metadata execId is not UTF-8")
                }
                metadata.execID = value
                sawMetadata = true
            case (39, .varint(let raw)):
                metadata.localExecutionTimeMs = Int(Int32(bitPattern: UInt32(truncatingIfNeeded: raw)))
                sawMetadata = true
            case (45, .bytes(let raw)):
                metadata.hookAdditionalContexts.append(try decodeHookAdditionalContext(raw))
                sawMetadata = true
            default:
                break
            }
        }
        return sawMetadata ? metadata : nil
    }

    private static func decodeHookAdditionalContext(_ data: Data) throws -> HookAdditionalContext {
        var reader = Reader(data)
        var hookEventName = ""
        var content = ""
        for (field, value) in try reader.fields() {
            switch (field, value) {
            case (1, .bytes(let raw)):
                guard let value = String(data: raw, encoding: .utf8) else {
                    throw IOSRemoteExecDaemonError.invalidResponse("hook additional context name is not UTF-8")
                }
                hookEventName = value
            case (2, .bytes(let raw)):
                guard let value = String(data: raw, encoding: .utf8) else {
                    throw IOSRemoteExecDaemonError.invalidResponse("hook additional context content is not UTF-8")
                }
                content = value
            default:
                break
            }
        }
        return HookAdditionalContext(hookEventName: hookEventName, content: content)
    }

    static func decodeElement(_ data: Data) throws -> Event {
        var root = Reader(data)
        var event: Event = .ignore
        for (field, value) in try root.fields() {
            guard case .bytes(let nested) = value else { continue }
            switch field {
            case 1: event = try decodeClientMessage(nested)
            case 2: event = try decodeControl(nested)
            default: break
            }
        }
        return event
    }

    private static func decodeClientMessage(_ data: Data) throws -> Event {
        var reader = Reader(data)
        var event: Event = .ignore
        for (field, value) in try reader.fields() {
            guard case .bytes(let nested) = value else { continue }
            switch field {
            case 2: event = try decodeShellResult(nested)
            case 3: event = try decodeWriteResult(nested)
            case 4: event = try decodeDeleteResult(nested)
            case 5: event = try decodeGrepResult(nested)
            case 14: event = try decodeShellStream(nested)
            case 16: event = try decodeBackgroundShellSpawnResult(nested)
            case 7, 29: event = try decodeReadResult(nested)
            case 8: event = try decodeLsResult(nested)
            case 20: event = try decodeFetchResult(nested)
            case 23: event = try decodeWriteShellStdinResult(nested)
            case 30: event = try decodeForceBackgroundShellResult(nested)
            default: continue
            }
        }
        return event
    }

    private static func decodeControl(_ data: Data) throws -> Event {
        var reader = Reader(data)
        var event: Event = .ignore
        for (field, value) in try reader.fields() {
            guard case .bytes(let nested) = value else { continue }
            switch field {
            case 1:
                let rawID = try firstVarint(1, in: nested) ?? 0
                guard rawID <= UInt64(UInt32.max) else {
                    throw IOSRemoteExecDaemonError.invalidResponse("streamClose id overflow")
                }
                event = .streamClose(UInt32(rawID))
            case 2:
                var thrownReader = Reader(nested)
                var id: UInt32 = 0
                var message: String?
                var stackTrace: String?
                var errorCode: String?
                for (inner, innerValue) in try thrownReader.fields() {
                    switch (inner, innerValue) {
                    case (1, .varint(let rawID)):
                        guard rawID <= UInt64(UInt32.max) else {
                            throw IOSRemoteExecDaemonError.invalidResponse("throw id overflow")
                        }
                        id = UInt32(rawID)
                    case (2, .bytes(let raw)):
                        guard let value = String(data: raw, encoding: .utf8) else {
                            throw IOSRemoteExecDaemonError.invalidResponse("throw error is not UTF-8")
                        }
                        message = value
                    case (3, .bytes(let raw)):
                        guard let value = String(data: raw, encoding: .utf8) else {
                            throw IOSRemoteExecDaemonError.invalidResponse("throw stackTrace is not UTF-8")
                        }
                        stackTrace = value
                    case (4, .bytes(let raw)):
                        guard let value = String(data: raw, encoding: .utf8) else {
                            throw IOSRemoteExecDaemonError.invalidResponse("throw errorCode is not UTF-8")
                        }
                        errorCode = value
                    default:
                        break
                    }
                }
                event = .remoteFailure(
                    id: id,
                    message: message ?? "remote execution error",
                    stackTrace: stackTrace,
                    errorCode: errorCode
                )
            case 3:
                let rawID = try firstVarint(1, in: nested) ?? 0
                guard rawID <= UInt64(UInt32.max) else {
                    throw IOSRemoteExecDaemonError.invalidResponse("heartbeat id overflow")
                }
                event = .heartbeat(UInt32(rawID))
            default:
                break
            }
        }
        return event
    }

    private static func decodeShellStream(_ data: Data) throws -> Event {
        var reader = Reader(data)
        for (field, value) in try reader.fields() {
            guard case .bytes(let nested) = value else { continue }
            switch field {
            case 1:
                return .stdout(try firstString(1, in: nested) ?? "")
            case 2:
                return .stderr(try firstString(1, in: nested) ?? "")
            case 3:
                var exitReader = Reader(nested)
                var code = 0
                var cwd = ""
                var aborted = false
                for (inner, innerValue) in try exitReader.fields() {
                    switch (inner, innerValue) {
                    case (1, .varint(let raw)): code = Int(raw)
                    case (2, .bytes(let raw)): cwd = String(data: raw, encoding: .utf8) ?? ""
                    case (4, .varint(let raw)): aborted = raw != 0
                    default: break
                    }
                }
                return .exit(code: code, cwd: cwd, aborted: aborted)
            case 5:
                return .shellRejected(
                    command: try firstString(1, in: nested) ?? "",
                    workingDirectory: try firstString(2, in: nested) ?? "",
                    reason: try firstString(3, in: nested) ?? "shell request rejected"
                )
            case 6:
                return .shellPermissionDenied(
                    command: try firstString(1, in: nested) ?? "",
                    workingDirectory: try firstString(2, in: nested) ?? "",
                    error: try firstString(3, in: nested) ?? "shell permission denied",
                    isReadonly: (try firstVarint(4, in: nested) ?? 0) != 0
                )
            case 7:
                var result: [String: AnyHashable] = [
                    "case": "backgrounded",
                    "shellId": Int(try firstVarint(1, in: nested) ?? 0),
                    "command": try firstString(2, in: nested) ?? "",
                    "workingDirectory": try firstString(3, in: nested) ?? "",
                ]
                if let raw = try firstVarint(4, in: nested) { result["pid"] = Int(raw) }
                if let raw = try firstVarint(5, in: nested) { result["msToWait"] = signedInt32(raw) }
                if let raw = try firstVarint(6, in: nested) { result["reason"] = Int(raw) }
                return .shellBackgrounded(result)
            case 9:
                return .shellSandboxUnsupported([
                    "case": "sandboxUnsupported",
                    "command": try firstString(1, in: nested) ?? "",
                    "workingDirectory": try firstString(2, in: nested) ?? "",
                    "sandboxPolicyType": try firstString(3, in: nested) ?? "",
                    "reason": try firstString(4, in: nested) ?? "sandbox unsupported",
                    "isReadonly": (try firstVarint(5, in: nested) ?? 0) != 0,
                ])
            default:
                return .ignore
            }
        }
        return .ignore
    }


    private static func decodeShellResult(_ data: Data) throws -> Event {
        var reader = Reader(data)
        var result: [String: AnyHashable]?
        for (field, value) in try reader.fields() {
            switch (field, value) {
            case (1, .bytes(let nested)):
                result = try decodeShellExecutionOutcome(nested, caseName: "success", failure: false)
            case (2, .bytes(let nested)):
                result = try decodeShellExecutionOutcome(nested, caseName: "failure", failure: true)
            case (3, .bytes(let nested)):
                result = [
                    "case": "timeout",
                    "command": try firstString(1, in: nested) ?? "",
                    "workingDirectory": try firstString(2, in: nested) ?? "",
                    "timeoutMs": signedInt32(try firstVarint(3, in: nested) ?? 0),
                ]
            case (4, .bytes(let nested)):
                result = [
                    "case": "rejected",
                    "command": try firstString(1, in: nested) ?? "",
                    "workingDirectory": try firstString(2, in: nested) ?? "",
                    "reason": try firstString(3, in: nested) ?? "shell rejected",
                    "isReadonly": (try firstVarint(4, in: nested) ?? 0) != 0,
                ]
            case (5, .bytes(let nested)):
                result = [
                    "case": "spawnError",
                    "command": try firstString(1, in: nested) ?? "",
                    "workingDirectory": try firstString(2, in: nested) ?? "",
                    "error": try firstString(3, in: nested) ?? "shell spawn failed",
                ]
            case (7, .bytes(let nested)):
                result = [
                    "case": "permissionDenied",
                    "command": try firstString(1, in: nested) ?? "",
                    "workingDirectory": try firstString(2, in: nested) ?? "",
                    "error": try firstString(3, in: nested) ?? "shell permission denied",
                    "isReadonly": (try firstVarint(4, in: nested) ?? 0) != 0,
                ]
            case (102, .varint(let raw)):
                if result == nil { result = [:] }
                result?["isBackground"] = raw != 0
            case (103, .bytes(let raw)):
                if result == nil { result = [:] }
                result?["terminalsFolder"] = String(data: raw, encoding: .utf8) ?? ""
            case (104, .varint(let raw)):
                if result == nil { result = [:] }
                result?["pid"] = Int(raw)
            default:
                break
            }
        }
        guard let result, result["case"] != nil else {
            throw IOSRemoteExecDaemonError.invalidResponse("missing shell result")
        }
        return .shellResult(result)
    }

    private static func decodeShellExecutionOutcome(
        _ data: Data,
        caseName: String,
        failure: Bool
    ) throws -> [String: AnyHashable] {
        var result: [String: AnyHashable] = [
            "case": caseName,
            "command": try firstString(1, in: data) ?? "",
            "workingDirectory": try firstString(2, in: data) ?? "",
            "exitCode": signedInt32(try firstVarint(3, in: data) ?? 0),
            "signal": try firstString(4, in: data) ?? "",
            "stdout": try firstString(5, in: data) ?? "",
            "stderr": try firstString(6, in: data) ?? "",
            "executionTime": signedInt32(try firstVarint(7, in: data) ?? 0),
        ]
        if failure {
            if let value = try firstString(9, in: data) { result["interleavedOutput"] = value }
            if let raw = try firstVarint(10, in: data) { result["abortReason"] = Int(raw) }
            result["aborted"] = (try firstVarint(11, in: data) ?? 0) != 0
            if let raw = try firstVarint(12, in: data) { result["localExecutionTimeMs"] = signedInt32(raw) }
            if let value = try firstString(13, in: data) { result["outputHead"] = value }
            if let value = try firstString(14, in: data) { result["outputTail"] = value }
            if let raw = try firstVarint(15, in: data) { result["elidedChars"] = Int(raw) }
        } else {
            if let raw = try firstVarint(9, in: data) { result["shellId"] = Int(raw) }
            if let value = try firstString(10, in: data) { result["interleavedOutput"] = value }
            if let raw = try firstVarint(11, in: data) { result["pid"] = Int(raw) }
            if let raw = try firstVarint(12, in: data) { result["msToWait"] = signedInt32(raw) }
            if let raw = try firstVarint(13, in: data) { result["localExecutionTimeMs"] = signedInt32(raw) }
            if let raw = try firstVarint(14, in: data) { result["backgroundReason"] = Int(raw) }
            if let value = try firstString(15, in: data) { result["outputHead"] = value }
            if let value = try firstString(16, in: data) { result["outputTail"] = value }
            if let raw = try firstVarint(17, in: data) { result["elidedChars"] = Int(raw) }
        }
        return result
    }

    private static func decodeBackgroundShellSpawnResult(_ data: Data) throws -> Event {
        var reader = Reader(data)
        for (field, value) in try reader.fields() {
            guard case .bytes(let nested) = value else { continue }
            switch field {
            case 1:
                var result: [String: AnyHashable] = [
                    "case": "success",
                    "shellId": Int(try firstVarint(1, in: nested) ?? 0),
                    "command": try firstString(2, in: nested) ?? "",
                    "workingDirectory": try firstString(3, in: nested) ?? "",
                ]
                if let raw = try firstVarint(4, in: nested) { result["pid"] = Int(raw) }
                return .backgroundShellSpawn(result)
            case 2:
                return .backgroundShellSpawn([
                    "case": "error",
                    "command": try firstString(1, in: nested) ?? "",
                    "workingDirectory": try firstString(2, in: nested) ?? "",
                    "error": try firstString(3, in: nested) ?? "background shell failed",
                ])
            case 3:
                return .backgroundShellSpawn([
                    "case": "rejected",
                    "command": try firstString(1, in: nested) ?? "",
                    "workingDirectory": try firstString(2, in: nested) ?? "",
                    "reason": try firstString(3, in: nested) ?? "background shell rejected",
                    "isReadonly": (try firstVarint(4, in: nested) ?? 0) != 0,
                ])
            case 4:
                return .backgroundShellSpawn([
                    "case": "permissionDenied",
                    "command": try firstString(1, in: nested) ?? "",
                    "workingDirectory": try firstString(2, in: nested) ?? "",
                    "error": try firstString(3, in: nested) ?? "background shell permission denied",
                    "isReadonly": (try firstVarint(4, in: nested) ?? 0) != 0,
                ])
            case 5:
                return .backgroundShellSpawn([
                    "case": "sandboxUnsupported",
                    "command": try firstString(1, in: nested) ?? "",
                    "workingDirectory": try firstString(2, in: nested) ?? "",
                    "sandboxPolicyType": try firstString(3, in: nested) ?? "",
                    "reason": try firstString(4, in: nested) ?? "sandbox unsupported",
                    "isReadonly": (try firstVarint(5, in: nested) ?? 0) != 0,
                ])
            default:
                continue
            }
        }
        throw IOSRemoteExecDaemonError.invalidResponse("missing background shell result")
    }

    private static func decodeWriteShellStdinResult(_ data: Data) throws -> Event {
        var reader = Reader(data)
        for (field, value) in try reader.fields() {
            guard case .bytes(let nested) = value else { continue }
            switch field {
            case 1:
                return .writeShellStdin([
                    "case": "success",
                    "shellId": Int(try firstVarint(1, in: nested) ?? 0),
                    "terminalFileLengthBeforeInputWritten": Int(try firstVarint(2, in: nested) ?? 0),
                ])
            case 2:
                return .writeShellStdin([
                    "case": "error",
                    "error": try firstString(1, in: nested) ?? "stdin write failed",
                ])
            default:
                continue
            }
        }
        throw IOSRemoteExecDaemonError.invalidResponse("missing write shell stdin result")
    }

    private static func decodeForceBackgroundShellResult(_ data: Data) throws -> Event {
        var reader = Reader(data)
        var status: AnyHashable = AnyHashable("UNSPECIFIED")
        var shellResult: [String: AnyHashable]?
        for (field, value) in try reader.fields() {
            switch (field, value) {
            case (1, .varint(let raw)):
                switch raw {
                case 0: status = AnyHashable("UNSPECIFIED")
                case 1: status = AnyHashable("ACCEPTED")
                case 2: status = AnyHashable("NOT_FOUND")
                default:
                    status = AnyHashable(Int(Int32(bitPattern: UInt32(truncatingIfNeeded: raw))))
                }
            case (2, .bytes(let nested)):
                guard case .shellResult(let decoded) = try decodeShellResult(nested) else {
                    throw IOSRemoteExecDaemonError.invalidResponse(
                        "force background shell result contains malformed shell_result"
                    )
                }
                shellResult = decoded
            default:
                continue
            }
        }
        return .forceBackgroundShellResult(status: status, shellResult: shellResult)
    }

    private static func decodeLsResult(_ data: Data) throws -> Event {
        var reader = Reader(data)
        for (field, value) in try reader.fields() {
            guard case .bytes(let nested) = value else { continue }
            switch field {
            case 1:
                guard let tree = try firstBytes(1, in: nested) else {
                    throw IOSRemoteExecDaemonError.invalidResponse("ls success missing directory tree")
                }
                return .lsSuccess(try decodeLsNode(tree))
            case 2:
                return .lsError(
                    path: try firstString(1, in: nested) ?? "",
                    error: try firstString(2, in: nested) ?? "ls failed"
                )
            case 3:
                return .lsRejected(
                    path: try firstString(1, in: nested) ?? "",
                    reason: try firstString(2, in: nested) ?? "ls rejected"
                )
            case 4:
                guard let tree = try firstBytes(1, in: nested) else {
                    throw IOSRemoteExecDaemonError.invalidResponse("ls timeout missing directory tree")
                }
                return .lsTimeout(try decodeLsNode(tree))
            default:
                continue
            }
        }
        throw IOSRemoteExecDaemonError.invalidResponse("missing ls result")
    }

    private static func decodeLsNode(_ data: Data) throws -> LsNode {
        var reader = Reader(data)
        var absPath = ""
        var childrenDirs: [LsNode] = []
        var childrenFiles: [String] = []
        var childrenWereProcessed = false
        var extensionCounts: [String: Int] = [:]
        var numFiles = 0

        for (field, value) in try reader.fields() {
            switch (field, value) {
            case (1, .bytes(let raw)):
                absPath = String(data: raw, encoding: .utf8) ?? ""
            case (2, .bytes(let raw)):
                childrenDirs.append(try decodeLsNode(raw))
            case (3, .bytes(let raw)):
                if let name = try firstString(1, in: raw) {
                    childrenFiles.append(name)
                }
            case (4, .varint(let raw)):
                childrenWereProcessed = raw != 0
            case (5, .bytes(let raw)):
                let key = try firstString(1, in: raw) ?? ""
                if let value = try firstVarint(2, in: raw), !key.isEmpty {
                    extensionCounts[key] = Int(Int32(bitPattern: UInt32(truncatingIfNeeded: value)))
                }
            case (6, .varint(let raw)):
                numFiles = Int(Int32(bitPattern: UInt32(truncatingIfNeeded: raw)))
            default:
                break
            }
        }

        return LsNode(
            absPath: absPath,
            childrenDirs: childrenDirs,
            childrenFiles: childrenFiles,
            childrenWereProcessed: childrenWereProcessed,
            fullSubtreeExtensionCounts: extensionCounts,
            numFiles: numFiles
        )
    }

    private static func firstBytes(_ field: Int, in data: Data) throws -> Data? {
        var reader = Reader(data)
        for (current, value) in try reader.fields() where current == field {
            if case .bytes(let raw) = value { return raw }
        }
        return nil
    }

    private static func firstVarint(_ field: Int, in data: Data) throws -> UInt64? {
        var reader = Reader(data)
        for (current, value) in try reader.fields() where current == field {
            if case .varint(let raw) = value { return raw }
        }
        return nil
    }


    private static func decodeDeleteResult(_ data: Data) throws -> Event {
        var reader = Reader(data)
        for (field, value) in try reader.fields() {
            guard case .bytes(let nested) = value else { continue }
            switch field {
            case 1:
                return .delete([
                    "case": "success",
                    "path": try firstString(1, in: nested) ?? "",
                    "deletedFile": try firstString(2, in: nested) ?? "",
                    "fileSize": Int64(bitPattern: try firstVarint(3, in: nested) ?? 0),
                    "prevContent": try firstString(4, in: nested) ?? "",
                ])
            case 2:
                return .delete([
                    "case": "fileNotFound",
                    "path": try firstString(1, in: nested) ?? "",
                ])
            case 3:
                return .delete([
                    "case": "notFile",
                    "path": try firstString(1, in: nested) ?? "",
                    "actualType": try firstString(2, in: nested) ?? "",
                ])
            case 4:
                return .delete([
                    "case": "permissionDenied",
                    "path": try firstString(1, in: nested) ?? "",
                    "clientVisibleError": try firstString(2, in: nested) ?? "",
                    "isReadonly": (try firstVarint(3, in: nested) ?? 0) != 0,
                ])
            case 5:
                return .delete([
                    "case": "fileBusy",
                    "path": try firstString(1, in: nested) ?? "",
                ])
            case 6:
                return .delete([
                    "case": "rejected",
                    "path": try firstString(1, in: nested) ?? "",
                    "reason": try firstString(2, in: nested) ?? "",
                ])
            case 7:
                return .delete([
                    "case": "error",
                    "path": try firstString(1, in: nested) ?? "",
                    "error": try firstString(2, in: nested) ?? "delete failed",
                ])
            default:
                continue
            }
        }
        throw IOSRemoteExecDaemonError.invalidResponse("missing delete result")
    }

    private static func decodeFetchResult(_ data: Data) throws -> Event {
        var reader = Reader(data)
        for (field, value) in try reader.fields() {
            guard case .bytes(let nested) = value else { continue }
            switch field {
            case 1:
                return .fetch([
                    "case": "success",
                    "url": try firstString(1, in: nested) ?? "",
                    "content": try firstString(2, in: nested) ?? "",
                    "statusCode": Int(Int32(bitPattern: UInt32(truncatingIfNeeded: try firstVarint(3, in: nested) ?? 0))),
                    "contentType": try firstString(4, in: nested) ?? "",
                ])
            case 2:
                return .fetch([
                    "case": "error",
                    "url": try firstString(1, in: nested) ?? "",
                    "error": try firstString(2, in: nested) ?? "fetch failed",
                ])
            default:
                continue
            }
        }
        throw IOSRemoteExecDaemonError.invalidResponse("missing fetch result")
    }

    private static func decodeGrepResult(_ data: Data) throws -> Event {
        let foundation = try decodeGrepResultFoundation(data)
        guard JSONSerialization.isValidJSONObject(foundation),
              let encoded = try? JSONSerialization.data(withJSONObject: foundation, options: [.sortedKeys]),
              let json = String(data: encoded, encoding: .utf8)
        else {
            throw IOSRemoteExecDaemonError.invalidResponse("grep result could not be projected")
        }
        return .grep(json)
    }

    private static func decodeGrepResultFoundation(_ data: Data) throws -> [String: Any] {
        var reader = Reader(data)
        for (field, value) in try reader.fields() {
            guard case .bytes(let nested) = value else { continue }
            if field == 2 {
                return [
                    "case": "error",
                    "error": try firstString(1, in: nested) ?? "grep failed",
                ]
            }
            guard field == 1 else { continue }
            var success = Reader(nested)
            var result: [String: Any] = [
                "case": "success",
                "pattern": "",
                "path": "",
                "outputMode": "",
                "workspaceResults": [String: Any](),
            ]
            var workspaceResults: [String: Any] = [:]
            for (inner, innerValue) in try success.fields() {
                switch (inner, innerValue) {
                case (1, .bytes(let raw)):
                    result["pattern"] = String(data: raw, encoding: .utf8) ?? ""
                case (2, .bytes(let raw)):
                    result["path"] = String(data: raw, encoding: .utf8) ?? ""
                case (3, .bytes(let raw)):
                    result["outputMode"] = String(data: raw, encoding: .utf8) ?? ""
                case (4, .bytes(let entry)):
                    let key = try firstString(1, in: entry) ?? ""
                    if !key.isEmpty, let union = try firstBytes(2, in: entry) {
                        workspaceResults[key] = try decodeGrepUnionFoundation(union)
                    }
                case (5, .bytes(let raw)):
                    result["activeEditorResult"] = try decodeGrepUnionFoundation(raw)
                default:
                    break
                }
            }
            result["workspaceResults"] = workspaceResults
            return result
        }
        throw IOSRemoteExecDaemonError.invalidResponse("missing grep result")
    }

    private static func decodeGrepUnionFoundation(_ data: Data) throws -> [String: Any] {
        var reader = Reader(data)
        for (field, value) in try reader.fields() {
            guard case .bytes(let nested) = value else { continue }
            switch field {
            case 1:
                var countReader = Reader(nested)
                var counts: [[String: Any]] = []
                var result: [String: Any] = ["case": "count"]
                for (inner, innerValue) in try countReader.fields() {
                    switch (inner, innerValue) {
                    case (1, .bytes(let raw)):
                        counts.append([
                            "file": try firstString(1, in: raw) ?? "",
                            "count": signedInt32(try firstVarint(2, in: raw) ?? 0),
                        ])
                    case (2, .varint(let raw)): result["totalFiles"] = signedInt32(raw)
                    case (3, .varint(let raw)): result["totalMatches"] = signedInt32(raw)
                    case (4, .varint(let raw)): result["clientTruncated"] = raw != 0
                    case (5, .varint(let raw)): result["ripgrepTruncated"] = raw != 0
                    case (6, .varint(let raw)): result["headLimitApplied"] = signedInt32(raw)
                    case (7, .varint(let raw)): result["offsetApplied"] = signedInt32(raw)
                    default: break
                    }
                }
                result["counts"] = counts
                return result
            case 2:
                var filesReader = Reader(nested)
                var files: [String] = []
                var result: [String: Any] = ["case": "files"]
                for (inner, innerValue) in try filesReader.fields() {
                    switch (inner, innerValue) {
                    case (1, .bytes(let raw)): files.append(String(data: raw, encoding: .utf8) ?? "")
                    case (2, .varint(let raw)): result["totalFiles"] = signedInt32(raw)
                    case (3, .varint(let raw)): result["clientTruncated"] = raw != 0
                    case (4, .varint(let raw)): result["ripgrepTruncated"] = raw != 0
                    case (5, .varint(let raw)): result["headLimitApplied"] = signedInt32(raw)
                    case (6, .varint(let raw)): result["offsetApplied"] = signedInt32(raw)
                    default: break
                    }
                }
                result["files"] = files
                return result
            case 3:
                var contentReader = Reader(nested)
                var matches: [[String: Any]] = []
                var result: [String: Any] = ["case": "content"]
                for (inner, innerValue) in try contentReader.fields() {
                    switch (inner, innerValue) {
                    case (1, .bytes(let raw)):
                        var fileReader = Reader(raw)
                        var file = ""
                        var lineMatches: [[String: Any]] = []
                        for (fileField, fileValue) in try fileReader.fields() {
                            switch (fileField, fileValue) {
                            case (1, .bytes(let rawFile)):
                                file = String(data: rawFile, encoding: .utf8) ?? ""
                            case (2, .bytes(let rawMatch)):
                                lineMatches.append([
                                    "lineNumber": signedInt32(try firstVarint(1, in: rawMatch) ?? 0),
                                    "content": try firstString(2, in: rawMatch) ?? "",
                                    "contentTruncated": (try firstVarint(3, in: rawMatch) ?? 0) != 0,
                                    "isContextLine": (try firstVarint(4, in: rawMatch) ?? 0) != 0,
                                ])
                            default:
                                break
                            }
                        }
                        matches.append(["file": file, "matches": lineMatches])
                    case (2, .varint(let raw)): result["totalLines"] = signedInt32(raw)
                    case (3, .varint(let raw)): result["totalMatchedLines"] = signedInt32(raw)
                    case (4, .varint(let raw)): result["clientTruncated"] = raw != 0
                    case (5, .varint(let raw)): result["ripgrepTruncated"] = raw != 0
                    case (6, .varint(let raw)): result["headLimitApplied"] = signedInt32(raw)
                    case (7, .varint(let raw)): result["offsetApplied"] = signedInt32(raw)
                    default:
                        break
                    }
                }
                result["matches"] = matches
                return result
            default:
                continue
            }
        }
        throw IOSRemoteExecDaemonError.invalidResponse("missing grep union result")
    }

    private static func signedInt32(_ raw: UInt64) -> Int {
        Int(Int32(bitPattern: UInt32(truncatingIfNeeded: raw)))
    }

    private static func decodeWriteResult(_ data: Data) throws -> Event {
        var reader = Reader(data)
        for (field, value) in try reader.fields() {
            guard case .bytes(let nested) = value else { continue }
            switch field {
            case 1:
                var result: [String: AnyHashable] = [
                    "case": "success",
                    "success": true,
                    "path": try firstString(1, in: nested) ?? "",
                ]
                if let raw = try firstVarint(2, in: nested) {
                    result["linesCreated"] = Int(Int32(bitPattern: UInt32(truncatingIfNeeded: raw)))
                }
                if let raw = try firstVarint(3, in: nested) {
                    result["fileSize"] = Int(Int32(bitPattern: UInt32(truncatingIfNeeded: raw)))
                }
                if let content = try firstString(4, in: nested) {
                    result["fileContentAfterWrite"] = content
                }
                return .write(result)
            case 3:
                return .write([
                    "case": "permissionDenied",
                    "path": try firstString(1, in: nested) ?? "",
                    "directory": try firstString(2, in: nested) ?? "",
                    "operation": try firstString(3, in: nested) ?? "",
                    "error": try firstString(4, in: nested) ?? "write permission denied",
                    "isReadonly": (try firstVarint(5, in: nested) ?? 0) != 0,
                ])
            case 4:
                return .write([
                    "case": "noSpace",
                    "path": try firstString(1, in: nested) ?? "",
                ])
            case 5:
                return .write([
                    "case": "error",
                    "path": try firstString(1, in: nested) ?? "",
                    "error": try firstString(2, in: nested) ?? "write failed",
                ])
            case 6:
                return .write([
                    "case": "rejected",
                    "path": try firstString(1, in: nested) ?? "",
                    "reason": try firstString(2, in: nested) ?? "write rejected",
                ])
            default:
                continue
            }
        }
        throw IOSRemoteExecDaemonError.invalidResponse("missing write result")
    }

    private static func decodeReadResult(_ data: Data) throws -> Event {
        var reader = Reader(data)
        for (field, value) in try reader.fields() {
            guard case .bytes(let nested) = value else { continue }
            switch field {
            case 1:
                var success = Reader(nested)
                var result: [String: AnyHashable] = [:]
                for (inner, innerValue) in try success.fields() {
                    switch (inner, innerValue) {
                    case (1, .bytes(let raw)):
                        result["path"] = String(data: raw, encoding: .utf8) ?? ""
                    case (2, .bytes(let raw)):
                        result["content"] = String(data: raw, encoding: .utf8) ?? ""
                    case (3, .varint(let raw)):
                        result["totalLines"] = Int(raw)
                    case (4, .varint(let raw)):
                        result["fileSize"] = Int64(bitPattern: raw)
                    case (5, .bytes(let raw)):
                        result["dataBase64"] = raw.base64EncodedString()
                    case (6, .varint(let raw)):
                        result["truncated"] = raw != 0
                    case (8, .varint(let raw)):
                        result["rangeApplied"] = raw != 0
                    default: break
                    }
                }
                result["success"] = true
                return .read(result)
            case 2:
                return .remoteError(try firstString(2, in: nested) ?? "read failed")
            case 3:
                return .remoteError(try firstString(2, in: nested) ?? "read rejected")
            case 4:
                return .remoteError("file not found")
            case 5:
                return .remoteError("read permission denied")
            case 6:
                return .remoteError(try firstString(2, in: nested) ?? "invalid file")
            default:
                return .ignore
            }
        }
        return .ignore
    }

    private static func firstString(_ field: Int, in data: Data) throws -> String? {
        var reader = Reader(data)
        for (current, value) in try reader.fields() where current == field {
            if case .bytes(let raw) = value {
                return String(data: raw, encoding: .utf8)
            }
        }
        return nil
    }

    private static func varint(_ field: UInt64, _ value: UInt64, into data: inout Data) {
        rawVarint((field << 3) | 0, into: &data)
        rawVarint(value, into: &data)
    }

    private static func bytes(_ field: UInt64, _ value: Data, into data: inout Data) {
        rawVarint((field << 3) | 2, into: &data)
        rawVarint(UInt64(value.count), into: &data)
        data.append(value)
    }

    private static func rawVarint(_ value: UInt64, into data: inout Data) {
        var value = value
        repeat {
            var byte = UInt8(value & 0x7f)
            value >>= 7
            if value != 0 { byte |= 0x80 }
            data.append(byte)
        } while value != 0
    }
}

private struct IOSRemoteExecConnectDecoder {
    private var buffer: [UInt8] = []

    mutating func append(_ byte: UInt8) throws -> [(UInt8, Data)] {
        buffer.append(byte)
        return try drain()
    }

    mutating func append(_ data: Data) throws -> [(UInt8, Data)] {
        buffer.append(contentsOf: data)
        return try drain()
    }

    mutating func finish() throws {
        guard buffer.isEmpty else {
            throw IOSRemoteExecDaemonError.invalidResponse("Connect stream ended inside an envelope")
        }
    }

    private mutating func drain() throws -> [(UInt8, Data)] {
        var result: [(UInt8, Data)] = []
        while buffer.count >= 5 {
            let flags = buffer[0]
            guard flags & 0xfc == 0, flags & 0x01 == 0 else {
                throw IOSRemoteExecDaemonError.invalidResponse("unsupported Connect envelope flags")
            }
            let length =
                (UInt32(buffer[1]) << 24) |
                (UInt32(buffer[2]) << 16) |
                (UInt32(buffer[3]) << 8) |
                UInt32(buffer[4])
            let total = 5 + Int(length)
            guard buffer.count >= total else { break }
            result.append((flags, Data(buffer[5..<total])))
            buffer.removeFirst(total)
        }
        return result
    }
}

struct IOSRemoteExecDaemonClient: @unchecked Sendable {
    typealias RequestExecutor = @Sendable (URLRequest) async throws -> (Data, URLResponse)

    private let descriptor: IOSCursorSandBoxConnection
    private let session: URLSession
    private let requestExecutor: RequestExecutor?

    init(
        descriptor: IOSCursorSandBoxConnection,
        session: URLSession = .shared,
        requestExecutor: RequestExecutor? = nil
    ) {
        self.descriptor = descriptor
        self.session = session
        self.requestExecutor = requestExecutor
    }

    func dispatch(
        method: String,
        params: CoordinatorPayload
    ) async throws -> CoordinatorPayload {
        try Task.checkCancellation()
        guard case .object(let payload) = params,
              let object = params.foundationValue as? [String: Any]
        else {
            throw IOSRemoteExecDaemonError.invalidRequest("params must be an object")
        }
        let executionContext: [String: CoordinatorPayload]
        if let suppliedContext = payload["executionContext"] {
            guard case .object(let value) = suppliedContext else {
                throw IOSRemoteExecDaemonError.invalidRequest("executionContext must be an object")
            }
            executionContext = value
        } else {
            executionContext = [:]
        }

        let suppliedExecID = executionContext["execId"] ?? payload["execId"]
        let execID: String
        if let suppliedExecID {
            guard case .string(let value) = suppliedExecID,
                  !value.isEmpty,
                  value.utf8.count <= 256,
                  !value.contains("\0"),
                  !value.contains("\r"),
                  !value.contains("\n")
            else {
                throw IOSRemoteExecDaemonError.invalidRequest("execution context execId is invalid")
            }
            execID = value
        } else {
            execID = UUID().uuidString.lowercased()
        }

        let spanContext = executionContext["spanContext"] ?? payload["spanContext"]
        let acceptHookPayload = executionContext["acceptHookAdditionalContexts"] ?? payload["acceptHookAdditionalContexts"]
        let acceptHookAdditionalContexts: Bool?
        if let acceptHookPayload {
            guard case .bool(let value) = acceptHookPayload else {
                throw IOSRemoteExecDaemonError.invalidRequest("acceptHookAdditionalContexts must be a boolean")
            }
            acceptHookAdditionalContexts = value
        } else {
            acceptHookAdditionalContexts = nil
        }

        let body: Data
        switch method {
        case "local-exec.shell", "local-exec.shell.exec":
            guard let command = object["command"] as? String,
                  !command.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
                  command.utf8.count <= 256 * 1024
            else {
                throw IOSRemoteExecDaemonError.invalidRequest("shell command is required and must be <= 256 KiB")
            }
            let workingDirectory = object["workingDirectory"] as? String ?? ""
            try Self.validateWorkspacePath(
                workingDirectory,
                field: "shell workingDirectory",
                allowEmpty: true
            )
            body = try IOSRemoteExecDaemonWire.shellRequest(
                id: 1,
                execID: execID,
                command: command,
                workingDirectory: workingDirectory,
                toolCallID: object["toolCallId"] as? String ?? "",
                timeout: payload["timeout"],
                isBackground: payload["isBackground"],
                timeoutBehavior: payload["timeoutBehavior"],
                hardTimeout: payload["hardTimeout"],
                requestedSandboxPolicy: payload["requestedSandboxPolicy"],
                adminCommandDenylist: payload["adminCommandDenylist"],
                closeStdin: payload["closeStdin"]
            )
        case "local-exec.box.exec":
            guard let caseName = object["case"] as? String,
                  let args = object["args"] as? [String: Any]
            else {
                throw IOSRemoteExecDaemonError.unsupported(method)
            }
            let argsPayload: [String: CoordinatorPayload]
            if case .object(let value)? = payload["args"] {
                argsPayload = value
            } else {
                argsPayload = [:]
            }
            switch caseName {
            case "shellArgs", "shellStreamArgs":
                guard let command = args["command"] as? String,
                      !command.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
                      command.utf8.count <= 256 * 1024
                else {
                    throw IOSRemoteExecDaemonError.invalidRequest("shell command is required and must be <= 256 KiB")
                }
                let workingDirectory = args["workingDirectory"] as? String ?? ""
                try Self.validateWorkspacePath(
                    workingDirectory,
                    field: "shell workingDirectory",
                    allowEmpty: true
                )
                body = try IOSRemoteExecDaemonWire.shellRequest(
                    id: 1,
                    execID: execID,
                    command: command,
                    workingDirectory: workingDirectory,
                    toolCallID: args["toolCallId"] as? String ?? "",
                    timeout: argsPayload["timeout"],
                    isBackground: argsPayload["isBackground"],
                    timeoutBehavior: argsPayload["timeoutBehavior"],
                    hardTimeout: argsPayload["hardTimeout"],
                    requestedSandboxPolicy: argsPayload["requestedSandboxPolicy"],
                    adminCommandDenylist: argsPayload["adminCommandDenylist"],
                    closeStdin: argsPayload["closeStdin"],
                    messageField: caseName == "shellArgs" ? 2 : 14
                )
            case "backgroundShellSpawnArgs":
                guard let command = args["command"] as? String,
                      !command.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
                      command.utf8.count <= 256 * 1024
                else {
                    throw IOSRemoteExecDaemonError.invalidRequest("background shell command is required and must be <= 256 KiB")
                }
                let workingDirectory = args["workingDirectory"] as? String ?? ""
                try Self.validateWorkspacePath(
                    workingDirectory,
                    field: "background shell workingDirectory",
                    allowEmpty: true
                )
                body = try IOSRemoteExecDaemonWire.backgroundShellSpawnRequest(
                    id: 1,
                    execID: execID,
                    command: command,
                    workingDirectory: workingDirectory,
                    toolCallID: args["toolCallId"] as? String ?? "",
                    sandboxPolicy: argsPayload["sandboxPolicy"],
                    enableWriteShellStdinTool: argsPayload["enableWriteShellStdinTool"],
                    description: argsPayload["description"],
                    skipApproval: argsPayload["skipApproval"],
                    conversationID: argsPayload["conversationId"],
                    adminCommandDenylist: argsPayload["adminCommandDenylist"]
                )
            case "forceBackgroundShellArgs":
                guard let toolCallID = args["toolCallId"] as? String else {
                    throw IOSRemoteExecDaemonError.invalidRequest("forceBackgroundShell requires toolCallId")
                }
                body = try IOSRemoteExecDaemonWire.forceBackgroundShellRequest(
                    id: 1,
                    execID: execID,
                    toolCallID: toolCallID
                )
            case "writeShellStdinArgs":
                guard let shellID = Self.integer(args["shellId"]),
                      let chars = args["chars"] as? String
                else {
                    throw IOSRemoteExecDaemonError.invalidRequest("writeShellStdin requires shellId and chars")
                }
                body = try IOSRemoteExecDaemonWire.writeShellStdinRequest(
                    id: 1,
                    execID: execID,
                    shellID: shellID,
                    chars: chars
                )
            case "writeArgs":
                guard let path = args["path"] as? String else {
                    throw IOSRemoteExecDaemonError.invalidRequest("write path is required")
                }
                try Self.validateWorkspacePath(path, field: "box path")
                guard let encoded = args["fileBytes"] as? String,
                      encoded.utf8.count <= 12 * 1024 * 1024,
                      let bytes = Data(base64Encoded: encoded),
                      bytes.count <= 8 * 1024 * 1024
                else {
                    throw IOSRemoteExecDaemonError.invalidRequest("write fileBytes must be base64 and <= 8 MiB")
                }
                body = try IOSRemoteExecDaemonWire.writeRequest(
                    id: 1,
                    execID: execID,
                    path: path,
                    fileBytes: bytes,
                    toolCallID: args["toolCallId"] as? String ?? "",
                    returnFileContentAfterWrite: args["returnFileContentAfterWrite"] as? Bool ?? false,
                    encodingHint: args["encodingHint"] as? String
                )
            case "readArgs", "redactedReadArgs":
                guard let path = args["path"] as? String else {
                    throw IOSRemoteExecDaemonError.invalidRequest("read path is required")
                }
                try Self.validateWorkspacePath(path, field: "box path")
                body = try IOSRemoteExecDaemonWire.readRequest(
                    id: 1,
                    execID: execID,
                    path: path,
                    toolCallID: args["toolCallId"] as? String ?? "",
                    offset: Self.integer(args["offset"]),
                    limit: Self.integer(args["limit"]),
                    encodingHint: args["encodingHint"] as? String,
                    redacted: caseName == "redactedReadArgs"
                )
            case "lsArgs":
                guard let path = args["path"] as? String else {
                    throw IOSRemoteExecDaemonError.invalidRequest("ls path is required")
                }
                try Self.validateWorkspacePath(path, field: "box path")
                body = try IOSRemoteExecDaemonWire.lsRequest(
                    id: 1,
                    execID: execID,
                    path: path,
                    ignore: args["ignore"] as? [String] ?? [],
                    toolCallID: args["toolCallId"] as? String ?? "",
                    sandboxPolicy: argsPayload["sandboxPolicy"],
                    timeoutMs: Self.integer(args["timeoutMs"])
                )
            case "deleteArgs":
                guard let path = args["path"] as? String else {
                    throw IOSRemoteExecDaemonError.invalidRequest("delete path is required")
                }
                try Self.validateWorkspacePath(path, field: "box path")
                body = try IOSRemoteExecDaemonWire.deleteRequest(
                    id: 1,
                    execID: execID,
                    path: path,
                    toolCallID: args["toolCallId"] as? String ?? ""
                )
            case "fetchArgs":
                guard let url = args["url"] as? String else {
                    throw IOSRemoteExecDaemonError.invalidRequest("fetch url is required")
                }
                body = try IOSRemoteExecDaemonWire.fetchRequest(
                    id: 1,
                    execID: execID,
                    url: url,
                    toolCallID: args["toolCallId"] as? String ?? ""
                )
            case "grepArgs":
                guard let pattern = args["pattern"] as? String else {
                    throw IOSRemoteExecDaemonError.invalidRequest("grep pattern is required")
                }
                let grepPath = args["path"] as? String
                if let grepPath, !grepPath.isEmpty {
                    try Self.validateWorkspacePath(grepPath, field: "grep path")
                }
                body = try IOSRemoteExecDaemonWire.grepRequest(
                    id: 1,
                    execID: execID,
                    pattern: pattern,
                    path: grepPath,
                    glob: args["glob"] as? String,
                    outputMode: args["outputMode"] as? String,
                    contextBefore: Self.integer(args["contextBefore"]),
                    contextAfter: Self.integer(args["contextAfter"]),
                    context: Self.integer(args["context"]),
                    caseInsensitive: args["caseInsensitive"] as? Bool,
                    type: args["type"] as? String,
                    headLimit: Self.integer(args["headLimit"]),
                    multiline: args["multiline"] as? Bool,
                    sort: args["sort"] as? String,
                    sortAscending: args["sortAscending"] as? Bool,
                    toolCallID: args["toolCallId"] as? String ?? "",
                    sandboxPolicy: argsPayload["sandboxPolicy"],
                    offset: Self.integer(args["offset"])
                )
            default:
                throw IOSRemoteExecDaemonError.unsupported(method)
            }
        case "local-exec.browser.exec":
            guard let driverVersion = Self.integer(object["driverVersion"]),
                  driverVersion == 2,
                  let driverPath = object["driverPath"] as? String,
                  driverPath == "/workspace/.sand-browser/driver-v2.mjs",
                  let driverDirectory = object["driverDirectory"] as? String,
                  driverDirectory == "/workspace/.sand-browser",
                  let driverSource = object["driverSource"] as? String,
                  !driverSource.isEmpty,
                  driverSource.utf8.count <= 128 * 1024,
                  let resultMarker = object["resultMarker"] as? String,
                  resultMarker == "__SAND_BROWSER_RESULT__",
                  let requestObject = object["request"] as? [String: Any],
                  JSONSerialization.isValidJSONObject(requestObject),
                  let screenshotPath = object["screenshotPath"] as? String,
                  screenshotPath.hasPrefix("/workspace/.sand-browser/shot-"),
                  screenshotPath.hasSuffix(".png"),
                  screenshotPath.utf8.count <= 160
            else {
                throw IOSRemoteExecDaemonError.invalidRequest("browser driver contract is invalid")
            }
            let requestData = try JSONSerialization.data(withJSONObject: requestObject)
            guard requestData.count <= 64 * 1024 else {
                throw IOSRemoteExecDaemonError.invalidRequest("browser request exceeds size limit")
            }
            let sourceBase64 = Data(driverSource.utf8).base64EncodedString()
            let requestBase64 = requestData.base64EncodedString()
            let sourceToken = Self.shellQuotedArgument(sourceBase64)
            let requestToken = Self.shellQuotedArgument(requestBase64)
            let screenshotToken = Self.shellQuotedArgument(screenshotPath)
            let command = [
                "mkdir -p /workspace/.sand-browser && chmod 700 /workspace/.sand-browser",
                "printf %s \(sourceToken) | base64 -d > /workspace/.sand-browser/driver-v2.mjs",
                "rm -f \(screenshotToken)",
                "node /workspace/.sand-browser/driver-v2.mjs \(requestToken)",
                "if [ -f \(screenshotToken) ]; then printf '\\n__FABUSHI_BROWSER_SCREENSHOT__'; base64 < \(screenshotToken) | tr -d '\\n'; printf '\\n'; fi",
            ].joined(separator: " && ")
            body = try IOSRemoteExecDaemonWire.shellRequest(
                id: 1,
                execID: execID,
                command: command,
                workingDirectory: "/workspace",
                toolCallID: object["toolCallId"] as? String ?? ""
            )
        case "local-exec.process", "local-exec.process.exec", "local-exec.process.spawn":
            guard let program = object["program"] as? String,
                  !program.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
                  !program.contains("\0"),
                  !program.contains("\r"),
                  !program.contains("\n")
            else {
                throw IOSRemoteExecDaemonError.invalidRequest("process program is required")
            }
            let workingDirectory = object["workingDirectory"] as? String ?? ""
            try Self.validateWorkspacePath(
                workingDirectory,
                field: "process workingDirectory",
                allowEmpty: true
            )
            let args: [String]
            if let rawArgs = object["args"] {
                guard let values = rawArgs as? [String],
                      values.allSatisfy({ !$0.contains("\0") })
                else {
                    throw IOSRemoteExecDaemonError.invalidRequest("process args must be strings without NUL bytes")
                }
                guard values.count <= 256,
                      values.reduce(0, { $0 + $1.utf8.count }) <= 256 * 1024
                else {
                    throw IOSRemoteExecDaemonError.invalidRequest("process args exceed bounded request size")
                }
                args = values
            } else {
                args = []
            }
            let command = ([program] + args)
                .map(Self.shellQuotedArgument)
                .joined(separator: " ")
            body = try IOSRemoteExecDaemonWire.shellRequest(
                id: 1,
                execID: execID,
                command: command,
                workingDirectory: workingDirectory,
                toolCallID: object["toolCallId"] as? String ?? ""
            )
        default:
            throw IOSRemoteExecDaemonError.unsupported(method)
        }

        guard let baseURL = URL(string: descriptor.execDaemonURL),
              let scheme = baseURL.scheme?.lowercased(),
              (scheme == "http" || scheme == "https"),
              baseURL.host != nil,
              !descriptor.execDaemonAuthToken.isEmpty
        else {
            throw IOSRemoteExecDaemonError.invalidRequest("exec daemon authority is unavailable")
        }
        let url = baseURL
            .appendingPathComponent("agent.v1.ExecService")
            .appendingPathComponent("Exec")
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.setValue("Bearer \(descriptor.execDaemonAuthToken)", forHTTPHeaderField: "Authorization")
        request.setValue("application/proto", forHTTPHeaderField: "Content-Type")
        request.setValue("1", forHTTPHeaderField: "Connect-Protocol-Version")
        request.httpBody = try IOSRemoteExecDaemonWire.appendingExecutionContext(
            to: body,
            execID: execID,
            spanContext: spanContext,
            acceptHookAdditionalContexts: acceptHookAdditionalContexts
        )
        if method == "local-exec.browser.exec" {
            // Desktop c33a9495 gives ExecService enough time for bounded navigation/browser startup.
            request.timeoutInterval = 120
        }

        var stdout = ""
        var stderr = ""
        var exit: (code: Int, cwd: String, aborted: Bool)?
        var readResult: [String: AnyHashable]?
        var writeResult: [String: AnyHashable]?
        var deleteResult: [String: AnyHashable]?
        var fetchResult: [String: AnyHashable]?
        var typedShellResult: [String: AnyHashable]?
        var forceBackgroundShellResult: CoordinatorPayload?
        var grepResult: CoordinatorPayload?
        var lsResult: CoordinatorPayload?
        var shellTerminalResult: CoordinatorPayload?
        var decoder = IOSRemoteExecConnectDecoder()
        var sawEnd = false
        var sawStreamClose = false
        var executionMetadata = IOSRemoteExecDaemonWire.ExecutionMetadata()
        var remoteFailure: (id: UInt32, message: String, stackTrace: String?, errorCode: String?)?

        func consume(_ envelopes: [(UInt8, Data)]) throws {
            for (flags, payload) in envelopes {
                if flags & 0x02 != 0 {
                    guard !sawEnd else {
                        throw IOSRemoteExecDaemonError.invalidResponse("duplicate Connect end-stream frame")
                    }
                    sawEnd = true
                    if let object = try? JSONSerialization.jsonObject(with: payload) as? [String: Any],
                       let error = object["error"] as? [String: Any] {
                        let message = error["message"] as? String ?? "remote execution stream failed"
                        throw IOSRemoteExecDaemonError.remote(message)
                    }
                    continue
                }
                guard !sawEnd else {
                    throw IOSRemoteExecDaemonError.invalidResponse("received data after Connect end-stream frame")
                }
                guard !sawStreamClose else {
                    throw IOSRemoteExecDaemonError.invalidResponse("received execution data after streamClose")
                }
                if let metadata = try IOSRemoteExecDaemonWire.decodeExecutionMetadata(payload) {
                    if let responseID = metadata.id, responseID != 1 {
                        throw IOSRemoteExecDaemonError.invalidResponse("execution metadata id does not match request")
                    }
                    if let responseExecID = metadata.execID, responseExecID != execID {
                        throw IOSRemoteExecDaemonError.invalidResponse("execution metadata execId does not match request")
                    }
                    if let localExecutionTimeMs = metadata.localExecutionTimeMs {
                        executionMetadata.localExecutionTimeMs = localExecutionTimeMs
                    }
                    if let id = metadata.id { executionMetadata.id = id }
                    if let responseExecID = metadata.execID { executionMetadata.execID = responseExecID }
                    executionMetadata.hookAdditionalContexts.append(contentsOf: metadata.hookAdditionalContexts)
                }
                switch try IOSRemoteExecDaemonWire.decodeElement(payload) {
                case .stdout(let value): stdout += value
                case .stderr(let value): stderr += value
                case .exit(let code, let cwd, let aborted): exit = (code, cwd, aborted)
                case .read(let value): readResult = value
                case .write(let value): writeResult = value
                case .delete(let value): deleteResult = value
                case .fetch(let value): fetchResult = value
                case .shellResult(let value),
                     .backgroundShellSpawn(let value),
                     .writeShellStdin(let value),
                     .shellBackgrounded(let value),
                     .shellSandboxUnsupported(let value):
                    typedShellResult = value
                case .forceBackgroundShellResult(let status, let shellResult):
                    var foundation: [String: Any] = ["status": status.base]
                    if let shellResult {
                        foundation["shellResult"] = shellResult.mapValues { $0.base }
                    }
                    forceBackgroundShellResult = try CoordinatorPayload.fromFoundation(foundation)
                case .grep(let json):
                    guard let data = json.data(using: .utf8),
                          let object = try JSONSerialization.jsonObject(with: data) as? [String: Any]
                    else {
                        throw IOSRemoteExecDaemonError.invalidResponse("grep result JSON is invalid")
                    }
                    grepResult = try CoordinatorPayload.fromFoundation(object)
                case .lsSuccess(let tree):
                    lsResult = try CoordinatorPayload.fromFoundation([
                        "case": "success",
                        "directoryTreeRoot": tree.foundationValue,
                    ])
                case .lsTimeout(let tree):
                    lsResult = try CoordinatorPayload.fromFoundation([
                        "case": "timeout",
                        "directoryTreeRoot": tree.foundationValue,
                    ])
                case .lsError(let path, let error):
                    lsResult = try CoordinatorPayload.fromFoundation([
                        "case": "error",
                        "path": path,
                        "error": error,
                    ])
                case .lsRejected(let path, let reason):
                    lsResult = try CoordinatorPayload.fromFoundation([
                        "case": "rejected",
                        "path": path,
                        "reason": reason,
                    ])
                case .shellRejected(let command, let workingDirectory, let reason):
                    shellTerminalResult = try CoordinatorPayload.fromFoundation([
                        "case": "rejected",
                        "value": [
                            "command": command,
                            "workingDirectory": workingDirectory,
                            "reason": reason,
                        ],
                    ])
                case .shellPermissionDenied(let command, let workingDirectory, let error, let isReadonly):
                    shellTerminalResult = try CoordinatorPayload.fromFoundation([
                        "case": "permissionDenied",
                        "value": [
                            "command": command,
                            "workingDirectory": workingDirectory,
                            "error": error,
                            "isReadonly": isReadonly,
                        ],
                    ])
                case .heartbeat(let id):
                    guard id == 1 else {
                        throw IOSRemoteExecDaemonError.invalidResponse("heartbeat id does not match request")
                    }
                case .streamClose(let id):
                    guard id == 1 else {
                        throw IOSRemoteExecDaemonError.invalidResponse("streamClose id does not match request")
                    }
                    sawStreamClose = true
                case .remoteFailure(let id, let message, let stackTrace, let errorCode):
                    guard id == 1 else {
                        throw IOSRemoteExecDaemonError.invalidResponse("throw id does not match request")
                    }
                    if remoteFailure == nil {
                        remoteFailure = (id, message, stackTrace, errorCode)
                    }
                case .remoteError(let message):
                    if remoteFailure == nil {
                        remoteFailure = (1, message, nil, nil)
                    }
                case .ignore: break
                }
            }
        }

        func preservingExecutionMetadata(_ result: CoordinatorPayload) -> CoordinatorPayload {
            guard executionMetadata.hasForwardableMetadata else { return result }
            let metadata = executionMetadata.coordinatorPayload
            switch result {
            case .object(var fields):
                fields["_executionMetadata"] = metadata
                return .object(fields)
            default:
                return .object([
                    "value": result,
                    "_executionMetadata": metadata,
                ])
            }
        }

        if let requestExecutor {
            let (data, response) = try await requestExecutor(request)
            try Self.validate(response)
            try consume(try decoder.append(data))
        } else {
            let (bytes, response) = try await session.bytes(for: request)
            try Self.validate(response)
            for try await byte in bytes {
                try Task.checkCancellation()
                try consume(try decoder.append(byte))
            }
        }
        try decoder.finish()
        try Task.checkCancellation()
        guard sawEnd else {
            throw IOSRemoteExecDaemonError.invalidResponse("Connect stream ended without end-stream frame")
        }
        if let remoteFailure {
            var detail = remoteFailure.message
            if let errorCode = remoteFailure.errorCode, !errorCode.isEmpty {
                detail = "\(errorCode): \(detail)"
            }
            if let stackTrace = remoteFailure.stackTrace, !stackTrace.isEmpty {
                detail += "\n\(stackTrace)"
            }
            throw IOSRemoteExecDaemonError.remote(detail)
        }

        if let forceBackgroundShellResult {
            return preservingExecutionMetadata(forceBackgroundShellResult)
        }
        if let typedShellResult {
            var foundation: [String: Any] = [:]
            for (key, value) in typedShellResult { foundation[key] = value }
            return preservingExecutionMetadata(try CoordinatorPayload.fromFoundation(foundation))
        }
        if let readResult {
            var foundation: [String: Any] = [:]
            for (key, value) in readResult { foundation[key] = value }
            return preservingExecutionMetadata(try CoordinatorPayload.fromFoundation(foundation))
        }
        if let writeResult {
            var foundation: [String: Any] = [:]
            for (key, value) in writeResult { foundation[key] = value }
            return preservingExecutionMetadata(try CoordinatorPayload.fromFoundation(foundation))
        }
        if let deleteResult {
            var foundation: [String: Any] = [:]
            for (key, value) in deleteResult { foundation[key] = value }
            return preservingExecutionMetadata(try CoordinatorPayload.fromFoundation(foundation))
        }
        if let fetchResult {
            var foundation: [String: Any] = [:]
            for (key, value) in fetchResult { foundation[key] = value }
            return preservingExecutionMetadata(try CoordinatorPayload.fromFoundation(foundation))
        }
        if let grepResult {
            return preservingExecutionMetadata(grepResult)
        }
        if let lsResult {
            return preservingExecutionMetadata(lsResult)
        }
        if let shellTerminalResult {
            return preservingExecutionMetadata(shellTerminalResult)
        }
        guard let exit else {
            throw IOSRemoteExecDaemonError.invalidResponse("shell stream ended without exit")
        }
        if method == "local-exec.browser.exec" {
            guard exit.code == 0, !exit.aborted else {
                throw IOSRemoteExecDaemonError.remote(
                    stderr.isEmpty ? "browser driver shell failed with code \(exit.code)" : stderr
                )
            }
            return preservingExecutionMetadata(try Self.decodeBrowserDriverOutput(stdout))
        }
        return preservingExecutionMetadata(try CoordinatorPayload.fromFoundation([
            "success": exit.code == 0 && !exit.aborted,
            "code": exit.code,
            "stdout": stdout,
            "stderr": stderr,
            "cwd": exit.cwd,
            "aborted": exit.aborted,
        ]))
    }

    private static func validateWorkspacePath(
        _ value: String,
        field: String,
        allowEmpty: Bool = false
    ) throws {
        if value.isEmpty, allowEmpty { return }
        guard !value.isEmpty,
              value.utf8.count <= 4_096,
              !value.contains("\0"),
              !value.contains("\r"),
              !value.contains("\n")
        else {
            throw IOSRemoteExecDaemonError.invalidRequest("\(field) is invalid")
        }
        let path = NSString(string: value).standardizingPath
        guard path == value,
              path == "/workspace" || path.hasPrefix("/workspace/")
        else {
            throw IOSRemoteExecDaemonError.invalidRequest("\(field) must remain inside /workspace")
        }
    }

    private static func decodeBrowserDriverOutput(_ stdout: String) throws -> CoordinatorPayload {
        let screenshotMarker = "__FABUSHI_BROWSER_SCREENSHOT__"
        let screenshot: String?
        let driverText: String
        if let range = stdout.range(of: screenshotMarker, options: .backwards) {
            driverText = String(stdout[..<range.lowerBound])
            screenshot = stdout[range.upperBound...]
                .split(whereSeparator: \.isNewline)
                .map(String.init)
                .first(where: { !$0.isEmpty })
        } else {
            driverText = stdout
            screenshot = nil
        }
        let marker = "__SAND_BROWSER_RESULT__"
        guard let range = driverText.range(of: marker, options: .backwards) else {
            throw IOSRemoteExecDaemonError.invalidResponse("browser driver result marker is missing")
        }
        let tail = driverText[range.upperBound...]
        guard let line = tail.split(whereSeparator: \.isNewline).first(where: { !$0.isEmpty }),
              let data = String(line).data(using: .utf8),
              let object = try JSONSerialization.jsonObject(with: data) as? [String: Any]
        else {
            throw IOSRemoteExecDaemonError.invalidResponse("browser driver result JSON is invalid")
        }
        let ok = object["ok"] as? Bool == true
        var result: [String: Any] = [
            "text": (object["summary"] as? String)
                ?? (object["error"] as? String)
                ?? (ok ? "Browser action completed" : "Browser action failed"),
            "isError": !ok,
        ]
        for key in ["viewId", "generation", "frameName", "frameUrl"] {
            if let value = object[key] as? String, !value.isEmpty {
                result[key] = value
            }
        }
        if let screenshot, !screenshot.isEmpty {
            guard Data(base64Encoded: screenshot) != nil else {
                throw IOSRemoteExecDaemonError.invalidResponse("browser screenshot is not valid base64")
            }
            result["imageB64"] = screenshot
        }
        return try CoordinatorPayload.fromFoundation(result)
    }

    private static func shellQuotedArgument(_ value: String) -> String {
        "'\(value.replacingOccurrences(of: "'", with: "'\\''"))'"
    }

    private static func integer(_ value: Any?) -> Int? {
        switch value {
        case let value as Int:
            return value
        case let value as NSNumber:
            let double = value.doubleValue
            guard double.isFinite,
                  double.rounded(.towardZero) == double,
                  double >= Double(Int.min),
                  double <= Double(Int.max)
            else { return nil }
            return Int(double)
        case let value as Double:
            guard value.isFinite,
                  value.rounded(.towardZero) == value,
                  value >= Double(Int.min),
                  value <= Double(Int.max)
            else { return nil }
            return Int(value)
        default:
            return nil
        }
    }

    private static func validate(_ response: URLResponse) throws {
        guard let http = response as? HTTPURLResponse else {
            throw IOSRemoteExecDaemonError.invalidResponse("missing HTTP response")
        }
        guard (200..<300).contains(http.statusCode) else {
            throw IOSRemoteExecDaemonError.remote("HTTP \(http.statusCode)")
        }
    }
}


protocol IOSLocalCapabilityBackend: Sendable {
    func execute(
        capability: LocalCapabilityRunner.Capability,
        params: CoordinatorPayload
    ) async throws -> CoordinatorPayload
}

/// Production routing for Grok local-exec semantics on iOS.
///
/// Only explicitly allow-listed native capabilities execute on-device.
/// Shell/process/file-daemon semantics are delegated to RemoteRunner and are
/// never emulated with unrestricted native process-spawn APIs on the phone.
actor IOSProductionLocalExecutor {
    private let capabilityRunner: LocalCapabilityRunner
    private let localBackend: any IOSLocalCapabilityBackend
    private let remoteRunner: RemoteRunner

    init(
        capabilityRunner: LocalCapabilityRunner = LocalCapabilityRunner(),
        localBackend: any IOSLocalCapabilityBackend = IOSNativeLocalCapabilityBackend(),
        remoteTransport: any RemoteRunnerTransport
    ) {
        self.capabilityRunner = capabilityRunner
        self.localBackend = localBackend
        remoteRunner = RemoteRunner(transport: remoteTransport)
    }

    func execute(
        method: String,
        params: CoordinatorPayload = .object([:])
    ) async throws -> CoordinatorPayload {
        let normalized = method.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !normalized.isEmpty else {
            throw LocalCapabilityRunner.RunnerError.unsupported("empty-method")
        }

        if let capability = Self.localCapability(for: normalized) {
            guard await capabilityRunner.supports(capability) else {
                throw LocalCapabilityRunner.RunnerError.permissionDenied(capability.rawValue)
            }
            return try await localBackend.execute(capability: capability, params: params)
        }

        guard Self.requiresRemoteRunner(normalized) else {
            throw LocalCapabilityRunner.RunnerError.unsupported(normalized)
        }

        let remoteParams = ShellExecEnvironmentFilter.sanitizeRemoteRunnerParams(params)
        return try await remoteRunner.dispatch(
            method: "local-exec.\(normalized)",
            params: remoteParams
        )
    }

    private static func requiresRemoteRunner(_ method: String) -> Bool {
        remoteNamespace(method, root: "shell")
            || remoteNamespace(method, root: "process")
            || remoteNamespace(method, root: "box")
            || remoteNamespace(method, root: "browser")
    }

    private static func remoteNamespace(_ method: String, root: String) -> Bool {
        method == root || method.hasPrefix("\(root).")
    }

    private static func localCapability(for method: String) -> LocalCapabilityRunner.Capability? {
        switch method {
        case "openExternalURL": .openExternalURL
        case "backgroundTransfer": .backgroundTransfer
        case "shareItem": .shareItem
        case "clipboardRead": .clipboardRead
        case "clipboardWrite": .clipboardWrite
        default: nil
        }
    }
}


struct IOSRemoteRunnerExecutionIdentity: Equatable, Sendable {
    let accountScope: String
    let accountRevision: UInt64
    let hostGeneration: UInt64
}

enum IOSRemoteRunnerFenceError: LocalizedError, Equatable, Sendable {
    case unavailable
    case stale

    var errorDescription: String? {
        switch self {
        case .unavailable:
            return "Remote Runner is unavailable without an active account/Host generation."
        case .stale:
            return "Remote Runner result was discarded after the account or Host generation changed."
        }
    }
}

/// Coordinator-owned transport for the shipping local-exec replacement.
/// Authority stays on MahayanaCoordinator; this object only snapshots and
/// rechecks the settled account/Host generation around one remote dispatch.
struct IOSGenerationFencedRemoteRunnerTransport: @unchecked Sendable, RemoteRunnerTransport {
    typealias IdentityProvider = @MainActor () -> IOSRemoteRunnerExecutionIdentity?
    typealias Dispatch = @MainActor (String, CoordinatorPayload) async throws -> CoordinatorPayload

    private let currentIdentity: IdentityProvider
    private let dispatchImpl: Dispatch

    init(
        currentIdentity: @escaping IdentityProvider,
        dispatch: @escaping Dispatch
    ) {
        self.currentIdentity = currentIdentity
        dispatchImpl = dispatch
    }

    func dispatch(
        method: String,
        params: CoordinatorPayload
    ) async throws -> CoordinatorPayload {
        guard let token = await currentIdentity() else {
            throw IOSRemoteRunnerFenceError.unavailable
        }
        try Task.checkCancellation()
        let result = try await dispatchImpl(method, params)
        try Task.checkCancellation()
        guard await currentIdentity() == token else {
            throw IOSRemoteRunnerFenceError.stale
        }
        return result
    }
}
