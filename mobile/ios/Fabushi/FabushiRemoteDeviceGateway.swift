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
    static let sessionExpiryLeewayMilliseconds: Int64 = 30_000
    static let maximumResultBytes = 64 * 1024

    struct AgentSession: Equatable {
        let accessToken: String
        let deviceId: String
        let sessionId: String
        let username: String
        let accessTokenExpiresAt: Int64
    }

    struct SessionIdentity: Equatable {
        let deviceId: String
        let sessionId: String
        let accessToken: String
    }

    enum GatewayError: LocalizedError, Equatable {
        case invalidAgentSession
        case invalidCall
        case unknownTool
        case invalidGeneration
        case missingTarget
        case invalidRef
        case resultTooLarge

        var errorDescription: String? {
            switch self {
            case .invalidAgentSession: "invalid_device_agent_session"
            case .invalidCall: "invalid_device_gateway_call"
            case .unknownTool: "unknown_fabushi_app_tool"
            case .invalidGeneration: "invalid_app_surface_generation"
            case .missingTarget: "app_surface_action_target_missing"
            case .invalidRef: "invalid_app_surface_ref"
            case .resultTooLarge: "bounded_device_result_exceeded"
            }
        }
    }

    private let bridge: IOSPreloadBridge
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
    private var registeredVoIPToken: String?

    init(bridge: IOSPreloadBridge, surface: FabushiAppAgentSurface, traceURL: URL) {
        self.bridge = bridge
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
            await revokeVoIPRegistrationBeforeLogout()
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

    func resumeAfterBackground() async {
        guard desiredLoggedIn else { return }
        await refreshConnection()
    }

    func voIPTokenDidChange() async {
        guard desiredLoggedIn else { return }
        await refreshConnection()
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
            let raw = try await bridge.request(method: "feature.auth.deviceAgentSession")
            let candidate = try Self.parseAgentSession(raw.value)
            if let activeSession,
               Self.sessionIdentity(activeSession) == Self.sessionIdentity(candidate),
               registered,
               registeredVoIPToken == Self.currentVoIPToken(),
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
        let expectedIdentity = Self.sessionIdentity(agentSession)
        let task = urlSession.webSocketTask(with: request)
        socket = task
        activeSession = agentSession
        registered = false
        registeredVoIPToken = Self.currentVoIPToken()
        task.resume()

        let sessionStillCurrent = await sessionIsCurrent(agentSession)
        guard desiredLoggedIn,
              socket === task,
              let activeSession,
              Self.sessionIdentity(activeSession) == expectedIdentity,
              sessionStillCurrent
        else {
            if socket === task {
                stopConnection(reason: "stale account before registration")
            } else {
                task.cancel(with: .goingAway, reason: nil)
            }
            return
        }

        let registration = Self.registrationMessage(agentSession)
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

    private func revokeVoIPRegistrationBeforeLogout() async {
        guard let socket, socket.state == .running, activeSession != nil else { return }
        do {
            try await send(Self.logoutRegistrationMessage(), over: socket)
            appendTrace(["phase": "voip-registration-revoked"])
        } catch {
            appendTrace([
                "phase": "voip-registration-revoke-failed",
                "error": Self.safeErrorCode(error),
            ])
        }
    }

    static func logoutRegistrationMessage() -> [String: String] {
        ["type": "unregister", "reason": "logout"]
    }

    static func registrationMessage(_ session: AgentSession) -> [String: Any] {
        [
            "type": "register",
            "deviceId": session.deviceId,
            "name": Self.deviceName(deviceId: session.deviceId),
            "platform": "ios",
            "capabilities": FabushiAppAgentSurface.toolNames,
            "tools": Self.toolDescriptors,
            "toolSchemaVersion": "fabushi-app-agent-surface-v1",
            "generation": session.sessionId,
            "leaseSeconds": Self.leaseSeconds,
            "metadata": Self.gatewayMetadata(),
        ]
    }

    static let readOnlyToolNames: Set<String> = [
        "fabushi.app.status",
        "fabushi.app.snapshot",
        "fabushi.app.find",
        "fabushi.app.wait",
        "fabushi.app.assert",
    ]

    static func isReadOnlyDiskPreflight(_ value: Any?) -> Bool {
        guard let object = value as? [String: Any],
              object["impact"] as? String == "none",
              let targets = object["targets"] as? [Any]
        else { return false }
        return targets.isEmpty
    }

    static func acceptsDiskPreflight(toolName: String, value: Any?) -> Bool {
        guard FabushiAppAgentSurface.toolNames.contains(toolName) else { return false }
        if readOnlyToolNames.contains(toolName) {
            return value == nil || isReadOnlyDiskPreflight(value)
        }
        return isReadOnlyDiskPreflight(value)
    }

    static func decodeCallArguments(_ message: [String: Any]) throws -> [String: Any] {
        if let argumentsJson = message["argumentsJson"] as? String, !argumentsJson.isEmpty {
            guard let data = argumentsJson.data(using: .utf8) else { throw GatewayError.invalidCall }
            let decoded: Any
            do {
                decoded = try JSONSerialization.jsonObject(with: data)
            } catch {
                throw GatewayError.invalidCall
            }
            guard let object = decoded as? [String: Any] else { throw GatewayError.invalidCall }
            return object
        }
        if let object = message["arguments"] as? [String: Any] {
            return object
        }
        return [:]
    }

    static func sessionIdentity(_ session: AgentSession) -> SessionIdentity {
        SessionIdentity(
            deviceId: session.deviceId,
            sessionId: session.sessionId,
            accessToken: session.accessToken
        )
    }

    static func boundedResult(_ result: [String: Any], accessToken: String) throws -> [String: Any] {
        guard !accessToken.isEmpty else { throw GatewayError.invalidAgentSession }
        let redacted = try redactBearerMaterial(result, accessToken: accessToken)
        guard let safeResult = redacted as? [String: Any],
              JSONSerialization.isValidJSONObject(safeResult)
        else { throw GatewayError.invalidCall }
        let data: Data
        do {
            data = try JSONSerialization.data(withJSONObject: safeResult, options: [.sortedKeys])
        } catch {
            throw GatewayError.invalidCall
        }
        guard data.count <= maximumResultBytes else { throw GatewayError.resultTooLarge }
        return safeResult
    }

    private static func redactBearerMaterial(_ value: Any, accessToken: String) throws -> Any {
        if let text = value as? String {
            return text.replacingOccurrences(of: accessToken, with: "[redacted]")
        }
        if let object = value as? [String: Any] {
            var redacted: [String: Any] = [:]
            for (key, child) in object {
                let safeKey = key.replacingOccurrences(of: accessToken, with: "[redacted]")
                guard redacted[safeKey] == nil else { throw GatewayError.invalidCall }
                redacted[safeKey] = try redactBearerMaterial(child, accessToken: accessToken)
            }
            return redacted
        }
        if let array = value as? [Any] {
            return try array.map { try redactBearerMaterial($0, accessToken: accessToken) }
        }
        return value
    }

    private func sessionIsCurrent(_ expected: AgentSession) async -> Bool {
        do {
            let raw = try await bridge.request(method: "feature.auth.deviceAgentSession")
            let current = try Self.parseAgentSession(raw.value)
            return Self.sessionIdentity(current) == Self.sessionIdentity(expected)
        } catch {
            return false
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
        registeredVoIPToken = nil
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
        appendTrace([
            "phase": "call-started",
            "requestId": String(requestId),
            "toolName": String(toolName),
        ])
        do {
            guard let session = activeSession,
                  await sessionIsCurrent(session)
            else {
                stopConnection(reason: "account revoked before device call")
                return
            }
            let normalizedToolName = String(toolName)
            guard FabushiAppAgentSurface.toolNames.contains(normalizedToolName) else {
                throw GatewayError.unknownTool
            }
            guard Self.acceptsDiskPreflight(toolName: normalizedToolName, value: message["diskPreflight"]) else {
                throw GatewayError.invalidCall
            }
            let arguments = try Self.decodeCallArguments(message)
            let result = try await invokeTool(normalizedToolName, arguments: arguments)
            guard await sessionIsCurrent(session) else {
                stopConnection(reason: "account changed during device call")
                return
            }
            let boundedResult = try Self.boundedResult(result, accessToken: session.accessToken)
            try await send([
                "type": "result",
                "requestId": String(requestId),
                "ok": true,
                "result": [
                    "structuredContent": boundedResult,
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
            let includeText = arguments["includeText"] as? Bool ?? false
            return Self.snapshotDictionary(snapshot, maxElements: maxElements, includeText: includeText)
        case "fabushi.app.find":
            let snapshot = surface.snapshot()
            let ref = arguments["ref"] as? String
            let refTarget = try ref.map { try Self.parseRef($0, expectedGeneration: snapshot.generation) }
            let limit = Self.boundedInt(arguments["limit"], minimum: 1, maximum: 100) ?? 25
            let matches = surface.find(
                agentId: refTarget ?? arguments["agentId"] as? String,
                role: arguments["role"] as? String,
                name: arguments["name"] as? String,
                text: arguments["text"] as? String,
                limit: limit
            )
            return [
                "version": FabushiAppAgentSurface.version,
                "appId": snapshot.appId,
                "platform": snapshot.platform,
                "screen": snapshot.screen,
                "generation": snapshot.generation,
                "count": matches.count,
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
                name: arguments["name"] as? String,
                text: arguments["text"] as? String,
                state: arguments["state"] as? String ?? "present",
                timeoutMilliseconds: timeout
            )
            return Self.assertionDictionary(assertion)
        case "fabushi.app.assert":
            return Self.assertionDictionary(surface.assertState(
                screen: (arguments["screen"] as? String) ?? (arguments["route"] as? String),
                agentId: arguments["agentId"] as? String,
                role: arguments["role"] as? String,
                name: arguments["name"] as? String,
                text: arguments["text"] as? String,
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

    static func parseAgentSession(_ value: Any, nowMilliseconds: Int64 = Int64(Date().timeIntervalSince1970 * 1_000)) throws -> AgentSession {
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
              sessionId.count <= 200,
              expiry > nowMilliseconds,
              expiry - nowMilliseconds > sessionExpiryLeewayMilliseconds
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
        let empty: [String: Any] = ["type": "object", "properties": [:] as [String: Any], "additionalProperties": false]
        let output: [String: Any] = ["type": "object", "additionalProperties": true]
        let readOnly: [String: Any] = [
            "readOnlyHint": true,
            "destructiveHint": false,
            "idempotentHint": true,
            "openWorldHint": false,
        ]
        let mutable: [String: Any] = [
            "readOnlyHint": false,
            "destructiveHint": true,
            "idempotentHint": false,
            "openWorldHint": false,
        ]
        return [
            ["name": "fabushi.app.status", "title": "Fabushi app agent-surface status", "description": "Report whether the active Fabushi iOS application exposes its structured semantic surface.", "inputSchema": empty, "outputSchema": output, "annotations": readOnly],
            ["name": "fabushi.app.snapshot", "title": "Read the Fabushi semantic UI", "description": "Return a structured redacted semantic snapshot of the active Fabushi iOS UI.", "inputSchema": ["type": "object", "properties": ["maxElements": ["type": "integer"], "includeText": ["type": "boolean"]]], "outputSchema": output, "annotations": readOnly],
            ["name": "fabushi.app.find", "title": "Find Fabushi UI elements", "description": "Find semantic elements by stable id, generation-bound ref, role, accessible name, or visible text.", "inputSchema": ["type": "object", "properties": ["agentId": ["type": "string"], "ref": ["type": "string"], "role": ["type": "string"], "name": ["type": "string"], "text": ["type": "string"], "limit": ["type": "integer"]]], "outputSchema": output, "annotations": readOnly],
            ["name": "fabushi.app.action", "title": "Operate the Fabushi semantic UI", "description": "Perform one allowlisted action against the exact current semantic generation.", "inputSchema": ["type": "object", "properties": ["generation": ["type": "integer"], "ref": ["type": "string"], "agentId": ["type": "string"], "action": ["type": "string", "enum": ["invoke", "focus", "setValue", "pressKey", "scroll", "selectOption", "toggle"]], "value": ["type": "string"]], "required": ["generation", "action"]], "outputSchema": output, "annotations": mutable],
            ["name": "fabushi.app.wait", "title": "Wait for Fabushi UI state", "description": "Wait for a bounded semantic UI condition.", "inputSchema": stateQuerySchema(includeTimeout: true), "outputSchema": output, "annotations": readOnly],
            ["name": "fabushi.app.assert", "title": "Assert Fabushi UI state", "description": "Evaluate a deterministic semantic UI assertion for CI evidence.", "inputSchema": stateQuerySchema(includeTimeout: false), "outputSchema": output, "annotations": readOnly],
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


    static func currentVoIPToken(defaults: UserDefaults = .standard) -> String? {
        guard let raw = defaults.string(forKey: HumanCallSystemCoordinator.voIPTokenDefaultsKey) else {
            return nil
        }
        let token = raw.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard token.count >= 32, token.count <= 512, token.count.isMultiple(of: 2),
              token.range(of: #"^[0-9a-f]+$"#, options: .regularExpression) != nil
        else { return nil }
        return token
    }
    private static func gatewayMetadata() -> [String: Any] {
        let environment = ProcessInfo.processInfo.environment
        var metadata: [String: Any] = [
            "kind": environment["GITHUB_ACTIONS"] == "true" ? "github-actions-ios-app" : "fabushi-ios",
            "owner": "canonical-host",
        ]
        if let token = currentVoIPToken() {
            metadata["humanCallVoIPToken"] = token
        }
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

    static func snapshotDictionary(
        _ snapshot: FabushiAppAgentSurface.Snapshot,
        maxElements: Int,
        includeText: Bool = false
    ) -> [String: Any] {
        let boundedElements = Array(snapshot.elements.prefix(maxElements))
        let truncated = snapshot.elements.count > maxElements
            || snapshot.elements.last?.agentId == FabushiAppAgentSurface.truncationAgentId
        return [
            "version": snapshot.version,
            "appId": snapshot.appId,
            "platform": snapshot.platform,
            "screen": snapshot.screen,
            "generation": snapshot.generation,
            "truncated": truncated,
            "elements": boundedElements.map {
                elementDictionary($0, generation: snapshot.generation, includeText: includeText)
            },
        ]
    }

    private static func elementDictionary(
        _ element: FabushiAppAgentSurface.Element,
        generation: UInt64,
        includeText: Bool = true
    ) -> [String: Any] {
        var value: [String: Any] = [
            "agentId": element.agentId,
            "ref": makeRef(generation: generation, agentId: element.agentId),
            "role": element.role,
            "name": element.name,
            "visible": element.visible,
            "enabled": element.enabled,
            "sensitive": element.sensitive,
        ]
        if includeText, !element.sensitive, let text = element.text { value["text"] = text }
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
