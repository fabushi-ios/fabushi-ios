import Foundation

enum CoordinatorExitClass: String, Equatable, Sendable {
    case clean
    case breach
    case bootstrap
    case suspended
    case other
}

struct CoordinatorLifecycleTelemetry: Equatable, Sendable {
    let outcome: String
    let exitClass: CoordinatorExitClass?
    let uptimeMilliseconds: Int?
    let relaunchSequence: Int
    let delayMilliseconds: Int?
}

enum CoordinatorTelemetry {
    static let recentFailureWindowMilliseconds = 30_000
    static let healthyUptimeMilliseconds = 30_000

    static func classify(exitCode: Int?) -> CoordinatorExitClass {
        switch exitCode {
        case 0: .clean
        case 1: .breach
        case 2: .bootstrap
        case nil: .suspended
        default: .other
        }
    }

    static func relaunchDelayMilliseconds(attempt: Int) -> Int {
        let exponent = max(0, min(attempt, 8))
        return min(10_000, 250 * (1 << exponent))
    }
}



struct IOSCoordinatorFailureTelemetryProjection: Equatable, Sendable {
    enum State: String, Equatable, Sendable {
        case failed
        case recovered
    }

    let domain: String
    let operation: String
    let failureCode: String
    let boundary: String
    let retryOwner: String
    let transportKind: String?
    let state: State
    let failedMilliseconds: Int?

    var metadata: [String: String] {
        var value = [
            "domain": domain,
            "operation": operation,
            "failure_code": failureCode,
            "boundary": boundary,
            "retry_owner": retryOwner,
            "state": state.rawValue,
        ]
        if let transportKind {
            value["transport_kind"] = transportKind
        }
        if let failedMilliseconds {
            value["failed_ms"] = String(failedMilliseconds)
        }
        return value
    }
}

@MainActor
final class IOSCoordinatorFailureTelemetryTracker {
    typealias Reporter = @MainActor (IOSCoordinatorFailureTelemetryProjection) -> Void
    typealias NowMilliseconds = @MainActor () -> Int

    private struct Episode {
        let failureCode: String
        let boundary: String
        let retryOwner: String
        let transportKind: String?
        let failedAtMilliseconds: Int
    }

    private let reporter: Reporter
    private let nowMilliseconds: NowMilliseconds
    private var episodes: [String: Episode] = [:]
    private var activeGeneration: UInt64 = 0

    init(
        nowMilliseconds: @escaping NowMilliseconds = {
            Int(ProcessInfo.processInfo.systemUptime * 1_000)
        },
        reporter: @escaping Reporter
    ) {
        self.nowMilliseconds = nowMilliseconds
        self.reporter = reporter
    }

    func activate(generation: UInt64) {
        activeGeneration = generation
    }

    func noteSuccess(method: String, generation: UInt64) {
        guard generation == activeGeneration else { return }
        let domain = Self.domain(for: method)
        let key = "\(domain)|\(method)"
        guard let episode = episodes.removeValue(forKey: key) else { return }
        reporter(.init(
            domain: domain,
            operation: method,
            failureCode: episode.failureCode,
            boundary: episode.boundary,
            retryOwner: episode.retryOwner,
            transportKind: episode.transportKind,
            state: .recovered,
            failedMilliseconds: max(0, nowMilliseconds() - episode.failedAtMilliseconds)
        ))
    }

    func noteFailure(
        method: String,
        error: IOSCoordinatorPortClient.PortError,
        generation: UInt64
    ) {
        guard generation == activeGeneration, Self.shouldTrack(error) else { return }
        let domain = Self.domain(for: method)
        let key = "\(domain)|\(method)"
        let previous = episodes[key]
        if previous?.failureCode == error.code { return }

        let boundary = error.code.hasPrefix("port-") ? "client" : "host"
        let retryOwner = (
            error.transportKind != nil
                || error.code.contains("transport")
                || error.code.contains("unreachable")
                || error.code.hasPrefix("port-")
        ) ? "client" : "none"
        let episode = Episode(
            failureCode: error.code,
            boundary: boundary,
            retryOwner: retryOwner,
            transportKind: error.transportKind,
            failedAtMilliseconds: previous?.failedAtMilliseconds ?? nowMilliseconds()
        )
        episodes[key] = episode
        reporter(.init(
            domain: domain,
            operation: method,
            failureCode: episode.failureCode,
            boundary: episode.boundary,
            retryOwner: episode.retryOwner,
            transportKind: episode.transportKind,
            state: .failed,
            failedMilliseconds: nil
        ))
    }

    private static func shouldTrack(_ error: IOSCoordinatorPortClient.PortError) -> Bool {
        if error.code == "source/cancelled"
            || error.code == "source/capability-unavailable"
            || error.code == "coordinator-cancelled"
            || error.transportKind == "pre_port_settle"
        {
            return false
        }
        return true
    }

