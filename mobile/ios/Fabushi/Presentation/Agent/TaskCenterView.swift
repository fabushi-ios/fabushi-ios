//
//  TaskCenterView.swift
//  Fabushi
//
//  Feature 32: 7-State Global Task Center (1:1 Telegram iOS Native).
//  7-state task FSM with strict terminal state protection, 5-field cron routine
//  syntax validator and humanizer, filter pills, and task management.
//

import SwiftUI

// MARK: - Enums & Errors

public enum AgentTaskState: String, Sendable, CaseIterable, Codable, Equatable {
    case pending = "pending"
    case assigned = "assigned"
    case running = "running"
    case waitingAuth = "waiting_auth"
    case paused = "paused"
    case failed = "failed"
    case cancelled = "cancelled"
    case completed = "completed"

    public var isTerminal: Bool {
        self == .completed || self == .failed || self == .cancelled
    }

    public var title: String {
        switch self {
        case .pending: return "Pending"
        case .assigned: return "Assigned"
        case .running: return "Running"
        case .waitingAuth: return "Waiting Auth"
        case .paused: return "Paused"
        case .failed: return "Failed"
        case .cancelled: return "Cancelled"
        case .completed: return "Completed"
        }
    }
}

public enum TaskFsmError: Error, Equatable, LocalizedError {
    case illegalStateTransition(from: AgentTaskState, to: AgentTaskState)
    case taskNotFound

    public var errorDescription: String? {
        switch self {
        case .illegalStateTransition(let from, let to):
            return "Illegal transition from terminal or incompatible state \(from.rawValue) to \(to.rawValue)"
        case .taskNotFound:
            return "Task not found"
        }
    }
}

public enum CronError: Error, Equatable, LocalizedError {
    case invalidFormat(String)

    public var errorDescription: String? {
        switch self {
        case .invalidFormat(let msg): return msg
        }
    }
}

public enum TaskFilter: String, CaseIterable, Identifiable, Sendable {
    case all = "All"
    case active = "Active"
    case waitingAuth = "Waiting Auth"
    case scheduled = "Scheduled"
    case done = "Done"

    public var id: String { rawValue }
}

// MARK: - 5-Field Cron Routine Parser & Humanizer

public enum CronRoutineParser {
    /// Validates strictly 5 fields: minute, hour, day-of-month, month, day-of-week.
    public static func validate(expression: String) throws {
        let parts = expression.trimmingCharacters(in: .whitespacesAndNewlines)
            .split(whereSeparator: { $0.isWhitespace })
            .map(String.init)

        guard parts.count == 5 else {
            throw CronError.invalidFormat("Invalid 5-field cron format: expected 5 fields, got \(parts.count)")
        }

        // Validate minute (0-59 or * or */n)
        try validateComponent(parts[0], min: 0, max: 59, name: "minute")
        // Validate hour (0-23 or * or */n)
        try validateComponent(parts[1], min: 0, max: 23, name: "hour")
        // Validate day of month (1-31 or *)
        try validateComponent(parts[2], min: 1, max: 31, name: "day of month")
        // Validate month (1-12 or *)
        try validateComponent(parts[3], min: 1, max: 12, name: "month")
        // Validate day of week (0-7 or 1-5 or *)
        try validateComponent(parts[4], min: 0, max: 7, name: "day of week")
    }

    private static func validateComponent(_ part: String, min: Int, max: Int, name: String) throws {
        if part == "*" { return }
        if part.hasPrefix("*/") {
            let stepStr = String(part.dropFirst(2))
            if let step = Int(stepStr), step > 0 && step <= max {
                return
            }
            throw CronError.invalidFormat("Invalid step in \(name): \(part)")
        }

        if part.contains("-") {
            let range = part.split(separator: "-").map(String.init)
            if range.count == 2, let r1 = Int(range[0]), let r2 = Int(range[1]), r1 >= min && r2 <= max && r1 <= r2 {
                return
            }
            throw CronError.invalidFormat("Invalid range in \(name): \(part)")
        }

        if let val = Int(part), val >= min && val <= max {
            return
        }

        throw CronError.invalidFormat("Invalid value in \(name): \(part)")
    }

