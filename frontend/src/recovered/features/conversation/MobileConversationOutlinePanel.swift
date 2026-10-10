import SwiftUI

internal enum MobileConversationOutlineKind: String, Equatable {
    case user
    case assistantText = "assistant-text"
    case thinking
    case sendMessage = "send-message"
    case toolCall = "tool-call"
}

internal struct MobileConversationOutlineItem: Identifiable, Equatable {
    let id: String
    let kind: MobileConversationOutlineKind
    let label: String
    let preview: String
    let detail: String
    let status: String?

    init?(json: [String: Any]) {
        guard let rawKind = json["kind"] as? String,
              let kind = MobileConversationOutlineKind(rawValue: rawKind),
              let id = json["id"] as? String,
              !id.isEmpty
        else { return nil }

        self.id = id
        self.kind = kind
        switch kind {
        case .user:
            guard let text = json["text"] as? String else { return nil }
            label = "You"
            preview = text.mobileOutlineFirstLine
            detail = text
            status = nil
        case .assistantText:
            guard let text = json["text"] as? String else { return nil }
            label = "Agent"
            preview = text.mobileOutlineFirstLine
            detail = text
            status = nil
        case .thinking:
            guard let text = json["text"] as? String else { return nil }
            if let duration = json["durationMs"], !(duration is NSNumber) { return nil }
            label = "Thinking"
            preview = text.mobileOutlineFirstLine
            detail = text
            status = nil
        case .sendMessage:
            guard let message = json["message"] as? [String: Any],
                  let messageType = message["type"] as? String
            else { return nil }
            if messageType == "text" {
                guard let content = message["content"] as? String else { return nil }
                label = "Message"
                preview = content.mobileOutlineFirstLine
                detail = content
            } else if messageType == "attachment" {
                guard let url = message["url"] as? String, !url.isEmpty else { return nil }
                if let alt = message["alt"], !(alt is String) { return nil }
                label = "Message"
                preview = url.mobileOutlineFirstLine
                detail = (message["alt"] as? String).flatMap { $0.isEmpty ? nil : $0 }
                    .map { "\($0)\n\(url)" } ?? url
            } else {
                return nil
            }
            status = nil
        case .toolCall:
            guard let name = json["name"] as? String,
                  !name.isEmpty,
                  let status = json["status"] as? String,
                  ["pending", "failed", "done"].contains(status)
            else { return nil }
            if let summary = json["summary"], !(summary is String) { return nil }
            label = mobileOutlineToolLabel(name)
            let detail = (json["summary"] as? String) ?? ""
            preview = detail.mobileOutlineFirstLine
            self.detail = detail
            self.status = status
        }
    }
}

private extension String {
    var mobileOutlineFirstLine: String {
        trimmingCharacters(in: .whitespacesAndNewlines)
            .split(whereSeparator: { $0.isNewline })
            .first
            .map(String.init) ?? ""
    }
}

internal func mobileOutlineToolLabel(_ name: String) -> String {
    let base = name.hasSuffix("ToolCall") ? String(name.dropLast(8)) : name
    guard !base.isEmpty else { return name }
    var output = ""
    for character in base {
        if let last = output.last,
           last.isLowercase || last.isNumber,
           character.isUppercase {
            output.append(" ")
        }
        output.append(character)
    }
    return output.prefix(1).uppercased() + String(output.dropFirst())
}

internal func decodeMobileConversationOutline(_ raw: Any) -> [MobileConversationOutlineItem]? {
    guard let rows = raw as? [[String: Any]] else { return nil }
    var decoded: [MobileConversationOutlineItem] = []
    decoded.reserveCapacity(rows.count)
    for row in rows {
        guard let item = MobileConversationOutlineItem(json: row) else { return nil }
        decoded.append(item)
    }
    return decoded
}

internal struct MobileConversationOutlineScope: Equatable, Sendable {
    let accountKey: String
    let parentAgentId: String
    let selectedAgentId: String
    let reconnectGeneration: Int
    let generation: UInt64

    func accepts(
        accountKey: String,
        parentAgentId: String,
        selectedAgentId: String,
        reconnectGeneration: Int,
        generation: UInt64
    ) -> Bool {
        self.accountKey == accountKey
            && self.parentAgentId == parentAgentId
            && self.selectedAgentId == selectedAgentId
            && self.reconnectGeneration == reconnectGeneration
            && self.generation == generation
    }
}

@MainActor
internal struct MobileConversationOutlinePanel: View {
    let agentId: String
    let agentName: String
    let accountKey: String
    let bridge: IOSPreloadBridge
    let reconnectGeneration: Int
    let historicalSubagents: [MobileBotSubagent]
    let onClose: () -> Void

