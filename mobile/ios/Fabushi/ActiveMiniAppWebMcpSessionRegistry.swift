import CryptoKit
import Foundation
import OSLog

struct MiniAppWebMcpApprovalDenied: LocalizedError {
    var errorDescription: String? { "用户取消了 WebMCP Tool 调用" }
}

struct MiniAppWebMcpSchemaViolation: LocalizedError {
    let details: [String]

    var errorDescription: String? {
        let normalized = details.filter { !$0.isEmpty }
        return normalized.isEmpty
            ? "WebMCP Tool 参数不符合当前页面 Schema"
            : "WebMCP Tool 参数不符合当前页面 Schema：\(normalized.joined(separator: "；"))"
    }
}

@MainActor
final class ActiveMiniAppWebMcpSessionRegistry {
    static let shared = ActiveMiniAppWebMcpSessionRegistry()
    private static let logger = Logger(
        subsystem: "com.ombhrum.fabushi",
        category: "miniapp-webmcp"
    )

    struct Invocation: @unchecked Sendable {
        let callId: String
        let value: Any
    }

    struct Snapshot: Equatable {
        let accountKey: String
        let pluginId: String
        let version: String
        let artifactSha256: String
        let pluginInstanceId: String
        let nonce: String
        let generation: UInt64
        let toolNames: [String]
    }

    struct InvocationTrace: Equatable {
        let sequence: UInt64
        let callId: String
        let accountKey: String
        let pluginId: String
        let version: String
        let artifactSha256: String
        let pluginInstanceId: String
        let nonceIdentifier: String
        let generation: UInt64
        let toolName: String
        let permissionDecision: String
        let stateRevision: Int?
        let outcome: String
    }

    private struct Entry {
        let snapshot: Snapshot
        let tools: [String: MiniAppToolContract]
        let revoke: @MainActor () -> Void
        let invoke: @MainActor (String, [String: Any]) async throws -> Any
    }

    private var entriesByPluginId: [String: Entry] = [:]
    private var traceSequence: UInt64 = 0
    private(set) var invocationTraces: [InvocationTrace] = []

    private init() {}

    func register(
        accountKey: String,
        pluginId: String,
        version: String,
        artifactSha256: String,
        pluginInstanceId: String,
        nonce: String,
        generation: UInt64,
        tools: [MiniAppToolContract],
        revoke: @escaping @MainActor () -> Void = {},
        invoke: @escaping @MainActor (String, [String: Any]) async throws -> Any
    ) {
        let normalizedAccount = accountKey.trimmingCharacters(in: .whitespacesAndNewlines)
        let normalizedPluginId = pluginId.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !normalizedAccount.isEmpty,
              !normalizedPluginId.isEmpty,
              !version.isEmpty,
              artifactSha256.range(
                  of: #"^[0-9a-fA-F]{64}$"#,
                  options: .regularExpression
              ) != nil,
              !pluginInstanceId.isEmpty,
              !nonce.isEmpty,
              generation > 0,
              !tools.isEmpty
        else { return }

        let uniqueTools = Dictionary(
            tools.map { ($0.name, $0) },
            uniquingKeysWith: { _, newer in newer }
        )
        guard !uniqueTools.isEmpty else { return }

        entriesByPluginId[normalizedPluginId] = Entry(
            snapshot: Snapshot(
                accountKey: normalizedAccount,
                pluginId: normalizedPluginId,
                version: version,
                artifactSha256: artifactSha256.lowercased(),
                pluginInstanceId: pluginInstanceId,
                nonce: nonce,
                generation: generation,
                toolNames: uniqueTools.keys.sorted()
            ),
            tools: uniqueTools,
            revoke: revoke,
            invoke: invoke
        )
        let sessionEvidence = [
            "FABUSHI_WEBMCP_SESSION_REGISTER",
            "account_id=\(Self.accountIdentifier(normalizedAccount))",
            "plugin_id=\(normalizedPluginId)",
            "version=\(version)",
            "artifact_sha256=\(artifactSha256.lowercased())",
            "instance_id=\(pluginInstanceId)",
            "nonce_id=\(Self.nonceIdentifier(nonce))",
            "generation=\(generation)",
            "tool_count=\(uniqueTools.count)",
        ].joined(separator: " ")
        Self.logger.notice("\(sessionEvidence, privacy: .public)")
    }

    func unregister(
        pluginId: String,
        pluginInstanceId: String,
        generation: UInt64
    ) {
        guard let entry = entriesByPluginId[pluginId],
              entry.snapshot.pluginInstanceId == pluginInstanceId,
              entry.snapshot.generation == generation
        else { return }
        entriesByPluginId.removeValue(forKey: pluginId)
    }

