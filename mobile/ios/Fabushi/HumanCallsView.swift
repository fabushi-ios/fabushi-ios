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

internal struct HumanCallTransportLease: Equatable, Sendable {
    let userId: String
    let deviceId: String
    let role: String
    let isOwner: Bool
    let claimAvailable: Bool
    let generation: Int

    init?(raw: [String: Any]) {
        guard
            let userId = raw["userId"] as? String,
            !userId.isEmpty,
            let deviceId = raw["deviceId"] as? String,
            !deviceId.isEmpty,
            let role = raw["role"] as? String,
            role == "creator" || role == "peer",
            let isOwner = raw["isOwner"] as? Bool,
            let claimAvailable = raw["claimAvailable"] as? Bool
        else { return nil }
        self.userId = userId
        self.deviceId = deviceId
        self.role = role
        self.isOwner = isOwner
        self.claimAvailable = claimAvailable
        self.generation = (raw["generation"] as? NSNumber)?.intValue ?? 0
    }
}

internal struct HumanCallSignalRecord {
    let callId: String
    let generation: Int
    let seq: Int
    let senderDeviceId: String
    let kind: String
    let payload: [String: Any]

    init?(raw: [String: Any]) {
        guard
            let callId = raw["callId"] as? String,
            !callId.isEmpty,
            let senderDeviceId = raw["senderDeviceId"] as? String,
            !senderDeviceId.isEmpty,
            let kind = raw["kind"] as? String,
            ["offer", "answer", "candidate"].contains(kind),
            let payload = raw["payload"] as? [String: Any]
        else { return nil }
        self.callId = callId
        self.generation = (raw["generation"] as? NSNumber)?.intValue ?? 0
        self.seq = (raw["seq"] as? NSNumber)?.intValue ?? 0
        self.senderDeviceId = senderDeviceId
        self.kind = kind
        self.payload = payload
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
    @State private var peerConnection: HumanCallPeerConnection?
    @State private var activeMediaCallId: String?
    @State private var activeLease: HumanCallTransportLease?
    @State private var appliedSignalSeq = 0
    @State private var mediaState = "idle"
    @State private var muted = false
    @State private var cameraEnabled = false
    @State private var recovering = false

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
                } else {
                    let callable = conversations.filter { conversation in
                        conversation.kind != .channel
                            && conversation.kind != .savedMessages
                            && conversation.participants.contains { $0.actorId != messaging.currentActorId }
                    }
                    if !callable.isEmpty {
                        Section("发起通话") {
                            ForEach(callable.prefix(50)) { conversation in
                                HStack {
                                    VStack(alignment: .leading, spacing: 2) {
                                        Text(conversation.title)
                                        Text(conversation.kind.label)
                                            .font(.caption)
                                            .foregroundStyle(.secondary)
                                    }
                                    Spacer()
                                    Button {
                                        Task { await start(conversation, video: false) }
                                    } label: {
                                        Image(systemName: "phone.fill")
                                    }
                                    .disabled(actionCallId != nil)
                                    .accessibilityLabel("语音呼叫 \(conversation.title)")
                                    .accessibilityIdentifier("human-call-start-voice-\(conversation.id)")
                                    Button {
                                        Task { await start(conversation, video: true) }
                                    } label: {
                                        Image(systemName: "video.fill")
                                    }
                                    .disabled(actionCallId != nil)
                                    .accessibilityLabel("视频呼叫 \(conversation.title)")
                                    .accessibilityIdentifier("human-call-start-video-\(conversation.id)")
                                }
                            }
                        }
                    }
                }