    static func domain(for method: String) -> String {
        switch method {
        case "getAgentTranscriptWindow", "getAgentThread", "getAgentTranscriptTail",
             "openAgentTail", "getConversationOutline":
            return "transcript"
        case "sendPrompt", "interruptAgent", "promptAcceptanceStatus", "reactToMessage":
            return "send"
        case "listRoutedMcpTools", "executeRoutedMcpTool":
            return "plugins"
        case "respondToWidget", "dismissWidget", "submitSecret":
            return "widgets"
        case "resolveAutoReviewApproval", "resolveLocalToolPermission":
            return "approvals"
        case "listAgents", "countAgents", "searchAgents", "createAgent", "createGroup",
             "setGroupMembers", "updateAgent", "deleteAgents", "duplicateAgent",
             "kickstartAgent", "requestDiskSaverAudit", "broadcastToAgents",
             "setAgentUnread", "setAgentHiddenFromSidebar", "setAgentNotificationsEnabled",
             "setAgentNotifyOnUpdates", "setAgentAvatarBytes", "getAgentAvatar":
            return "roster"
        case "searchMedia":
            return "search"
        case "getCloudAgentInfo":
            return "cloud_agents"
        case "getListenerIntegrations", "getListenerConnectUrl":
            return "listeners"
        case "getAgentWorkflows", "createAgentWorkflow", "updateAgentWorkflow",
             "setAgentWorkflowEnabled", "deleteAgentWorkflow", "runAgentWorkflowNow",
             "importAgentWorkflowText", "importAgentWorkflowUrl", "portAgentLocalSkills":
            return "workflows"
        case "skillsCatalog", "syncPluginSkills", "getPluginSyncStatus",
             "getSkillPublishTargets", "publishSkill", "resyncPublishedSkill", "unpublishSkill":
            return "skills"
        case "getSubagents", "getAsyncTasks":
            return "subagents"
        case "getForeverBoxStatus", "ensureForeverBox", "handBackForeverBox",
             "isEgressTunnelAvailable":
            return "computer"
        case "startTeachRecording", "stopTeachRecording", "getTeachRecordingStatus":
            return "teach"
        case "getTrays", "dismissTray", "clearTrays":
            return "trays"
        case "getAgentChannels", "connectChannel", "disconnectChannel", "refreshChannel":
            return "channels"
        case "getBoxSecretsStatus":
            return "secrets"
        case "getAgentAutomations", "listAllAutomations", "setAgentAutomationEnabled",
             "createAgentAutomation", "updateAgentAutomation", "deleteAgentAutomation",
             "runAgentAutomationNow":
            return "automations"
        case "isAgentNetworkEnabled", "isGlobalSearchEnabled":
            return "capabilities"
        case "getSharingState", "createRoomFromAgent", "createRoomInvite", "joinSharedRoom",
             "respondToRoomJoinRequest", "createSharedRoom", "addOwnAgentToSharedRoom",
             "removeOwnAgentFromSharedRoom", "setSharedRoomTyping", "leaveSharedRoom":
            return "sharing"
        default:
            return "coordinator"
        }
    }
}



struct IOSCoordinatorBroadTelemetryProjection: Equatable, Sendable {
    let event: String
    let level: IOSLifecycleTelemetryLevel
    let metadata: [String: String]

    static let routedMethods: Set<String> = [
        "reportAgentsUnreachable",
        "reportRecoveryAction",
        "reportRebuildLifecycle",
        "reportReconciliation",
        "reportResyncCompleted",
    ]

    private static let agentsUnreachablePhases: Set<String> = ["entered", "retry", "recovered"]
    private static let recoveryActions: Set<String> = ["retry", "recover", "reset", "relaunch"]
    private static let rebuildKinds: Set<String> = ["update", "reset", "recover", "reconnecting"]
    private static let rebuildEscalations: Set<String> = ["taking-longer", "unreachable"]
    private static let rebuildEscalationActions: Set<String> = ["shown", "keep_waiting", "retry"]
    private static let reconciliationReplicas: Set<String> = ["roster", "transcript"]
    private static let replicaResyncOutcomes: Set<String> = ["covered", "superseded", "abandoned"]
    private static let journalRestoreOutcomes: Set<String> = ["dispatched", "retired", "parked", "failed"]
    private static let journalRestorePhases: Set<String> = [
        "queued", "prepared", "dispatching", "accepted-awaiting-echo",
    ]
    private static let replicaResyncWarnDurationMilliseconds = 60_000.0

    static func project(method: String, report: [String: Any]) -> IOSCoordinatorBroadTelemetryProjection? {
        switch method {
        case "reportAgentsUnreachable":
            return projectAgentsUnreachable(report)
        case "reportRecoveryAction":
            return projectRecoveryAction(report)
        case "reportRebuildLifecycle":
            return projectRebuildLifecycle(report)
        case "reportReconciliation":
            return projectReconciliation(report)
        case "reportResyncCompleted":
            return projectResyncCompleted(report)
        default:
            return nil
        }
    }

    private static func projectAgentsUnreachable(
        _ report: [String: Any]
    ) -> IOSCoordinatorBroadTelemetryProjection? {
        guard let phase = report["phase"] as? String,
              agentsUnreachablePhases.contains(phase),
              let isManual = report["isManual"] as? Bool
        else {
            return nil
        }
        let errorClass: String?
        if report.keys.contains("errorClass") {
            guard let value = report["errorClass"] as? String else { return nil }
            errorClass = value
        } else {
            errorClass = nil
        }
        let msSinceLastReached: Double?
        if report.keys.contains("msSinceLastReachedMs") {
            guard let value = finiteNonnegative(report["msSinceLastReachedMs"]) else { return nil }
            msSinceLastReached = value
        } else {
            msSinceLastReached = nil
        }

        var metadata = baseMetadata(event: "agents_unreachable")
        metadata["phase"] = phase
        metadata["is_manual"] = String(isManual)
        if let errorClass { metadata["error_class"] = errorClass }
        if let msSinceLastReached {
            metadata["ms_since_last_reached"] = roundedString(msSinceLastReached)
        }
        return .init(
            event: "agents_unreachable",
            level: phase == "entered" ? .warn : .info,
            metadata: metadata
        )
    }

    private static func projectRecoveryAction(
        _ report: [String: Any]
    ) -> IOSCoordinatorBroadTelemetryProjection? {
        guard let action = report["action"] as? String,
              recoveryActions.contains(action),
              let failureClass = report["failureClass"] as? String,
              isBoundedFailureClass(failureClass),
              let consecutiveIdentical = integerAtLeastOne(report["consecutiveIdentical"])
        else {
            return nil
        }
        var metadata = baseMetadata(event: "recovery_action")
        metadata["action"] = action
        metadata["failure_class"] = failureClass.lowercased()
        metadata["consecutive_identical"] = String(consecutiveIdentical)
        return .init(
            event: "recovery_action",
            level: consecutiveIdentical >= 3 ? .warn : .info,
            metadata: metadata
        )
    }

