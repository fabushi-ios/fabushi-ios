import Combine
import Foundation
import SwiftUI

internal let mobileBotRoutineRunHistoryClockName = "agents-now-tick"
internal let mobileBotRoutineRunHistoryClockIntervalMilliseconds = 30_000

internal struct MobileBotRoutineTimeZoneState: Equatable {
    let detectedTimeZone: String?
    let overrideTimeZone: String?
}

internal final class MobileBotRoutineRunHistoryClock {
    internal typealias Cancellation = () -> Void
    internal typealias Scheduler = (
        _ name: String,
        _ intervalMilliseconds: Int,
        _ callback: @escaping () -> Void
    ) -> Cancellation

    private let nowProvider: () -> Date
    private let scheduler: Scheduler
    private var listeners: [UUID: () -> Void] = [:]
    private var cancelTimer: Cancellation?
    private var timeZone: String
    private var disposed = false

    init(
        initialTimeZone: MobileBotRoutineTimeZoneState,
        now: @escaping () -> Date = { Date() },
        scheduler: Scheduler? = nil
    ) {
        self.nowProvider = now
        self.scheduler = scheduler ?? Self.liveScheduler
        self.timeZone = Self.effectiveTimeZone(initialTimeZone)
    }

    static func detectTimeZone() -> MobileBotRoutineTimeZoneState {
        MobileBotRoutineTimeZoneState(
            detectedTimeZone: TimeZone.autoupdatingCurrent.identifier,
            overrideTimeZone: nil
        )
    }

    var nowMilliseconds: Int64 {
        Int64((nowProvider().timeIntervalSince1970 * 1_000).rounded(.towardZero))
    }

    var timeZoneIdentifier: String {
        timeZone
    }

    @discardableResult
    func subscribe(_ listener: @escaping () -> Void) -> Cancellation {
        guard !disposed else { return { } }
        let id = UUID()
        listeners[id] = listener
        ensureTimer()
        return { [weak self] in
            self?.unsubscribe(id)
        }
    }

    func ingestTimeZone(_ state: MobileBotRoutineTimeZoneState) {
        guard !disposed else { return }
        let next = Self.effectiveTimeZone(state)
        guard next != timeZone else { return }
        timeZone = next
        notify()
    }

    func dispose() {
        guard !disposed else { return }
        disposed = true
        stopTimer()
        listeners.removeAll()
    }

    private func unsubscribe(_ id: UUID) {
        guard !disposed else { return }
        listeners.removeValue(forKey: id)
        if listeners.isEmpty {
            stopTimer()
        }
    }

    private func ensureTimer() {
        guard !disposed, cancelTimer == nil, !listeners.isEmpty else { return }
        cancelTimer = scheduler(
            mobileBotRoutineRunHistoryClockName,
            mobileBotRoutineRunHistoryClockIntervalMilliseconds
        ) { [weak self] in
            self?.notify()
        }
    }

    private func stopTimer() {
        cancelTimer?()
        cancelTimer = nil
    }

    private func notify() {
        guard !disposed else { return }
        for listener in Array(listeners.values) {
            listener()
        }
    }

    private static func effectiveTimeZone(_ state: MobileBotRoutineTimeZoneState) -> String {
        state.overrideTimeZone ?? state.detectedTimeZone ?? "UTC"
    }

    private static func liveScheduler(
        _ name: String,
        _ intervalMilliseconds: Int,
        _ callback: @escaping () -> Void
    ) -> Cancellation {
        _ = name
        let timer = Timer(
            timeInterval: TimeInterval(intervalMilliseconds) / 1_000,
            repeats: true
        ) { _ in
            callback()
        }
        RunLoop.main.add(timer, forMode: .common)
        return {
            timer.invalidate()
        }
    }
}

internal enum MobileBotRoutineRunStatus: String, Equatable {
    case running
    case ok
    case error
}

internal struct MobileBotRoutineRun: Identifiable, Equatable {
    let id: String
    let status: MobileBotRoutineRunStatus
    let startedAt: Int64
    let detail: String?
    let event: String?
}

internal struct MobileBotRoutineRunPresentation: Identifiable, Equatable {
    let id: String
    let title: String?
    let timestampLabel: String
    let status: MobileBotRoutineRunStatus
    let accessibilityLabel: String
    let iconName: String
    let statusRole: Bool
}

internal struct MobileBotRoutineRunHistoryPresentation: Equatable {
    let empty: Bool
    let rows: [MobileBotRoutineRunPresentation]
}

internal enum MobileBotRoutineRunHistoryModel {
    private static let months = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun",
        "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ]
    private static let weekdays = [
        "Sunday", "Monday", "Tuesday", "Wednesday",
        "Thursday", "Friday", "Saturday",
    ]

    static func formatTimestamp(
        startedAt: Int64,
        now: Int64,
        timeZoneIdentifier: String?
    ) -> String {
        let delta = startedAt - now
        if delta > 0, delta < 3_600_000 {
            let minutes = Int(ceil(Double(delta) / 60_000))
            return "In \(minutes) min"
        }
        if delta <= 0, -delta < 60_000 {
            return "Just now"
        }
        if delta <= 0, -delta < 3_600_000 {
            return "\(Int((-delta) / 60_000)) min ago"
        }

        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = timeZoneIdentifier
            .flatMap(TimeZone.init(identifier:))
            ?? TimeZone(secondsFromGMT: 0)!
        let currentDate = Date(timeIntervalSince1970: Double(now) / 1_000)
        let startedDate = Date(timeIntervalSince1970: Double(startedAt) / 1_000)
        let current = calendar.dateComponents(
            [.year, .month, .day, .weekday, .hour, .minute],
            from: currentDate
        )
        let started = calendar.dateComponents(
            [.year, .month, .day, .weekday, .hour, .minute],
            from: startedDate
        )
        let currentStart = calendar.startOfDay(for: currentDate)
        let startedStart = calendar.startOfDay(for: startedDate)
        let dayDelta = calendar.dateComponents(
            [.day],
            from: currentStart,
            to: startedStart
        ).day ?? 0
        let hour24 = started.hour ?? 0
        let hour12 = hour24 % 12 == 0 ? 12 : hour24 % 12
        let minute = started.minute ?? 0
        let clock = String(
            format: "%d:%02d %@",
            hour12,
            minute,
            hour24 < 12 ? "AM" : "PM"
        )

        switch dayDelta {
        case 0:
            return "Today at \(clock)"
        case 1:
            return "Tomorrow at \(clock)"
        case -1:
            return "Yesterday at \(clock)"
        case 2...6:
            let weekday = weekdays[max(0, min(6, (started.weekday ?? 1) - 1))]
            return "\(weekday) at \(clock)"
        case -6 ... -2:
            let weekday = weekdays[max(0, min(6, (started.weekday ?? 1) - 1))]
            return "Last \(weekday) at \(clock)"
        default:
            let monthIndex = max(0, min(11, (started.month ?? 1) - 1))
            let date = "\(months[monthIndex]) \(started.day ?? 1)"
            if started.year == current.year {
                return "\(date) at \(clock)"
            }
            return "\(date), \(started.year ?? 0) at \(clock)"
        }
    }

