import SwiftUI

internal struct HumanCallSessionRecord: Identifiable, Equatable, Sendable {
    let id: String
    let scopeId: String
    let creatorId: String
    let state: String
    let generation: Int
    let signalSeq: Int
    let participantIds: [String]
    let mediaCapabilities: [String: String]
    let terminalReason: String?
    let updatedAtMs: Int64

    init?(raw: [String: Any]) {
        guard
            let id = raw["id"] as? String,
            !id.isEmpty,
            let scopeId = raw["scopeId"] as? String,
            !scopeId.isEmpty,
            let creatorId = raw["creatorId"] as? String,
            !creatorId.isEmpty,
            let state = raw["state"] as? String,
            !state.isEmpty
        else { return nil }

        self.id = id
        self.scopeId = scopeId
        self.creatorId = creatorId
        self.state = state
        self.generation = (raw["generation"] as? NSNumber)?.intValue ?? 0
        self.signalSeq = (raw["signalSeq"] as? NSNumber)?.intValue ?? 0
        self.participantIds = raw["participantIds"] as? [String] ?? []
        self.mediaCapabilities = (raw["mediaCapabilities"] as? [String: Any] ?? [:])
            .reduce(into: [:]) { result, pair in
                if let value = pair.value as? String {
                    result[pair.key] = value
                } else if let value = pair.value as? Bool {
                    result[pair.key] = value ? "granted" : "denied"
                }
            }
        self.terminalReason = raw["terminalReason"] as? String
        self.updatedAtMs = (raw["updatedAtMs"] as? NSNumber)?.int64Value ?? 0
    }

    var isTerminal: Bool {
        state == "ended" || state == "failed"
    }

    var canAccept: Bool {
        state == "invited" || state == "ringing"
    }

    var canDecline: Bool {
        state == "invited" || state == "ringing"
    }

    var canHangUp: Bool {
        !isTerminal
    }

    var stateLabel: String {
        switch state {
        case "invited": "已邀请"
        case "ringing": "响铃中"
        case "negotiating": "正在连接"
        case "connected": "通话中"
        case "reconnecting": "正在重连"
        case "ended": "已结束"
        case "failed": "失败"
        default: state
        }
    }
}

@MainActor
internal struct HumanCallsView: View {
    let messaging: MessagingModel
    let bridge: IOSPreloadBridge?

    private var conversations: [ConversationSummary] { messaging.conversations }
    let onClose: () -> Void

    @State private var calls: [HumanCallSessionRecord] = []
    @State private var loading = false
    @State private var actionCallId: String?
    @State private var errorText: String?
    @State private var refreshGeneration = 0
    @State private var mediaPort = HumanCallMediaPort()