    func revokeSession(accountKey: String, pluginId: String) {
        let normalizedAccount = accountKey.trimmingCharacters(in: .whitespacesAndNewlines)
        let normalizedPluginId = pluginId.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !normalizedAccount.isEmpty,
              !normalizedPluginId.isEmpty,
              let entry = entriesByPluginId[normalizedPluginId],
              entry.snapshot.accountKey == normalizedAccount
        else { return }

        entriesByPluginId.removeValue(forKey: normalizedPluginId)
        entry.revoke()
        let sessionEvidence = [
            "FABUSHI_WEBMCP_SESSION_REVOKE_PLUGIN",
            "account_id=\(Self.accountIdentifier(normalizedAccount))",
            "plugin_id=\(normalizedPluginId)",
            "generation=\(entry.snapshot.generation)",
        ].joined(separator: " ")
        Self.logger.notice("\(sessionEvidence, privacy: .public)")
    }

    func revokeSessions(accountKey: String) {
        let normalizedAccount = accountKey.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !normalizedAccount.isEmpty else { return }

        let revokedEntries = entriesByPluginId.compactMap { pluginId, entry -> (String, Entry)? in
            entry.snapshot.accountKey == normalizedAccount ? (pluginId, entry) : nil
        }
        guard !revokedEntries.isEmpty else { return }

        for (pluginId, _) in revokedEntries {
            entriesByPluginId.removeValue(forKey: pluginId)
        }
        for (_, entry) in revokedEntries {
            entry.revoke()
        }

        let sessionEvidence = [
            "FABUSHI_WEBMCP_SESSION_REVOKE",
            "account_id=\(Self.accountIdentifier(normalizedAccount))",
            "session_count=\(revokedEntries.count)",
        ].joined(separator: " ")
        Self.logger.notice("\(sessionEvidence, privacy: .public)")
    }

    func snapshot(
        accountKey: String,
        pluginId: String
    ) -> Snapshot? {
        guard let entry = entriesByPluginId[pluginId],
              entry.snapshot.accountKey == accountKey
        else { return nil }
        return entry.snapshot
    }

    func tools(
        accountKey: String,
        pluginId: String
    ) -> [MiniAppToolContract]? {
        guard let entry = entriesByPluginId[pluginId],
              entry.snapshot.accountKey == accountKey
        else { return nil }
        return entry.snapshot.toolNames.compactMap { entry.tools[$0] }
    }

    func callIfActive(
        accountKey: String,
        pluginId: String,
        name: String,
        arguments: [String: Any]
    ) async throws -> Invocation? {
        guard let entry = entriesByPluginId[pluginId],
              entry.snapshot.accountKey == accountKey,
              let tool = entry.tools[name]
        else { return nil }

        let callId = UUID().uuidString.lowercased()
        let expectedVersion = entry.snapshot.version
        let expectedArtifactSha256 = entry.snapshot.artifactSha256
        let expectedInstanceId = entry.snapshot.pluginInstanceId
        let expectedGeneration = entry.snapshot.generation
        let permissionRequired = tool.approval != "none"

        let argumentValue = JSONSerialization.isValidJSONObject(arguments)
            ? MiniAppCommandValue.fromJSON(arguments, schema: tool.inputSchemaObject)
            : nil
        let schemaErrors = validateMiniAppSchemaValue(
            tool.inputSchemaObject,
            value: argumentValue
        )
        guard argumentValue != nil, schemaErrors.isEmpty else {
            appendTrace(
                entry: entry,
                callId: callId,
                toolName: name,
                permissionDecision: permissionRequired ? "not-reached" : "not-required",
                stateRevision: nil,
                outcome: "schema-rejected"
            )
            throw MiniAppWebMcpSchemaViolation(
                details: argumentValue == nil
                    ? ["$ 包含不支持的 JSON 参数"]
                    : schemaErrors
            )
        }

        do {
            let value = try await entry.invoke(name, arguments)
            let revision = Self.stateRevision(from: value)
            guard let current = entriesByPluginId[pluginId],
                  current.snapshot.accountKey == accountKey,
                  current.snapshot.version == expectedVersion,
                  current.snapshot.artifactSha256 == expectedArtifactSha256,
                  current.snapshot.pluginInstanceId == expectedInstanceId,
                  current.snapshot.generation == expectedGeneration
            else {
                appendTrace(
                    entry: entry,
                    callId: callId,
                    toolName: name,
                    permissionDecision: permissionRequired ? "approved" : "not-required",
                    stateRevision: revision,
                    outcome: "stale-session"
                )
                throw CancellationError()
            }
            appendTrace(
                entry: entry,
                callId: callId,
                toolName: name,
                permissionDecision: permissionRequired ? "approved" : "not-required",
                stateRevision: revision,
                outcome: "success"
            )
            return Invocation(callId: callId, value: value)
        } catch is MiniAppWebMcpApprovalDenied {
            appendTrace(
                entry: entry,
                callId: callId,
                toolName: name,
                permissionDecision: "denied",
                stateRevision: nil,
                outcome: "denied"
            )
            throw MiniAppWebMcpApprovalDenied()
        } catch is CancellationError {
            if invocationTraces.last?.callId != callId {
                appendTrace(
                    entry: entry,
                    callId: callId,
                    toolName: name,
                    permissionDecision: permissionRequired ? "not-reached" : "not-required",
                    stateRevision: nil,
                    outcome: "cancelled"
                )
            }
            throw CancellationError()
        } catch {
            appendTrace(
                entry: entry,
                callId: callId,
                toolName: name,
                permissionDecision: permissionRequired ? "not-observed" : "not-required",
                stateRevision: nil,
                outcome: "failed"
            )
            throw error
        }
    }

