import Foundation

/// Account-scoped remote-device transport owned by the installed iOS app.
///
/// This adapter deliberately publishes only the existing `fabushi.app.*`
/// semantic surface. It never exposes arbitrary Swift execution, shell access,
/// JavaScript execution, or credential-field writes.
@MainActor
final class FabushiRemoteDeviceGateway {
    static let officialGatewayURL = URL(string: "wss://fabushi-mcp.ombhrum.com/agent")!
    static let leaseSeconds = 14_400

    struct AgentSession: Equatable {
        let accessToken: String
        let deviceId: String
        let sessionId: String
        let username: String
        let accessTokenExpiresAt: Int64
    }

    enum GatewayError: LocalizedError, Equatable {
        case invalidAgentSession
        case invalidCall
        case unknownTool
        case invalidGeneration
        case missingTarget
        case invalidRef

        var errorDescription: String? {
            switch self {
            case .invalidAgentSession: "invalid_device_agent_session"
            case .invalidCall: "invalid_device_gateway_call"
            case .unknownTool: "unknown_fabushi_app_tool"
            case .invalidGeneration: "invalid_app_surface_generation"
            case .missingTarget: "app_surface_action_target_missing"
            case .invalidRef: "invalid_app_surface_ref"
            }
        }
    }

    private let host: MahayanaHost
    private let surface: FabushiAppAgentSurface
    private let traceURL: URL
    private let urlSession: URLSession
    private var socket: URLSessionWebSocketTask?
    private var monitorTask: Task<Void, Never>?
    private var receiveTask: Task<Void, Never>?
    private var heartbeatTask: Task<Void, Never>?
    private var desiredLoggedIn = false
    private var activeSession: AgentSession?
    private var registered = false

    init(host: MahayanaHost, surface: FabushiAppAgentSurface, traceURL: URL) {
        self.host = host
        self.surface = surface
        self.traceURL = traceURL
        let configuration = URLSessionConfiguration.ephemeral
        configuration.requestCachePolicy = .reloadIgnoringLocalAndRemoteCacheData
        configuration.urlCache = nil
        self.urlSession = URLSession(configuration: configuration)
    }

    func setLoggedIn(_ loggedIn: Bool) async {
        desiredLoggedIn = loggedIn
        if !loggedIn {
            stopConnection(reason: "logged-out")
            monitorTask?.cancel()
            monitorTask = nil
            return
        }
        guard monitorTask == nil else { return }
        monitorTask = Task { [weak self] in
            guard let self else { return }
            while !Task.isCancelled {
                await self.refreshConnection()
                do { try await Task.sleep(for: .seconds(10)) }
                catch { return }
            }
        }
    }

    func stop() {
        desiredLoggedIn = false
        monitorTask?.cancel()
        monitorTask = nil
        stopConnection(reason: "stopped")
    }

    private func refreshConnection() async {
        guard desiredLoggedIn else { return }
        do {
            let raw = try await host.request(method: "feature.auth.deviceAgentSession")
            let candidate = try Self.parseAgentSession(raw.value)
            if let activeSession,
               activeSession.deviceId == candidate.deviceId,
               activeSession.sessionId == candidate.sessionId,
               activeSession.accessToken == candidate.accessToken,
               registered,
               socket?.state == .running {
                return
            }
            try await connect(candidate)
        } catch {
            appendTrace([
                "phase": "connection-refresh-failed",
                "error": Self.safeErrorCode(error),
            ])
            stopConnection(reason: "refresh-failed")
        }
    }