    static func present(
        _ run: MobileBotRoutineRun,
        now: Int64,
        timeZoneIdentifier: String?
    ) -> MobileBotRoutineRunPresentation {
        let title = run.detail ?? run.event
        switch run.status {
        case .running:
            return MobileBotRoutineRunPresentation(
                id: run.id,
                title: title,
                timestampLabel: formatTimestamp(
                    startedAt: run.startedAt,
                    now: now,
                    timeZoneIdentifier: timeZoneIdentifier
                ),
                status: .running,
                accessibilityLabel: "Running",
                iconName: "loading",
                statusRole: true
            )
        case .ok:
            return MobileBotRoutineRunPresentation(
                id: run.id,
                title: title,
                timestampLabel: formatTimestamp(
                    startedAt: run.startedAt,
                    now: now,
                    timeZoneIdentifier: timeZoneIdentifier
                ),
                status: .ok,
                accessibilityLabel: "Succeeded",
                iconName: "check",
                statusRole: false
            )
        case .error:
            return MobileBotRoutineRunPresentation(
                id: run.id,
                title: title,
                timestampLabel: formatTimestamp(
                    startedAt: run.startedAt,
                    now: now,
                    timeZoneIdentifier: timeZoneIdentifier
                ),
                status: .error,
                accessibilityLabel: "Failed",
                iconName: "close",
                statusRole: false
            )
        }
    }

    static func presentHistory(
        _ runs: [MobileBotRoutineRun],
        now: Int64,
        timeZoneIdentifier: String?
    ) -> MobileBotRoutineRunHistoryPresentation {
        MobileBotRoutineRunHistoryPresentation(
            empty: runs.isEmpty,
            rows: runs.map {
                present(
                    $0,
                    now: now,
                    timeZoneIdentifier: timeZoneIdentifier
                )
            }
        )
    }
}

internal struct MobileBotRoutineRunHistoryScope: Equatable {
    let accountKey: String?
    let agentId: String
    let automationId: String
}

internal enum MobileBotRoutineRunHistorySnapshot: Equatable {
    case unavailable
    case loading(
        scope: MobileBotRoutineRunHistoryScope,
        rows: [MobileBotRoutineRunPresentation],
        pending: Bool
    )
    case empty(
        scope: MobileBotRoutineRunHistoryScope,
        pending: Bool
    )
    case ready(
        scope: MobileBotRoutineRunHistoryScope,
        rows: [MobileBotRoutineRunPresentation],
        pending: Bool
    )
    case failed(
        scope: MobileBotRoutineRunHistoryScope,
        rows: [MobileBotRoutineRunPresentation],
        pending: Bool,
        message: String
    )
}

@MainActor
internal protocol MobileBotRoutinesControlling: AnyObject {
    var snapshot: MobileBotRoutinesSnapshot { get }
    var runPending: Set<String> { get }
    @discardableResult
    func subscribe(_ listener: @escaping () -> Void) -> () -> Void
    func refresh(agentId: String) async
    func runNow(agentId: String, automationId: String) async throws
    func reset()
}

@MainActor
internal final class MobileBotRoutineRunHistoryProvider {
    private let controller: MobileBotRoutinesControlling
    private let clock: MobileBotRoutineRunHistoryClock
    private var scope: MobileBotRoutineRunHistoryScope?
    private var listeners: [UUID: () -> Void] = [:]
    private var stopController: (() -> Void)?
    private var stopClock: (() -> Void)?
    private var generation = 0
    private var request = 0
    private var refreshing = false
    private var disposed = false

    init(
        controller: MobileBotRoutinesControlling,
        clock: MobileBotRoutineRunHistoryClock,
        initialScope: MobileBotRoutineRunHistoryScope? = nil
    ) {
        self.controller = controller
        self.clock = clock
        self.scope = initialScope
    }

    @discardableResult
    func subscribe(_ listener: @escaping () -> Void) -> () -> Void {
        guard !disposed else { return { } }
        if listeners.isEmpty {
            startSources()
        }
        let id = UUID()
        listeners[id] = listener
        return { [weak self] in
            self?.unsubscribe(id)
        }
    }

    func snapshot() -> MobileBotRoutineRunHistorySnapshot {
        guard !disposed, let scope else {
            return .unavailable
        }
        let routineSnapshot = controller.snapshot
        let routine = routineSnapshot.value.first {
            $0.id == scope.automationId
        }
        let history = MobileBotRoutineRunHistoryModel.presentHistory(
            routine?.runs ?? [],
            now: clock.nowMilliseconds,
            timeZoneIdentifier: clock.timeZoneIdentifier
        )
        let pending = controller.runPending.contains(scope.automationId)
        if refreshing {
            return .loading(scope: scope, rows: history.rows, pending: pending)
        }
        switch routineSnapshot {
        case .loading:
            return .loading(scope: scope, rows: history.rows, pending: pending)
        case .failed(_, _, let message):
            return .failed(
                scope: scope,
                rows: history.rows,
                pending: pending,
                message: message
            )
        case .empty, .ready, .unavailable:
            if routine == nil || history.empty {
                return .empty(scope: scope, pending: pending)
            }
            return .ready(scope: scope, rows: history.rows, pending: pending)
        }
    }

