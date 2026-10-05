import SwiftUI

internal struct MobileBotSharedRoomTrigger: View {
    let agent: MobileBotSummary
    let roster: [MobileBotSummary]
    let bridge: IOSPreloadBridge
    let accountScopeKey: String

    @State private var state: GrokMobileSharedRoomModel.SharingState?
    @State private var opened = false
    @State private var generation = 0
    @State private var refreshTask: Task<Void, Never>?

    private var snapshot: GrokMobileSharedRoomModel.Snapshot? {
        state.map {
            GrokMobileSharedRoomModel.snapshot(agent: agent, roster: roster, state: $0)
        }
    }

    var body: some View {
        Group {
            if let snapshot,
               snapshot.state.isEnabled,
               snapshot.room != nil {
                Section("共享房间") {
                    Button {
                        opened = true
                    } label: {
                        HStack {
                            Label("管理共享房间", systemImage: "person.2")
                            Spacer()
                            if !snapshot.requests.isEmpty {
                                Text("\(snapshot.requests.count)")
                                    .font(.caption.bold())
                                    .padding(.horizontal, 7)
                                    .padding(.vertical, 3)
                                    .background(.red.opacity(0.14), in: Capsule())
                            }
                        }
                    }
                    .accessibilityIdentifier("mobile-shared-room-open")
                }
            }
        }
        .task(id: lifecycleKey) { await refresh() }
        .onDisappear { invalidate() }
        .sheet(isPresented: $opened) {
            MobileBotSharedRoomSheet(
                agent: agent,
                roster: roster,
                bridge: bridge,
                accountScopeKey: accountScopeKey,
                initialState: state,
                onStateChanged: { state = $0 },
                onClose: { opened = false }
            )
        }
    }

    private var lifecycleKey: String {
        [accountScopeKey, agent.id].joined(separator: ":")
    }

    @MainActor
    private func refresh() async {
        generation += 1
        refreshTask?.cancel()
        let fence = GrokMobileSharedRoomModel.LifecycleFence(
            accountScopeKey: accountScopeKey,
            agentId: agent.id,
            generation: generation
        )
        let task = Task { @MainActor in
            do {
                let result = try await bridge.request(method: "sharing.state")
                try Task.checkCancellation()
                guard accepts(fence),
                      let projected = GrokMobileSharedRoomModel.projectSharingState(result.value)
                else { return }
                state = projected
            } catch is CancellationError {
                return
            } catch {
                guard accepts(fence) else { return }
                state = nil
            }
        }
        refreshTask = task
        await task.value
    }

    @MainActor
    private func accepts(_ fence: GrokMobileSharedRoomModel.LifecycleFence) -> Bool {
        GrokMobileSharedRoomModel.accepts(
            fence,
            accountScopeKey: accountScopeKey,
            agentId: agent.id,
            generation: generation
        )
    }

    @MainActor
    private func invalidate() {
        generation += 1
        refreshTask?.cancel()
        refreshTask = nil
    }
}

internal struct MobileBotSharedRoomSheet: View {
    let agent: MobileBotSummary
    let roster: [MobileBotSummary]
    let bridge: IOSPreloadBridge
    let accountScopeKey: String
    let initialState: GrokMobileSharedRoomModel.SharingState?
    let onStateChanged: (GrokMobileSharedRoomModel.SharingState) -> Void
    let onClose: () -> Void

    @State private var state: GrokMobileSharedRoomModel.SharingState?
    @State private var invite: GrokMobileSharedRoomModel.InviteResult?
    @State private var pendingKey: String?
    @State private var failure: String?
    @State private var generation = 0
    @State private var operationTask: Task<Void, Never>?

    init(
        agent: MobileBotSummary,
        roster: [MobileBotSummary],
        bridge: IOSPreloadBridge,
        accountScopeKey: String,
        initialState: GrokMobileSharedRoomModel.SharingState?,
        onStateChanged: @escaping (GrokMobileSharedRoomModel.SharingState) -> Void,
        onClose: @escaping () -> Void
    ) {
        self.agent = agent
        self.roster = roster
        self.bridge = bridge
        self.accountScopeKey = accountScopeKey
        self.initialState = initialState
        self.onStateChanged = onStateChanged
        self.onClose = onClose
        _state = State(initialValue: initialState)
    }

    private var snapshot: GrokMobileSharedRoomModel.Snapshot? {
        state.map {
            GrokMobileSharedRoomModel.snapshot(agent: agent, roster: roster, state: $0)
        }
    }