    var body: some View {
        NavigationStack {
            List {
                if let errorText {
                    Section {
                        Label(errorText, systemImage: "exclamationmark.triangle.fill")
                            .foregroundStyle(.red)
                    }
                }

                if bridge == nil {
                    ContentUnavailableView(
                        "通话运行时不可用",
                        systemImage: "phone.down.fill",
                        description: Text("当前页面没有连接到 Coordinator/Host，无法读取通话状态。")
                    )
                } else if loading && calls.isEmpty {
                    HStack {
                        Spacer()
                        ProgressView("正在加载通话…")
                        Spacer()
                    }
                } else if calls.isEmpty {
                    ContentUnavailableView(
                        "暂无通话",
                        systemImage: "phone",
                        description: Text("收到或建立的 Human 通话会显示在这里。")
                    )
                } else {
                    Section("最近通话") {
                        ForEach(calls) { call in
                            callRow(call)
                        }
                    }
                }
            }
            .navigationTitle("通话")
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("完成") { onClose() }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Button {
                        refreshGeneration &+= 1
                    } label: {
                        Image(systemName: "arrow.clockwise")
                    }
                    .disabled(loading)
                    .accessibilityIdentifier("human-calls-refresh")
                }
            }
            .task(id: refreshGeneration) {
                await reload()
            }
        }
        .accessibilityIdentifier("human-calls-surface")
    }

    @ViewBuilder
    private func callRow(_ call: HumanCallSessionRecord) -> some View {
        VStack(alignment: .leading, spacing: 9) {
            HStack(alignment: .firstTextBaseline) {
                VStack(alignment: .leading, spacing: 3) {
                    Text(conversationTitle(for: call.scopeId))
                        .font(.headline)
                    Text(call.stateLabel)
                        .font(.caption)
                        .foregroundStyle(call.isTerminal ? Color.secondary : Color.accentColor)
                }
                Spacer()
                if actionCallId == call.id {
                    ProgressView()
                }
            }

            Text(participantSummary(call))
                .font(.caption)
                .foregroundStyle(.secondary)
                .lineLimit(2)

            if let reason = call.terminalReason, !reason.isEmpty {
                Text(reason)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }

            if !call.isTerminal {
                HStack(spacing: 10) {
                    if call.canAccept {
                        Button("接听") {
                            Task { await accept(call) }
                        }
                        .buttonStyle(.borderedProminent)
                        .disabled(actionCallId != nil)
                        .accessibilityIdentifier("human-call-accept-\(call.id)")
                    }
                    if call.canDecline {
                        Button("拒绝", role: .destructive) {
                            Task { await transition(call, action: "decline") }
                        }
                        .buttonStyle(.bordered)
                        .disabled(actionCallId != nil)
                        .accessibilityIdentifier("human-call-decline-\(call.id)")
                    } else if call.canHangUp {
                        Button("挂断", role: .destructive) {
                            Task { await transition(call, action: "hangup") }
                        }
                        .buttonStyle(.bordered)
                        .disabled(actionCallId != nil)
                        .accessibilityIdentifier("human-call-hangup-\(call.id)")
                    }
                }
            }
        }
        .padding(.vertical, 5)
    }

    private func conversationTitle(for scopeId: String) -> String {
        conversations.first(where: { $0.id == scopeId })?.title ?? scopeId
    }

    private func participantSummary(_ call: HumanCallSessionRecord) -> String {
        let participants = call.participantIds.isEmpty ? "无参与者信息" : call.participantIds.joined(separator: " · ")
        return "参与者：\(participants)"
    }

    private func reload() async {
        guard let bridge else {
            calls = []
            return
        }
        loading = true
        defer { loading = false }

        var collected: [HumanCallSessionRecord] = []
        var failures: [String] = []

        do {
            let result = try await bridge.request(
                method: "syncHumanCalls",
                params: [:]
            )
            if let rawCalls = result.value as? [[String: Any]] {
                collected.append(contentsOf: rawCalls.compactMap(HumanCallSessionRecord.init(raw:)))
            }
            await messaging.refresh()
        } catch is CancellationError {
            return
        } catch {
            failures.append("remote-call-sync")
        }

        for conversation in conversations {
            do {
                let result = try await bridge.request(
                    method: "listCallSessions",
                    params: [
                        "scopeId": conversation.id,
                        "limit": 50,
                    ]
                )
                guard let rawCalls = result.value as? [[String: Any]] else {
                    failures.append(conversation.title)
                    continue
                }
                collected.append(contentsOf: rawCalls.compactMap(HumanCallSessionRecord.init(raw:)))
            } catch is CancellationError {
                return
            } catch {
                failures.append(conversation.title)
            }
        }

        var deduplicated: [String: HumanCallSessionRecord] = [:]
        for call in collected {
            if let existing = deduplicated[call.id], existing.updatedAtMs > call.updatedAtMs {
                continue
            }
            deduplicated[call.id] = call
        }
        calls = deduplicated.values.sorted {
            if $0.updatedAtMs == $1.updatedAtMs { return $0.id > $1.id }
            return $0.updatedAtMs > $1.updatedAtMs
        }

        if !failures.isEmpty, calls.isEmpty {
            errorText = "无法读取通话状态，请稍后重试。"
        } else {
            errorText = nil
        }
    }

    private func accept(_ call: HumanCallSessionRecord) async {
        guard let bridge else { return }
        actionCallId = call.id
        defer { actionCallId = nil }

        do {
            let permissions = await mediaPort.requestPermissions(audio: true, video: true)
            _ = try await bridge.request(
                method: "updateCallMedia",
                params: [
                    "callId": call.id,
                    "generation": call.generation,
                    "mediaCapabilities": [
                        "microphone": permissions.microphone.rawValue,
                        "camera": permissions.camera.rawValue,
                    ],
                ]
            )
            _ = try await bridge.request(
                method: "transitionCallSession",
                params: [
                    "callId": call.id,
                    "generation": call.generation,
                    "action": "accept",
                ]
            )
            errorText = nil
            await reload()
        } catch {
            errorText = "接听失败：\(error.localizedDescription)"
        }
    }

    private func transition(_ call: HumanCallSessionRecord, action: String) async {
        guard let bridge else { return }
        actionCallId = call.id
        defer { actionCallId = nil }

        do {
            _ = try await bridge.request(
                method: "transitionCallSession",
                params: [
                    "callId": call.id,
                    "generation": call.generation,
                    "action": action,
                ]
            )
            errorText = nil
            await reload()
        } catch {
            errorText = "通话操作失败：\(error.localizedDescription)"
        }
    }
}