    func setScope(_ next: MobileBotRoutineRunHistoryScope?) {
        guard !disposed, scope != next else { return }
        let accountChanged = scope?.accountKey != next?.accountKey
        scope = next
        generation += 1
        request += 1
        refreshing = false
        if accountChanged {
            controller.reset()
        }
        notify()
    }

    func refresh() async -> MobileBotRoutineRunHistorySnapshot {
        guard !disposed, let scope else {
            return snapshot()
        }
        let operationGeneration = generation
        request += 1
        let operationRequest = request
        refreshing = true
        notify()
        await controller.refresh(agentId: scope.agentId)
        guard !disposed,
              operationGeneration == generation,
              operationRequest == request
        else {
            return snapshot()
        }
        refreshing = false
        notify()
        return snapshot()
    }

    func refreshOnReconnect() async -> MobileBotRoutineRunHistorySnapshot {
        await refresh()
    }

    func runNow() async throws -> Bool {
        guard !disposed, let scope else { return false }
        let operationGeneration = generation
        guard controller.snapshot.value.contains(where: {
            $0.id == scope.automationId
        }) else {
            return false
        }
        try await controller.runNow(
            agentId: scope.agentId,
            automationId: scope.automationId
        )
        return !disposed && operationGeneration == generation
    }

    func dispose() {
        guard !disposed else { return }
        disposed = true
        generation += 1
        request += 1
        refreshing = false
        stopSources()
        listeners.removeAll()
    }

    private func startSources() {
        stopController = controller.subscribe { [weak self] in
            self?.notify()
        }
        stopClock = clock.subscribe { [weak self] in
            self?.notify()
        }
    }

    private func stopSources() {
        stopController?()
        stopController = nil
        stopClock?()
        stopClock = nil
    }

    private func unsubscribe(_ id: UUID) {
        guard !disposed else { return }
        listeners.removeValue(forKey: id)
        if listeners.isEmpty {
            stopSources()
        }
    }

    private func notify() {
        guard !disposed else { return }
        for listener in Array(listeners.values) {
            listener()
        }
    }
}

internal struct MobileBotRoutine: Identifiable, Equatable {
    let id: String
    let agentId: String
    let name: String
    let prompt: String
    let schedule: String
    let trigger: AutomationTrigger
    let isEnabled: Bool
    let createdAtMs: Int64
    let runs: [MobileBotRoutineRun]
    let lastRunAtMs: Int64?
    let nextRunAtMs: Int64?
}

internal let mobileBotRoutineScheduleIntervalMinutes = 15

internal struct MobileBotRoutineSchedulePickerOption: Identifiable, Equatable {
    let label: String
    let schedule: String

    var id: String { schedule }
}

internal struct MobileBotRoutineCustomScheduleBlurResult: Equatable {
    let schedule: String
    let isInvalid: Bool
    let shouldCommit: Bool
}

internal enum MobileBotRoutineSchedule {
    static func normalize(_ value: String) -> String {
        normalizeSchedule(value)
    }

    static func pickerOptions(days: String = "*") -> [MobileBotRoutineSchedulePickerOption] {
        stride(from: 0, to: 24 * 60, by: mobileBotRoutineScheduleIntervalMinutes).map { totalMinutes in
            let hour = totalMinutes / 60
            let minute = totalMinutes % 60
            let hour12 = hour % 12 == 0 ? 12 : hour % 12
            let suffix = hour < 12 ? "AM" : "PM"
            return MobileBotRoutineSchedulePickerOption(
                label: String(format: "%d:%02d %@", hour12, minute, suffix),
                schedule: "\(minute) \(hour) * * \(days)"
            )
        }
    }

    static func resolveCustomBlur(_ value: String) -> MobileBotRoutineCustomScheduleBlurResult {
        let schedule = normalizeSchedule(value)
        if schedule.isEmpty {
            return .init(schedule: schedule, isInvalid: false, shouldCommit: false)
        }
        guard isValid(schedule) else {
            return .init(schedule: schedule, isInvalid: true, shouldCommit: false)
        }
        return .init(schedule: schedule, isInvalid: false, shouldCommit: true)
    }

    static func isValid(_ value: String) -> Bool {
        let normalized = normalizeSchedule(value)
        guard !normalized.isEmpty else { return false }
        if normalized.lowercased().hasPrefix("@every") {
            return parseEveryIntervalMs(normalized) != nil
        }
        return compileCronMatcher(normalized) != nil
    }
}

@MainActor
internal final class MobileBotRoutineTriggerDraftController: ObservableObject {
    internal typealias Persist = @MainActor ([RoutineTriggerForm]) async throws -> Void
    internal static let maximumRows = TRIGGER_MAX_GROUP_LISTENERS

    @Published private(set) var rows: [RoutineTriggerForm]
    @Published private(set) var lastValidRows: [RoutineTriggerForm]
    @Published private(set) var menuOpen = false
    @Published private(set) var editingRow: Int?
    @Published private(set) var customInvalid = false
    @Published private(set) var hoveredRow: Int?
    @Published private(set) var focusReturnRow: Int?
    @Published private(set) var pending = false
    @Published private(set) var error: String?

    private let onDraftChange: @MainActor ([RoutineTriggerForm]) -> Void
    private let onDraftCommit: Persist
    private let onCommitOrRevert: Persist
    private var generation = 0
    private var disposed = false
    private var menuMutation = false
    private var skipNextMenuCommit = false

    init(
        initialRows: [RoutineTriggerForm],
        onDraftChange: @escaping @MainActor ([RoutineTriggerForm]) -> Void = { _ in },
        onDraftCommit: @escaping Persist = { _ in },
        onCommitOrRevert: @escaping Persist = { _ in }
    ) {
        self.rows = initialRows
        self.lastValidRows = initialRows
        self.onDraftChange = onDraftChange
        self.onDraftCommit = onDraftCommit
        self.onCommitOrRevert = onCommitOrRevert
    }

    var isValid: Bool {
        routineTriggerFromForms(rows) != nil
    }

