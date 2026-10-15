import Foundation
#if canImport(MetricKit)
import MetricKit
#endif

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
    case update
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
    nonisolated static let preAttachBufferLimit = 100
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


let IOS_MAIN_THREAD_PRESSURE_RESOLUTION_MS = 20.0
let IOS_MAIN_THREAD_PRESSURE_WINDOW_MS = 60_000.0
let IOS_MAIN_THREAD_PRESSURE_P95_MS = 100.0
let IOS_MAIN_THREAD_SEVERE_P95_MS = 1_000.0
let IOS_MAIN_THREAD_DELAY_REPORT_CAP_MS = 60_000.0

enum IOSMainThreadPressureSeverity: String, Equatable, Sendable {
    case elevated
    case severe
}

struct IOSMainThreadPressureSummary: Equatable, Sendable {
    let p50Ms: Double
    let p95Ms: Double
    let maxMs: Double
    let windowMs: Double
}

struct IOSMainThreadPressureAccumulator {
    private var delays: [Double] = []
    private var elapsedMs = 0.0

    mutating func reset() {
        delays.removeAll(keepingCapacity: true)
        elapsedMs = 0
    }

    mutating func record(
        delayMilliseconds: Double,
        elapsedMilliseconds: Double
    ) -> (summary: IOSMainThreadPressureSummary, severity: IOSMainThreadPressureSeverity)? {
        guard delayMilliseconds.isFinite,
              elapsedMilliseconds.isFinite,
              delayMilliseconds >= 0,
              elapsedMilliseconds >= 0
        else {
            return nil
        }
        delays.append(delayMilliseconds)
        elapsedMs += elapsedMilliseconds
        guard elapsedMs >= IOS_MAIN_THREAD_PRESSURE_WINDOW_MS, !delays.isEmpty else {
            return nil
        }

        let sorted = delays.sorted()
        let summary = IOSMainThreadPressureSummary(
            p50Ms: Self.percentile(sorted, 0.50),
            p95Ms: Self.percentile(sorted, 0.95),
            maxMs: sorted.last ?? 0,
            windowMs: elapsedMs
        )
        reset()
        let severity: IOSMainThreadPressureSeverity?
        if summary.p95Ms >= IOS_MAIN_THREAD_SEVERE_P95_MS {
            severity = .severe
        } else if summary.p95Ms >= IOS_MAIN_THREAD_PRESSURE_P95_MS {
            severity = .elevated
        } else {
            severity = nil
        }
        guard let severity else { return nil }
        return (summary, severity)
    }

    private static func percentile(_ sorted: [Double], _ percentile: Double) -> Double {
        guard !sorted.isEmpty else { return 0 }
        let rank = max(1, Int(ceil(percentile * Double(sorted.count))))
        return sorted[min(sorted.count - 1, rank - 1)]
    }
}

func iosMainThreadPressureMetadata(
    summary: IOSMainThreadPressureSummary,
    severity: IOSMainThreadPressureSeverity
) -> [String: String] {
    func capped(_ value: Double) -> String {
        String(Int(min(IOS_MAIN_THREAD_DELAY_REPORT_CAP_MS, value).rounded()))
    }
    return [
        "trigger": "pressure",
        "severity": severity.rawValue,
        "p50_ms": capped(summary.p50Ms),
        "p95_ms": capped(summary.p95Ms),
        "max_ms": capped(summary.maxMs),
        "window_ms": String(Int(summary.windowMs.rounded())),
        "platform_replacement": "ios_main_run_loop",
    ]
}

@MainActor
final class IOSMainThreadPressureMonitor: NSObject {
    private let reporter: IOSLifecycleReporter
    private var timer: Timer?
    private var accumulator = IOSMainThreadPressureAccumulator()
    private var lastTickUptime: TimeInterval?

    init(reporter: IOSLifecycleReporter) {
        self.reporter = reporter
        super.init()
    }