    private static func projectRebuildLifecycle(
        _ report: [String: Any]
    ) -> IOSCoordinatorBroadTelemetryProjection? {
        guard let rebuildKind = report["rebuildKind"] as? String,
              rebuildKinds.contains(rebuildKind),
              let elapsedMs = finiteNonnegative(report["elapsedMs"]),
              let kind = report["kind"] as? String
        else {
            return nil
        }

        if kind == "escalation" {
            guard let action = report["action"] as? String,
                  rebuildEscalationActions.contains(action),
                  let escalation = report["escalation"] as? String,
                  rebuildEscalations.contains(escalation)
            else {
                return nil
            }
            var metadata = baseMetadata(event: "escalation")
            metadata["action"] = action
            metadata["escalation"] = escalation
            metadata["kind"] = rebuildKind
            metadata["elapsed_ms"] = roundedString(elapsedMs)
            return .init(
                event: "escalation",
                level: action == "shown" ? .warn : .info,
                metadata: metadata
            )
        }

        if kind == "pending_stall" {
            guard let hasAck = report["hasAck"] as? Bool,
                  let streamAttached = report["streamAttached"] as? Bool
            else {
                return nil
            }
            var metadata = baseMetadata(event: "pending_stall")
            metadata["kind"] = rebuildKind
            metadata["has_ack"] = String(hasAck)
            metadata["stream_attached"] = String(streamAttached)
            metadata["elapsed_ms"] = roundedString(elapsedMs)
            return .init(event: "pending_stall", level: .warn, metadata: metadata)
        }

        return nil
    }

    private static func projectReconciliation(
        _ report: [String: Any]
    ) -> IOSCoordinatorBroadTelemetryProjection? {
        guard let kind = report["kind"] as? String else { return nil }

        if kind == "replica_resync" {
            guard let replica = report["replica"] as? String,
                  reconciliationReplicas.contains(replica),
                  let outcome = report["outcome"] as? String,
                  replicaResyncOutcomes.contains(outcome),
                  let attempts = finiteNonnegative(report["attempts"]),
                  let durationMs = finiteNonnegative(report["durationMs"]),
                  let bufferOverflowed = report["bufferOverflowed"] as? Bool,
                  let epochChanged = report["epochChanged"] as? Bool
            else {
                return nil
            }
            var metadata = baseMetadata(event: "replica_resync")
            metadata["replica"] = replica
            metadata["outcome"] = outcome
            metadata["attempts"] = roundedString(attempts)
            metadata["duration_ms"] = roundedString(durationMs)
            metadata["buffer_overflowed"] = String(bufferOverflowed)
            metadata["epoch_changed"] = String(epochChanged)
            let level: IOSLifecycleTelemetryLevel = (
                outcome == "abandoned" || durationMs > replicaResyncWarnDurationMilliseconds
            ) ? .warn : .info
            return .init(event: "replica_resync", level: level, metadata: metadata)
        }

        if kind == "send_journal_restore" {
            guard let outcome = report["outcome"] as? String,
                  journalRestoreOutcomes.contains(outcome),
                  let phase = report["phase"] as? String,
                  journalRestorePhases.contains(phase),
                  let ageMs = finiteNonnegative(report["ageMs"]),
                  let recordCount = finiteNonnegative(report["recordCount"])
            else {
                return nil
            }
            var metadata = baseMetadata(event: "send_journal_restore")
            metadata["outcome"] = outcome
            metadata["phase"] = phase
            metadata["age_ms"] = roundedString(ageMs)
            metadata["record_count"] = roundedString(recordCount)
            let level: IOSLifecycleTelemetryLevel = (
                outcome == "parked" || outcome == "failed"
            ) ? .warn : .info
            return .init(event: "send_journal_restore", level: level, metadata: metadata)
        }

        return nil
    }

    private static func projectResyncCompleted(
        _ report: [String: Any]
    ) -> IOSCoordinatorBroadTelemetryProjection? {
        let failed = finiteNonnegative(report["failed"]) ?? 0
        var metadata = baseMetadata(event: "resync_completed")
        var accepted = 0
        for key in report.keys.sorted() {
            guard accepted < 64, metadata[key] == nil,
                  let value = boundedScalar(report[key])
            else { continue }
            metadata[key] = value
            accepted += 1
        }
        return .init(
            event: "resync_completed",
            level: failed > 0 ? .warn : .info,
            metadata: metadata
        )
    }

    private static let clientFailureResourceStates: Set<String> = ["failed", "recovered"]
    private static let clientFailureUnhandledKinds: Set<String> = ["error", "unhandledrejection"]
    private static let clientFailureBoundaries: Set<String> = [
        "app", "chat_screen", "info_pane", "transcript_row", "mermaid", "other",
    ]
    private static let clientFailureEntrypointSurfaces: Set<String> = ["workspace", "overlay", "critical"]
    private static let clientFailureEntrypointPhases: Set<String> = ["chunk_load", "render"]
    private static let clientFailureQueuedCauses: Set<String> = ["SAND-E0703", "SAND-E0704", "SAND-E0705"]
    private static let clientFailureErrnos: Set<String> = [
        "ECONNREFUSED", "ECONNRESET", "ECONNABORTED", "ETIMEDOUT", "EPIPE", "ENETRESET",
        "ENETDOWN", "ENETUNREACH", "EHOSTUNREACH", "EHOSTDOWN", "EAI_AGAIN", "ENOTFOUND",
        "EADDRINUSE", "EACCES", "EPERM", "ENOENT", "ENOSPC", "EDQUOT", "EROFS", "EBUSY",
        "EMFILE", "EIO", "E_OTHER",
    ]