    func openMenu() {
        guard !disposed else { return }
        menuOpen = true
        error = nil
    }

    func setHoveredRow(_ row: Int?) {
        guard !disposed else { return }
        hoveredRow = row
    }

    func openEditor(_ row: Int) {
        guard !disposed, rows.indices.contains(row) else { return }
        menuOpen = false
        customInvalid = false
        editingRow = row
        focusReturnRow = nil
        error = nil
    }

    func setMenuOpen(_ open: Bool) async -> Bool {
        guard !disposed else { return false }
        if open {
            openMenu()
            return false
        }
        menuOpen = false
        if menuMutation {
            menuMutation = false
            return false
        }
        if skipNextMenuCommit {
            skipNextMenuCommit = false
            return false
        }
        return await persist(lastValidRows, using: onCommitOrRevert)
    }

    func handleMenuEscape() async -> Bool {
        await setMenuOpen(false)
    }

    func closeEditor() async -> Bool {
        guard !disposed, let row = editingRow else { return false }
        editingRow = nil
        customInvalid = false
        let didPersist = await persist(lastValidRows, using: onCommitOrRevert)
        if !disposed {
            focusReturnRow = row
        }
        return didPersist
    }

    func replaceDraft(_ next: [RoutineTriggerForm]) {
        rawChange(next)
    }

    func updateRow(_ row: Int, value: RoutineTriggerForm, commit: Bool = false) async -> Bool {
        guard !disposed, rows.indices.contains(row) else { return false }
        var next = rows
        next[row] = value
        customInvalid = false
        rawChange(next)
        return commit ? await commitCurrent() : false
    }

    func updateCustomSchedule(_ row: Int, value: String) {
        guard rows.indices.contains(row) else { return }
        Task { _ = await updateRow(row, value: .schedule(value), commit: false) }
    }

    func blurCustomSchedule(_ row: Int, value: String) async -> Bool {
        guard !disposed, rows.indices.contains(row) else { return false }
        let schedule = normalizeSchedule(value)
        customInvalid = !schedule.isEmpty
            && !(parseEveryIntervalMs(schedule) != nil || compileCronMatcher(schedule) != nil)
        var next = rows
        next[row] = .schedule(schedule)
        rawChange(next)
        return customInvalid ? false : await commitCurrent()
    }

    func addRow(_ value: RoutineTriggerForm, openEditor: Bool = false) async -> Bool {
        guard !disposed, !pending, rows.count < Self.maximumRows else { return false }
        menuMutation = true
        skipNextMenuCommit = false
        let next = rows + [value]
        rawChange(next)
        if openEditor {
            editingRow = next.count - 1
            menuOpen = false
            focusReturnRow = nil
            return await commitCurrent()
        }
        return false
    }

    func addRowAndCommit(_ value: RoutineTriggerForm) async -> Bool {
        guard !disposed, !pending, rows.count < Self.maximumRows else { return false }
        menuMutation = true
        skipNextMenuCommit = false
        rawChange(rows + [value])
        return await commitCurrent()
    }

    func removeRow(_ row: Int) async -> Bool {
        guard !disposed, rows.indices.contains(row) else { return false }
        menuOpen = false
        editingRow = nil
        customInvalid = false
        if rows.count <= 1 {
            skipNextMenuCommit = true
            rawChange([])
            return false
        }
        var next = rows
        next.remove(at: row)
        rawChange(next)
        return await commitCurrent()
    }

    func clearFocusReturnRow() {
        focusReturnRow = nil
    }

    func reset(_ next: [RoutineTriggerForm]? = nil) {
        guard !disposed else { return }
        generation += 1
        let value = next ?? lastValidRows
        rows = value
        lastValidRows = value
        menuOpen = false
        editingRow = nil
        customInvalid = false
        hoveredRow = nil
        focusReturnRow = nil
        pending = false
        error = nil
        menuMutation = false
        skipNextMenuCommit = false
        onDraftChange(rows)
    }

    func dispose() {
        guard !disposed else { return }
        generation += 1
        disposed = true
        pending = false
    }

    private func rawChange(_ next: [RoutineTriggerForm]) {
        guard !disposed else { return }
        rows = next
        onDraftChange(next)
    }

    private func commitCurrent() async -> Bool {
        guard routineTriggerFromForms(rows) != nil else { return false }
        return await persist(rows, using: onDraftCommit)
    }

    private func persist(_ next: [RoutineTriggerForm], using callback: Persist) async -> Bool {
        guard !disposed, !pending, routineTriggerFromForms(next) != nil else { return false }
        generation += 1
        let token = generation
        pending = true
        error = nil
        do {
            try await callback(next)
            guard !disposed, generation == token else { return false }
            lastValidRows = next
            pending = false
            return true
        } catch {
            guard !disposed, generation == token else { return false }
            self.error = error.localizedDescription
            pending = false
            return false
        }
    }
}

internal struct MobileBotRoutineSpec: Equatable {
    let name: String
    let prompt: String
    let schedule: String
    let isEnabled: Bool
    let trigger: AutomationTrigger? = nil
}

internal enum MobileBotRoutinesSnapshot: Equatable {
    case loading(previous: [MobileBotRoutine])
    case empty
    case ready([MobileBotRoutine])
    case failed(value: [MobileBotRoutine], previous: [MobileBotRoutine]?, message: String)
    case unavailable

    var value: [MobileBotRoutine] {
        switch self {
        case .loading(let previous):
            return previous
        case .empty, .unavailable:
            return []
        case .ready(let value):
            return value
        case .failed(let value, _, _):
            return value
        }
    }
}

