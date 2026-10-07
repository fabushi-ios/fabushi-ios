import SwiftUI

internal struct MobileAsyncTask: Identifiable, Equatable {
    let kind: String
    let id: String
    let label: String
    let startedAtMs: Double
    let detail: String?
    let subagentType: String?

    init?(json: [String: Any]) {
        guard let kind = json["kind"] as? String,
              ["subagent", "shell", "cloud-agent"].contains(kind),
              let id = json["id"] as? String,
              !id.isEmpty,
              let label = json["label"] as? String,
              !label.isEmpty,
              (json["status"] as? String) == "running",
              let startedAtMs = json["startedAtMs"] as? Double,
              startedAtMs.isFinite
        else { return nil }
        if let detail = json["detail"], !(detail is String) { return nil }
        if let subagentType = json["subagentType"], !(subagentType is String) { return nil }
        self.kind = kind
        self.id = id
        self.label = label
        self.startedAtMs = startedAtMs
        self.detail = json["detail"] as? String
        self.subagentType = json["subagentType"] as? String
    }
}

@MainActor
internal struct MobileAsyncTasksPanel: View {
    let agentId: String
    let agentName: String
    let bridge: IOSPreloadBridge
    let onClose: () -> Void

    @State private var tasks: [MobileAsyncTask] = []
    @State private var loading = true
    @State private var errorText: String?
    @State private var generation: UInt64 = 0
    @State private var now = Date()

    private static let refreshInterval: Duration = .seconds(30)

    var body: some View {
        NavigationStack {
            Group {
                if loading && tasks.isEmpty {
                    ProgressView("Loading async tasks…")
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                } else if let errorText, tasks.isEmpty {
                    ContentUnavailableView(
                        "Async tasks unavailable",
                        systemImage: "exclamationmark.triangle",
                        description: Text(errorText)
                    )
                } else if tasks.isEmpty {
                    ContentUnavailableView(
                        "No async tasks in progress",
                        systemImage: "clock",
                        description: Text("Background subagents, shell work, and cloud-agent work will appear here.")
                    )
                } else {
                    List(tasks) { task in
                        HStack(spacing: 12) {
                            Image(systemName: iconName(for: task.kind))
                                .frame(width: 24)
                                .accessibilityHidden(true)
                            VStack(alignment: .leading, spacing: 3) {
                                Text(task.label)
                                    .font(.body.weight(.medium))
                                Text(taskMetadata(task))
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                            }
                            Spacer(minLength: 8)
                            Text(relativeTime(task.startedAtMs))
                                .font(.caption.monospacedDigit())
                                .foregroundStyle(.secondary)
                        }
                        .accessibilityElement(children: .combine)
                        .accessibilityIdentifier("mobile-async-task-(task.kind)-(task.id)")
                    }
                    .refreshable { await refresh() }
                    .overlay(alignment: .top) {
                        if errorText != nil {
                            Text("Showing the last known async-task snapshot.")
                                .font(.caption)
                                .padding(.horizontal, 12)
                                .padding(.vertical, 6)
                                .background(.thinMaterial, in: Capsule())
                                .padding(.top, 8)
                        }
                    }
                }
            }
            .navigationTitle("Async tasks · \(agentName)")
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Close", action: onClose)
                }
            }
        }
        .accessibilityIdentifier("mobile-async-tasks-panel")
        .task(id: agentId) {
            generation &+= 1
            let ownedGeneration = generation
            await refresh(expectedGeneration: ownedGeneration)
            while !Task.isCancelled {
                do { try await Task.sleep(for: Self.refreshInterval) }
                catch { return }
                guard generation == ownedGeneration else { return }
                now = Date()
                await refresh(expectedGeneration: ownedGeneration)
            }
        }
    }

    private func refresh(expectedGeneration: UInt64? = nil) async {
        let ownedGeneration = expectedGeneration ?? generation
        loading = true
        do {
            let result = try await bridge.request(method: "getAsyncTasks", params: ["id": agentId])
            guard generation == ownedGeneration, !Task.isCancelled else { return }
            guard let rows = result.value as? [[String: Any]] else {
                throw MahayanaCoordinator.CoordinatorError.invalidResponse
            }
            var decoded: [MobileAsyncTask] = []
            decoded.reserveCapacity(rows.count)
            for row in rows {
                guard let task = MobileAsyncTask(json: row) else {
                    throw MahayanaCoordinator.CoordinatorError.invalidResponse
                }
                decoded.append(task)
            }
            tasks = decoded
            errorText = nil
        } catch is CancellationError {
            return
        } catch {
            guard generation == ownedGeneration else { return }
            errorText = error.localizedDescription
        }
        guard generation == ownedGeneration else { return }
        loading = false
        now = Date()
    }

    private func iconName(for kind: String) -> String {
        switch kind {
        case "subagent": "person.2"
        case "shell": "terminal"
        case "cloud-agent": "icloud.and.arrow.up"
        default: "clock"
        }
    }

    private func taskMetadata(_ task: MobileAsyncTask) -> String {
        let kind: String
        switch task.kind {
        case "subagent": kind = "Subagent"
        case "shell": kind = "Shell"
        case "cloud-agent": kind = "Cloud agent"
        default: kind = task.kind
        }
        if let detail = task.detail?.trimmingCharacters(in: .whitespacesAndNewlines), !detail.isEmpty {
            return "(kind) · (detail)"
        }
        return kind
    }

    private func relativeTime(_ timestampMs: Double) -> String {
        guard timestampMs > 0, timestampMs.isFinite else { return "" }
        let elapsed = max(0, now.timeIntervalSince1970 - timestampMs / 1_000)
        if elapsed < 60 { return "now" }
        let minutes = Int(elapsed / 60)
        if minutes < 60 { return "(minutes)m ago" }
        let hours = minutes / 60
        if hours < 24 { return "(hours)h ago" }
        let days = hours / 24
        if days < 30 { return "(days)d ago" }
        let months = days / 30
        if months < 12 { return "(months)mo ago" }
        return "(months / 12)y ago"
    }
}