                if bridge != nil && loading && calls.isEmpty {
                    HStack {
                        Spacer()
                        ProgressView("正在加载通话…")
                        Spacer()
                    }
                } else if bridge != nil && calls.isEmpty {
                    ContentUnavailableView(
                        "暂无通话",
                        systemImage: "phone",
                        description: Text("收到或建立的 Human 通话会显示在这里。")
                    )
                } else if bridge != nil {
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
            .task(id: activeMediaCallId) {
                guard activeMediaCallId != nil else { return }
                while !Task.isCancelled, activeMediaCallId != nil {
                    await pollActiveMedia()
                    do {
                        try await Task.sleep(for: .seconds(1))
                    } catch {
                        break
                    }
                }
            }
        }
        .onDisappear {
            closeActiveMedia()
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
                        Button("语音接听") {
                            Task { await accept(call, video: false) }
                        }
                        .buttonStyle(.borderedProminent)
                        .disabled(actionCallId != nil)
                        .accessibilityIdentifier("human-call-accept-\(call.id)")
                        Button("视频接听") {
                            Task { await accept(call, video: true) }
                        }
                        .buttonStyle(.bordered)
                        .disabled(actionCallId != nil)
                        .accessibilityIdentifier("human-call-accept-video-\(call.id)")
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
                if activeMediaCallId == call.id {
                    HStack(spacing: 10) {
                        Button(muted ? "取消静音" : "静音") {
                            Task { await setMuted(!muted, call: call) }
                        }
                        .buttonStyle(.bordered)
                        .accessibilityIdentifier("human-call-mute-\(call.id)")
                        Button(cameraEnabled ? "关闭摄像头" : "开启摄像头") {
                            Task { await setCameraEnabled(!cameraEnabled, call: call) }
                        }
                        .buttonStyle(.bordered)
                        .accessibilityIdentifier("human-call-camera-\(call.id)")
                        Text(mediaState)
                            .font(.caption2)
                            .foregroundStyle(.secondary)
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

    private func start(_ conversation: ConversationSummary, video: Bool) async {
        guard let bridge else { return }
        let participantIds = conversation.participants
            .map(\.actorId)
            .filter { !$0.isEmpty }
        guard participantIds.contains(where: { $0 != messaging.currentActorId }) else {
            errorText = "此会话没有可呼叫的其他参与者。"
            return
        }

        actionCallId = "start:\(conversation.id)"
        defer { actionCallId = nil }

        do {
            let permissions = await mediaPort.requestPermissions(audio: true, video: video)
            guard permissions.microphone == .granted else {
                throw HumanCallPeerConnection.Failure.microphoneUnavailable
            }
            if video, permissions.camera != .granted {
                throw HumanCallPeerConnection.Failure.cameraUnavailable
            }

            let createdResult = try await bridge.request(
                method: "createCallSession",
                params: [
                    "scopeId": conversation.id,
                    "participantIds": participantIds,
                ]
            )
            guard
                let createdRaw = createdResult.value as? [String: Any],
                let created = HumanCallSessionRecord(raw: createdRaw)
            else {
                throw NSError(
                    domain: "Fabushi.HumanCall",
                    code: 2,
                    userInfo: [NSLocalizedDescriptionKey: "通话服务返回了无效的会话。"]
                )
            }

            let ringingResult = try await bridge.request(
                method: "transitionCallSession",
                params: [
                    "callId": created.id,
                    "generation": created.generation,
                    "action": "ring",
                ]
            )
            let ringing = (ringingResult.value as? [String: Any])
                .flatMap(HumanCallSessionRecord.init(raw:)) ?? created

            _ = try await bridge.request(
                method: "updateCallMedia",
                params: [
                    "callId": ringing.id,
                    "generation": ringing.generation,
                    "mediaCapabilities": [
                        "audio": true,
                        "video": video,
                        "screenShare": false,
                    ],
                ]
            )
            try await configureMedia(for: ringing, enableVideo: video)
            errorText = nil
            await reload()
        } catch {
            closeActiveMedia()
            errorText = "发起通话失败：\(error.localizedDescription)"
        }
    }

    private func accept(_ call: HumanCallSessionRecord, video: Bool) async {
        guard let bridge else { return }
        actionCallId = call.id
        defer { actionCallId = nil }

        do {
            let permissions = await mediaPort.requestPermissions(audio: true, video: video)
            guard permissions.microphone == .granted else {
                throw HumanCallPeerConnection.Failure.microphoneUnavailable
            }
            if video, permissions.camera != .granted {
                throw HumanCallPeerConnection.Failure.cameraUnavailable
            }

            let transitionResult = try await bridge.request(
                method: "transitionCallSession",
                params: [
                    "callId": call.id,
                    "generation": call.generation,
                    "action": "accept",
                ]
            )
            let accepted = (transitionResult.value as? [String: Any])
                .flatMap(HumanCallSessionRecord.init(raw:)) ?? call

            _ = try await bridge.request(
                method: "updateCallMedia",
                params: [
                    "callId": accepted.id,
                    "generation": accepted.generation,
                    "mediaCapabilities": [
                        "audio": true,
                        "video": video,
                        "screenShare": false,
                    ],
                ]
            )

            try await configureMedia(for: accepted, enableVideo: video)
            errorText = nil
            await reload()
        } catch {
            closeActiveMedia()
            errorText = "接听失败：\(error.localizedDescription)"
        }
    }

    private func configureMedia(
        for call: HumanCallSessionRecord,
        enableVideo: Bool,
        iceRestart: Bool = false
    ) async throws {
        guard let bridge else { return }
        closeActiveMedia()

        let leaseResult = try await bridge.request(
            method: "getCallTransportLease",
            params: ["callId": call.id]
        )
        guard
            let leaseRaw = leaseResult.value as? [String: Any],
            let lease = HumanCallTransportLease(raw: leaseRaw),
            lease.isOwner || lease.claimAvailable
        else {
            throw NSError(
                domain: "Fabushi.HumanCall",
                code: 1,
                userInfo: [NSLocalizedDescriptionKey: "此通话正在另一台设备上进行。"]
            )
        }

        let iceResult = try await bridge.request(method: "getCallIceServers", params: [:])
        guard
            let iceRaw = iceResult.value as? [String: Any],
            let rows = iceRaw["iceServers"] as? [[String: Any]]
        else {
            throw HumanCallPeerConnection.Failure.malformedICE
        }
        let servers = rows.compactMap(HumanCallPeerConnection.IceServer.init(raw:))
        guard !servers.isEmpty else {
            throw HumanCallPeerConnection.Failure.malformedICE
        }

        let peer = HumanCallPeerConnection()
        peer.onLocalCandidate = { payload in
            Task { @MainActor in
                await sendSignal(
                    call: call,
                    lease: lease,
                    kind: "candidate",
                    payload: payload
                )
            }
        }
        peer.onStateChange = { state in
            Task { @MainActor in
                switch state {
                case .new:
                    mediaState = "准备中"
                case .connecting:
                    mediaState = "正在连接"
                case .connected:
                    mediaState = "通话中"
                    await transitionSilently(call, action: "connected")
                case .disconnected:
                    mediaState = "正在重连"
                    if !recovering {
                        await recoverMedia(from: call)
                    }
                case .failed:
                    mediaState = "正在重连"
                    if !recovering {
                        await recoverMedia(from: call)
                    }
                case .closed:
                    mediaState = "已关闭"
                }
            }
        }
        try await peer.configure(iceServers: servers, enableVideo: enableVideo)

        peerConnection = peer
        activeMediaCallId = call.id
        activeLease = lease
        appliedSignalSeq = 0
        mediaState = "正在连接"
        muted = false
        cameraEnabled = enableVideo

        if lease.role == "creator" {
            let offer = try await peer.makeOffer(iceRestart: iceRestart)
            await sendSignal(call: call, lease: lease, kind: "offer", payload: offer)
        }
        await consumeSignals(for: call)
    }

    private func recoverMedia(from call: HumanCallSessionRecord) async {
        guard
            let bridge,
            activeMediaCallId == call.id,
            !recovering
        else { return }

        recovering = true
        defer { recovering = false }

        let restoreMuted = muted
        let restoreVideo = cameraEnabled
        do {
            let reconnectResult = try await bridge.request(
                method: "transitionCallSession",
                params: [
                    "callId": call.id,
                    "generation": call.generation,
                    "action": "reconnect",
                ]
            )
            let reconnecting = (reconnectResult.value as? [String: Any])
                .flatMap(HumanCallSessionRecord.init(raw:)) ?? call
            let resumeResult = try await bridge.request(
                method: "transitionCallSession",
                params: [
                    "callId": reconnecting.id,
                    "generation": reconnecting.generation,
                    "action": "resume",
                ]
            )
            let resumed = (resumeResult.value as? [String: Any])
                .flatMap(HumanCallSessionRecord.init(raw:)) ?? reconnecting

            try await configureMedia(
                for: resumed,
                enableVideo: restoreVideo,
                iceRestart: true
            )
            if restoreMuted {
                peerConnection?.setMuted(true)
                muted = true
                await updateMediaState(call: resumed)
            }
            mediaState = "正在重连"
            await reload()
        } catch {
            mediaState = "连接失败"
            errorText = "通话重连失败：\(error.localizedDescription)"
            _ = try? await bridge.request(
                method: "transitionCallSession",
                params: [
                    "callId": call.id,
                    "generation": call.generation,
                    "action": "fail",
                    "terminalReason": "media-reconnect-failed",
                ]
            )
            closeActiveMedia()
        }
    }

    private func pollActiveMedia() async {
        guard
            let callId = activeMediaCallId,
            let call = calls.first(where: { $0.id == callId }),
            !call.isTerminal
        else {
            if activeMediaCallId != nil {
                closeActiveMedia()
            }
            return
        }
        await consumeSignals(for: call)
    }

    private func consumeSignals(for call: HumanCallSessionRecord) async {
        guard let bridge, let peerConnection, let lease = activeLease else { return }
        do {
            let result = try await bridge.request(
                method: "listCallSignals",
                params: [
                    "callId": call.id,
                    "generation": call.generation,
                    "afterSeq": appliedSignalSeq,
                    "limit": 100,
                ]
            )
            guard let rows = result.value as? [[String: Any]] else { return }
            for raw in rows {
                guard
                    let signal = HumanCallSignalRecord(raw: raw),
                    signal.generation == call.generation
                else { continue }
                appliedSignalSeq = max(appliedSignalSeq, signal.seq)
                guard signal.senderDeviceId != lease.deviceId else { continue }

                switch signal.kind {
                case "offer":
                    let answer = try await peerConnection.applyOffer(signal.payload)
                    await sendSignal(call: call, lease: lease, kind: "answer", payload: answer)
                case "answer":
                    try await peerConnection.applyAnswer(signal.payload)
                case "candidate":
                    try await peerConnection.applyCandidate(signal.payload)
                default:
                    break
                }
            }
        } catch {
            errorText = "通话信令失败：\(error.localizedDescription)"
        }
    }

    private func sendSignal(
        call: HumanCallSessionRecord,
        lease: HumanCallTransportLease,
        kind: String,
        payload: [String: Any]
    ) async {
        guard let bridge else { return }
        do {
            let result = try await bridge.request(
                method: "sendCallSignal",
                params: [
                    "callId": call.id,
                    "generation": call.generation,
                    "seq": max(appliedSignalSeq + 1, call.signalSeq + 1),
                    "senderDeviceId": lease.deviceId,
                    "kind": kind,
                    "payload": payload,
                ]
            )
            if let raw = result.value as? [String: Any],
               let signal = HumanCallSignalRecord(raw: raw)
            {
                appliedSignalSeq = max(appliedSignalSeq, signal.seq)
            }
        } catch {
            errorText = "通话信令发送失败：\(error.localizedDescription)"
        }
    }

    private func setMuted(_ next: Bool, call: HumanCallSessionRecord) async {
        peerConnection?.setMuted(next)
        muted = next
        await updateMediaState(call: call)
    }

    private func setCameraEnabled(_ next: Bool, call: HumanCallSessionRecord) async {
        do {
            if next {
                let permissions = await mediaPort.requestPermissions(audio: false, video: true)
                guard permissions.camera == .granted else {
                    throw HumanCallPeerConnection.Failure.cameraUnavailable
                }
            }
            try await peerConnection?.setCameraEnabled(next)
            cameraEnabled = next
            await updateMediaState(call: call)
        } catch {
            errorText = "摄像头切换失败：\(error.localizedDescription)"
        }
    }

    private func updateMediaState(call: HumanCallSessionRecord) async {
        guard let bridge else { return }
        do {
            _ = try await bridge.request(
                method: "updateCallMedia",
                params: [
                    "callId": call.id,
                    "generation": call.generation,
                    "mediaCapabilities": [
                        "audio": !muted,
                        "video": cameraEnabled,
                        "screenShare": false,
                    ],
                ]
            )
        } catch {
            errorText = "通话媒体状态同步失败：\(error.localizedDescription)"
        }
    }

    private func transitionSilently(_ call: HumanCallSessionRecord, action: String) async {
        guard let bridge else { return }
        _ = try? await bridge.request(
            method: "transitionCallSession",
            params: [
                "callId": call.id,
                "generation": call.generation,
                "action": action,
            ]
        )
    }

    private func closeActiveMedia() {
        peerConnection?.close()
        peerConnection = nil
        activeMediaCallId = nil
        activeLease = nil
        appliedSignalSeq = 0
        mediaState = "idle"
        muted = false
        cameraEnabled = false
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
            if action == "decline" || action == "hangup" || action == "fail" {
                closeActiveMedia()
            }
            errorText = nil
            await reload()
        } catch {
            errorText = "通话操作失败：\(error.localizedDescription)"
        }
    }
}