internal enum MobileBotRoutinesModel {
    static func parseAutomation(_ row: [String: Any]) -> MobileBotRoutine? {
        guard let id = nonEmptyString(row["id"]),
              let agentId = nonEmptyString(row["agentId"]),
              let name = row["name"] as? String,
              let prompt = row["prompt"] as? String,
              let schedule = row["schedule"] as? String,
              let isEnabled = row["enabled"] as? Bool,
              let createdAtMs = integer(row["createdAtMs"]),
              let rawRuns = row["runs"] as? [[String: Any]]
        else {
            return nil
        }

        let parsedRuns = rawRuns.map(parseRun)
        let lastRunAt = optionalInteger(row["lastRunAtMs"])
        let nextRunAt = optionalInteger(row["nextRunAtMs"])
        let trigger: AutomationTrigger?
        if let rawTrigger = row["trigger"], !(rawTrigger is NSNull) {
            trigger = routineTriggerFromWireValue(rawTrigger)
        } else {
            trigger = routineTriggerFromForms([.schedule(schedule)])
        }
        guard parsedRuns.allSatisfy({ $0 != nil }),
              let trigger,
              case .value(let lastRunAtMs) = lastRunAt,
              case .value(let nextRunAtMs) = nextRunAt
        else {
            return nil
        }

        return MobileBotRoutine(
            id: id,
            agentId: agentId,
            name: name,
            prompt: prompt,
            schedule: schedule,
            trigger: trigger,
            isEnabled: isEnabled,
            createdAtMs: createdAtMs,
            runs: parsedRuns.compactMap { $0 },
            lastRunAtMs: lastRunAtMs,
            nextRunAtMs: nextRunAtMs
        )
    }

    static func parseRun(_ row: [String: Any]) -> MobileBotRoutineRun? {
        guard let id = row["id"] as? String,
              let statusRaw = row["status"] as? String,
              let status = MobileBotRoutineRunStatus(rawValue: statusRaw),
              let startedAt = integer(row["startedAt"])
        else {
            return nil
        }

        let detail: String?
        if row["detail"] == nil || row["detail"] is NSNull {
            detail = nil
        } else if let value = row["detail"] as? String {
            detail = value
        } else {
            return nil
        }

        let event: String?
        if row["event"] == nil || row["event"] is NSNull {
            event = nil
        } else if let value = row["event"] as? String {
            event = value
        } else {
            return nil
        }

        return MobileBotRoutineRun(
            id: id,
            status: status,
            startedAt: startedAt,
            detail: detail,
            event: event
        )
    }

    static func parseAutomations(_ value: Any, agentId: String) throws -> [MobileBotRoutine] {
        guard let rows = value as? [[String: Any]] else {
            throw projectionError("Malformed routines response")
        }
        let parsed = rows.map(parseAutomation)
        guard parsed.allSatisfy({ $0 != nil }) else {
            throw projectionError("Malformed routines response")
        }
        return parsed.compactMap { $0 }.filter { $0.agentId == agentId }
    }

    static func snapshot(
        value: [MobileBotRoutine]?,
        error: String?,
        refreshing: Bool,
        capabilityUnavailable: Bool
    ) -> MobileBotRoutinesSnapshot {
        if capabilityUnavailable {
            return .unavailable
        }
        if let error {
            let current = value ?? []
            return .failed(
                value: current,
                previous: value,
                message: error
            )
        }
        guard let value else {
            return .loading(previous: [])
        }
        if refreshing, value.isEmpty {
            return .loading(previous: value)
        }
        return value.isEmpty ? .empty : .ready(value)
    }

    static func canBegin(_ key: String, pending: Set<String>) -> Bool {
        !pending.contains(key)
    }

    static func commandList(agentId: String, requestId: String) -> [String: Any] {
        [
            "type": "automation.list",
            "requestId": requestId,
            "agentId": agentId,
        ]
    }

    static func commandUpsert(
        agentId: String,
        id: String?,
        spec: MobileBotRoutineSpec,
        requestId: String
    ) -> [String: Any] {
        var command: [String: Any] = [
            "type": "automation.upsert",
            "requestId": requestId,
            "agentId": agentId,
            "name": spec.name,
            "prompt": spec.prompt,
            "schedule": spec.trigger.flatMap(triggerSchedule) ?? spec.schedule,
            "enabled": spec.isEnabled,
        ]
        if let trigger = spec.trigger {
            command["trigger"] = routineTriggerWireValue(trigger)
        }
        if let id {
            command["id"] = id
        }
        return command
    }

    static func commandSetEnabled(
        agentId: String,
        automationId: String,
        isEnabled: Bool,
        requestId: String
    ) -> [String: Any] {
        [
            "type": "automation.setEnabled",
            "requestId": requestId,
            "agentId": agentId,
            "id": automationId,
            "enabled": isEnabled,
        ]
    }

    static func commandDelete(
        agentId: String,
        automationId: String,
        requestId: String
    ) -> [String: Any] {
        [
            "type": "automation.delete",
            "requestId": requestId,
            "agentId": agentId,
            "id": automationId,
        ]
    }

    static func commandRun(
        agentId: String,
        automationId: String,
        requestId: String
    ) -> [String: Any] {
        [
            "type": "automation.run",
            "requestId": requestId,
            "agentId": agentId,
            "id": automationId,
        ]
    }

    private enum OptionalInteger {
        case value(Int64?)
        case invalid

        var value: Int64? {
            if case .value(let value) = self { return value }
            return nil
        }
    }

    private static func optionalInteger(_ value: Any?) -> OptionalInteger {
        if value == nil || value is NSNull {
            return .value(nil)
        }
        guard let parsed = integer(value) else {
            return .invalid
        }
        return .value(parsed)
    }

    private static func integer(_ value: Any?) -> Int64? {
        if let value = value as? Int64 { return value }
        if let value = value as? Int { return Int64(value) }
        if let value = value as? NSNumber { return value.int64Value }
        return nil
    }

    private static func nonEmptyString(_ value: Any?) -> String? {
        guard let value = value as? String else { return nil }
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }

    private static func projectionError(_ message: String) -> NSError {
        NSError(
            domain: "Fabushi.MobileBotRoutines",
            code: 1,
            userInfo: [NSLocalizedDescriptionKey: message]
        )
    }
}

@MainActor
internal struct MobileBotRoutinesSource {
    let bridge: IOSPreloadBridge

