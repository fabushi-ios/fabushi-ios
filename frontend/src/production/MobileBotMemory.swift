import SwiftUI

internal enum MobileMemoryScope: String, CaseIterable, Identifiable {
    case agent
    case user
    case project

    var id: String { rawValue }

    var title: String {
        switch self {
        case .agent: "Agent"
        case .user: "User"
        case .project: "Project"
        }
    }
}

internal enum MobileMemoryKind: String, CaseIterable, Identifiable {
    case profile
    case log

    var id: String { rawValue }

    var title: String {
        switch self {
        case .profile: "长期"
        case .log: "近期"
        }
    }
}

internal struct MobileMemoryRecord: Identifiable, Equatable {
    let id: String
    let content: String
    let createdAtMs: Int64
    let kind: MobileMemoryKind
}

internal enum MobileBotMemoryModel {
    static func normalizedProject(_ value: String, scope: MobileMemoryScope) -> String? {
        guard scope == .project else { return nil }
        let normalized = value.trimmingCharacters(in: .whitespacesAndNewlines)
        return normalized.isEmpty ? nil : normalized
    }

    static func command(
        type: String,
        agentId: String,
        scope: MobileMemoryScope,
        project: String?,
        requestId: String,
        additional: [String: Any] = [:]
    ) -> [String: Any] {
        var command: [String: Any] = [
            "type": type,
            "requestId": requestId,
            "agentId": agentId,
            "scope": scope.rawValue,
        ]
        if scope == .project, let project, !project.isEmpty {
            command["project"] = project
        }
        for (key, value) in additional {
            command[key] = value
        }
        return command
    }

    static func listCommand(
        agentId: String,
        scope: MobileMemoryScope,
        project: String?,
        requestId: String,
        limit: Int = 200
    ) -> [String: Any] {
        command(
            type: "memory.scopedList",
            agentId: agentId,
            scope: scope,
            project: project,
            requestId: requestId,
            additional: ["limit": min(max(limit, 1), 1_000)]
        )
    }

    static func addCommand(
        agentId: String,
        scope: MobileMemoryScope,
        project: String?,
        content: String,
        kind: MobileMemoryKind,
        requestId: String
    ) -> [String: Any]? {
        let content = content.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !content.isEmpty else { return nil }
        return command(
            type: "memory.scopedAdd",
            agentId: agentId,
            scope: scope,
            project: project,
            requestId: requestId,
            additional: ["content": content, "kind": kind.rawValue]
        )
    }

    static func removeCommand(
        agentId: String,
        scope: MobileMemoryScope,
        project: String?,
        memoryId: String,
        requestId: String
    ) -> [String: Any] {
        command(
            type: "memory.scopedRemove",
            agentId: agentId,
            scope: scope,
            project: project,
            requestId: requestId,
            additional: ["id": memoryId]
        )
    }

    static func clearCommand(
        agentId: String,
        scope: MobileMemoryScope,
        project: String?,
        requestId: String
    ) -> [String: Any] {
        command(
            type: "memory.scopedClear",
            agentId: agentId,
            scope: scope,
            project: project,
            requestId: requestId
        )
    }

    static func matches(
        event: [String: Any],
        type: String,
        agentId: String,
        scope: MobileMemoryScope,
        project: String?
    ) -> Bool {
        guard event["type"] as? String == type,
              event["agentId"] as? String == agentId,
              event["scope"] as? String == scope.rawValue
        else { return false }
        if scope == .project {
            return event["project"] as? String == project
        }
        return event["project"] == nil
    }

    static func records(from event: [String: Any]) -> [MobileMemoryRecord]? {
        guard let rows = event["memories"] as? [[String: Any]] else { return nil }
        var parsed: [MobileMemoryRecord] = []
        for row in rows {
            guard let id = row["id"] as? String,
                  !id.isEmpty,
                  let content = row["content"] as? String,
                  let rawKind = row["kind"] as? String,
                  let kind = MobileMemoryKind(rawValue: rawKind)
            else { return nil }
            let createdAt: Int64
            if let value = row["createdAt"] as? Int64 {
                createdAt = value
            } else if let value = row["createdAt"] as? Int {
                createdAt = Int64(value)
            } else if let value = row["createdAt"] as? NSNumber {
                createdAt = value.int64Value
            } else {
                return nil
            }
            parsed.append(.init(id: id, content: content, createdAtMs: createdAt, kind: kind))
        }
        return parsed
    }
}