    private static func projectClientFailure(
        _ report: [String: Any]
    ) -> IOSMainRPCTelemetryProjection? {
        guard let kind = report["kind"] as? String else { return nil }
        switch kind {
        case "client_resource":
            guard let domain = boundedClientFailureToken(report["domain"]),
                  let operation = boundedClientFailureToken(report["operation"]),
                  let state = report["state"] as? String,
                  clientFailureResourceStates.contains(state),
                  let failureCode = boundedClientFailureToken(report["failureCode"]),
                  let boundary = boundedClientFailureToken(report["boundary"]),
                  optionalBoundedClientFailureToken(report, key: "retryOwner"),
                  optionalBoundedClientFailureToken(report, key: "transportKind"),
                  optionalFinite(report, key: "failedMs")
            else { return nil }
            var metadata = baseMetadata(event: "sand.client_resource")
            metadata["domain"] = domain
            metadata["operation"] = operation
            metadata["state"] = state
            metadata["failure_code"] = failureCode
            metadata["boundary"] = boundary
            copyString(report, from: "retryOwner", to: "retry_owner", into: &metadata)
            copyString(report, from: "transportKind", to: "transport_kind", into: &metadata)
            if let failedMs = finiteNonnegative(report["failedMs"]) {
                metadata["failed_ms"] = roundedString(failedMs)
            }
            return .init(
                event: "sand.client_resource",
                level: state == "failed" ? .warn : .info,
                metadata: metadata
            )

        case "renderer_unhandled":
            guard let handler = report["handler"] as? String,
                  clientFailureUnhandledKinds.contains(handler),
                  let errorClass = boundedClientFailureToken(report["errorClass"]),
                  let occurrence = positiveInteger(report["occurrence"])
            else { return nil }
            var metadata = baseMetadata(event: "sand.renderer_unhandled")
            metadata["kind"] = handler
            metadata["error_class"] = errorClass
            metadata["occurrence"] = String(occurrence)
            return .init(event: "sand.renderer_unhandled", level: .error, metadata: metadata)

        case "renderer_boundary_caught":
            guard let boundary = report["boundary"] as? String,
                  clientFailureBoundaries.contains(boundary),
                  let errorClass = boundedClientFailureToken(report["errorClass"]),
                  let occurrence = positiveInteger(report["occurrence"])
            else { return nil }
            var metadata = baseMetadata(event: "sand.renderer.boundary_caught")
            metadata["boundary"] = boundary
            metadata["error_class"] = errorClass
            metadata["occurrence"] = String(occurrence)
            return .init(event: "sand.renderer.boundary_caught", level: .warn, metadata: metadata)

        case "entrypoint_error":
            guard let entrypoint = boundedClientFailureToken(report["entrypoint"]),
                  let surface = report["surface"] as? String,
                  clientFailureEntrypointSurfaces.contains(surface),
                  let phase = report["phase"] as? String,
                  clientFailureEntrypointPhases.contains(phase),
                  let errorClass = boundedClientFailureToken(report["errorClass"])
            else { return nil }
            var metadata = baseMetadata(event: "sand.entrypoint_error")
            metadata["entrypoint"] = entrypoint
            metadata["surface"] = surface
            metadata["phase"] = phase
            metadata["error_class"] = errorClass
            return .init(event: "sand.entrypoint_error", level: .warn, metadata: metadata)

        case "send_queued_flush":
            guard let queuedMs = finiteNonnegative(report["queuedMs"]),
                  let queueDepth = finiteNonnegative(report["queueDepth"]),
                  let outcome = report["outcome"] as? String
            else { return nil }
            let cause = report["cause"] as? String
            switch outcome {
            case "delivered":
                guard !report.keys.contains("cause") else { return nil }
            case "superseded":
                guard cause == "SAND-E0706" else { return nil }
            case "expired":
                guard cause == "SAND-E0707" else { return nil }
            case "flush_failed":
                guard let cause, clientFailureQueuedCauses.contains(cause) else { return nil }
            default:
                return nil
            }
            var metadata = baseMetadata(event: "sand.send.queued_flush")
            metadata["outcome"] = outcome
            metadata["queued_ms"] = roundedString(queuedMs)
            metadata["queue_depth"] = roundedString(queueDepth)
            if let cause { metadata["cause"] = cause }
            let level: IOSLifecycleTelemetryLevel =
                (outcome == "expired" || outcome == "flush_failed") ? .warn : .info
            return .init(event: "sand.send.queued_flush", level: level, metadata: metadata)

        case "client_persistence":
            return projectClientPersistenceFailure(report)

        case "connector_auth_clicked":
            if report.keys.contains("serverName") {
                guard let serverName = report["serverName"] as? String,
                      serverName.count <= 256 else { return nil }
            }
            guard optionalBoundedClientFailureToken(report, key: "serverId") else { return nil }
            var metadata = baseMetadata(event: "sand.connector_auth")
            metadata["phase"] = "clicked"
            metadata["connector"] = boundedConnectorTag(report["serverName"] as? String)
            metadata["outcome"] = "ok"
            metadata["surface"] = "ios"
            copyString(report, from: "serverId", to: "server_id", into: &metadata)
            return .init(event: "sand.connector_auth", level: .info, metadata: metadata)

        default:
            return nil
        }
    }