    @State private var selectedAgentId: String
    @State private var runningSubagents: [MobileAsyncTask] = []
    @State private var items: [MobileConversationOutlineItem] = []
    @State private var loading = true
    @State private var errorText: String?
    @State private var expanded: Set<String> = []
    @State private var generation: UInt64 = 0

    init(
        agentId: String,
        agentName: String,
        accountKey: String,
        bridge: IOSPreloadBridge,
        reconnectGeneration: Int,
        historicalSubagents: [MobileBotSubagent] = [],
        onClose: @escaping () -> Void
    ) {
        self.agentId = agentId
        self.agentName = agentName
        self.accountKey = accountKey
        self.bridge = bridge
        self.reconnectGeneration = reconnectGeneration
        self.historicalSubagents = historicalSubagents
        self.onClose = onClose
        _selectedAgentId = State(initialValue: agentId)
    }

    private var mergedSubagents: [MobileBotSubagent] {
        var order: [String] = []
        var byId: [String: MobileBotSubagent] = [:]
        for subagent in historicalSubagents {
            if byId[subagent.subagentId] == nil { order.append(subagent.subagentId) }
            byId[subagent.subagentId] = subagent
        }
        for task in runningSubagents where task.kind == "subagent" {
            if byId[task.id] == nil { order.append(task.id) }
            byId[task.id] = .init(
                subagentId: task.id,
                subagentType: task.subagentType ?? "subagent",
                title: task.label,
                status: "running"
            )
        }
        return order.compactMap { byId[$0] }
    }

    private var selectedRunningSubagent: MobileBotSubagent? {
        mergedSubagents.first { $0.subagentId == selectedAgentId && $0.status == "running" }
    }

    private var selectedHistoricalSubagent: MobileBotSubagent? {
        mergedSubagents.first { $0.subagentId == selectedAgentId && $0.status != "running" }
    }

    private var refreshInterval: Duration {
        selectedRunningSubagent == nil ? .seconds(5) : .seconds(2)
    }

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                if !mergedSubagents.isEmpty {
                    ScrollView(.horizontal, showsIndicators: false) {
                        HStack(spacing: 8) {
                            outlineTab(id: agentId, label: agentName, status: nil)
                            ForEach(mergedSubagents) { subagent in
                                let title = subagent.title.trimmingCharacters(in: .whitespacesAndNewlines)
                                outlineTab(
                                    id: subagent.subagentId,
                                    label: title.isEmpty
                                        ? subagent.subagentType
                                        : "\(subagent.subagentType): \(title)",
                                    status: subagent.status
                                )
                            }
                        }
                        .padding(.horizontal, 12)
                        .padding(.vertical, 8)
                    }
                    Divider()
                }