    /// Humanizes canonical 5-field cron patterns
    public static func humanize(expression: String) -> String {
        let parts = expression.trimmingCharacters(in: .whitespacesAndNewlines)
            .split(whereSeparator: { $0.isWhitespace })
            .map(String.init)

        guard parts.count == 5 else { return expression }

        let (min, hr, dom, mon, dow) = (parts[0], parts[1], parts[2], parts[3], parts[4])

        if min == "0" && hr == "9" && dom == "*" && mon == "*" && (dow == "1-5" || dow == "1,2,3,4,5") {
            return "Weekdays at 9:00 AM"
        }
        if min == "0" && hr == "9" && dom == "*" && mon == "*" && dow == "*" {
            return "Every day at 9:00 AM"
        }
        if min == "0" && hr == "0" && dom == "*" && mon == "*" && dow == "*" {
            return "Every day at midnight"
        }
        if min == "0" && hr == "12" && dom == "*" && mon == "*" && dow == "*" {
            return "Every day at 12:00 PM"
        }
        if min.hasPrefix("*/") && hr == "*" && dom == "*" && mon == "*" && dow == "*" {
            let step = String(min.dropFirst(2))
            return "Every \(step) minutes"
        }

        return "Cron routine: \(expression)"
    }
}

// MARK: - Models

public struct AgentTaskModel: Identifiable, Sendable, Equatable {
    public let id: String
    public var title: String
    public var state: AgentTaskState
    public let createdAt: Date
    public var cronExpression: String?
    public var errorReason: String?

    public init(
        id: String,
        title: String,
        state: AgentTaskState = .pending,
        createdAt: Date = Date(),
        cronExpression: String? = nil,
        errorReason: String? = nil
    ) {
        self.id = id
        self.title = title
        self.state = state
        self.createdAt = createdAt
        self.cronExpression = cronExpression
        self.errorReason = errorReason
    }
}

public struct CronRoutineModel: Identifiable, Sendable, Equatable {
    public let id: String
    public var name: String
    public var cronExpression: String
    public var humanSchedule: String
    public var nextFireDate: Date

    public init(
        id: String,
        name: String,
        cronExpression: String,
        humanSchedule: String? = nil,
        nextFireDate: Date = Date().addingTimeInterval(3600)
    ) {
        self.id = id
        self.name = name
        self.cronExpression = cronExpression
        self.humanSchedule = humanSchedule ?? CronRoutineParser.humanize(expression: cronExpression)
        self.nextFireDate = nextFireDate
    }
}

// MARK: - View Model

@MainActor
public final class TaskCenterViewModel: ObservableObject {
    @Published public var tasks: [AgentTaskModel] = []
    @Published public var routines: [CronRoutineModel] = []
    @Published public var selectedFilter: TaskFilter = .all

    public init(tasks: [AgentTaskModel] = [], routines: [CronRoutineModel] = []) {
        self.tasks = tasks
        self.routines = routines
    }

    public func transitionTask(id: String, to newState: AgentTaskState, reason: String? = nil) throws {
        guard let index = tasks.firstIndex(where: { $0.id == id }) else {
            throw TaskFsmError.taskNotFound
        }

        let current = tasks[index].state

        // Strict terminal state protection
        if current.isTerminal {
            throw TaskFsmError.illegalStateTransition(from: current, to: newState)
        }

        // Validate allowed transitions
        switch (current, newState) {
        case (.pending, .assigned), (.pending, .running), (.pending, .cancelled):
            break
        case (.assigned, .running), (.assigned, .cancelled):
            break
        case (.running, .waitingAuth), (.running, .paused), (.running, .completed), (.running, .failed), (.running, .cancelled):
            break
        case (.waitingAuth, .running), (.waitingAuth, .failed), (.waitingAuth, .cancelled):
            break
        case (.paused, .running), (.paused, .cancelled):
            break
        default:
            throw TaskFsmError.illegalStateTransition(from: current, to: newState)
        }

        tasks[index].state = newState
        if let reason = reason {
            tasks[index].errorReason = reason
        }
    }

    public func cancelTask(id: String) {
        try? transitionTask(id: id, to: .cancelled, reason: "Cancelled by user")
    }

    public func retryTask(id: String) {
        guard let index = tasks.firstIndex(where: { $0.id == id }) else { return }
        tasks[index].state = .pending
        tasks[index].errorReason = nil
    }

    public func addRoutine(name: String, expression: String) throws {
        try CronRoutineParser.validate(expression: expression)
        let routine = CronRoutineModel(
            id: UUID().uuidString,
            name: name,
            cronExpression: expression
        )
        routines.append(routine)
    }

    public var filteredTasks: [AgentTaskModel] {
        switch selectedFilter {
        case .all:
            return tasks
        case .active:
            return tasks.filter { $0.state == .running || $0.state == .assigned || $0.state == .pending }
        case .waitingAuth:
            return tasks.filter { $0.state == .waitingAuth }
        case .scheduled:
            return tasks.filter { $0.cronExpression != nil }
        case .done:
            return tasks.filter { $0.state.isTerminal }
        }
    }
}

// MARK: - Presentation View

public struct TaskCenterView: View {
    @StateObject public var viewModel: TaskCenterViewModel