    private static func projectClientPersistenceFailure(
        _ report: [String: Any]
    ) -> IOSMainRPCTelemetryProjection? {
        guard let slice = report["slice"] as? String,
              Set(SAND_CLIENT_PERSISTENCE_SLICES).contains(slice),
              let operation = report["op"] as? String,
              let outcome = report["outcome"] as? String,
              optionalFinite(report, key: "bytes")
        else { return nil }

        let error = report["error"] as? [String: Any]
        var metadata = baseMetadata(event: "sand.client_persistence")
        metadata["op"] = operation
        metadata["outcome"] = outcome
        metadata["slice"] = slice
        if let bytes = finiteNonnegative(report["bytes"]) {
            metadata["bytes"] = roundedString(bytes)
        }

        switch operation {
        case "writeback":
            guard outcome == "ok",
                  !report.keys.contains("error"),
                  !report.keys.contains("bytes")
            else { return nil }
        case "load":
            if outcome == "ok" {
                guard !report.keys.contains("error") else { return nil }
            } else if outcome == "corrupt" {
                guard error?["code"] as? String == "SAND-E0700" else { return nil }
                appendClientPersistenceErrorTags(code: "SAND-E0700", error: error, into: &metadata)
            } else if outcome == "io_error" {
                guard validClientPersistenceIOError(error) else { return nil }
                appendClientPersistenceErrorTags(code: "SAND-E0701", error: error, into: &metadata)
            } else {
                return nil
            }
        case "persist":
            if outcome == "quota" {
                guard error?["code"] as? String == "SAND-E0702" else { return nil }
                appendClientPersistenceErrorTags(code: "SAND-E0702", error: error, into: &metadata)
            } else if outcome == "io_error" {
                guard validClientPersistenceIOError(error) else { return nil }
                appendClientPersistenceErrorTags(code: "SAND-E0701", error: error, into: &metadata)
            } else {
                return nil
            }
        default:
            return nil
        }

        return .init(
            event: "sand.client_persistence",
            level: outcome == "ok" ? .info : .warn,
            metadata: metadata
        )
    }

    private static func validClientPersistenceIOError(_ error: [String: Any]?) -> Bool {
        guard error?["code"] as? String == "SAND-E0701" else { return false }
        guard let errno = error?["errno"] else { return true }
        guard let errno = errno as? String else { return false }
        return clientFailureErrnos.contains(errno)
    }

    private static func appendClientPersistenceErrorTags(
        code: String,
        error: [String: Any]?,
        into metadata: inout [String: String]
    ) {
        metadata["error_code"] = code
        metadata["error_domain"] = code == "SAND-E0001" ? "registry" : "storage"
        metadata["error_retryable"] = String(code == "SAND-E0701")
        if code == "SAND-E0701",
           let errno = error?["errno"] as? String {
            metadata["errno"] = clientFailureErrnos.contains(errno) ? errno : "E_OTHER"
        }
    }

    private static func boundedClientFailureToken(_ raw: Any?) -> String? {
        guard let value = raw as? String,
              value.range(
                of: "^[0-9A-Za-z._/:|\\-]{1,64}$",
                options: .regularExpression
              ) != nil
        else { return nil }
        return value
    }

    private static func optionalBoundedClientFailureToken(
        _ report: [String: Any],
        key: String
    ) -> Bool {
        !report.keys.contains(key) || boundedClientFailureToken(report[key]) != nil
    }

    private static func positiveInteger(_ raw: Any?) -> Int? {
        guard let value = finiteNonnegative(raw),
              value >= 1,
              value.rounded(.towardZero) == value,
              value <= Double(Int.max)
        else { return nil }
        return Int(value)
    }

    private static func baseMetadata(event: String) -> [String: String] {
        [
            "event": event,
            "platform_replacement": "ios_lifecycle_reporter",
        ]
    }

    private static func finiteNonnegative(_ raw: Any?) -> Double? {
        guard !(raw is Bool), let number = raw as? NSNumber else { return nil }
        let value = number.doubleValue
        return value.isFinite && value >= 0 ? value : nil
    }

    private static func integerAtLeastOne(_ raw: Any?) -> Int? {
        guard let value = finiteNonnegative(raw),
              value >= 1,
              value.rounded(.towardZero) == value,
              value <= Double(Int.max)
        else {
            return nil
        }
        return Int(value)
    }

    private static func roundedString(_ value: Double) -> String {
        String(Int(value.rounded()))
    }

    private static func isBoundedFailureClass(_ value: String) -> Bool {
        value.range(
            of: "^[A-Za-z0-9_]{1,64}$",
            options: .regularExpression
        ) != nil
    }

    private static func boundedScalar(_ raw: Any?) -> String? {
        let value: String
        switch raw {
        case let string as String:
            value = string
        case let bool as Bool:
            value = bool ? "true" : "false"
        case let number as NSNumber:
            value = number.stringValue
        case is NSNull:
            value = "null"
        default:
            return nil
        }
        return String(value.prefix(512))
    }
}


struct IOSMainRPCTelemetryProjection: Equatable, Sendable {
    let event: String
    let level: IOSLifecycleTelemetryLevel
    let metadata: [String: String]

    static let routedMethods: Set<String> = [
        "reportAgentLoad",
        "reportAccessBlocked",
        "reportSendLatency",
        "reportSendAck",
        "reportReactionAck",
        "reportRenderTtfr",
        "reportRenderStream",
        "reportClientFailure",
    ]

    private static let agentLoadOutcomes: Set<String> = ["ready", "superseded", "failed"]
    private static let knownAgentLoadBoxStates: Set<String> = [
        "pulling", "starting", "running", "local", "sleeping", "off", "unknown",
    ]
    private static let accessBlockedEdges: Set<String> = ["entered", "recovered"]
    private static let accessBlockReasons: Set<String> = [
        "unspecified", "none", "teamPrivacyMode", "teamSetupRequired", "teamAccessRequired",
        "notOffered", "freeTrialAvailable", "paywallIndividual", "paywallTeamMember", "paywallTeamAdmin",
    ]
    private static let accessBlockedCapMilliseconds = 7.0 * 24 * 60 * 60 * 1_000
    private static let sendAckOutcomes: Set<String> = ["acked", "failed", "timeout", "queued_offline"]
    private static let reactionAckOutcomes: Set<String> = ["landed", "timeout"]
    private static let reactionIDs = [
        "👍": "thumbs_up",
        "👎": "thumbs_down",
        "❤️": "heart",
        "😂": "laugh",
        "🎉": "celebrate",
        "😮": "surprise",
    ]
    private static let renderTtfrOutcomes: Set<String> = ["rendered", "timeout", "switched"]
    private static let renderStreamOutcomes: Set<String> = ["turn_ended", "switched", "superseded", "timeout"]
    private static let renderChunkTypes: Set<String> = ["assistant_text", "send_message", "tool_call"]