internal struct MobileBotMemorySection: View {
    let agentId: String
    let bridge: IOSPreloadBridge
    let accountScopeKey: String
    let reconnectGeneration: Int

    @State private var scope: MobileMemoryScope = .agent
    @State private var projectDraft = ""
    @State private var kind: MobileMemoryKind = .profile
    @State private var contentDraft = ""
    @State private var records: [MobileMemoryRecord] = []
    @State private var loading = false
    @State private var mutating = false
    @State private var failure: String?
    @State private var generation = 0
    @State private var clearConfirmation = false

    private var project: String? {
        MobileBotMemoryModel.normalizedProject(projectDraft, scope: scope)
    }

    private var canRequest: Bool {
        scope != .project || project != nil
    }

    private var taskIdentity: String {
        [
            agentId,
            accountScopeKey,
            String(reconnectGeneration),
            scope.rawValue,
            project ?? "",
        ].joined(separator: "|")
    }

    var body: some View {
        Section {
            Picker("作用域", selection: $scope) {
                ForEach(MobileMemoryScope.allCases) { scope in
                    Text(scope.title).tag(scope)
                }
            }
            .pickerStyle(.segmented)
            .disabled(mutating)
            .accessibilityIdentifier("mobile-agent-memory-scope")

            if scope == .project {
                TextField("已加入的 Project slug", text: $projectDraft)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .disabled(mutating)
                    .accessibilityIdentifier("mobile-agent-memory-project")
            }

            if loading {
                ProgressView("读取 Memory…")
                    .accessibilityIdentifier("mobile-agent-memory-loading")
            } else if !canRequest {
                Text("输入当前 Agent 已加入的 Project slug 后读取共享 Memory。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            } else if records.isEmpty, failure == nil {
                Text("当前作用域没有 Memory。")
                    .foregroundStyle(.secondary)
            }

            ForEach(records) { record in
                HStack(alignment: .top, spacing: 10) {
                    VStack(alignment: .leading, spacing: 3) {
                        Text(record.content)
                            .textSelection(.enabled)
                        Text(record.kind.title)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    Spacer(minLength: 8)
                    Button(role: .destructive) {
                        Task { await remove(record) }
                    } label: {
                        Image(systemName: "trash")
                    }
                    .disabled(mutating)
                    .accessibilityLabel("删除 Memory")
                    .accessibilityIdentifier("mobile-agent-memory-remove-(record.id)")
                }
            }

            Picker("类型", selection: $kind) {
                ForEach(MobileMemoryKind.allCases) { kind in
                    Text(kind.title).tag(kind)
                }
            }
            .disabled(mutating || !canRequest)

            TextField("新增 Memory", text: $contentDraft, axis: .vertical)
                .lineLimit(2...5)
                .disabled(mutating || !canRequest)
                .accessibilityIdentifier("mobile-agent-memory-draft")

            HStack {
                Button(mutating ? "保存中…" : "添加") {
                    Task { await add() }
                }
                .disabled(
                    mutating
                        || !canRequest
                        || contentDraft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                )
                .accessibilityIdentifier("mobile-agent-memory-add")

                Spacer()

                Button("刷新") {
                    Task { await reload() }
                }
                .disabled(mutating || loading || !canRequest)
                .accessibilityIdentifier("mobile-agent-memory-refresh")

                Button("清空", role: .destructive) {
                    clearConfirmation = true
                }
                .disabled(mutating || records.isEmpty || !canRequest)
                .accessibilityIdentifier("mobile-agent-memory-clear")
            }

            if let failure {
                Text(failure)
                    .font(.caption)
                    .foregroundStyle(.red)
                    .accessibilityIdentifier("mobile-agent-memory-error")
            }
        } header: {
            Text("Memory")
        } footer: {
            switch scope {
            case .agent:
                Text("Agent Memory 仅属于当前 Agent；持久化与去重由 Rust Host 持有。")
            case .user:
                Text("User Memory 可由同一账号下的 Agents 共享；每条事实保留来源 Agent。")
            case .project:
                Text("Project Memory 仅允许当前 Agent 已加入的 Project；Host 会拒绝未加入或不安全的 slug。")
            }
        }
        .task(id: taskIdentity) {
            generation &+= 1
            await reload()
        }
        .confirmationDialog(
            "清空当前 Memory 作用域？",
            isPresented: $clearConfirmation,
            titleVisibility: .visible
        ) {
            Button("清空", role: .destructive) {
                Task { await clear() }
            }
            Button("取消", role: .cancel) {}
        } message: {
            Text("该操作会写入删除 tombstone，避免后台记忆合成自动恢复已明确删除的事实。")
        }
        .onDisappear {
            generation &+= 1
        }
    }

    @MainActor
    private func reload() async {
        let fence = generation
        guard canRequest else {
            records = []
            failure = nil
            loading = false
            return
        }
        loading = true
        failure = nil
        do {
            let requestId = "ios-mobile-memory-list-(UUID().uuidString.lowercased())"
            let command = MobileBotMemoryModel.listCommand(
                agentId: agentId,
                scope: scope,
                project: project,
                requestId: requestId
            )
            _ = try await bridge.request(
                method: "feature.execute",
                params: ["command": command]
            )
            let expectedScope = scope
            let expectedProject = project
            let result = try await bridge.receiveFeatureEvent(deadlineMilliseconds: 2_560) { event in
                MobileBotMemoryModel.matches(
                    event: event,
                    type: "memory.listed",
                    agentId: agentId,
                    scope: expectedScope,
                    project: expectedProject
                )
            }
            try Task.checkCancellation()
            guard fence == generation,
                  let event = result.value as? [String: Any],
                  let next = MobileBotMemoryModel.records(from: event)
            else { return }
            records = next
        } catch is CancellationError {
            return
        } catch {
            guard fence == generation else { return }
            records = []
            failure = "读取 Memory 失败：(error.localizedDescription)"
        }
        if fence == generation {
            loading = false
        }
    }

    @MainActor
    private func add() async {
        guard !mutating,
              canRequest,
              let command = MobileBotMemoryModel.addCommand(
                agentId: agentId,
                scope: scope,
                project: project,
                content: contentDraft,
                kind: kind,
                requestId: "ios-mobile-memory-add-(UUID().uuidString.lowercased())"
              )
        else { return }
        let savedDraft = contentDraft
        let success = await mutate(command)
        if success, contentDraft == savedDraft {
            contentDraft = ""
        }
    }

    @MainActor
    private func remove(_ record: MobileMemoryRecord) async {
        guard !mutating, canRequest else { return }
        _ = await mutate(
            MobileBotMemoryModel.removeCommand(
                agentId: agentId,
                scope: scope,
                project: project,
                memoryId: record.id,
                requestId: "ios-mobile-memory-remove-(UUID().uuidString.lowercased())"
            )
        )
    }

    @MainActor
    private func clear() async {
        guard !mutating, canRequest else { return }
        _ = await mutate(
            MobileBotMemoryModel.clearCommand(
                agentId: agentId,
                scope: scope,
                project: project,
                requestId: "ios-mobile-memory-clear-(UUID().uuidString.lowercased())"
            )
        )
    }

    @MainActor
    private func mutate(_ command: [String: Any]) async -> Bool {
        let fence = generation
        let expectedScope = scope
        let expectedProject = project
        mutating = true
        failure = nil
        do {
            _ = try await bridge.request(
                method: "feature.execute",
                params: ["command": command]
            )
            _ = try await bridge.receiveFeatureEvent(deadlineMilliseconds: 2_560) { event in
                MobileBotMemoryModel.matches(
                    event: event,
                    type: "memory.changed",
                    agentId: agentId,
                    scope: expectedScope,
                    project: expectedProject
                )
            }
            try Task.checkCancellation()
            guard fence == generation else { return false }
            mutating = false
            await reload()
            return true
        } catch is CancellationError {
            return false
        } catch {
            guard fence == generation else { return false }
            mutating = false
            failure = "更新 Memory 失败：(error.localizedDescription)"
            return false
        }
    }
}
