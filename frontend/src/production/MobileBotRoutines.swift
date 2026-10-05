import Combine
import Foundation
import SwiftUI

internal struct MobileBotRoutine: Identifiable, Equatable {
    let id: String
    let agentId: String
    let name: String
    let prompt: String
    let schedule: String
    let isEnabled: Bool
    let createdAtMs: Int64
    let lastRunAtMs: Int64?
    let nextRunAtMs: Int64?
}

internal struct MobileBotRoutineSpec: Equatable {
    let name: String
    let prompt: String
    let schedule: String
    let isEnabled: Bool
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
              let createdAtMs = integer(row["createdAtMs"])
        else {
            return nil
        }

        guard optionalInteger(row["lastRunAtMs"]) != .invalid,
              optionalInteger(row["nextRunAtMs"]) != .invalid
        else {
            return nil
        }

        return MobileBotRoutine(
            id: id,
            agentId: agentId,
            name: name,
            prompt: prompt,
            schedule: schedule,
            isEnabled: isEnabled,
            createdAtMs: createdAtMs,
            lastRunAtMs: optionalInteger(row["lastRunAtMs"]).value,
            nextRunAtMs: optionalInteger(row["nextRunAtMs"]).value
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
            "schedule": spec.schedule,
            "enabled": spec.isEnabled,
        ]
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

    init(source: MobileBotRoutinesSource) {
        self.source = source
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
        defer {
            if generation == operationGeneration {
                runPending.remove(automationId)
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
    }

    private static func isCapabilityUnavailable(_ error: Error) -> Bool {
        error.localizedDescription.contains("source/capability-unavailable")
    }
}

@MainActor
internal struct MobileBotRoutinesSection: View {
    let agentId: String
    let accountScopeKey: String

    @StateObject private var controller: MobileBotRoutinesController

    init(
        agentId: String,
        bridge: IOSPreloadBridge,
        accountScopeKey: String
    ) {
        self.agentId = agentId
        self.accountScopeKey = accountScopeKey
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
        }
        .task(id: "\(accountScopeKey)|\(agentId)") {
            controller.reset()
            await controller.refresh(agentId: agentId)
        }
        .onDisappear {
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
                        Text(routine.schedule)
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
