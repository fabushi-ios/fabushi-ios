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