    func list(agentId: String) async throws -> [MobileBotRoutine] {
        let requestId = requestId("list")
        _ = try await bridge.request(
            method: "feature.execute",
            params: [
                "command": MobileBotRoutinesModel.commandList(
                    agentId: agentId,
                    requestId: requestId
                ),
            ]
        )

        for _ in 0..<64 {
            try Task.checkCancellation()
            let result = try await bridge.request(
                method: "feature.receive",
                params: ["timeoutMs": 80]
            )
            guard let event = result.value as? [String: Any],
                  event["type"] as? String == "automation.listed",
                  let rows = event["automations"]
            else {
                continue
            }
            return try MobileBotRoutinesModel.parseAutomations(rows, agentId: agentId)
        }
        throw NSError(
            domain: "Fabushi.MobileBotRoutines",
            code: 2,
            userInfo: [NSLocalizedDescriptionKey: "Timed out waiting for Host automation.listed"]
        )
    }

    func create(agentId: String, spec: MobileBotRoutineSpec) async throws {
        try await execute(
            MobileBotRoutinesModel.commandUpsert(
                agentId: agentId,
                id: nil,
                spec: spec,
                requestId: requestId("create")
            )
        )
    }

    func update(
        agentId: String,
        automationId: String,
        spec: MobileBotRoutineSpec
    ) async throws {
        try await execute(
            MobileBotRoutinesModel.commandUpsert(
                agentId: agentId,
                id: automationId,
                spec: spec,
                requestId: requestId("update")
            )
        )
    }

    func setEnabled(
        agentId: String,
        automationId: String,
        isEnabled: Bool
    ) async throws {
        try await execute(
            MobileBotRoutinesModel.commandSetEnabled(
                agentId: agentId,
                automationId: automationId,
                isEnabled: isEnabled,
                requestId: requestId("enabled")
            )
        )
    }

    func remove(agentId: String, automationId: String) async throws {
        try await execute(
            MobileBotRoutinesModel.commandDelete(
                agentId: agentId,
                automationId: automationId,
                requestId: requestId("delete")
            )
        )
    }

    func runNow(agentId: String, automationId: String) async throws {
        try await execute(
            MobileBotRoutinesModel.commandRun(
                agentId: agentId,
                automationId: automationId,
                requestId: requestId("run")
            )
        )
    }

    private func execute(_ command: [String: Any]) async throws {
        _ = try await bridge.request(
            method: "feature.execute",
            params: ["command": command]
        )
        try Task.checkCancellation()
    }

    private func requestId(_ operation: String) -> String {
        "ios-mobile-routine-\(operation)-\(UUID().uuidString.lowercased())"
    }
}

@MainActor
internal final class MobileBotRoutinesController: ObservableObject {
    @Published private(set) var snapshot: MobileBotRoutinesSnapshot = .loading(previous: [])
    @Published private(set) var createPending = false
    @Published private(set) var pending: Set<String> = []
    @Published private(set) var runPending: Set<String> = []
    @Published private(set) var mutationErrors: [String: String] = [:]
    @Published private(set) var refreshing = false

    private let source: MobileBotRoutinesSource
    private var value: [MobileBotRoutine]?
    private var refreshRequest = 0
    private var generation = 0
    private var disposed = false
    private var listeners: [UUID: () -> Void] = [:]

    init(source: MobileBotRoutinesSource) {
        self.source = source
    }

    @discardableResult
    func subscribe(_ listener: @escaping () -> Void) -> () -> Void {
        guard !disposed else { return { } }
        let id = UUID()
        listeners[id] = listener
        return { [weak self] in
            self?.listeners.removeValue(forKey: id)
        }
    }

    func mutationError(for automationId: String) -> String? {
        mutationErrors[automationId]
    }

    func refresh(agentId: String) async {
        guard !disposed else { return }
        refreshRequest += 1
        let request = refreshRequest
        let operationGeneration = generation
        refreshing = true
        publish(error: nil)

        do {
            let loaded = try await source.list(agentId: agentId)
            try Task.checkCancellation()
            guard accepts(request: request, generation: operationGeneration) else { return }
            value = loaded
            refreshing = false
            publish(error: nil)
        } catch is CancellationError {
            return
        } catch {
            guard accepts(request: request, generation: operationGeneration) else { return }
            refreshing = false
            publish(
                error: error.localizedDescription,
                capabilityUnavailable: Self.isCapabilityUnavailable(error)
            )
        }
    }

    func ingest(_ automations: Any, agentId: String) {
        guard !disposed else { return }
        do {
            value = try MobileBotRoutinesModel.parseAutomations(
                automations,
                agentId: agentId
            )
            refreshing = false
            publish(error: nil)
        } catch {
            refreshing = false
            publish(error: error.localizedDescription)
        }
    }

    func create(agentId: String, spec: MobileBotRoutineSpec) async throws -> MobileBotRoutine? {
        guard !disposed, !createPending else { return nil }
        createPending = true
        mutationErrors["create"] = nil
        let operationGeneration = generation
        defer {
            if generation == operationGeneration {
                createPending = false
            }
        }

        do {
            try await source.create(agentId: agentId, spec: spec)
            guard generation == operationGeneration, !disposed else { return nil }
            await refresh(agentId: agentId)
            return snapshot.value.first { $0.name == spec.name }
        } catch {
            if generation == operationGeneration, !disposed {
                mutationErrors["create"] = error.localizedDescription
            }
            throw error
        }
    }

    func setEnabled(
        agentId: String,
        automationId: String,
        isEnabled: Bool
    ) async throws {
        try await withPending(agentId: agentId, automationId: automationId) {
            try await self.source.setEnabled(
                agentId: agentId,
                automationId: automationId,
                isEnabled: isEnabled
            )
        }
    }

    func update(
        agentId: String,
        automationId: String,
        spec: MobileBotRoutineSpec
    ) async throws {
        try await withPending(agentId: agentId, automationId: automationId) {
            try await self.source.update(
                agentId: agentId,
                automationId: automationId,
                spec: spec
            )
        }
    }

    func remove(agentId: String, automationId: String) async throws {
        try await withPending(agentId: agentId, automationId: automationId) {
            try await self.source.remove(
                agentId: agentId,
                automationId: automationId
            )
            if let value = self.value {
                self.value = value.filter { $0.id != automationId }
                self.publish(error: nil)
            }
        }
    }