    private func connect(_ agentSession: AgentSession) async throws {
        stopConnection(reason: "reconnect")
        var request = URLRequest(url: Self.officialGatewayURL)
        request.timeoutInterval = 30
        request.setValue("Bearer \(agentSession.accessToken)", forHTTPHeaderField: "Authorization")
        let task = urlSession.webSocketTask(with: request)
        socket = task
        activeSession = agentSession
        registered = false
        task.resume()

        let registration: [String: Any] = [
            "type": "register",
            "deviceId": agentSession.deviceId,
            "name": Self.deviceName(deviceId: agentSession.deviceId),
            "platform": "ios",
            "capabilities": FabushiAppAgentSurface.toolNames,
            "tools": Self.toolDescriptors,
            "leaseSeconds": Self.leaseSeconds,
            "metadata": Self.gatewayMetadata(),
        ]
        try await send(registration, over: task)
        appendTrace([
            "phase": "register-sent",
            "deviceId": agentSession.deviceId,
            "sessionId": String(agentSession.sessionId.prefix(96)),
        ])

        receiveTask = Task { [weak self, weak task] in
            guard let self, let task else { return }
            await self.receiveLoop(task)
        }
        heartbeatTask = Task { [weak self, weak task] in
            guard let self, let task else { return }
            while !Task.isCancelled {
                do { try await Task.sleep(for: .seconds(25)) }
                catch { return }
                guard self.socket === task, task.state == .running else { return }
                do { try await self.send(["type": "heartbeat"], over: task) }
                catch {
                    self.appendTrace(["phase": "heartbeat-failed", "error": Self.safeErrorCode(error)])
                    return
                }
            }
        }
    }

    private func stopConnection(reason: String) {
        receiveTask?.cancel()
        receiveTask = nil
        heartbeatTask?.cancel()
        heartbeatTask = nil
        if let socket {
            socket.cancel(with: .goingAway, reason: nil)
        }
        socket = nil
        activeSession = nil
        if registered {
            appendTrace(["phase": "disconnected", "reason": String(reason.prefix(80))])
        }
        registered = false
    }

    private func receiveLoop(_ task: URLSessionWebSocketTask) async {
        while !Task.isCancelled, socket === task {
            do {
                let message = try await task.receive()
                let data: Data
                switch message {
                case .string(let string):
                    guard let encoded = string.data(using: .utf8) else { continue }
                    data = encoded
                case .data(let raw):
                    data = raw
                @unknown default:
                    continue
                }
                guard data.count <= 32 * 1024 * 1024,
                      let object = try JSONSerialization.jsonObject(with: data) as? [String: Any],
                      let type = object["type"] as? String
                else { continue }

                switch type {
                case "registered":
                    registered = true
                    appendTrace([
                        "phase": "registered",
                        "deviceId": String((object["deviceId"] as? String ?? "").prefix(128)),
                    ])
                case "call":
                    await handleCall(object, over: task)
                default:
                    continue
                }
            } catch {
                guard !Task.isCancelled, socket === task else { return }
                appendTrace(["phase": "receive-failed", "error": Self.safeErrorCode(error)])
                registered = false
                task.cancel(with: .goingAway, reason: nil)
                socket = nil
                activeSession = nil
                return
            }
        }
    }

    private func handleCall(_ message: [String: Any], over task: URLSessionWebSocketTask) async {
        let requestId = String(message["requestId"] as? String ?? "").prefix(128)
        let toolName = String(message["toolName"] as? String ?? "").prefix(128)
        guard !requestId.isEmpty, !toolName.isEmpty else { return }
        let arguments = message["arguments"] as? [String: Any] ?? [:]
        appendTrace([
            "phase": "call-started",
            "requestId": String(requestId),
            "toolName": String(toolName),
        ])
        do {
            let result = try await invokeTool(String(toolName), arguments: arguments)
            try await send([
                "type": "result",
                "requestId": String(requestId),
                "ok": true,
                "result": [
                    "structuredContent": result,
                    "content": [["type": "text", "text": "Fabushi iOS semantic tool completed."]],
                ],
            ], over: task)
            appendTrace([
                "phase": "call-completed",
                "requestId": String(requestId),
                "toolName": String(toolName),
                "ok": true,
            ])
        } catch {
            let safeError = Self.safeErrorCode(error)
            try? await send([
                "type": "result",
                "requestId": String(requestId),
                "ok": false,
                "error": safeError,
            ], over: task)
            appendTrace([
                "phase": "call-completed",
                "requestId": String(requestId),
                "toolName": String(toolName),
                "ok": false,
                "error": safeError,
            ])
        }
    }

