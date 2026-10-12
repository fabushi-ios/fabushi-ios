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

    enum Event: Equatable {
        case stdout(String)
        case stderr(String)
        case exit(code: Int, cwd: String, aborted: Bool)
        case read([String: AnyHashable])
        case rejected(String)
        case remoteError(String)
        case ignore
    }

    static func shellRequest(
        id: UInt32,
        execID: String,
        command: String,
        workingDirectory: String,
        toolCallID: String
    ) throws -> Data {
        guard !command.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            throw IOSRemoteExecDaemonError.invalidRequest("shell command is empty")
        }
        var shell = Data()
        bytes(1, Data(command.utf8), into: &shell)
        if !workingDirectory.isEmpty { bytes(2, Data(workingDirectory.utf8), into: &shell) }
        if !toolCallID.isEmpty { bytes(4, Data(toolCallID.utf8), into: &shell) }
        var root = Data()
        varint(1, UInt64(id), into: &root)
        if !execID.isEmpty { bytes(15, Data(execID.utf8), into: &root) }
        bytes(14, shell, into: &root)
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
            case 14: return try decodeShellStream(nested)
            case 7, 29: return try decodeReadResult(nested)
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
        guard let object = params.foundationValue as? [String: Any] else {
            throw IOSRemoteExecDaemonError.invalidRequest("params must be an object")
        }
        let execID = UUID().uuidString.lowercased()
        let body: Data
        switch method {
        case "local-exec.shell", "local-exec.shell.exec":
            guard let command = object["command"] as? String else {
                throw IOSRemoteExecDaemonError.invalidRequest("shell command is required")
            }
            body = try IOSRemoteExecDaemonWire.shellRequest(
                id: 1,
                execID: execID,
                command: command,
                workingDirectory: object["workingDirectory"] as? String ?? "",
                toolCallID: object["toolCallId"] as? String ?? ""
            )
        case "local-exec.box.exec":
            guard let caseName = object["case"] as? String,
                  caseName == "readArgs" || caseName == "redactedReadArgs",
                  let args = object["args"] as? [String: Any],
                  let path = args["path"] as? String
            else {
                throw IOSRemoteExecDaemonError.unsupported(method)
            }
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
        case "local-exec.process", "local-exec.process.exec", "local-exec.process.spawn":
            throw IOSRemoteExecDaemonError.unsupported(method)
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

        var stdout = ""
        var stderr = ""
        var exit: (code: Int, cwd: String, aborted: Bool)?
        var readResult: [String: AnyHashable]?
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
        guard let exit else {
            throw IOSRemoteExecDaemonError.invalidResponse("shell stream ended without exit")
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