    public init(viewModel: TaskCenterViewModel) {
        _viewModel = StateObject(wrappedValue: viewModel)
    }

    public var body: some View {
        VStack(spacing: 0) {
            // Filter Pills Bar
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 8) {
                    ForEach(TaskFilter.allCases) { filter in
                        Button(action: { viewModel.selectedFilter = filter }) {
                            Text(filter.rawValue)
                                .font(.system(size: 13, weight: .medium))
                                .padding(.horizontal, 12)
                                .padding(.vertical, 6)
                                .background(
                                    viewModel.selectedFilter == filter ?
                                    Color.accentColor :
                                    Color.taskSecondaryGroupedBackground,
                                    in: Capsule()
                                )
                                .foregroundStyle(
                                    viewModel.selectedFilter == filter ?
                                    Color.white :
                                    Color.primary
                                )
                        }
                    }
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 10)
            }

            // Tasks List
            List {
                if viewModel.filteredTasks.isEmpty && viewModel.routines.isEmpty {
                    VStack(spacing: 8) {
                        Image(systemName: "checklist")
                            .font(.system(size: 40))
                            .foregroundStyle(.secondary)
                        Text("No Tasks Found")
                            .font(.system(size: 16, weight: .semibold))
                            .foregroundStyle(.secondary)
                    }
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 40)
                    .listRowBackground(Color.clear)
                } else {
                    if !viewModel.filteredTasks.isEmpty {
                        Section("Tasks (\(viewModel.filteredTasks.count))") {
                            ForEach(viewModel.filteredTasks) { task in
                                taskRow(task)
                            }
                        }
                    }

                    if (viewModel.selectedFilter == .all || viewModel.selectedFilter == .scheduled) && !viewModel.routines.isEmpty {
                        Section("Cron Routines (\(viewModel.routines.count))") {
                            ForEach(viewModel.routines) { routine in
                                routineRow(routine)
                            }
                        }
                    }
                }
            }
            #if os(iOS)
            .listStyle(.insetGrouped)
            #endif
        }
        .navigationTitle("Global Task Center")
    }

    private func taskRow(_ task: AgentTaskModel) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text(task.title)
                    .font(.system(size: 15, weight: .medium))
                Spacer()
                taskStateBadge(task.state)
            }

            if let reason = task.errorReason {
                Text("Reason: \(reason)")
                    .font(.system(size: 12))
                    .foregroundStyle(.red)
            }

            if let cron = task.cronExpression {
                Text("Schedule: \(CronRoutineParser.humanize(expression: cron))")
                    .font(.system(size: 12))
                    .foregroundStyle(.secondary)
            }

            if !task.state.isTerminal {
                HStack {
                    Spacer()
                    Button("Cancel", role: .destructive) {
                        viewModel.cancelTask(id: task.id)
                    }
                    .font(.system(size: 12))
                    .buttonStyle(.bordered)
                    .controlSize(.mini)
                }
            } else if task.state == .failed {
                HStack {
                    Spacer()
                    Button("Retry") {
                        viewModel.retryTask(id: task.id)
                    }
                    .font(.system(size: 12))
                    .buttonStyle(.bordered)
                    .controlSize(.mini)
                }
            }
        }
        .padding(.vertical, 4)
    }

    private func routineRow(_ routine: CronRoutineModel) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(routine.name)
                .font(.system(size: 15, weight: .medium))
            Text(routine.humanSchedule)
                .font(.system(size: 13))
                .foregroundStyle(Color.accentColor)
            Text("Expression: \(routine.cronExpression)")
                .font(.system(size: 11, design: .monospaced))
                .foregroundStyle(.secondary)
        }
        .padding(.vertical, 2)
    }

    @ViewBuilder
    private func taskStateBadge(_ state: AgentTaskState) -> some View {
        let (color, text) = badgeStyle(for: state)
        Text(text)
            .font(.system(size: 11, weight: .semibold))
            .foregroundStyle(color)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(color.opacity(0.15), in: Capsule())
    }

    private func badgeStyle(for state: AgentTaskState) -> (Color, String) {
        switch state {
        case .pending: return (.secondary, "Pending")
        case .assigned: return (.blue, "Assigned")
        case .running: return (.indigo, "Running")
        case .waitingAuth: return (.orange, "Waiting Auth")
        case .paused: return (.yellow, "Paused")
        case .completed: return (.green, "Completed")
        case .failed: return (.red, "Failed")
        case .cancelled: return (.gray, "Cancelled")
        }
    }
}

private extension Color {
    static var taskSecondaryGroupedBackground: Color {
        #if canImport(UIKit)
        return Color(uiColor: .secondarySystemGroupedBackground)
        #else
        return Color.secondary.opacity(0.12)
        #endif
    }
}
