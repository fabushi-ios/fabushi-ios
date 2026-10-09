import Foundation

enum IOSLifecycleTelemetryLevel: String, Equatable, Sendable {
    case info
    case warn
    case error
}

enum IOSLifecycleTelemetryFamily: String, CaseIterable, Equatable, Sendable {
    case startup
    case processRecovery
    case rendererLifecycle
    case coordinatorHandoff
    case localExecLifecycle
    case uncleanExit
    case desktopSession
    case desktopSignin
    case boxSetupVisible
    case boxRecreateVisible
    case boxRebuildStage
}

struct IOSLifecycleTelemetryRecord: Equatable, Sendable {
    let family: IOSLifecycleTelemetryFamily
    let level: IOSLifecycleTelemetryLevel
    let metadata: [String: String]
}

@MainActor
final class IOSLifecycleReporter {
    static let preAttachBufferLimit = 100
    private var uploader: (@MainActor (IOSLifecycleTelemetryRecord) -> Void)?
    private var buffered: [IOSLifecycleTelemetryRecord] = []

    func attach(_ uploader: @escaping @MainActor (IOSLifecycleTelemetryRecord) -> Void) {
        self.uploader = uploader
        let pending = buffered
        buffered.removeAll(keepingCapacity: true)
        pending.forEach(uploader)
    }

    func report(
        _ family: IOSLifecycleTelemetryFamily,
        level: IOSLifecycleTelemetryLevel = .info,
        metadata: [String: String] = [:]
    ) {
        let record = IOSLifecycleTelemetryRecord(family: family, level: level, metadata: metadata)
        guard let uploader else {
            if buffered.count >= Self.preAttachBufferLimit { buffered.removeFirst() }
            buffered.append(record)
            return
        }
        uploader(record)
    }

    var bufferedCount: Int { buffered.count }
}


private let IOS_BOX_RECREATE_STALLED_AFTER_MS: Int64 = 600_000
private let IOS_SETUP_REBEGIN_DEDUPE_MS: Int64 = 1_000