    var body: some View {
        NavigationStack {
            Form {
                if let snapshot, let room = snapshot.room {
                    Section("房间") {
                        LabeledContent("名称", value: room.name)
                        LabeledContent("成员", value: "\(room.members.count)")
                        if snapshot.isHost {
                            Button(pendingKey == "invite" ? "正在创建…" : "创建邀请链接") {
                                beginInvite(roomId: room.roomId)
                            }
                            .disabled(pendingKey != nil)
                            .accessibilityIdentifier("mobile-shared-room-invite")
                        }
                    }

                    if let invite {
                        Section("邀请") {
                            switch invite {
                            case let .ok(shareURL, expiresAtMs, _):
                                Text(shareURL)
                                    .textSelection(.enabled)
                                    .font(.footnote)
                                    .accessibilityIdentifier("mobile-shared-room-invite-url")
                                if let url = URL(string: shareURL) {
                                    ShareLink(item: url) {
                                        Label("分享邀请", systemImage: "square.and.arrow.up")
                                    }
                                }
                                Text(
                                    Date(timeIntervalSince1970: expiresAtMs / 1000),
                                    style: .relative
                                )
                                .font(.caption)
                                .foregroundStyle(.secondary)
                            case let .error(message):
                                Text(message).foregroundStyle(.red)
                            }
                        }
                    }

                    if snapshot.isHost, !snapshot.requests.isEmpty {
                        Section("加入请求") {
                            ForEach(snapshot.requests) { request in
                                VStack(alignment: .leading, spacing: 8) {
                                    Text(request.requesterName)
                                    HStack {
                                        Button("批准") {
                                            beginRespond(requestId: request.requestId, approved: true)
                                        }
                                        .disabled(pendingKey != nil)
                                        Button("拒绝", role: .destructive) {
                                            beginRespond(requestId: request.requestId, approved: false)
                                        }
                                        .disabled(pendingKey != nil)
                                    }
                                }
                                .accessibilityIdentifier("mobile-shared-room-request-\(request.requestId)")
                            }
                        }
                    }

                    if !snapshot.selfAgentIds.isEmpty {
                        Section("我的 Agent") {
                            ForEach(snapshot.selfAgentIds, id: \.self) { agentId in
                                let name = roster.first(where: { $0.id == agentId })?.name ?? agentId
                                HStack {
                                    Text(name)
                                    Spacer()
                                    if snapshot.isHost && snapshot.selfAgentIds.count > 1 {
                                        Button("移除", role: .destructive) {
                                            beginRemoveAgent(roomId: room.roomId, agentId: agentId)
                                        }
                                        .disabled(pendingKey != nil)
                                    }
                                }
                            }
                        }
                    }

                    if snapshot.isHost, !snapshot.candidates.isEmpty {
                        Section("添加 Agent") {
                            ForEach(snapshot.candidates) { candidate in
                                Button {
                                    beginAddAgent(
                                        roomId: room.roomId,
                                        agentId: candidate.id,
                                        agentName: candidate.name
                                    )
                                } label: {
                                    Label(candidate.name, systemImage: "plus.circle")
                                }
                                .disabled(pendingKey != nil)
                            }
                        }
                    }

                    let removableHumans = room.members.filter {
                        $0.kind == .human && $0.authId != room.hostAuthId
                    }
                    if snapshot.isHost, !removableHumans.isEmpty {
                        Section("其他成员") {
                            ForEach(removableHumans) { member in
                                HStack {
                                    Text(member.displayName)
                                    Spacer()
                                    Button("移除", role: .destructive) {
                                        beginLeave(roomId: room.roomId, targetAuthId: member.authId)
                                    }
                                    .disabled(pendingKey != nil)
                                }
                            }
                        }
                    }

                    Section {
                        Button("离开共享房间", role: .destructive) {
                            beginLeave(roomId: room.roomId, targetAuthId: nil)
                        }
                        .disabled(pendingKey != nil)
                        .accessibilityIdentifier("mobile-shared-room-leave")
                    }
                } else if state == nil {
                    Section {
                        ProgressView("正在读取共享房间…")
                    }
                } else {
                    Section {
                        Text("这个 Agent 当前没有可管理的共享房间。")
                            .foregroundStyle(.secondary)
                    }
                }

                if let failure {
                    Section {
                        Text(failure)
                            .foregroundStyle(.red)
                            .accessibilityIdentifier("mobile-shared-room-error")
                    }
                }
            }
            .navigationTitle("共享房间")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("完成", action: onClose)
                        .disabled(pendingKey != nil)
                }
            }
        }
        .accessibilityIdentifier("mobile-shared-room")
        .task(id: lifecycleKey) { await refresh() }
        .onDisappear { invalidate() }
    }

    private var lifecycleKey: String {
        [accountScopeKey, agent.id].joined(separator: ":")
    }

    @MainActor
    private func refresh() async {
        startStateOperation(key: "refresh", method: "sharing.state", params: [:])
    }

    @MainActor
    private func beginInvite(roomId: String) {
        guard pendingKey == nil else { return }
        let fence = makeFence()
        pendingKey = "invite"
        failure = nil
        invite = nil
        operationTask?.cancel()
        operationTask = Task { @MainActor in
            defer { finish(fence) }
            do {
                let result = try await bridge.request(
                    method: "sharing.createRoomInvite",
                    params: ["roomId": roomId]
                )
                try Task.checkCancellation()
                guard accepts(fence),
                      let projected = GrokMobileSharedRoomModel.projectInviteResult(result.value)
                else { return }
                invite = projected
                if case let .error(message) = projected {
                    failure = message
                }
            } catch is CancellationError {
                return
            } catch {
                guard accepts(fence) else { return }
                failure = error.localizedDescription
            }
        }
    }

    @MainActor
    private func beginRespond(requestId: String, approved: Bool) {
        startStateOperation(
            key: "request:\(requestId)",
            method: "sharing.respondToRoomJoinRequest",
            params: ["requestId": requestId, "isApproved": approved]
        )
    }

    @MainActor
    private func beginAddAgent(roomId: String, agentId: String, agentName: String) {
        startStateOperation(
            key: "agent:\(agentId)",
            method: "sharing.addOwnAgent",
            params: ["roomId": roomId, "agentId": agentId, "agentName": agentName]
        )
    }

    @MainActor
    private func beginRemoveAgent(roomId: String, agentId: String) {
        startStateOperation(
            key: "agent:\(agentId)",
            method: "sharing.removeOwnAgent",
            params: ["roomId": roomId, "agentId": agentId]
        )
    }

    @MainActor
    private func beginLeave(roomId: String, targetAuthId: String?) {
        var params: [String: Any] = ["roomId": roomId]
        if let targetAuthId {
            params["targetAuthId"] = targetAuthId
        }
        startStateOperation(
            key: targetAuthId.map { "member:\($0)" } ?? "leave",
            method: "sharing.leaveRoom",
            params: params
        )
    }

    @MainActor
    private func startStateOperation(
        key: String,
        method: String,
        params: [String: Any]
    ) {
        guard pendingKey == nil || key == "refresh" else { return }
        let fence = makeFence()
        pendingKey = key
        failure = nil
        operationTask?.cancel()
        operationTask = Task { @MainActor in
            defer { finish(fence) }
            do {
                let result = try await bridge.request(method: method, params: params)
                try Task.checkCancellation()
                guard accepts(fence),
                      let projected = GrokMobileSharedRoomModel.projectSharingState(result.value)
                else {
                    if accepts(fence) {
                        failure = "共享房间状态响应无效。"
                    }
                    return
                }
                state = projected
                onStateChanged(projected)
            } catch is CancellationError {
                return
            } catch {
                guard accepts(fence) else { return }
                failure = error.localizedDescription
            }
        }
    }

    @MainActor
    private func makeFence() -> GrokMobileSharedRoomModel.LifecycleFence {
        GrokMobileSharedRoomModel.LifecycleFence(
            accountScopeKey: accountScopeKey,
            agentId: agent.id,
            generation: generation
        )
    }

    @MainActor
    private func accepts(_ fence: GrokMobileSharedRoomModel.LifecycleFence) -> Bool {
        GrokMobileSharedRoomModel.accepts(
            fence,
            accountScopeKey: accountScopeKey,
            agentId: agent.id,
            generation: generation
        )
    }

    @MainActor
    private func finish(_ fence: GrokMobileSharedRoomModel.LifecycleFence) {
        guard accepts(fence) else { return }
        pendingKey = nil
        operationTask = nil
    }

    @MainActor
    private func invalidate() {
        generation += 1
        operationTask?.cancel()
        operationTask = nil
        pendingKey = nil
        failure = nil
    }
}