    static func project(method: String, report: [String: Any]) -> IOSMainRPCTelemetryProjection? {
        switch method {
        case "reportAgentLoad":
            return projectAgentLoad(report)
        case "reportAccessBlocked":
            return projectAccessBlocked(report)
        case "reportSendLatency":
            return projectSendLatency(report)
        case "reportSendAck":
            return projectSendAck(report)
        case "reportReactionAck":
            return projectReactionAck(report)
        case "reportRenderTtfr":
            return projectRenderTtfr(report)
        case "reportRenderStream":
            return projectRenderStream(report)
        case "reportClientFailure":
            return projectClientFailure(report)
        default:
            return nil
        }
    }

    private static func projectAgentLoad(_ report: [String: Any]) -> IOSMainRPCTelemetryProjection? {
        guard let durationMs = finiteNonnegative(report["durationMs"]),
              let cold = report["cold"] as? Bool,
              let boxState = report["boxState"] as? String,
              let outcome = report["outcome"] as? String,
              agentLoadOutcomes.contains(outcome)
        else {
            return nil
        }
        guard optionalFinite(report, key: "openRpcMs"),
              optionalFinite(report, key: "queueMs"),
              optionalFinite(report, key: "renderMs"),
              optionalBool(report, key: "stale")
        else {
            return nil
        }

        var metadata = baseMetadata(event: "agent_load")
        metadata["duration_ms"] = roundedString(durationMs)
        metadata["cold"] = String(cold)
        metadata["box_state"] = knownAgentLoadBoxStates.contains(boxState) ? boxState : "other"
        metadata["outcome"] = outcome
        if let value = finiteNonnegative(report["openRpcMs"]) { metadata["open_rpc_ms"] = roundedString(value) }
        if let value = finiteNonnegative(report["queueMs"]) { metadata["queue_ms"] = roundedString(value) }
        if let value = finiteNonnegative(report["renderMs"]) { metadata["render_ms"] = roundedString(value) }
        if let stale = report["stale"] as? Bool { metadata["stale"] = String(stale) }
        return .init(event: "agent_load", level: outcome == "failed" ? .warn : .info, metadata: metadata)
    }

    private static func projectAccessBlocked(_ report: [String: Any]) -> IOSMainRPCTelemetryProjection? {
        guard let edge = report["edge"] as? String,
              accessBlockedEdges.contains(edge),
              let cause = report["cause"] as? String,
              Set(SAND_ACCESS_BLOCKED_CAUSES).contains(cause),
              let accessReason = report["accessReason"] as? String,
              accessBlockReasons.contains(accessReason),
              optionalFinite(report, key: "msBlocked")
        else {
            return nil
        }

        var metadata = baseMetadata(event: "access_blocked")
        metadata["edge"] = edge
        metadata["cause"] = cause
        metadata["access_reason"] = accessReason
        if let blocked = finiteNonnegative(report["msBlocked"]) {
            metadata["ms_blocked"] = roundedString(min(blocked, accessBlockedCapMilliseconds))
        }
        let paging = edge == "entered" && cause != "no_plan"
        return .init(event: "access_blocked", level: paging ? .warn : .info, metadata: metadata)
    }

    private static func projectSendLatency(_ report: [String: Any]) -> IOSMainRPCTelemetryProjection? {
        guard let parsed = parseValidSendLatencyReport(report) else { return nil }
        let value = sendLatencyReportToTelemetry(parsed)
        var metadata = baseMetadata(event: "send_latency")
        metadata.merge(value.metadata) { _, incoming in incoming }
        return .init(
            event: "send_latency",
            level: IOSLifecycleTelemetryLevel(rawValue: value.level) ?? .info,
            metadata: metadata
        )
    }

    private static func projectSendAck(_ report: [String: Any]) -> IOSMainRPCTelemetryProjection? {
        guard let ackMs = finiteNonnegative(report["ackMs"]),
              let outcome = report["outcome"] as? String,
              sendAckOutcomes.contains(outcome),
              finiteNonnegative(report["enterEpochMs"]) != nil,
              let isFork = report["isFork"] as? Bool,
              let attachmentCount = finiteNonnegative(report["attachmentCount"]),
              optionalString(report, key: "traceparent"),
              optionalString(report, key: "traceId"),
              optionalString(report, key: "spanId"),
              optionalString(report, key: "clientNonce"),
              optionalString(report, key: "conversationId"),
              optionalFinite(report, key: "commitOffsetMs"),
              optionalFinite(report, key: "commitMs"),
              optionalBool(report, key: "clientOnlineAtTimeout")
        else {
            return nil
        }

        var metadata = baseMetadata(event: "send_ack")
        metadata["ack_ms"] = roundedString(ackMs)
        metadata["outcome"] = outcome
        metadata["attachment_count"] = roundedString(attachmentCount)
        metadata["is_fork"] = String(isFork)
        copyString(report, from: "traceId", to: "trace_id", into: &metadata)
        copyString(report, from: "spanId", to: "span_id", into: &metadata)
        copyString(report, from: "clientNonce", to: "client_nonce", into: &metadata)
        copyString(report, from: "conversationId", to: "conversation_id", into: &metadata)
        if let commitMs = finiteNonnegative(report["commitMs"]) { metadata["commit_ms"] = roundedString(commitMs) }
        if let online = report["clientOnlineAtTimeout"] as? Bool {
            metadata["client_online_at_timeout"] = String(online)
        }
        return .init(event: "send_ack", level: outcome == "timeout" ? .warn : .info, metadata: metadata)
    }

