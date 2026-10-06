import SwiftUI

internal func isMobileBotVisibleAssistantCompletion(
    _ event: [String: Any],
    operationId: String
) -> Bool {
    guard event["type"] as? String == "chat.message" else { return false }
    let eventOperationId = event["operationId"] as? String ?? operationId
    guard eventOperationId == operationId, event["role"] as? String != "user" else { return false }
    let text = event["text"] as? String ?? ""
    let attachment = event["attachment"] as? [String: Any]
    return !text.isEmpty || attachment != nil
}

internal struct MobileBotChat: View {
    let bot: MobileBotSummary
    let bridge: IOSPreloadBridge
    let model: MarketplaceModel
    let appAgentSurface: FabushiAppAgentSurface
    let onClose: () -> Void
    let onOpenSettings: () -> Void
    let onOpenAutomation: (String) -> Void

    @Binding var draft: String
    @Binding var entries: [MobileChatMessage]
    @State private var busy = false
    @State private var activeOperationId: String?
    @State private var errorText: String?
    @State private var openedMiniApp = false
    @State private var replyTargetId: String?
    @State private var replyIsFork = false
    @State private var voiceRecorder = VoiceRecorder()
    @State private var voiceTranscriber = OfflineSpeechTranscriber()
    @State private var transcribingVoice = false
    @State private var voiceInputGeneration = 0

    var body: some View {
        VStack(spacing: 0) {
            chatHeader
            Divider().opacity(0.35)
            transcriptList
            replyBanner
            voiceStatusBanner
            composer
        }
        .background(Color(red: 0.985, green: 0.985, blue: 0.975))
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("mobile-bot-chat")
        .task(id: semanticFingerprint) { publishAppAgentSurface() }
        .onChange(of: bot.id) { _, _ in cancelVoiceInput() }
        .onDisappear { cancelVoiceInput() }
        .fullScreenCover(isPresented: $openedMiniApp) {
            miniAppCover
        }
    }