private func iosBoundedBoxVisibilityToken(_ value: Any?) -> String? {
    guard let value = value as? String,
          !value.isEmpty,
          value.count <= 128,
          value.range(of: #"^[0-9A-Za-z._:|\\-]+$"#, options: .regularExpression) != nil
    else { return nil }
    return value
}

private func iosBoundedBoxVisibilitySurface(_ value: Any?) -> String? {
    guard let value = value as? String,
          value == "foreground" || value == "background"
    else { return nil }
    return value
}

@MainActor
final class IOSBoxVisibilityTracker {
    private struct OpenWindow {
        let trigger: String
        let startedAtMs: Int64
        let documentKey: String
        let operationID: String?
        let kind: String?
        var surface: String?
    }

    private let reporter: IOSLifecycleReporter
    private let nowMs: () -> Int64
    private var open: [String: OpenWindow] = [:]
    private var setupEnd: (documentKey: String, atMs: Int64)?
    private var accountSlot: String?
    private var accountSlotSeen = false

    init(
        reporter: IOSLifecycleReporter,
        nowMs: @escaping () -> Int64 = {
            Int64(Date().timeIntervalSince1970 * 1_000)
        }
    ) {
        self.reporter = reporter
        self.nowMs = nowMs
    }

    func noteAccountSlot(_ slot: String?) {
        let changed = accountSlotSeen && slot != accountSlot
        accountSlotSeen = true
        accountSlot = slot
        guard changed else { return }
        if let setup = open.removeValue(forKey: "setup") {
            emit(event: "setup", window: setup, outcome: "abandoned", surface: setup.surface)
        }
        setupEnd = nil
    }

    func handle(_ report: [String: Any], documentKey: String) {
        guard let event = report["event"] as? String,
              event == "setup" || event == "recreate",
              let phase = report["phase"] as? String,
              ["begin", "heartbeat", "end", "stage_transition"].contains(phase)
        else { return }

        switch phase {
        case "begin":
            begin(event: event, report: report, documentKey: documentKey)
        case "heartbeat":
            heartbeat(event: event, report: report, documentKey: documentKey)
        case "stage_transition":
            stageTransition(event: event, report: report, documentKey: documentKey)
        default:
            end(event: event, report: report, documentKey: documentKey)
        }
    }

    func abandonAll() {
        let pending = open
        open.removeAll()
        for (event, window) in pending {
            emit(event: event, window: window, outcome: "abandoned", surface: window.surface)
        }
    }

    var isRecreateCoverOpen: Bool {
        open["recreate"] != nil
    }

    private func begin(event: String, report: [String: Any], documentKey: String) {
        if event == "setup", let setupEnd {
            if setupEnd.documentKey == documentKey,
               nowMs() - setupEnd.atMs < IOS_SETUP_REBEGIN_DEDUPE_MS
            {
                return
            }
            self.setupEnd = nil
        }
        if open[event]?.documentKey == documentKey { return }

        open[event] = OpenWindow(
            trigger: ((report["trigger"] as? String)?.isEmpty == false)
                ? (report["trigger"] as! String)
                : "unknown",
            startedAtMs: nowMs(),
            documentKey: documentKey,
            operationID: iosBoundedBoxVisibilityToken(report["operationId"]),
            kind: iosBoundedBoxVisibilityToken(report["kind"]),
            surface: iosBoundedBoxVisibilitySurface(report["surface"])
        )
    }

    private func heartbeat(event: String, report: [String: Any], documentKey: String) {
        guard event == "recreate",
              var window = open["recreate"],
              window.documentKey == documentKey
        else { return }

        if let surface = iosBoundedBoxVisibilitySurface(report["surface"]),
           surface != window.surface
        {
            window.surface = surface
            open["recreate"] = window
        }

        let elapsedMs = max(0, nowMs() - window.startedAtMs)
        var metadata = [
            "phase": "heartbeat",
            "trigger": window.trigger,
            "elapsed_ms": String(elapsedMs),
        ]
        if let stage = iosBoundedBoxVisibilityToken(report["stage"]) {
            metadata["stage"] = stage
        }
        if let migration = iosBoundedBoxVisibilityToken(report["migrationPhase"]) {
            metadata["migration_phase"] = migration
        }
        if let operationID = iosBoundedBoxVisibilityToken(report["operationId"]) ?? window.operationID {
            metadata["operation_id"] = operationID
        }
        reporter.report(
            .boxRecreateVisible,
            level: elapsedMs >= IOS_BOX_RECREATE_STALLED_AFTER_MS ? .warn : .info,
            metadata: metadata
        )
    }

    private func stageTransition(event: String, report: [String: Any], documentKey: String) {
        guard event == "recreate",
              let window = open["recreate"],
              window.documentKey == documentKey,
              let kind = iosBoundedBoxVisibilityToken(report["kind"]),
              let toStage = iosBoundedBoxVisibilityToken(report["stage"]),
              !(report["stageElapsedMs"] is Bool),
              let stageElapsed = report["stageElapsedMs"] as? NSNumber
        else { return }

        let value = stageElapsed.doubleValue
        guard value.isFinite, value >= 0 else { return }

        var metadata = [
            "kind": kind,
            "to_stage": toStage,
            "stage_elapsed_ms": String(Int64(value.rounded())),
        ]
        if let fromStage = iosBoundedBoxVisibilityToken(report["fromStage"]) {
            metadata["from_stage"] = fromStage
        }
        if let migration = iosBoundedBoxVisibilityToken(report["migrationPhase"]) {
            metadata["migration_phase"] = migration
        }
        if let operationID = iosBoundedBoxVisibilityToken(report["operationId"]) ?? window.operationID {
            metadata["operation_id"] = operationID
        }
        reporter.report(.boxRebuildStage, metadata: metadata)
    }

    private func end(event: String, report: [String: Any], documentKey: String) {
        guard let window = open[event], window.documentKey == documentKey else { return }
        open[event] = nil
        let outcome = (report["outcome"] as? String) ?? "abandoned"
        emit(
            event: event,
            window: window,
            outcome: outcome,
            surface: iosBoundedBoxVisibilitySurface(report["surface"]) ?? window.surface
        )
    }

    private func emit(
        event: String,
        window: OpenWindow,
        outcome: String,
        surface: String?
    ) {
        if event == "setup" {
            setupEnd = (window.documentKey, nowMs())
        }

        var metadata = [
            "trigger": window.trigger,
            "outcome": outcome,
            "duration_ms": String(max(0, nowMs() - window.startedAtMs)),
        ]
        if let operationID = window.operationID { metadata["operation_id"] = operationID }
        if let kind = window.kind { metadata["kind"] = kind }
        if let surface { metadata["surface"] = surface }

        reporter.report(
            event == "setup" ? .boxSetupVisible : .boxRecreateVisible,
            level: event == "setup" || outcome != "ready" ? .warn : .info,
            metadata: metadata
        )
    }
}