    private static func projectReactionAck(_ report: [String: Any]) -> IOSMainRPCTelemetryProjection? {
        guard let ackMs = finiteNonnegative(report["ackMs"]),
              let outcome = report["outcome"] as? String,
              reactionAckOutcomes.contains(outcome),
              let conversationID = report["conversationId"] as? String,
              !conversationID.isEmpty,
              conversationID.count <= 256,
              let emoji = report["emoji"] as? String,
              !emoji.isEmpty,
              emoji.count <= 64
        else {
            return nil
        }
        var metadata = baseMetadata(event: "reaction_ack")
        metadata["ack_ms"] = roundedString(ackMs)
        metadata["outcome"] = outcome
        metadata["conversation_id"] = conversationID
        metadata["reaction_id"] = reactionIDs[emoji] ?? "other"
        return .init(event: "reaction_ack", level: outcome == "timeout" ? .warn : .info, metadata: metadata)
    }

    private static func projectRenderTtfr(_ report: [String: Any]) -> IOSMainRPCTelemetryProjection? {
        guard let ttfrMs = finiteNonnegative(report["ttfrMs"]),
              let outcome = report["outcome"] as? String,
              renderTtfrOutcomes.contains(outcome),
              finiteNonnegative(report["enterEpochMs"]) != nil,
              let isFork = report["isFork"] as? Bool,
              optionalString(report, key: "traceparent"),
              optionalString(report, key: "traceId"),
              optionalString(report, key: "spanId"),
              optionalString(report, key: "clientNonce"),
              optionalString(report, key: "conversationId")
        else {
            return nil
        }
        if report.keys.contains("chunkType") {
            guard let chunkType = report["chunkType"] as? String,
                  renderChunkTypes.contains(chunkType)
            else { return nil }
        }

        var metadata = baseMetadata(event: "render_ttfr")
        metadata["render_ttfr_ms"] = roundedString(ttfrMs)
        metadata["outcome"] = outcome
        metadata["is_fork"] = String(isFork)
        copyString(report, from: "chunkType", to: "chunk_type", into: &metadata)
        copyString(report, from: "traceId", to: "trace_id", into: &metadata)
        copyString(report, from: "spanId", to: "span_id", into: &metadata)
        copyString(report, from: "clientNonce", to: "client_nonce", into: &metadata)
        copyString(report, from: "conversationId", to: "conversation_id", into: &metadata)
        return .init(event: "render_ttfr", level: outcome == "timeout" ? .warn : .info, metadata: metadata)
    }

    private static func projectRenderStream(_ report: [String: Any]) -> IOSMainRPCTelemetryProjection? {
        guard let outcome = report["outcome"] as? String,
              renderStreamOutcomes.contains(outcome),
              let chunkCount = finiteNonnegative(report["chunkCount"]),
              let maxGapMs = finiteNonnegative(report["maxGapMs"]),
              let stalledMs = finiteNonnegative(report["stalledMs"]),
              let stallCount = finiteNonnegative(report["stallCount"]),
              let streamMs = finiteNonnegative(report["streamMs"]),
              let isFork = report["isFork"] as? Bool,
              optionalString(report, key: "traceId"),
              optionalString(report, key: "spanId"),
              optionalString(report, key: "clientNonce"),
              optionalString(report, key: "conversationId")
        else {
            return nil
        }

        var metadata = baseMetadata(event: "render_stream")
        metadata["outcome"] = outcome
        metadata["chunk_count"] = roundedString(chunkCount)
        metadata["stream_gap_max_ms"] = roundedString(maxGapMs)
        metadata["stream_stalled_ms"] = roundedString(stalledMs)
        metadata["stall_count"] = roundedString(stallCount)
        metadata["stream_ms"] = roundedString(streamMs)
        metadata["is_fork"] = String(isFork)
        copyString(report, from: "traceId", to: "trace_id", into: &metadata)
        copyString(report, from: "spanId", to: "span_id", into: &metadata)
        copyString(report, from: "clientNonce", to: "client_nonce", into: &metadata)
        copyString(report, from: "conversationId", to: "conversation_id", into: &metadata)
        return .init(event: "render_stream", level: outcome == "timeout" ? .warn : .info, metadata: metadata)
    }

    private static func baseMetadata(event: String) -> [String: String] {
        [
            "event": event,
            "platform_replacement": "ios_main_rpc_telemetry",
        ]
    }

    private static func finiteNonnegative(_ raw: Any?) -> Double? {
        guard !(raw is Bool), let number = raw as? NSNumber else { return nil }
        let value = number.doubleValue
        return value.isFinite && value >= 0 ? value : nil
    }

    private static func optionalFinite(_ report: [String: Any], key: String) -> Bool {
        !report.keys.contains(key) || finiteNonnegative(report[key]) != nil
    }

    private static func optionalString(_ report: [String: Any], key: String) -> Bool {
        !report.keys.contains(key) || report[key] is String
    }

    private static func optionalBool(_ report: [String: Any], key: String) -> Bool {
        !report.keys.contains(key) || report[key] is Bool
    }

    private static func roundedString(_ value: Double) -> String {
        String(Int(value.rounded()))
    }

    private static func copyString(
        _ report: [String: Any],
        from source: String,
        to target: String,
        into metadata: inout [String: String]
    ) {
        if let value = report[source] as? String {
            metadata[target] = value
        }
    }
}


enum IOSAuthTelemetryStream: String, Equatable, Sendable {
    case session
    case signin
}

struct IOSAuthTelemetryProjection: Equatable, Sendable {
    let stream: IOSAuthTelemetryStream
    let level: IOSLifecycleTelemetryLevel
    let metadata: [String: String]
}

enum IOSCursorSessionRefreshFailure: Equatable, Sendable {
    case httpStatus(Int)
    case network(String)
    case badPayload
}