                if loading && items.isEmpty {
                    ProgressView("Loading full conversation…")
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                } else if items.isEmpty, let errorText {
                    ContentUnavailableView(
                        "Full conversation unavailable",
                        systemImage: "exclamationmark.triangle",
                        description: Text(errorText)
                    )
                } else if items.isEmpty {
                    ContentUnavailableView(
                        "No conversation activity yet",
                        systemImage: "list.bullet.rectangle"
                    )
                } else {
                    List(items) { item in
                        DisclosureGroup(
                            isExpanded: Binding(
                                get: { expanded.contains(item.id) },
                                set: { value in
                                    if value { expanded.insert(item.id) }
                                    else { expanded.remove(item.id) }
                                }
                            )
                        ) {
                            if item.detail.isEmpty {
                                Text("No additional details.")
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                            } else {
                                Text(item.detail)
                                    .font(.caption.monospaced())
                                    .textSelection(.enabled)
                            }
                        } label: {
                            HStack(spacing: 9) {
                                Image(systemName: iconName(item))
                                    .foregroundStyle(item.status == "failed" ? .red : .secondary)
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(item.label).font(.subheadline.weight(.semibold))
                                    if !item.preview.isEmpty {
                                        Text(item.preview)
                                            .font(.caption)
                                            .foregroundStyle(.secondary)
                                            .lineLimit(1)
                                    }
                                }
                                Spacer()
                                if item.status == "pending" {
                                    ProgressView().controlSize(.mini)
                                }
                            }
                        }
                        .accessibilityIdentifier("mobile-outline-item-\(item.id)")
                    }
                    .listStyle(.plain)
                    .refreshable {
                        generation &+= 1
                        await refreshAll()
                    }
                    .overlay(alignment: .top) {
                        if errorText != nil {
                            Text("Showing the last known conversation outline.")
                                .font(.caption)
                                .padding(.horizontal, 12)
                                .padding(.vertical, 6)
                                .background(.thinMaterial, in: Capsule())
                                .padding(.top, 8)
                        }
                    }
                }
            }
            .navigationTitle("Full conversation")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Close", action: onClose)
                }
            }
        }
        .accessibilityIdentifier("mobile-conversation-outline")
        .task(id: "\(accountKey)|\(agentId)|\(reconnectGeneration)") {
            generation &+= 1
            selectedAgentId = agentId
            expanded.removeAll()
            await refreshAll()
            while !Task.isCancelled {
                let interval = refreshInterval
                do { try await Task.sleep(for: interval) }
                catch { return }
                if selectedHistoricalSubagent == nil {
                    await refreshAll()
                } else {
                    await refreshSubagents()
                }
            }
        }
        .onChange(of: selectedAgentId) { _, _ in
            generation &+= 1
            items = []
            errorText = nil
            expanded.removeAll()
            Task { await refreshOutline() }
        }
    }

    @ViewBuilder
    private func outlineTab(id: String, label: String, status: String?) -> some View {
        Button {
            selectedAgentId = id
        } label: {
            HStack(spacing: 5) {
                if let status {
                    Circle()
                        .fill(status == "running" ? Color.accentColor : status == "done" ? Color.green : Color.red)
                        .frame(width: 6, height: 6)
                        .accessibilityHidden(true)
                }
                Text(label)
                    .font(.caption.weight(.semibold))
                    .lineLimit(1)
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 7)
            .background(
                selectedAgentId == id ? Color.accentColor.opacity(0.16) : Color.black.opacity(0.05),
                in: Capsule()
            )
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("mobile-outline-tab-\(id)")
    }

    private func iconName(_ item: MobileConversationOutlineItem) -> String {
        switch item.kind {
        case .user: "person.crop.circle"
        case .assistantText: "sparkles"
        case .thinking: "brain.head.profile"
        case .sendMessage: "paperplane"
        case .toolCall:
            item.status == "failed" ? "xmark.circle" : "wrench.and.screwdriver"
        }
    }

    private func currentScope() -> MobileConversationOutlineScope {
        .init(
            accountKey: accountKey,
            parentAgentId: agentId,
            selectedAgentId: selectedAgentId,
            reconnectGeneration: reconnectGeneration,
            generation: generation
        )
    }

    private func scopeStillCurrent(_ scope: MobileConversationOutlineScope) -> Bool {
        scope.accepts(
            accountKey: accountKey,
            parentAgentId: agentId,
            selectedAgentId: selectedAgentId,
            reconnectGeneration: reconnectGeneration,
            generation: generation
        ) && !Task.isCancelled
    }

    private func refreshAll() async {
        async let tasks: Void = refreshSubagents()
        async let outline: Void = refreshOutline()
        _ = await (tasks, outline)
    }

    private func refreshSubagents() async {
        let scope = currentScope()
        do {
            let result = try await bridge.request(method: "getAsyncTasks", params: ["id": agentId])
            guard scopeStillCurrent(scope),
                  let rows = result.value as? [[String: Any]]
            else { return }
            var decoded: [MobileAsyncTask] = []
            for row in rows {
                guard let task = MobileAsyncTask(json: row) else {
                    throw MahayanaCoordinator.CoordinatorError.invalidResponse
                }
                if task.kind == "subagent" { decoded.append(task) }
            }
            runningSubagents = decoded
            if selectedAgentId != agentId
                && !mergedSubagents.contains(where: { $0.subagentId == selectedAgentId }) {
                selectedAgentId = agentId
            }
        } catch is CancellationError {
            return
        } catch {
            // Outline content remains useful even when the additive subagent tab list is unavailable.
        }
    }

    private func refreshOutline() async {
        let scope = currentScope()
        if items.isEmpty { loading = true }
        do {
            let result = try await bridge.request(
                method: "getConversationOutline",
                params: ["id": selectedAgentId]
            )
            guard scopeStillCurrent(scope) else { return }
            guard let decoded = decodeMobileConversationOutline(result.value) else {
                throw MahayanaCoordinator.CoordinatorError.invalidResponse
            }
            items = decoded
            errorText = nil
        } catch is CancellationError {
            return
        } catch {
            guard scopeStillCurrent(scope) else { return }
            errorText = error.localizedDescription
        }
        guard scopeStillCurrent(scope) else { return }
        loading = false
    }
}