    private func invokeTool(_ toolName: String, arguments: [String: Any]) async throws -> [String: Any] {
        guard FabushiAppAgentSurface.toolNames.contains(toolName) else { throw GatewayError.unknownTool }
        switch toolName {
        case "fabushi.app.status":
            return Self.statusDictionary(surface.status())
        case "fabushi.app.snapshot":
            let snapshot = surface.snapshot()
            let maxElements = Self.boundedInt(arguments["maxElements"], minimum: 1, maximum: 500) ?? 500
            return Self.snapshotDictionary(snapshot, maxElements: maxElements)
        case "fabushi.app.find":
            let snapshot = surface.snapshot()
            let ref = arguments["ref"] as? String
            let refTarget = try ref.map { try Self.parseRef($0, expectedGeneration: snapshot.generation) }
            let requestedName = (arguments["name"] as? String) ?? (arguments["text"] as? String)
            let limit = Self.boundedInt(arguments["limit"], minimum: 1, maximum: 100) ?? 25
            let matches = surface.find(
                agentId: refTarget ?? arguments["agentId"] as? String,
                role: arguments["role"] as? String,
                name: requestedName,
                limit: limit
            )
            return [
                "version": FabushiAppAgentSurface.version,
                "appId": snapshot.appId,
                "platform": snapshot.platform,
                "screen": snapshot.screen,
                "generation": snapshot.generation,
                "matches": matches.map { Self.elementDictionary($0, generation: snapshot.generation) },
            ]
        case "fabushi.app.action":
            guard let generation = Self.uint64(arguments["generation"]) else { throw GatewayError.invalidGeneration }
            let requestedAgentId: String?
            if let ref = arguments["ref"] as? String {
                requestedAgentId = try Self.parseRef(ref, expectedGeneration: generation)
            } else {
                requestedAgentId = arguments["agentId"] as? String
            }
            guard let agentId = requestedAgentId, !agentId.isEmpty else { throw GatewayError.missingTarget }
            guard let action = arguments["action"] as? String, !action.isEmpty else { throw GatewayError.invalidCall }
            let snapshot = try surface.perform(
                expectedGeneration: generation,
                agentId: agentId,
                action: action,
                value: arguments["value"] as? String
            )
            await Task.yield()
            return Self.snapshotDictionary(snapshot, maxElements: 500)
        case "fabushi.app.wait":
            let timeout = UInt64(Self.boundedInt(arguments["timeoutMs"], minimum: 100, maximum: 30_000) ?? 10_000)
            let assertion = await surface.waitFor(
                screen: (arguments["screen"] as? String) ?? (arguments["route"] as? String),
                agentId: arguments["agentId"] as? String,
                role: arguments["role"] as? String,
                name: (arguments["name"] as? String) ?? (arguments["text"] as? String),
                state: arguments["state"] as? String ?? "present",
                timeoutMilliseconds: timeout
            )
            return Self.assertionDictionary(assertion)
        case "fabushi.app.assert":
            return Self.assertionDictionary(surface.assertState(
                screen: (arguments["screen"] as? String) ?? (arguments["route"] as? String),
                agentId: arguments["agentId"] as? String,
                role: arguments["role"] as? String,
                name: (arguments["name"] as? String) ?? (arguments["text"] as? String),
                state: arguments["state"] as? String ?? "present"
            ))
        default:
            throw GatewayError.unknownTool
        }
    }

    private func send(_ object: [String: Any], over task: URLSessionWebSocketTask) async throws {
        guard JSONSerialization.isValidJSONObject(object) else { throw GatewayError.invalidCall }
        let data = try JSONSerialization.data(withJSONObject: object, options: [.sortedKeys])
        guard let string = String(data: data, encoding: .utf8) else { throw GatewayError.invalidCall }
        try await task.send(.string(string))
    }

    private func appendTrace(_ record: [String: Any]) {
        var safe = record
        safe["at"] = ISO8601DateFormatter().string(from: Date())
        safe["platform"] = "ios"
        guard JSONSerialization.isValidJSONObject(safe),
              var data = try? JSONSerialization.data(withJSONObject: safe, options: [.sortedKeys])
        else { return }
        data.append(0x0A)
        do {
            try FileManager.default.createDirectory(at: traceURL.deletingLastPathComponent(), withIntermediateDirectories: true)
            if !FileManager.default.fileExists(atPath: traceURL.path) {
                _ = FileManager.default.createFile(atPath: traceURL.path, contents: data)
                return
            }
            let handle = try FileHandle(forWritingTo: traceURL)
            try handle.seekToEnd()
            try handle.write(contentsOf: data)
            try handle.close()
        } catch {
            // Evidence logging must never break the product control channel.
        }
    }