    func start() {
        guard timer == nil else { return }
        accumulator.reset()
        lastTickUptime = ProcessInfo.processInfo.systemUptime
        let timer = Timer(
            timeInterval: IOS_MAIN_THREAD_PRESSURE_RESOLUTION_MS / 1_000,
            target: self,
            selector: #selector(tick),
            userInfo: nil,
            repeats: true
        )
        self.timer = timer
        RunLoop.main.add(timer, forMode: .common)
    }

    func stop() {
        timer?.invalidate()
        timer = nil
        lastTickUptime = nil
        accumulator.reset()
    }

    @objc private func tick() {
        let now = ProcessInfo.processInfo.systemUptime
        guard let previous = lastTickUptime else {
            lastTickUptime = now
            return
        }
        lastTickUptime = now
        let elapsedMs = max(0, (now - previous) * 1_000)
        let delayMs = max(0, elapsedMs - IOS_MAIN_THREAD_PRESSURE_RESOLUTION_MS)
        guard let projection = accumulator.record(
            delayMilliseconds: delayMs,
            elapsedMilliseconds: elapsedMs
        ) else {
            return
        }
        reporter.report(
            .processRecovery,
            level: projection.severity == .severe ? .error : .warn,
            metadata: iosMainThreadPressureMetadata(
                summary: projection.summary,
                severity: projection.severity
            )
        )
    }
}


let IOS_PROCESS_CRASH_EVENT_CAP = 20

struct IOSProcessCrashTelemetryLimiter {
    private(set) var crashSequence = 0

    mutating func nextMetadata() -> [String: String]? {
        crashSequence += 1
        guard crashSequence <= IOS_PROCESS_CRASH_EVENT_CAP else { return nil }
        var metadata = [
            "kind": "ios_metric_kit_crash",
            "delivery": "post_launch",
            "crash_seq": String(crashSequence),
            "platform_replacement": "metric_kit",
        ]
        if crashSequence == IOS_PROCESS_CRASH_EVENT_CAP {
            metadata["capped"] = "true"
        }
        metadata.merge(sandErrorTags(SandError.desktopUncaughtException())) { _, incoming in
            incoming
        }
        return metadata
    }
}

#if canImport(MetricKit)
final class IOSProcessCrashDiagnosticMonitor: NSObject, MXMetricManagerSubscriber, @unchecked Sendable {
    private let reporter: IOSLifecycleReporter
    private var limiter = IOSProcessCrashTelemetryLimiter()
    private var started = false

    @MainActor
    init(reporter: IOSLifecycleReporter) {
        self.reporter = reporter
        super.init()
    }

    @MainActor
    func start() {
        guard !started else { return }
        started = true
        MXMetricManager.shared.add(self)
    }

    @MainActor
    func stop() {
        guard started else { return }
        started = false
        MXMetricManager.shared.remove(self)
    }

    func didReceive(_ payloads: [MXDiagnosticPayload]) {
        let crashCount = payloads.reduce(into: 0) { count, payload in
            count += payload.crashDiagnostics?.count ?? 0
        }
        guard crashCount > 0 else { return }
        Task { @MainActor [weak self] in
            self?.reportCrashDeliveries(count: crashCount)
        }
    }

    @MainActor
    private func reportCrashDeliveries(count: Int) {
        guard started else { return }
        for _ in 0..<count {
            guard let metadata = limiter.nextMetadata() else { break }
            reporter.report(
                .processRecovery,
                level: .error,
                metadata: metadata
            )
        }
    }
}
#else
@MainActor
final class IOSProcessCrashDiagnosticMonitor {
    init(reporter: IOSLifecycleReporter) {}
    func start() {}
    func stop() {}
}
#endif


private let IOS_BOX_RECREATE_STALLED_AFTER_MS: Int64 = 600_000
private let IOS_SETUP_REBEGIN_DEDUPE_MS: Int64 = 1_000

private func iosBoundedBoxVisibilityToken(_ value: Any?) -> String? {
    guard let value = value as? String,
          !value.isEmpty,
          value.count <= 128,
          value.range(of: #"^[0-9A-Za-z._:|\-]+$"#, options: .regularExpression) != nil
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
        guard value.isFinite, value >= 0, value <= Double(Int64.max) else { return }

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