    func runNow(agentId: String, automationId: String) async throws {
        guard !disposed,
              MobileBotRoutinesModel.canBegin(automationId, pending: runPending)
        else {
            return
        }
        let operationGeneration = generation
        runPending.insert(automationId)
        notify()
        defer {
            if generation == operationGeneration {
                runPending.remove(automationId)
                notify()
            }
        }
        try await source.runNow(agentId: agentId, automationId: automationId)
        guard generation == operationGeneration, !disposed else { return }
        await refresh(agentId: agentId)
    }

    func reset() {
        generation += 1
        refreshRequest += 1
        value = nil
        refreshing = false
        createPending = false
        pending.removeAll()
        runPending.removeAll()
        mutationErrors.removeAll()
        publish(error: nil)
    }

    func dispose() {
        guard !disposed else { return }
        reset()
        disposed = true
        listeners.removeAll()
    }

    private func withPending(
        agentId: String,
        automationId: String,
        operation: @escaping @MainActor () async throws -> Void
    ) async throws {
        guard !disposed,
              MobileBotRoutinesModel.canBegin(automationId, pending: pending)
        else {
            return
        }
        let operationGeneration = generation
        pending.insert(automationId)
        mutationErrors[automationId] = nil
        defer {
            if generation == operationGeneration {
                pending.remove(automationId)
            }
        }

        do {
            try await operation()
            guard generation == operationGeneration, !disposed else { return }
            await refresh(agentId: agentId)
        } catch {
            if generation == operationGeneration, !disposed {
                mutationErrors[automationId] = error.localizedDescription
            }
            throw error
        }
    }

    private func accepts(request: Int, generation: Int) -> Bool {
        !disposed && request == refreshRequest && generation == self.generation
    }

    private func publish(
        error: String?,
        capabilityUnavailable: Bool = false
    ) {
        snapshot = MobileBotRoutinesModel.snapshot(
            value: value,
            error: error,
            refreshing: refreshing,
            capabilityUnavailable: capabilityUnavailable
        )
        notify()
    }

    private func notify() {
        guard !disposed else { return }
        for listener in Array(listeners.values) {
            listener()
        }
    }

    private static func isCapabilityUnavailable(_ error: Error) -> Bool {
        error.localizedDescription.contains("source/capability-unavailable")
    }
}

extension MobileBotRoutinesController: MobileBotRoutinesControlling {}

@MainActor
internal struct MobileBotRoutinesSection: View {
    let agentId: String
    let accountScopeKey: String

    @StateObject private var controller: MobileBotRoutinesController
    @State private var showingEditor = false
    @State private var editingRoutine: MobileBotRoutine?
    @State private var runHistoryClock: MobileBotRoutineRunHistoryClock
    @State private var runHistoryClockStop: (() -> Void)?
    @State private var runHistoryNowMilliseconds: Int64
    @State private var runHistoryTimeZoneIdentifier: String

    init(
        agentId: String,
        bridge: IOSPreloadBridge,
        accountScopeKey: String
    ) {
        self.agentId = agentId
        self.accountScopeKey = accountScopeKey
        let clock = MobileBotRoutineRunHistoryClock(
            initialTimeZone: MobileBotRoutineRunHistoryClock.detectTimeZone()
        )
        _runHistoryClock = State(initialValue: clock)
        _runHistoryClockStop = State(initialValue: nil)
        _runHistoryNowMilliseconds = State(initialValue: clock.nowMilliseconds)
        _runHistoryTimeZoneIdentifier = State(initialValue: clock.timeZoneIdentifier)
        _controller = StateObject(
            wrappedValue: MobileBotRoutinesController(
                source: MobileBotRoutinesSource(bridge: bridge)
            )
        )
    }

    var body: some View {
        Section("自动化") {
            switch controller.snapshot {
            case .loading:
                ProgressView("正在载入自动化…")
                    .accessibilityIdentifier("mobile-agent-routines-loading")
            case .empty:
                Text("暂无自动化")
                    .foregroundStyle(.secondary)
                    .accessibilityIdentifier("mobile-agent-routines-empty")
            case .unavailable:
                Text("此运行环境暂不支持自动化")
                    .foregroundStyle(.secondary)
                    .accessibilityIdentifier("mobile-agent-routines-unavailable")
            case .failed(let value, _, let message):
                if value.isEmpty {
                    Text("载入自动化失败：\(message)")
                        .foregroundStyle(.red)
                        .accessibilityIdentifier("mobile-agent-routines-error")
                } else {
                    routines(value)
                    Text(message)
                        .font(.caption)
                        .foregroundStyle(.red)
                }
            case .ready(let value):
                routines(value)
            }

            Button("添加自动化") {
                editingRoutine = nil
                showingEditor = true
            }
            .disabled(controller.createPending)

            if let error = controller.mutationError(for: "create") {
                Text(error)
                    .font(.caption)
                    .foregroundStyle(.red)
            }
        }
        .sheet(isPresented: $showingEditor) {
            MobileBotRoutineEditorSheet(initial: editingRoutine) { spec in
                Task {
                    do {
                        if let editingRoutine {
                            try await controller.update(
                                agentId: agentId,
                                automationId: editingRoutine.id,
                                spec: spec
                            )
                        } else {
                            _ = try await controller.create(agentId: agentId, spec: spec)
                        }
                        showingEditor = false
                        self.editingRoutine = nil
                    } catch {
                        // Controller retains the scoped mutation error for the shipping section.
                    }
                }
            }
        }
        .task(id: "\(accountScopeKey)|\(agentId)") {
            controller.reset()
            await controller.refresh(agentId: agentId)
        }
        .onAppear {
            guard runHistoryClockStop == nil else { return }
            runHistoryNowMilliseconds = runHistoryClock.nowMilliseconds
            runHistoryTimeZoneIdentifier = runHistoryClock.timeZoneIdentifier
            runHistoryClockStop = runHistoryClock.subscribe {
                Task { @MainActor in
                    runHistoryNowMilliseconds = runHistoryClock.nowMilliseconds
                    runHistoryTimeZoneIdentifier = runHistoryClock.timeZoneIdentifier
                }
            }
        }
        .onDisappear {
            runHistoryClockStop?()
            runHistoryClockStop = nil
            controller.reset()
        }
    }