    static func parseAgentSession(_ value: Any) throws -> AgentSession {
        guard let object = value as? [String: Any] else { throw GatewayError.invalidAgentSession }
        let accessToken = String(object["accessToken"] as? String ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        let deviceId = String(object["deviceId"] as? String ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        let sessionId = String(object["sessionId"] as? String ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        let usernameValue = (object["username"] as? String) ?? ((object["user"] as? [String: Any])?["username"] as? String) ?? ""
        let username = String(usernameValue.prefix(200))
        let expiry = (object["accessTokenExpiresAt"] as? NSNumber)?.int64Value ?? 0
        guard accessToken.count >= 24,
              accessToken.count <= 16 * 1024,
              accessToken.rangeOfCharacter(from: .whitespacesAndNewlines) == nil,
              deviceId.range(of: #"^[A-Za-z0-9._:-]{1,128}$"#, options: .regularExpression) != nil,
              !sessionId.isEmpty,
              sessionId.count <= 200
        else { throw GatewayError.invalidAgentSession }
        return AgentSession(
            accessToken: accessToken,
            deviceId: deviceId,
            sessionId: sessionId,
            username: username,
            accessTokenExpiresAt: expiry
        )
    }

    static var toolDescriptors: [[String: Any]] {
        let empty: [String: Any] = ["type": "object", "properties": [:] as [String: Any]]
        let readOnly: [String: Any] = ["readOnlyHint": true]
        let mutable: [String: Any] = ["readOnlyHint": false]
        return [
            ["name": "fabushi.app.status", "title": "Fabushi app agent-surface status", "description": "Report whether the active Fabushi iOS application exposes its structured semantic surface.", "inputSchema": empty, "annotations": readOnly],
            ["name": "fabushi.app.snapshot", "title": "Read the Fabushi semantic UI", "description": "Return a structured redacted semantic snapshot of the active Fabushi iOS UI.", "inputSchema": ["type": "object", "properties": ["maxElements": ["type": "integer"], "includeText": ["type": "boolean"]]], "annotations": readOnly],
            ["name": "fabushi.app.find", "title": "Find Fabushi UI elements", "description": "Find semantic elements by stable id, generation-bound ref, role, accessible name, or visible text.", "inputSchema": ["type": "object", "properties": ["agentId": ["type": "string"], "ref": ["type": "string"], "role": ["type": "string"], "name": ["type": "string"], "text": ["type": "string"], "limit": ["type": "integer"]]], "annotations": readOnly],
            ["name": "fabushi.app.action", "title": "Operate the Fabushi semantic UI", "description": "Perform one allowlisted action against the exact current semantic generation.", "inputSchema": ["type": "object", "properties": ["generation": ["type": "integer"], "ref": ["type": "string"], "agentId": ["type": "string"], "action": ["type": "string", "enum": ["invoke", "focus", "setValue", "pressKey", "scroll", "selectOption", "toggle"]], "value": ["type": "string"]], "required": ["generation", "action"]], "annotations": mutable],
            ["name": "fabushi.app.wait", "title": "Wait for Fabushi UI state", "description": "Wait for a bounded semantic UI condition.", "inputSchema": stateQuerySchema(includeTimeout: true), "annotations": readOnly],
            ["name": "fabushi.app.assert", "title": "Assert Fabushi UI state", "description": "Evaluate a deterministic semantic UI assertion for CI evidence.", "inputSchema": stateQuerySchema(includeTimeout: false), "annotations": readOnly],
        ]
    }

    private static func stateQuerySchema(includeTimeout: Bool) -> [String: Any] {
        var properties: [String: Any] = [
            "route": ["type": "string"],
            "screen": ["type": "string"],
            "agentId": ["type": "string"],
            "role": ["type": "string"],
            "name": ["type": "string"],
            "text": ["type": "string"],
            "state": ["type": "string", "enum": ["present", "absent", "enabled", "disabled", "visible", "hidden"]],
        ]
        if includeTimeout { properties["timeoutMs"] = ["type": "integer"] }
        return ["type": "object", "properties": properties]
    }

    private static func gatewayMetadata() -> [String: Any] {
        let environment = ProcessInfo.processInfo.environment
        var metadata: [String: Any] = [
            "kind": environment["GITHUB_ACTIONS"] == "true" ? "github-actions-ios-app" : "fabushi-ios",
        ]
        let mapping = [
            "GITHUB_REPOSITORY": "repository",
            "GITHUB_WORKFLOW": "workflow",
            "GITHUB_JOB": "job",
            "GITHUB_RUN_ID": "runId",
            "GITHUB_RUN_ATTEMPT": "runAttempt",
            "GITHUB_SHA": "sha",
            "RUNNER_NAME": "runnerName",
            "RUNNER_OS": "runnerOs",
            "RUNNER_ARCH": "runnerArch",
        ]
        for (source, destination) in mapping {
            if let value = environment[source], !value.isEmpty {
                metadata[destination] = String(value.prefix(300))
            }
        }
        return metadata
    }

    private static func deviceName(deviceId: String) -> String {
        let configured = ProcessInfo.processInfo.environment["FABUSHI_DEVICE_NAME"]?.trimmingCharacters(in: .whitespacesAndNewlines)
        if let configured, !configured.isEmpty { return String(configured.prefix(200)) }
        return String("Fabushi iOS \(deviceId)".prefix(200))
    }

    private static func statusDictionary(_ status: FabushiAppAgentSurface.Status) -> [String: Any] {
        [
            "version": status.version,
            "appId": status.appId,
            "platform": status.platform,
            "available": status.available,
            "screen": status.screen,
            "generation": status.generation,
        ]
    }

    private static func snapshotDictionary(_ snapshot: FabushiAppAgentSurface.Snapshot, maxElements: Int) -> [String: Any] {
        [
            "version": snapshot.version,
            "appId": snapshot.appId,
            "platform": snapshot.platform,
            "screen": snapshot.screen,
            "generation": snapshot.generation,
            "elements": snapshot.elements.prefix(maxElements).map { elementDictionary($0, generation: snapshot.generation) },
        ]
    }

    private static func elementDictionary(_ element: FabushiAppAgentSurface.Element, generation: UInt64) -> [String: Any] {
        var value: [String: Any] = [
            "agentId": element.agentId,
            "ref": makeRef(generation: generation, agentId: element.agentId),
            "role": element.role,
            "name": element.name,
            "visible": element.visible,
            "enabled": element.enabled,
            "sensitive": element.sensitive,
        ]
        if let present = element.valuePresent { value["valuePresent"] = present }
        if let length = element.valueLength { value["valueLength"] = length }
        return value
    }

    private static func assertionDictionary(_ assertion: FabushiAppAgentSurface.Assertion) -> [String: Any] {
        [
            "passed": assertion.passed,
            "screen": assertion.screen,
            "generation": assertion.generation,
            "matches": assertion.matches.map { elementDictionary($0, generation: assertion.generation) },
            "failures": assertion.failures,
        ]
    }

    private static func makeRef(generation: UInt64, agentId: String) -> String {
        "g\(generation):\(agentId)"
    }

    private static func parseRef(_ ref: String, expectedGeneration: UInt64) throws -> String {
        guard ref.hasPrefix("g"), let separator = ref.firstIndex(of: ":") else { throw GatewayError.invalidRef }
        let generationText = ref[ref.index(after: ref.startIndex)..<separator]
        guard UInt64(generationText) == expectedGeneration else { throw FabushiAppAgentSurface.SurfaceError.staleGeneration }
        let id = String(ref[ref.index(after: separator)...])
        guard !id.isEmpty else { throw GatewayError.invalidRef }
        return id
    }

    private static func uint64(_ value: Any?) -> UInt64? {
        guard let number = value as? NSNumber else { return nil }
        let double = number.doubleValue
        guard double.isFinite, double >= 0, double.rounded(.towardZero) == double, double <= Double(UInt64.max) else { return nil }
        return number.uint64Value
    }

    private static func boundedInt(_ value: Any?, minimum: Int, maximum: Int) -> Int? {
        guard let number = value as? NSNumber else { return nil }
        return max(minimum, min(maximum, number.intValue))
    }

    private static func safeErrorCode(_ error: Error) -> String {
        if let error = error as? GatewayError { return error.errorDescription ?? "device_gateway_error" }
        if let error = error as? FabushiAppAgentSurface.SurfaceError { return error.errorDescription ?? "app_surface_error" }
        let value = error as NSError
        return "transport_error:\(String(value.domain.prefix(80))):\(value.code)"
    }
}