enum IOSCursorSessionSignoutCause: String, Equatable, Sendable {
    case policy
    case sessionRevoked = "session_revoked"
    case unparseable
    case userAction = "user_action"
    case loginCancelled = "login_cancelled"
    case accountRefused = "account_refused"
}

enum IOSCursorSessionReport: Equatable, Sendable {
    case refreshFailed(IOSCursorSessionRefreshFailure)
    case refreshRecovered(consecutiveFailures: Int, degradedMs: Int)
    case rotationRescued
    case signedOut(cause: IOSCursorSessionSignoutCause, durable: Bool)
    case keychainUnavailable
}

enum IOSCursorSigninReport: Equatable, Sendable {
    case loginStarted
    case loginCompleted
    case loginFailed(cause: String)
    case signedOut(cause: String)
    case gate(String)
    case consult(String)
}

private let IOS_AUTH_CONSECUTIVE_FAILURES_CAP = 10_000
private let IOS_AUTH_DEGRADED_MS_BUCKET_CAP = 86_400_000
private let IOS_AUTH_DEGRADED_MS_BUCKET_CEILINGS = [
    5_000, 30_000, 60_000, 300_000, 1_800_000, 21_600_000, IOS_AUTH_DEGRADED_MS_BUCKET_CAP,
]

func iosAuthDegradedMsBucket(_ degradedMs: Int) -> Int {
    IOS_AUTH_DEGRADED_MS_BUCKET_CEILINGS.first(where: { degradedMs < $0 })
        ?? IOS_AUTH_DEGRADED_MS_BUCKET_CAP
}

private func iosAuthErrorTags(
    code: String,
    retryable: Bool,
    extra: [String: String] = [:]
) -> [String: String] {
    var tags = [
        "error_code": code,
        "error_domain": "auth",
        "error_retryable": String(retryable),
    ]
    tags.merge(extra) { _, incoming in incoming }
    return tags
}

func iosCursorSessionTelemetry(_ report: IOSCursorSessionReport) -> IOSAuthTelemetryProjection {
    switch report {
    case .refreshFailed(let failure):
        let tags: [String: String]
        switch failure {
        case .httpStatus(let status):
            tags = iosAuthErrorTags(
                code: "SAND-E0214",
                retryable: true,
                extra: ["http_status": String(status)]
            )
        case .network(let errno):
            tags = iosAuthErrorTags(
                code: "SAND-E0215",
                retryable: true,
                extra: ["errno": String(errno.prefix(64))]
            )
        case .badPayload:
            tags = iosAuthErrorTags(code: "SAND-E0216", retryable: true)
        }
        return .init(
            stream: .session,
            level: .warn,
            metadata: ["phase": "refresh_failed"].merging(tags) { _, incoming in incoming }
        )
    case .refreshRecovered(let consecutiveFailures, let degradedMs):
        return .init(
            stream: .session,
            level: .info,
            metadata: [
                "phase": "refresh_recovered",
                "consecutive_failures": String(min(consecutiveFailures, IOS_AUTH_CONSECUTIVE_FAILURES_CAP)),
                "degraded_ms": String(iosAuthDegradedMsBucket(degradedMs)),
            ]
        )
    case .rotationRescued:
        return .init(stream: .session, level: .info, metadata: ["phase": "rotation_rescued"])
    case .signedOut(let cause, let durable):
        var metadata = [
            "phase": "signed_out",
            "cause": cause.rawValue,
            "durable": String(durable),
        ]
        if cause == .policy {
            metadata.merge(iosAuthErrorTags(code: "SAND-E0218", retryable: false)) { _, incoming in incoming }
        } else if cause == .sessionRevoked || cause == .unparseable {
            metadata.merge(iosAuthErrorTags(code: "SAND-E0217", retryable: false)) { _, incoming in incoming }
        }
        return .init(
            stream: .session,
            level: (!durable || metadata["error_code"] != nil) ? .warn : .info,
            metadata: metadata
        )
    case .keychainUnavailable:
        return .init(
            stream: .session,
            level: .warn,
            metadata: ["phase": "keychain_unavailable"].merging(
                iosAuthErrorTags(code: "SAND-E0219", retryable: false)
            ) { _, incoming in incoming }
        )
    }
}

func iosCursorSigninTelemetry(_ report: IOSCursorSigninReport) -> IOSAuthTelemetryProjection {
    switch report {
    case .loginStarted:
        return .init(stream: .signin, level: .warn, metadata: ["phase": "login_started"])
    case .loginCompleted:
        return .init(stream: .signin, level: .warn, metadata: ["phase": "login_completed"])
    case .loginFailed(let cause):
        return .init(
            stream: .signin,
            level: .warn,
            metadata: ["phase": "login_failed", "cause": cause]
        )
    case .signedOut(let cause):
        return .init(
            stream: .signin,
            level: .warn,
            metadata: ["phase": "signed_out", "cause": cause]
        )
    case .gate(let gate):
        return .init(
            stream: .signin,
            level: .warn,
            metadata: ["phase": "boot_gate", "gate": gate]
        )
    case .consult(let outcome):
        return .init(
            stream: .signin,
            level: .warn,
            metadata: ["phase": "account_consult", "outcome": outcome]
        )
    }
}

@MainActor
final class IOSAuthTelemetryRelay {
    static let preAttachBufferLimit = 16
    typealias Sink = @MainActor (IOSAuthTelemetryProjection) -> Void

    private var sink: Sink?
    private var pending: [IOSAuthTelemetryProjection] = []

    func attach(_ sink: @escaping Sink) {
        self.sink = sink
        let buffered = pending
        pending.removeAll(keepingCapacity: true)
        buffered.forEach(sink)
    }

    func report(_ projection: IOSAuthTelemetryProjection) {
        guard let sink else {
            if pending.count < Self.preAttachBufferLimit {
                pending.append(projection)
            }
            return
        }
        sink(projection)
    }

    var pendingCount: Int { pending.count }
}