    private var chatHeader: some View {
        HStack(spacing: 12) {
            Button(action: onClose) {
                Image(systemName: "chevron.left")
                    .font(.system(size: 17, weight: .semibold))
                    .foregroundStyle(.black)
                    .frame(width: 38, height: 38)
                    .background(Color.black.opacity(0.045), in: Circle())
            }
            Spacer()
            Button(action: onOpenSettings) {
                HStack(spacing: 8) {
                    ClothGhostAvatar(botId: bot.id, size: 28, active: busy)
                    Text(bot.name).font(.system(size: 17, weight: .semibold))
                }
                .padding(.horizontal, 13).padding(.vertical, 7)
                .background(.white, in: Capsule())
                .shadow(color: .black.opacity(0.08), radius: 12, y: 3)
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Bot settings")
            .accessibilityIdentifier("mobile-bot-settings")
            Spacer()
            Image(systemName: "desktopcomputer")
                .font(.system(size: 16, weight: .medium))
                .frame(width: 38, height: 38)
                .background(Color.black.opacity(0.045), in: Circle())
        }
        .padding(.horizontal, 14).padding(.vertical, 8)
        .background(Color.white.opacity(0.97))
    }

    private var transcriptList: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 7) {
                    if entries.isEmpty {
                        VStack(spacing: 13) {
                            ClothGhostAvatar(botId: bot.id, size: 82)
                            Text(bot.name).font(.title2.bold())
                            if !bot.description.isEmpty {
                                Text(bot.description)
                                    .font(.subheadline)
                                    .foregroundStyle(.secondary)
                                    .multilineTextAlignment(.center)
                            }
                        }
                        .frame(maxWidth: .infinity)
                        .padding(.top, 96)
                        .padding(.horizontal, 30)
                    }

                    ForEach(entries) { entry in
                        transcript(entry)
                            .id(entry.id)
                    }
                    if let errorText {
                        Text(errorText)
                            .font(.caption)
                            .foregroundStyle(.red)
                            .padding(.top, 4)
                            .accessibilityIdentifier("mobile-bot-error")
                    }
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 18)
            }
            .background(Color(red: 0.985, green: 0.985, blue: 0.975))
            .onChange(of: entries.count) { _, _ in
                if let last = entries.last {
                    withAnimation(.easeOut(duration: 0.16)) {
                        proxy.scrollTo(last.id, anchor: .bottom)
                    }
                }
            }
        }
    }

    @ViewBuilder
    private var replyBanner: some View {
        if let replyTargetId {
            HStack(spacing: 8) {
                Image(systemName: replyIsFork ? "arrow.triangle.branch" : "arrowshape.turn.up.left")
                Text(replyIsFork ? "Fork reply · \(replyTargetId)" : "Replying · \(replyTargetId)")
                    .font(.caption)
                    .lineLimit(1)
                Spacer()
                Button {
                    self.replyTargetId = nil
                    replyIsFork = false
                } label: {
                    Image(systemName: "xmark.circle.fill")
                }
            }
            .foregroundStyle(.secondary)
            .padding(.horizontal, 16)
            .padding(.top, 7)
        }
    }

    @ViewBuilder
    private var voiceStatusBanner: some View {
        if voiceRecorder.isRecording || transcribingVoice {
            HStack(spacing: 9) {
                if transcribingVoice {
                    ProgressView().controlSize(.small)
                    Text("正在离线转写…").font(.caption.weight(.semibold))
                } else {
                    Circle().fill(Color.red).frame(width: 8, height: 8)
                    Text("正在录音 \(voiceRecorder.elapsedSeconds / 60):\(String(format: "%02d", voiceRecorder.elapsedSeconds % 60))")
                        .font(.caption.weight(.semibold))
                }
                Spacer()
                Button("取消") { cancelVoiceInput() }
                    .font(.caption.weight(.semibold))
                    .disabled(transcribingVoice)
            }
            .padding(.horizontal, 16)
            .padding(.top, 7)
        }
    }

    private var composer: some View {
        HStack(alignment: .bottom, spacing: 8) {
            TextField("Message", text: $draft, axis: .vertical)
                .lineLimit(1...5)
                .padding(.horizontal, 14)
                .padding(.vertical, 11)
                .background(Color.black.opacity(0.055), in: RoundedRectangle(cornerRadius: 18, style: .continuous))
                .onSubmit {
                    if !busy {
                        Task { await send() }
                    }
                }
                .accessibilityIdentifier("mobile-bot-draft")

            miniAppButton
            voiceInputButton
            sendButton
        }
        .padding(.horizontal, 12)
        .padding(.top, 8)
        .padding(.bottom, 10)
        .background(.ultraThinMaterial)
    }

    @ViewBuilder
    private var miniAppButton: some View {
        if bot.miniAppId == GlobalDharmaMiniAppBridge.globalDharmaId {
            Button {
                openedMiniApp = true
            } label: {
                Text(bot.menuButtonText ?? "打开应用")
                    .font(.caption.bold())
                    .lineLimit(1)
                    .padding(.horizontal, 10)
                    .frame(height: 39)
                    .background(Color.black.opacity(0.075), in: Capsule())
            }
            .buttonStyle(.plain)
            .accessibilityLabel(bot.menuButtonText ?? "打开应用")
            .accessibilityIdentifier("mobile-bot-open-miniapp")
        }
    }

    @ViewBuilder
    private var voiceInputButton: some View {
        if !busy && draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            Button {
                if voiceRecorder.isRecording {
                    Task { await finishVoiceInput() }
                } else {
                    Task { await startVoiceInput() }
                }
            } label: {
                Image(systemName: voiceRecorder.isRecording ? "stop.fill" : "mic.fill")
                    .font(.system(size: 16, weight: .bold))
                    .foregroundStyle(.white)
                    .frame(width: 39, height: 39)
                    .background(voiceRecorder.isRecording ? Color.red : Color.black, in: Circle())
            }
            .disabled(transcribingVoice)
            .accessibilityIdentifier(voiceRecorder.isRecording ? "mobile-bot-voice-stop" : "mobile-bot-voice-start")
        }
    }

    private var sendButton: some View {
        Button {
            if busy {
                Task { await stop() }
            } else {
                Task { await send() }
            }
        } label: {
            Image(systemName: busy ? "stop.fill" : "arrow.up")
                .font(.system(size: 16, weight: .bold))
                .foregroundStyle(.white)
                .frame(width: 39, height: 39)
                .background(busy ? Color.red : Color.black, in: Circle())
        }
        .disabled(!busy && draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
        .accessibilityIdentifier(busy ? "mobile-bot-stop" : "mobile-bot-send")
    }

    @ViewBuilder
    private var miniAppCover: some View {
        if let miniAppId = bot.miniAppId,
           let plugin = model.plugins.first(where: { $0.pluginId == miniAppId }) {
            MiniAppWebMcpSurface(plugin: plugin, model: model)
        } else {
            VStack(spacing: 12) {
                ProgressView()
                Text("正在加载应用…")
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
            }
            .task {
                if model.plugins.first(where: { $0.pluginId == bot.miniAppId }) == nil {
                    await model.refresh()
                }
            }
        }
    }

    private var semanticFingerprint: String {
        let botIdFingerprint: String = bot.id
        let botNameFingerprint: String = bot.name
        let botDescriptionFingerprint: String = bot.description
        let botTitleFingerprint: String = bot.title ?? ""
        let notifyOnUpdatesFingerprint: String = String(bot.notifyOnUpdatesEnabled)
        let miniAppIdFingerprint: String = bot.miniAppId ?? ""
        let menuButtonTextFingerprint: String = bot.menuButtonText ?? ""
        let draftFingerprint: String = draft
        let busyFingerprint: String = String(busy)
        let openedMiniAppFingerprint: String = String(openedMiniApp)
        let activeOperationFingerprint: String = activeOperationId ?? ""
        let errorFingerprint: String = errorText ?? ""

        let entryFingerprints: [String] = entries.map { entry -> String in
            let entryId: String = "\(entry.id)"
            let entryKind: String = "\(entry.kind.rawValue)"
            let entryRole: String = "\(entry.role.rawValue)"
            return "\(entryId):\(entryKind):\(entryRole)"
        }
        let entriesFingerprint: String = entryFingerprints.joined(separator: ",")

        let fields: [String] = [
            botIdFingerprint,
            botNameFingerprint,
            botDescriptionFingerprint,
            botTitleFingerprint,
            notifyOnUpdatesFingerprint,
            miniAppIdFingerprint,
            menuButtonTextFingerprint,
            draftFingerprint,
            busyFingerprint,
            openedMiniAppFingerprint,
            activeOperationFingerprint,
            errorFingerprint,
            entriesFingerprint,
        ]
        return fields.joined(separator: "|")
    }

    @MainActor
    private func publishAppAgentSurface() {
        var elements: [FabushiAppAgentSurface.Element] = [
            .init(agentId: "mobile-bot-chat", role: "application", name: "Bot \(String(bot.name.prefix(160)))"),
            .init(agentId: "mobile-bot-close", role: "button", name: "关闭 Bot 对话"),
            .init(agentId: "mobile-bot-settings", role: "button", name: "Bot 设置"),
            .init(agentId: "mobile-bot-draft", role: "textbox", name: "Bot 消息"),
        ]
        let sendId = busy ? "mobile-bot-stop" : "mobile-bot-send"
        elements.append(.init(
            agentId: sendId,
            role: "button",
            name: busy ? "停止 Bot" : "发送 Bot 消息",
            enabled: busy || !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        ))
        if bot.miniAppId == GlobalDharmaMiniAppBridge.globalDharmaId {
            elements.append(.init(
                agentId: "mobile-bot-open-miniapp",
                role: "button",
                name: bot.menuButtonText ?? "打开应用",
                enabled: !openedMiniApp
            ))
        }
        if errorText != nil {
            elements.append(.init(agentId: "mobile-bot-error", role: "status", name: "Bot 或 Mini App 调用失败"))
        }
        for entry in entries.suffix(50) {
            let id = Self.semanticId("mobile-bot-entry-\(entry.id)")
            let roleName = entry.role == .user ? "用户消息"
                : entry.kind == .handoff ? "等待用户接管"
                : entry.kind == .action ? "Bot 动作"
                : entry.kind == .thinking ? "Bot 思考"
                : entry.kind == .notice ? "通知"
                : entry.kind == .permissionRequest ? "权限请求记录"
                : entry.kind == .timelineEvent ? "时间线事件"
                : "Bot 消息"
            elements.append(.init(agentId: id, role: "log", name: roleName))
        }
        for entry in entries where entry.kind == .handoff && entry.actionStatus == "pending" {
            let completeId = Self.semanticId("mobile-bot-handoff-complete-\(entry.id)")
            let dismissId = Self.semanticId("mobile-bot-handoff-dismiss-\(entry.id)")
            elements.append(.init(agentId: completeId, role: "button", name: "已完成并归还控制"))
            elements.append(.init(agentId: dismissId, role: "button", name: "无法完成此步骤"))
        }
        var actions: [String: FabushiAppAgentSurface.Action] = [
            "mobile-bot-close": .init(allowed: ["invoke"]) { _ in onClose() },
            "mobile-bot-settings": .init(allowed: ["invoke"]) { _ in onOpenSettings() },
            "mobile-bot-draft": .init(allowed: ["setValue"]) { value in draft = value ?? "" },
        ]
        actions[sendId] = .init(allowed: ["invoke"]) { _ in
            if busy { Task { await stop() } } else { Task { await send() } }
        }
        if bot.miniAppId == GlobalDharmaMiniAppBridge.globalDharmaId {
            actions["mobile-bot-open-miniapp"] = .init(allowed: ["invoke"]) { _ in openedMiniApp = true }
        }
        for entry in entries where entry.kind == .handoff && entry.actionStatus == "pending" {
            let completeId = Self.semanticId("mobile-bot-handoff-complete-\(entry.id)")
            let dismissId = Self.semanticId("mobile-bot-handoff-dismiss-\(entry.id)")
            actions[completeId] = .init(allowed: ["invoke"]) { _ in
                Task { await resolveBoxHandoff(entry, resolution: "completed") }
            }
            actions[dismissId] = .init(allowed: ["invoke"]) { _ in
                Task { await resolveBoxHandoff(entry, resolution: "dismissed") }
            }
        }
        try? appAgentSurface.publish(screen: "bot-chat", elements: elements, actions: actions)
    }

    private static func semanticId(_ value: String) -> String {
        String(value.map { character in
            character.isASCII && (character.isLetter || character.isNumber || "._:/@-".contains(character)) ? character : "-"
        }.prefix(200))
    }

    @ViewBuilder
    private func transcript(_ entry: MobileChatMessage) -> some View {
        if entry.kind == .thinking {
            HStack(spacing: 7) {
                ClothGhostAvatar(botId: bot.id, size: 22, active: true)
                Text(entry.actionTitle ?? "Thinking…").font(.caption).foregroundStyle(.secondary)
                ProgressView().controlSize(.mini)
            }
            .padding(.vertical, 4)
        } else if entry.kind == .handoff {
            VStack(alignment: .leading, spacing: 10) {
                HStack(spacing: 7) {
                    Image(systemName: "person.crop.circle.badge.exclamationmark")
                    Text("需要你完成一步").font(.caption.weight(.semibold))
                }
                Text(entry.text).font(.system(size: 15))
                if entry.actionStatus == "pending" {
                    HStack(spacing: 8) {
                        Button("已完成，继续") { Task { await resolveBoxHandoff(entry, resolution: "completed") } }
                            .buttonStyle(.borderedProminent)
                            .accessibilityIdentifier(Self.semanticId("mobile-bot-handoff-complete-\(entry.id)"))
                        Button("无法完成") { Task { await resolveBoxHandoff(entry, resolution: "dismissed") } }
                            .buttonStyle(.bordered)
                            .accessibilityIdentifier(Self.semanticId("mobile-bot-handoff-dismiss-\(entry.id)"))
                    }
                } else {
                    Text(entry.actionStatus == "completed" ? "已归还控制" : "已结束接管")
                        .font(.caption).foregroundStyle(.secondary)
                }
            }
            .padding(12)
            .background(Color.orange.opacity(0.10), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
        } else if entry.kind == .notice {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Image(systemName: "info.circle")
                    .foregroundStyle(.secondary)
                Text(entry.text)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Spacer(minLength: 8)
                Text(entry.createdAt, style: .time)
                    .font(.caption2)
                    .foregroundStyle(.tertiary)
            }
            .padding(.vertical, 4)
            .accessibilityIdentifier(Self.semanticId("mobile-bot-notice-\(entry.id)"))
        } else if entry.kind == .permissionRequest {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Image(systemName: "lock.shield")
                    .foregroundStyle(.secondary)
                Text(entry.text)
                    .font(.caption)
                Spacer(minLength: 8)
                Text(entry.createdAt, style: .time)
                    .font(.caption2)
                    .foregroundStyle(.tertiary)
            }
            .padding(.vertical, 4)
            .accessibilityIdentifier(Self.semanticId("mobile-bot-permission-request-\(entry.id)"))
        } else if entry.kind == .timelineEvent {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Image(systemName: entry.timelineAutomationId == nil ? "clock" : "calendar")
                    .foregroundStyle(.secondary)
                Text(entry.text)
                    .font(.caption)
                    .lineLimit(1)
                if let automationId = entry.timelineAutomationId,
                   case let .automationChanged(_, automationName)? = entry.timelineEvent
                {
                    Button(automationName) {
                        onOpenAutomation(automationId)
                    }
                    .buttonStyle(.bordered)
                    .controlSize(.mini)
                    .accessibilityLabel("Open routine \(automationName)")
                    .accessibilityIdentifier(Self.semanticId("mobile-bot-open-routine-\(automationId)"))
                }
                Spacer(minLength: 8)
                Text(entry.createdAt, style: .time)
                    .font(.caption2)
                    .foregroundStyle(.tertiary)
            }
            .padding(.vertical, 4)
            .accessibilityIdentifier(Self.semanticId("mobile-bot-timeline-event-\(entry.id)"))
        } else if entry.kind == .action {
            HStack(spacing: 7) {
                Circle().fill(entry.actionStatus == "failed" ? Color.red : Color.orange).frame(width: 7, height: 7)
                Text(entry.actionTitle ?? "Working").font(.caption.weight(.medium))
                if let detail = entry.actionDetail, !detail.isEmpty { Text(detail).font(.caption).foregroundStyle(.secondary).lineLimit(1) }
            }
            .padding(.vertical, 2)
        } else if entry.role == .user {
            HStack {
                Spacer(minLength: 54)
                Text(entry.text)
                    .font(.system(size: 16))
                    .foregroundStyle(.white)
                    .padding(.horizontal, 15).padding(.vertical, 10)
                    .background(.black, in: RoundedRectangle(cornerRadius: 18, style: .continuous))
                    .contextMenu {
                        Button("Reply") { replyTargetId = entry.canonicalMessageId ?? entry.id; replyIsFork = false }
                        Button("Reply in Fork") { replyTargetId = entry.canonicalMessageId ?? entry.id; replyIsFork = true }
                    }
            }
        } else {
            VStack(alignment: .leading, spacing: 3) {
                Text(bot.name).font(.caption).foregroundStyle(.secondary).padding(.leading, 12)
                HStack(alignment: .bottom, spacing: 7) {
                    ClothGhostAvatar(botId: bot.id, size: 20)
                    VStack(alignment: .leading, spacing: 7) {
                        if !entry.text.isEmpty {
                            Text(entry.text)
                                .overlay(alignment: .trailing) {
                                    if entry.streaming {
                                        Text("▌").foregroundStyle(.black.opacity(0.65))
                                    }
                                }
                                .font(.system(size: 16))
                                .foregroundStyle(.black)
                        }
                        if let rawURL = entry.attachmentURL, let url = URL(string: rawURL) {
                            Link(destination: url) {
                                Label(entry.attachmentFileName ?? entry.attachmentAlt ?? "Open attachment", systemImage: "paperclip")
                                    .font(.caption.weight(.medium))
                            }
                        }
                    }
                    .padding(.horizontal, 15).padding(.vertical, 10)
                    .background(Color.black.opacity(0.055), in: RoundedRectangle(cornerRadius: 18, style: .continuous))
                    .contextMenu {
                        Button("Reply") { replyTargetId = entry.canonicalMessageId ?? entry.id; replyIsFork = false }
                        Button("Reply in Fork") { replyTargetId = entry.canonicalMessageId ?? entry.id; replyIsFork = true }
                    }
                    Spacer(minLength: 30)
                }
            }
        }
    }

    @MainActor
    private func startVoiceInput() async {
        guard !busy, !transcribingVoice, !voiceRecorder.isRecording else { return }
        voiceInputGeneration += 1
        errorText = nil
        await voiceRecorder.start()
        if let recorderError = voiceRecorder.errorMessage {
            errorText = recorderError
        }
    }

    @MainActor
    private func finishVoiceInput() async {
        guard !busy, !transcribingVoice, let recording = voiceRecorder.stop() else { return }
        let generation = voiceInputGeneration
        let agentId = bot.id
        transcribingVoice = true
        defer {
            transcribingVoice = false
            try? FileManager.default.removeItem(at: recording.url)
        }
        do {
            let text = try await voiceTranscriber.transcribe(fileURL: recording.url)
            guard generation == voiceInputGeneration, agentId == bot.id else { return }
            draft = text
            errorText = nil
        } catch is CancellationError {
            return
        } catch {
            guard generation == voiceInputGeneration, agentId == bot.id else { return }
            errorText = error.localizedDescription
        }
    }

    @MainActor
    private func cancelVoiceInput() {
        voiceInputGeneration += 1
        voiceRecorder.cancel()
        voiceTranscriber.cancel()
        transcribingVoice = false
    }

    @MainActor
    private func send() async {
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, !busy else { return }
        draft = ""
        busy = true
        errorText = nil
        let requestId = "ios-mobile-bot-chat-\(UUID().uuidString.lowercased())"
        let replyTarget = replyTargetId
        let sendAsFork = replyIsFork
        replyTargetId = nil
        replyIsFork = false
        entries.append(MobileChatMessage(id: requestId, role: .user, text: text, canonicalMessageId: requestId, replyToMessageId: replyTarget, branched: sendAsFork))

        if let miniAppId = bot.miniAppId {
            await sendMiniApp(pluginId: miniAppId, text: text, operationId: requestId)
            activeOperationId = nil
            busy = false
            return
        }

        do {
            var command: [String: Any] = ["type": "chat.send", "requestId": requestId, "text": text, "agentId": bot.id, "mode": "agent", "isFork": sendAsFork]
            if let replyTarget { command["replyToMessageId"] = replyTarget }
            let result = try await bridge.request(
                method: "feature.execute",
                params: ["command": command]
            )
            let accepted = result.value as? [String: Any]
            let operationId = accepted?["operationId"] as? String ?? requestId
            activeOperationId = operationId
            entries.append(MobileChatMessage(id: "thinking:\(operationId)", role: .assistant, text: "", kind: .thinking, operationId: operationId, actionTitle: "Thinking", actionStatus: "running"))
            await pump(operationId: operationId)
        } catch {
            errorText = error.localizedDescription
        }
        activeOperationId = nil
        busy = false
    }

    @MainActor
    private func sendMiniApp(pluginId: String, text: String, operationId: String) async {
        activeOperationId = operationId
        entries.append(MobileChatMessage(
            id: "thinking:\(operationId)",
            role: .assistant,
            text: "",
            kind: .thinking,
            operationId: operationId,
            actionTitle: "正在通过 WebMCP 理解并执行",
            actionStatus: "running"
        ))
        do {
            let bridge = GlobalDharmaMiniAppBridge(bridge: bridge)
            let routed = try await bridge.routeInput(pluginId: pluginId, input: text)
            guard let execution = routed["execution"] as? [String: Any] else {
                removeThinking(operationId)
                let reply = (routed["message"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines)
                entries.append(MobileChatMessage(
                    id: "assistant:\(operationId)",
                    role: .assistant,
                    text: reply?.isEmpty == false ? reply! : "全球法布施没有把这条输入解析成可执行命令。",
                    operationId: operationId
                ))
                return
            }
            let command = routed["command"] as? [String: Any]
            let slash = command?["slash"] as? String ?? ""
            if routed["requiresApproval"] as? Bool == true {
                removeThinking(operationId)
                entries.append(MobileChatMessage(
                    id: "assistant:\(operationId)",
                    role: .assistant,
                    text: "已通过统一 Mini App 路由解析\(slash.isEmpty ? "" : "为 \(slash)")。该 Tool 需要宿主明确批准；iOS 不会静默执行写入或破坏性调用。",
                    operationId: operationId
                ))
                return
            }
            guard (execution["kind"] as? String) == "mcp-http",
                  let tool = execution["tool"] as? String,
                  !tool.isEmpty
            else {
                throw MahayanaCoordinator.CoordinatorError.requestFailed("iOS Mini App Bot only accepts governed mcp-http execution")
            }
            let arguments = routed["arguments"] as? [String: Any] ?? [:]
            let result = try await bridge.callOfficialMcpTool(pluginId: pluginId, name: tool, arguments: arguments)
            if pluginId == GlobalDharmaMiniAppBridge.globalDharmaId {
                model.recordGlobalDharmaExecution(tool: tool, result: result, source: "bot")
            }
            removeThinking(operationId)
            entries.append(MobileChatMessage(
                id: "assistant:\(operationId)",
                role: .assistant,
                text: GlobalDharmaMiniAppBridge.resultText(result),
                operationId: operationId
            ))
        } catch {
            removeThinking(operationId)
            errorText = error.localizedDescription
            entries.append(MobileChatMessage(
                id: "assistant:\(operationId):error",
                role: .assistant,
                text: "Mini App 调用失败：\(error.localizedDescription)",
                operationId: operationId
            ))
        }
    }

    @MainActor
    private func resolveBoxHandoff(_ entry: MobileChatMessage, resolution: String) async {
        guard entry.actionStatus == "pending",
              let handoffRequestId = entry.handoffRequestId,
              let handoffAgentId = entry.handoffAgentId
        else { return }
        do {
            let result = try await bridge.request(
                method: "feature.execute",
                params: [
                    "command": [
                        "type": "box.handoff.resolve",
                        "requestId": "ios-box-handoff-\(UUID().uuidString.lowercased())",
                        "handoffRequestId": handoffRequestId,
                        "agentId": handoffAgentId,
                        "resolution": resolution,
                    ],
                ]
            )
            if let index = entries.firstIndex(where: { $0.handoffRequestId == handoffRequestId }) {
                entries[index].actionStatus = resolution
            }
            guard let accepted = result.value as? [String: Any],
                  let operationId = accepted["operationId"] as? String,
                  !operationId.isEmpty
            else { return }
            busy = true
            activeOperationId = operationId
            entries.append(MobileChatMessage(
                id: "thinking:\(operationId)",
                role: .assistant,
                text: "",
                kind: .thinking,
                operationId: operationId,
                actionTitle: "Resuming after handoff",
                actionStatus: "running"
            ))
            await pump(operationId: operationId)
            activeOperationId = nil
            busy = false
        } catch {
            errorText = error.localizedDescription
        }
    }

    @MainActor
    private func stop() async {
        guard bot.miniAppId == nil, let activeOperationId else { return }
        _ = try? await bridge.request(method: "feature.interrupt", params: ["operationId": activeOperationId])
    }

    @MainActor
    private func pump(operationId: String) async {
        for _ in 0..<1800 {
            if Task.isCancelled { return }
            do {
                let ownedHandoffRequestIDs = Set(
                    entries.compactMap(\.handoffRequestId)
                )
                let result = try await bridge.receiveFeatureEvent(
                    deadlineMilliseconds: 450_000
                ) { event in
                    guard let type = event["type"] as? String else { return false }
                    if type == "box.handoff.resolved" {
                        guard let requestID = event["requestId"] as? String else { return false }
                        return ownedHandoffRequestIDs.contains(requestID)
                    }
                    let acceptedTypes: Set<String> = [
                        "box.handoff.requested",
                        "chat.message",
                        "chat.delta",
                        "agent.step",
                        "transcript.card",
                        "operation.started",
                        "operation.completed",
                        "operation.interrupted",
                        "operation.failed",
                        "model.routed",
                    ]
                    guard acceptedTypes.contains(type) else { return false }
                    return (event["operationId"] as? String ?? operationId) == operationId
                }
                guard let event = result.value as? [String: Any],
                      let type = event["type"] as? String
                else { continue }
                let eventOperationId = event["operationId"] as? String ?? operationId
                switch type {
                case "box.handoff.requested":
                    guard eventOperationId == operationId,
                          let requestId = event["requestId"] as? String,
                          let agentId = event["agentId"] as? String
                    else { continue }
                    let row = MobileChatMessage(
                        id: "handoff:\(requestId)",
                        role: .assistant,
                        text: event["instruction"] as? String ?? "Please complete the requested step.",
                        kind: .handoff,
                        operationId: operationId,
                        actionTitle: "Waiting for user help",
                        actionDetail: [event["reason"] as? String, event["domain"] as? String].compactMap { $0 }.joined(separator: " · "),
                        actionStatus: "pending",
                        handoffRequestId: requestId,
                        handoffAgentId: agentId
                    )
                    if let index = entries.firstIndex(where: { $0.handoffRequestId == requestId }) { entries[index] = row } else { entries.append(row) }
                case "box.handoff.resolved":
                    guard let requestId = event["requestId"] as? String else { continue }
                    if let index = entries.firstIndex(where: { $0.handoffRequestId == requestId }) {
                        entries[index].actionStatus = event["resolution"] as? String ?? "completed"
                    }
                case "chat.message":
                    guard isMobileBotVisibleAssistantCompletion(event, operationId: operationId) else { continue }
                    removeThinking(operationId)
                    let eventText = event["text"] as? String ?? ""
                    let generatedAttachment = event["attachment"] as? [String: Any]
                    if eventText.isEmpty, generatedAttachment != nil, !entries.contains(where: { $0.kind == .message && $0.role == .assistant && $0.operationId == operationId }) {
                        entries.append(MobileChatMessage(id: "assistant:\(operationId)", role: .assistant, text: "", operationId: operationId))
                    } else {
                        upsertAssistant(operationId, text: eventText, append: false, streaming: false)
                    }
                    if let index = entries.lastIndex(where: { $0.kind == .message && $0.role == .assistant && $0.operationId == operationId }) {
                        entries[index].canonicalMessageId = event["messageId"] as? String
                        entries[index].replyToMessageId = event["replyToMessageId"] as? String
                        entries[index].attachmentBatchId = event["attachmentBatchId"] as? String
                        if let attachment = event["attachment"] as? [String: Any] {
                            entries[index].attachmentURL = attachment["url"] as? String
                            entries[index].attachmentFileName = attachment["file_name"] as? String
                            entries[index].attachmentAlt = attachment["alt"] as? String
                        }
                        entries[index].branched = event["branched"] as? Bool ?? false
                    }
                case "chat.delta":
                    removeThinking(operationId)
                    upsertAssistant(operationId, text: event["delta"] as? String ?? "", append: true, streaming: true)
                case "agent.step":
                    let id = "action:\(operationId):\((event["stepId"] as? String) ?? UUID().uuidString)"
                    let row = MobileChatMessage(id: id, role: .assistant, text: "", kind: .action, operationId: operationId, actionTitle: event["title"] as? String ?? "Working", actionDetail: event["detail"] as? String, actionStatus: event["status"] as? String ?? "completed")
                    if let index = entries.firstIndex(where: { $0.id == id }) { entries[index] = row } else { entries.append(row) }
                case "model.routed":
                    let id = "action:\(operationId):model"
                    let provider = event["provider"] as? String ?? ""
                    let model = event["model"] as? String ?? ""
                    let row = MobileChatMessage(id: id, role: .assistant, text: "", kind: .action, operationId: operationId, actionTitle: "Model", actionDetail: [provider, model].filter { !$0.isEmpty }.joined(separator: " · "), actionStatus: "completed")
                    if let index = entries.firstIndex(where: { $0.id == id }) { entries[index] = row } else { entries.append(row) }
                case "transcript.card":
                    guard let row = projectMobileTranscriptCard(event: event, operationId: eventOperationId) else { continue }
                    if let index = entries.firstIndex(where: { $0.id == row.id }) { entries[index] = row } else { entries.append(row) }
                case "operation.completed", "operation.interrupted":
                    removeThinking(operationId)
                    finishAssistant(operationId)
                    return
                case "operation.failed":
                    removeThinking(operationId)
                    finishAssistant(operationId)
                    errorText = event["message"] as? String ?? "Bot run failed"
                    return
                default:
                    break
                }
            } catch {
                errorText = error.localizedDescription
                return
            }
            try? await Task.sleep(for: .milliseconds(60))
        }
    }

    private func removeThinking(_ operationId: String) {
        entries.removeAll { $0.kind == .thinking && $0.operationId == operationId }
    }

    private func upsertAssistant(_ operationId: String, text: String, append: Bool, streaming: Bool) {
        guard !text.isEmpty else { return }
        if let index = entries.lastIndex(where: { $0.kind == .message && $0.role == .assistant && $0.operationId == operationId }) {
            entries[index].text = append ? entries[index].text + text : text
            entries[index].streaming = streaming
        } else {
            entries.append(MobileChatMessage(id: "assistant:\(operationId)", role: .assistant, text: text, operationId: operationId, streaming: streaming))
        }
    }

    private func finishAssistant(_ operationId: String) {
        for index in entries.indices where entries[index].operationId == operationId && entries[index].role == .assistant {
            entries[index].streaming = false
        }
    }
}