    func traces(accountKey: String, pluginId: String) -> [InvocationTrace] {
        invocationTraces.filter {
            $0.accountKey == accountKey && $0.pluginId == pluginId
        }
    }

    private func appendTrace(
        entry: Entry,
        callId: String,
        toolName: String,
        permissionDecision: String,
        stateRevision: Int?,
        outcome: String
    ) {
        traceSequence = traceSequence == UInt64.max ? 1 : traceSequence + 1
        let snapshot = entry.snapshot
        let trace = InvocationTrace(
            sequence: traceSequence,
            callId: callId,
            accountKey: snapshot.accountKey,
            pluginId: snapshot.pluginId,
            version: snapshot.version,
            artifactSha256: snapshot.artifactSha256,
            pluginInstanceId: snapshot.pluginInstanceId,
            nonceIdentifier: Self.nonceIdentifier(snapshot.nonce),
            generation: snapshot.generation,
            toolName: toolName,
            permissionDecision: permissionDecision,
            stateRevision: stateRevision,
            outcome: outcome
        )
        invocationTraces.append(trace)
        let traceEvidence = [
            "FABUSHI_WEBMCP_TRACE",
            "sequence=\(trace.sequence)",
            "call_id=\(trace.callId)",
            "account_id=\(Self.accountIdentifier(trace.accountKey))",
            "plugin_id=\(trace.pluginId)",
            "version=\(trace.version)",
            "artifact_sha256=\(trace.artifactSha256)",
            "instance_id=\(trace.pluginInstanceId)",
            "nonce_id=\(trace.nonceIdentifier)",
            "generation=\(trace.generation)",
            "tool_name=\(trace.toolName)",
            "permission=\(trace.permissionDecision)",
            "state_revision=\(trace.stateRevision.map(String.init) ?? "none")",
            "outcome=\(trace.outcome)",
        ].joined(separator: " ")
        Self.logger.notice("\(traceEvidence, privacy: .public)")
        if invocationTraces.count > 256 {
            invocationTraces.removeFirst(invocationTraces.count - 256)
        }
    }

    private nonisolated static func accountIdentifier(_ accountKey: String) -> String {
        let digest = SHA256.hash(data: Data(accountKey.utf8))
        return "sha256-" + digest.map { String(format: "%02x", $0) }.joined()
    }

    private nonisolated static func nonceIdentifier(_ nonce: String) -> String {
        var hash: UInt64 = 14_695_981_039_346_656_037
        for byte in nonce.utf8 {
            hash ^= UInt64(byte)
            hash &*= 1_099_511_628_211
        }
        return String(format: "fnv64-%016llx", hash)
    }

    private nonisolated static func stateRevision(from value: Any, depth: Int = 0) -> Int? {
        guard depth < 5 else { return nil }
        if let object = value as? [String: Any] {
            if let revision = object["revision"] as? Int { return revision }
            if let revision = object["revision"] as? NSNumber { return revision.intValue }
            for key in ["state", "structuredContent", "runtime", "result"] {
                if let nested = object[key],
                   let revision = stateRevision(from: nested, depth: depth + 1) {
                    return revision
                }
            }
        }
        return nil
    }

    func resetForTesting() {
        entriesByPluginId.removeAll()
        invocationTraces.removeAll()
        traceSequence = 0
    }
}
