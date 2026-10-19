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

private enum IOSRemoteExecDaemonWire {
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
                guard field > 0 else {
                    throw IOSRemoteExecDaemonError.invalidResponse("protobuf field zero")
                }
                switch Int(key & 7) {
                case 0:
                    result.append((field, .varint(try varint())))
                case 1:
                    try skip(8)
                case 2:
                    let count = try length()
                    guard index + count <= bytes.count else {
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
            var shift: UInt64 = 0
            while index < bytes.count, shift < 64 {
                let byte = bytes[index]
                index += 1
                value |= UInt64(byte & 0x7f) << shift
                if byte & 0x80 == 0 { return value }
                shift += 7
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
            guard index + count <= bytes.count else {
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

    enum Event: Equatable {
        case stdout(String)
        case stderr(String)
        case exit(code: Int, cwd: String, aborted: Bool)
        case read([String: AnyHashable])
        case write([String: AnyHashable])
        case lsSuccess(LsNode)
        case lsTimeout(LsNode)
        case lsError(path: String, error: String)
        case lsRejected(path: String, reason: String)
        case rejected(String)
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
        adminCommandDenylist: CoordinatorPayload? = nil
    ) throws -> Data {
        guard !command.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            throw IOSRemoteExecDaemonError.invalidRequest("shell command is empty")
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
        for rule in try stringArray(adminCommandDenylist, field: "adminCommandDenylist") {
            bytes(22, Data(rule.utf8), into: &shell)
        }
        var root = Data()
        varint(1, UInt64(id), into: &root)
        if !execID.isEmpty { bytes(15, Data(execID.utf8), into: &root) }
        bytes(14, shell, into: &root)
        return root
    }

    static func writeRequest(
        id: UInt32,
        execID: String,
        path: String,
        fileBytes: Data,
        toolCallID: String
    ) throws -> Data {
        guard !path.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            throw IOSRemoteExecDaemonError.invalidRequest("write path is empty")
        }
        var write = Data()
        bytes(1, Data(path.utf8), into: &write)
        if !toolCallID.isEmpty { bytes(3, Data(toolCallID.utf8), into: &write) }
        bytes(5, fileBytes, into: &write)

        var root = Data()
        varint(1, UInt64(id), into: &root)
        if !execID.isEmpty { bytes(15, Data(execID.utf8), into: &root) }
        bytes(3, write, into: &root)
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

    static func decodeElement(_ data: Data) throws -> Event {
        var root = Reader(data)
        for (field, value) in try root.fields() {
            guard case .bytes(let nested) = value else { continue }
            if field == 1 { return try decodeClientMessage(nested) }
            if field == 2 { return try decodeControl(nested) }
        }
        return .ignore
    }

    private static func decodeClientMessage(_ data: Data) throws -> Event {
        var reader = Reader(data)
        for (field, value) in try reader.fields() {
            guard case .bytes(let nested) = value else { continue }
            switch field {
            case 3: return try decodeWriteResult(nested)
            case 14: return try decodeShellStream(nested)
            case 7, 29: return try decodeReadResult(nested)
            case 8: return try decodeLsResult(nested)
            default: continue
            }
        }
        return .ignore
    }

    private static func decodeControl(_ data: Data) throws -> Event {
        var reader = Reader(data)
        for (field, value) in try reader.fields() {
            guard field == 2, case .bytes(let thrown) = value else { continue }
            var thrownReader = Reader(thrown)
            for (inner, innerValue) in try thrownReader.fields() {
                if inner == 2, case .bytes(let message) = innerValue {
                    return .remoteError(String(data: message, encoding: .utf8) ?? "remote execution error")
                }
            }
            return .remoteError("remote execution error")
        }
        return .ignore
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
                return .rejected(try firstString(3, in: nested) ?? "shell request rejected")
            case 6:
                return .rejected(try firstString(3, in: nested) ?? "shell permission denied")
            default:
                return .ignore
            }
        }
        return .ignore
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

    private static func decodeWriteResult(_ data: Data) throws -> Event {
        var reader = Reader(data)
        for (field, value) in try reader.fields() {
            guard case .bytes(let nested) = value else { continue }
            switch field {
            case 1:
                return .write([
                    "success": true,
                    "path": try firstString(1, in: nested) ?? "",
                ])
            case 5:
                return .remoteError(try firstString(2, in: nested) ?? "write failed")
            case 6:
                return .rejected(try firstString(2, in: nested) ?? "write rejected")
            default:
                return .remoteError("unsupported write result")
            }
        }
        return .remoteError("missing write result")
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
                return .rejected(try firstString(2, in: nested) ?? "read rejected")
            case 4:
                return .remoteError("file not found")
            case 5:
                return .rejected("read permission denied")
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
        let execID = UUID().uuidString.lowercased()
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
                adminCommandDenylist: payload["adminCommandDenylist"]
            )
        case "local-exec.box.exec":
            guard let caseName = object["case"] as? String,
                  let args = object["args"] as? [String: Any],
                  let path = args["path"] as? String
            else {
                throw IOSRemoteExecDaemonError.unsupported(method)
            }
            try Self.validateWorkspacePath(path, field: "box path")
            if caseName == "writeArgs" {
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
                    toolCallID: args["toolCallId"] as? String ?? ""
                )
            } else if caseName == "readArgs" || caseName == "redactedReadArgs" {
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
            } else if caseName == "lsArgs" {
                let ignore = args["ignore"] as? [String] ?? []
                let sandboxPolicy: CoordinatorPayload?
                if case .object(let argsPayload)? = payload["args"] {
                    sandboxPolicy = argsPayload["sandboxPolicy"]
                } else {
                    sandboxPolicy = nil
                }
                body = try IOSRemoteExecDaemonWire.lsRequest(
                    id: 1,
                    execID: execID,
                    path: path,
                    ignore: ignore,
                    toolCallID: args["toolCallId"] as? String ?? "",
                    sandboxPolicy: sandboxPolicy,
                    timeoutMs: Self.integer(args["timeoutMs"])
                )
            } else {
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
        request.httpBody = body
        if method == "local-exec.browser.exec" {
            // Desktop c33a9495 gives ExecService enough time for bounded navigation/browser startup.
            request.timeoutInterval = 120
        }

        var stdout = ""
        var stderr = ""
        var exit: (code: Int, cwd: String, aborted: Bool)?
        var readResult: [String: AnyHashable]?
        var writeResult: [String: AnyHashable]?
        var lsResult: CoordinatorPayload?
        var decoder = IOSRemoteExecConnectDecoder()
        var sawEnd = false

        func consume(_ envelopes: [(UInt8, Data)]) throws {
            for (flags, payload) in envelopes {
                if flags & 0x02 != 0 {
                    sawEnd = true
                    if let object = try? JSONSerialization.jsonObject(with: payload) as? [String: Any],
                       let error = object["error"] as? [String: Any] {
                        let message = error["message"] as? String ?? "remote execution stream failed"
                        throw IOSRemoteExecDaemonError.remote(message)
                    }
                    continue
                }
                switch try IOSRemoteExecDaemonWire.decodeElement(payload) {
                case .stdout(let value): stdout += value
                case .stderr(let value): stderr += value
                case .exit(let code, let cwd, let aborted): exit = (code, cwd, aborted)
                case .read(let value): readResult = value
                case .write(let value): writeResult = value
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
                case .rejected(let message): throw IOSRemoteExecDaemonError.remote(message)
                case .remoteError(let message): throw IOSRemoteExecDaemonError.remote(message)
                case .ignore: break
                }
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

        if let readResult {
            var foundation: [String: Any] = [:]
            for (key, value) in readResult { foundation[key] = value }
            return try CoordinatorPayload.fromFoundation(foundation)
        }
        if let writeResult {
            var foundation: [String: Any] = [:]
            for (key, value) in writeResult { foundation[key] = value }
            return try CoordinatorPayload.fromFoundation(foundation)
        }
        if let lsResult {
            return lsResult
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
            return try Self.decodeBrowserDriverOutput(stdout)
        }
        return try CoordinatorPayload.fromFoundation([
            "success": exit.code == 0 && !exit.aborted,
            "code": exit.code,
            "stdout": stdout,
            "stderr": stderr,
            "cwd": exit.cwd,
            "aborted": exit.aborted,
        ])
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