    @ViewBuilder
    private func routines(_ routines: [MobileBotRoutine]) -> some View {
        ForEach(routines) { routine in
            VStack(alignment: .leading, spacing: 8) {
                HStack {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(routine.name)
                            .font(.headline)
                        Text(routine.isEnabled ? describeSchedule(routine.schedule) : "Paused")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    Spacer()
                    Toggle(
                        "启用",
                        isOn: Binding(
                            get: { routine.isEnabled },
                            set: { isEnabled in
                                Task {
                                    try? await controller.setEnabled(
                                        agentId: agentId,
                                        automationId: routine.id,
                                        isEnabled: isEnabled
                                    )
                                }
                            }
                        )
                    )
                    .labelsHidden()
                    .disabled(controller.pending.contains(routine.id))
                }

                if !routine.prompt.isEmpty {
                    Text(routine.prompt)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(3)
                }

                HStack {
                    Button(
                        controller.runPending.contains(routine.id)
                            ? "运行中…"
                            : "立即运行"
                    ) {
                        Task {
                            try? await controller.runNow(
                                agentId: agentId,
                                automationId: routine.id
                            )
                        }
                    }
                    .disabled(controller.runPending.contains(routine.id))

                    Button("编辑") {
                        editingRoutine = routine
                        showingEditor = true
                    }
                    .disabled(controller.pending.contains(routine.id))

                    Button("删除", role: .destructive) {
                        Task {
                            try? await controller.remove(
                                agentId: agentId,
                                automationId: routine.id
                            )
                        }
                    }
                    .disabled(controller.pending.contains(routine.id))
                }

                MobileBotRoutineInlineRunHistory(
                    runs: routine.runs,
                    nowMilliseconds: runHistoryNowMilliseconds,
                    timeZoneIdentifier: runHistoryTimeZoneIdentifier
                )

                if let error = controller.mutationError(for: routine.id) {
                    Text(error)
                        .font(.caption)
                        .foregroundStyle(.red)
                }
            }
            .accessibilityIdentifier("mobile-agent-routine-\(routine.id)")
        }
    }
}

@MainActor
internal struct MobileBotRoutineEditorSheet: View {
    let initial: MobileBotRoutine?
    let onSave: (MobileBotRoutineSpec) -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var name: String
    @State private var prompt: String
    @State private var schedule: String
    @State private var isEnabled: Bool
    @State private var scheduleInvalid = false

    init(
        initial: MobileBotRoutine?,
        onSave: @escaping (MobileBotRoutineSpec) -> Void
    ) {
        self.initial = initial
        self.onSave = onSave
        _name = State(initialValue: initial?.name ?? "")
        _prompt = State(initialValue: initial?.prompt ?? "")
        _schedule = State(initialValue: initial?.schedule ?? "0 * * * *")
        _isEnabled = State(initialValue: initial?.isEnabled ?? true)
    }

    var body: some View {
        NavigationStack {
            Form {
                Section("自动化") {
                    TextField("名称", text: $name)
                    TextField("提示词", text: $prompt, axis: .vertical)
                        .lineLimit(3...8)
                    Toggle("启用", isOn: $isEnabled)
                }

                Section("触发时间") {
                    Picker("时间", selection: $schedule) {
                        ForEach(MobileBotRoutineSchedule.pickerOptions()) { option in
                            Text(option.label).tag(option.schedule)
                        }
                    }

                    TextField("Schedule", text: $schedule)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .onSubmit {
                            validateSchedule()
                        }
                        .accessibilityValue(scheduleInvalid ? "invalid" : "valid")

                    if scheduleInvalid {
                        Text("请输入有效的 cron、@every 或带时区的 cron 表达式")
                            .font(.caption)
                            .foregroundStyle(.red)
                    }
                }
            }
            .navigationTitle(initial == nil ? "添加自动化" : "编辑自动化")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("取消") {
                        dismiss()
                    }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("保存") {
                        save()
                    }
                    .disabled(
                        name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                            || prompt.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                    )
                }
            }
        }
    }

    private func validateSchedule() {
        let result = MobileBotRoutineSchedule.resolveCustomBlur(schedule)
        schedule = result.schedule
        scheduleInvalid = result.isInvalid || !result.shouldCommit
    }

    private func save() {
        let result = MobileBotRoutineSchedule.resolveCustomBlur(schedule)
        schedule = result.schedule
        scheduleInvalid = result.isInvalid || !result.shouldCommit
        guard result.shouldCommit else { return }
        let trimmedName = name.trimmingCharacters(in: .whitespacesAndNewlines)
        let trimmedPrompt = prompt.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmedName.isEmpty, !trimmedPrompt.isEmpty else { return }
        onSave(
            MobileBotRoutineSpec(
                name: trimmedName,
                prompt: trimmedPrompt,
                schedule: result.schedule,
                isEnabled: isEnabled
            )
        )
    }
}

@MainActor
internal struct MobileBotRoutineInlineRunHistory: View {
    let runs: [MobileBotRoutineRun]
    let nowMilliseconds: Int64
    let timeZoneIdentifier: String

    var body: some View {
        let rows = MobileBotRoutineRunHistoryModel.presentHistory(
            runs,
            now: nowMilliseconds,
            timeZoneIdentifier: timeZoneIdentifier
        ).rows

        VStack(alignment: .leading, spacing: 4) {
            Text("Run history")
                .font(.caption)
                .foregroundStyle(.secondary)

            if rows.isEmpty {
                Text("No runs yet")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            } else {
                ForEach(rows.prefix(5)) { row in
                    HStack(spacing: 6) {
                        Text(row.timestampLabel)
                            .font(.caption2)
                        Spacer()
                        Image(systemName: symbolName(row.iconName))
                            .accessibilityLabel(row.accessibilityLabel)
                    }
                    .help(row.title ?? row.accessibilityLabel)
                }
            }
        }
    }

    private func symbolName(_ iconName: String) -> String {
        switch iconName {
        case "loading": return "progress.indicator"
        case "check": return "checkmark.circle"
        default: return "xmark.circle"
        }
    }
}

