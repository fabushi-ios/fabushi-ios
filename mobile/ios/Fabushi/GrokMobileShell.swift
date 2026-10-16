import SwiftUI

internal struct MobileBotSummary: Identifiable, Equatable, Sendable {
    let id: String
    let name: String
    let description: String
    let miniAppId: String?
    let menuButtonText: String?

    init(
        id: String,
        name: String,
        description: String,
        miniAppId: String? = nil,
        menuButtonText: String? = nil
    ) {
        self.id = id
        self.name = name
        self.description = description
        self.miniAppId = miniAppId
        self.menuButtonText = menuButtonText
    }
}

private enum MobileGhostPalette {
    static let colors: [Color] = [
        Color(red: 0.00, green: 0.76, blue: 0.45),
        Color(red: 0.08, green: 0.49, blue: 0.98),
        Color(red: 0.53, green: 0.30, blue: 1.00),
        Color(red: 1.00, green: 0.31, blue: 0.10),
        Color(red: 0.94, green: 0.12, blue: 0.28),
        Color(red: 0.98, green: 0.62, blue: 0.04),
    ]

    static func color(for identity: String) -> Color {
        let value = identity.unicodeScalars.reduce(UInt64(1469598103934665603)) { partial, scalar in
            (partial ^ UInt64(scalar.value)) &* 1099511628211
        }
        return colors[Int(value % UInt64(colors.count))]
    }
}

private struct ClothGhostShape: Shape {
    var phase: CGFloat

    var animatableData: CGFloat {
        get { phase }
        set { phase = newValue }
    }

    func path(in rect: CGRect) -> Path {
        var path = Path()
        let w = rect.width
        let h = rect.height
        let top = h * 0.08
        let shoulder = h * 0.26
        let hem = h * 0.80
        let wave = h * 0.055
        let drift = sin(phase) * w * 0.018

        path.move(to: CGPoint(x: w * 0.16 + drift, y: hem))
        path.addLine(to: CGPoint(x: w * 0.16, y: shoulder))
        path.addCurve(
            to: CGPoint(x: w * 0.50, y: top),
            control1: CGPoint(x: w * 0.17, y: h * 0.13),
            control2: CGPoint(x: w * 0.34, y: top)
        )
        path.addCurve(
            to: CGPoint(x: w * 0.84, y: shoulder),
            control1: CGPoint(x: w * 0.66, y: top),
            control2: CGPoint(x: w * 0.83, y: h * 0.13)
        )
        path.addLine(to: CGPoint(x: w * 0.84 + drift, y: hem))

        let segment = w * 0.68 / 3.0
        for index in stride(from: 3, through: 1, by: -1) {
            let right = w * 0.16 + segment * CGFloat(index)
            let left = right - segment
            let local = phase + CGFloat(index) * 0.9
            let crest = hem + sin(local) * wave
            path.addQuadCurve(
                to: CGPoint(x: left, y: hem + sin(local + 0.7) * wave),
                control: CGPoint(x: (left + right) * 0.5, y: crest + h * 0.14)
            )
        }
        path.closeSubpath()
        return path
    }
}

/// Lightweight native avatar inspired by the reference video's cloth/ghost silhouette.
/// It intentionally uses one animated vector path instead of a physics/3D engine so it
/// stays inexpensive in dense mobile lists while retaining the soft floating motion.
internal struct ClothGhostAvatar: View {
    let botId: String
    var size: CGFloat = 44
    var active = false
    var badge: Color? = nil

    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        TimelineView(.animation(minimumInterval: 1.0 / 30.0, paused: reduceMotion)) { timeline in
            let seconds = timeline.date.timeIntervalSinceReferenceDate
            let phase = reduceMotion ? 0.0 : seconds * (active ? 3.0 : 1.55)
            let sway = reduceMotion ? 0.0 : sin(phase * 0.72) * 1.7
            let lift = reduceMotion ? 0.0 : sin(phase) * 1.2
            let gazeX = reduceMotion ? 0.0 : sin(phase * 0.48) * size * 0.026
            let gazeY = reduceMotion ? 0.0 : cos(phase * 0.39) * size * 0.017
            let shape = ClothGhostShape(phase: CGFloat(phase))
            let base = MobileGhostPalette.color(for: botId)

            ZStack(alignment: .topTrailing) {
                ZStack {
                    shape
                        .fill(
                            LinearGradient(
                                colors: [base.opacity(0.98), base.opacity(0.86), base.opacity(0.98)],
                                startPoint: .topLeading,
                                endPoint: .bottomTrailing
                            )
                        )
                    shape
                        .fill(
                            LinearGradient(
                                colors: [.white.opacity(0.24), .clear, .black.opacity(0.10)],
                                startPoint: .topLeading,
                                endPoint: .bottomTrailing
                            )
                        )
                        .blendMode(.softLight)
                    shape.stroke(.white.opacity(0.18), lineWidth: max(0.5, size * 0.012))

                    HStack(spacing: size * 0.115) {
                        Capsule().fill(.white).frame(width: size * 0.105, height: size * 0.23)
                        Capsule().fill(.white).frame(width: size * 0.105, height: size * 0.23)
                    }
                    .offset(x: gazeX, y: -size * 0.035 + gazeY)
                }
                .rotationEffect(.degrees(sway))
                .offset(y: lift)
                .scaleEffect(active && !reduceMotion ? 1.015 + sin(phase * 1.2) * 0.008 : 1)

                if let badge {
                    Circle()
                        .fill(badge)
                        .overlay(Circle().stroke(.white, lineWidth: 2))
                        .frame(width: size * 0.23, height: size * 0.23)
                        .offset(x: size * 0.02, y: size * 0.02)
                }
            }
            .frame(width: size, height: size)
        }
        .frame(width: size, height: size)
        .accessibilityLabel("Bot 头像")
        .accessibilityIdentifier("cloth-ghost-avatar")
    }
}

private struct MobileBotChat: View {
    let bot: MobileBotSummary
    let host: MahayanaHost
    let model: MarketplaceModel
    let appAgentSurface: FabushiAppAgentSurface
    let onClose: () -> Void

    @Binding var draft: String
    @Binding var entries: [MobileChatMessage]
    @State private var busy = false
    @State private var activeOperationId: String?
    @State private var errorText: String?
    @State private var openedMiniApp = false

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 12) {
                Button(action: onClose) {
                    Image(systemName: "chevron.left")
                        .font(.system(size: 17, weight: .semibold))
                        .foregroundStyle(.black)
                        .frame(width: 38, height: 38)
                        .background(Color.black.opacity(0.045), in: Circle())
                }
                Spacer()
                HStack(spacing: 8) {
                    ClothGhostAvatar(botId: bot.id, size: 28, active: busy)
                    Text(bot.name).font(.system(size: 17, weight: .semibold))
                }
                .padding(.horizontal, 13).padding(.vertical, 7)
                .background(.white, in: Capsule())
                .shadow(color: .black.opacity(0.08), radius: 12, y: 3)
                Spacer()
                Image(systemName: "desktopcomputer")
                    .font(.system(size: 16, weight: .medium))
                    .frame(width: 38, height: 38)
                    .background(Color.black.opacity(0.045), in: Circle())
            }
            .padding(.horizontal, 14).padding(.vertical, 8)
            .background(Color.white.opacity(0.97))

            Divider().opacity(0.35)

            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 7) {
                        if entries.isEmpty {
                            VStack(spacing: 13) {
                                ClothGhostAvatar(botId: bot.id, size: 82)
                                Text(bot.name).font(.title2.bold())
                                if !bot.description.isEmpty {
                                    Text(bot.description).font(.subheadline).foregroundStyle(.secondary).multilineTextAlignment(.center)
                                }
                            }
                            .frame(maxWidth: .infinity).padding(.top, 96).padding(.horizontal, 30)
                        }

                        ForEach(entries) { entry in
                            transcript(entry)
                                .id(entry.id)
                        }
                        if let errorText {
                            Text(errorText).font(.caption).foregroundStyle(.red).padding(.top, 4)
                                .accessibilityIdentifier("mobile-bot-error")
                        }
                    }
                    .padding(.horizontal, 16).padding(.vertical, 18)
                }
                .background(Color(red: 0.985, green: 0.985, blue: 0.975))
                .onChange(of: entries.count) { _, _ in
                    if let last = entries.last { withAnimation(.easeOut(duration: 0.16)) { proxy.scrollTo(last.id, anchor: .bottom) } }
                }
            }

            HStack(alignment: .bottom, spacing: 8) {
                TextField("Message", text: $draft, axis: .vertical)
                    .lineLimit(1...5)
                    .padding(.horizontal, 14).padding(.vertical, 11)
                    .background(Color.black.opacity(0.055), in: RoundedRectangle(cornerRadius: 18, style: .continuous))
                    .onSubmit { if !busy { Task { await send() } } }
                    .accessibilityIdentifier("mobile-bot-draft")

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

                Button {
                    if busy { Task { await stop() } } else { Task { await send() } }
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
            .padding(.horizontal, 12).padding(.top, 8).padding(.bottom, 10)
            .background(.ultraThinMaterial)
        }
        .background(Color(red: 0.985, green: 0.985, blue: 0.975))
        .accessibilityIdentifier("mobile-bot-chat")
        .task(id: semanticFingerprint) { publishAppAgentSurface() }
        .fullScreenCover(isPresented: $openedMiniApp) {
            GlobalDharmaMiniAppView(model: model, host: host)
        }
    }

    private var semanticFingerprint: String {
        [
            bot.id,
            bot.miniAppId ?? "",
            bot.menuButtonText ?? "",
            draft,
            String(busy),
            String(openedMiniApp),
            activeOperationId ?? "",
            errorText ?? "",
            entries.map { "\($0.id):\($0.kind.rawValue):\($0.role.rawValue)" }.joined(separator: ","),
        ].joined(separator: "|")
    }

    @MainActor
    private func publishAppAgentSurface() {
        var elements: [FabushiAppAgentSurface.Element] = [
            .init(agentId: "mobile-bot-chat", role: "application", name: "Bot \(String(bot.name.prefix(160)))"),
            .init(agentId: "mobile-bot-close", role: "button", name: "关闭 Bot 对话"),
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
            let roleName = entry.role == .user ? "用户消息" : entry.kind == .action ? "Bot 动作" : entry.kind == .thinking ? "Bot 思考" : "Bot 消息"
            elements.append(.init(agentId: id, role: "log", name: roleName))
        }
        var actions: [String: FabushiAppAgentSurface.Action] = [
            "mobile-bot-close": .init(allowed: ["invoke"]) { _ in onClose() },
            "mobile-bot-draft": .init(allowed: ["setValue"]) { value in draft = value ?? "" },
        ]
        actions[sendId] = .init(allowed: ["invoke"]) { _ in
            if busy { Task { await stop() } } else { Task { await send() } }
        }
        if bot.miniAppId == GlobalDharmaMiniAppBridge.globalDharmaId {
            actions["mobile-bot-open-miniapp"] = .init(allowed: ["invoke"]) { _ in openedMiniApp = true }
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
            }
        } else {
            VStack(alignment: .leading, spacing: 3) {
                Text(bot.name).font(.caption).foregroundStyle(.secondary).padding(.leading, 12)
                HStack(alignment: .bottom, spacing: 7) {
                    ClothGhostAvatar(botId: bot.id, size: 20)
                    Text(entry.text)
                        .overlay(alignment: .trailing) {
                            if entry.streaming {
                                Text("▌").foregroundStyle(.black.opacity(0.65))
                            }
                        }
                        .font(.system(size: 16))
                        .foregroundStyle(.black)
                        .padding(.horizontal, 15).padding(.vertical, 10)
                        .background(Color.black.opacity(0.055), in: RoundedRectangle(cornerRadius: 18, style: .continuous))
                    Spacer(minLength: 30)
                }
            }
        }
    }

    @MainActor
    private func send() async {
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, !busy else { return }
        draft = ""
        busy = true
        errorText = nil
        let requestId = "ios-mobile-bot-chat-\(UUID().uuidString.lowercased())"
        entries.append(MobileChatMessage(id: requestId, role: .user, text: text))

        if let miniAppId = bot.miniAppId {
            await sendMiniApp(pluginId: miniAppId, text: text, operationId: requestId)
            activeOperationId = nil
            busy = false
            return
        }

        do {
            let result = try await host.request(
                method: "feature.execute",
                params: ["command": ["type": "chat.send", "requestId": requestId, "text": text, "agentId": bot.id, "mode": "agent"]]
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
            let bridge = GlobalDharmaMiniAppBridge(host: host)
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
                throw MahayanaHost.HostError.requestFailed("iOS Mini App Bot only accepts governed mcp-http execution")
            }
            let arguments = routed["arguments"] as? [String: Any] ?? [:]
            let result = try await bridge.callOfficialMcpTool(pluginId: pluginId, name: tool, arguments: arguments)
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
    private func stop() async {
        guard bot.miniAppId == nil, let activeOperationId else { return }
        _ = try? await host.request(method: "feature.interrupt", params: ["operationId": activeOperationId])
    }

    @MainActor
    private func pump(operationId: String) async {
        for _ in 0..<1800 {
            if Task.isCancelled { return }
            do {
                let result = try await host.request(method: "feature.receive", params: ["timeoutMs": 250])
                guard let event = result.value as? [String: Any], let type = event["type"] as? String else {
                    try? await Task.sleep(for: .milliseconds(60)); continue
                }
                let eventOperationId = event["operationId"] as? String ?? operationId
                if ["chat.message", "chat.delta", "agent.step", "operation.started", "operation.completed", "operation.interrupted", "operation.failed", "model.routed"].contains(type), eventOperationId != operationId { continue }
                switch type {
                case "chat.message":
                    guard (event["role"] as? String) != "user" else { continue }
                    removeThinking(operationId)
                    upsertAssistant(operationId, text: event["text"] as? String ?? "", append: false, streaming: false)
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

internal struct GrokMobileShell: View {
    @Bindable var model: MarketplaceModel
    @Bindable var messaging: MessagingModel
    let host: MahayanaHost
    let appAgentSurface: FabushiAppAgentSurface

    @State private var query = ""
    @State private var searchOpen = false
    @State private var composeOpen = false
    @State private var createBotOpen = false
    @State private var botName = ""
    @State private var botDescription = ""
    @State private var botBusy = false
    @State private var botError: String?
    @State private var bots: [MobileBotSummary] = []
    @State private var selectedBot: MobileBotSummary?
    @State private var botDrafts: [String: String] = [:]
    @State private var botTranscripts: [String: [MobileChatMessage]] = [:]
    @State private var legacyOpen = false

    var body: some View {
        if model.onboardingStep < 3 || !model.authResolved || !model.loggedIn {
            ContentView(model: model, messaging: messaging, appAgentSurface: appAgentSurface)
        } else if let selectedBot {
            MobileBotChat(
                bot: selectedBot,
                host: host,
                model: model,
                appAgentSurface: appAgentSurface,
                onClose: { self.selectedBot = nil },
                draft: Binding(
                    get: { botDrafts[selectedBot.id] ?? "" },
                    set: { botDrafts[selectedBot.id] = $0 }
                ),
                entries: Binding(
                    get: { botTranscripts[selectedBot.id] ?? [] },
                    set: { botTranscripts[selectedBot.id] = $0 }
                )
            )
        } else if legacyOpen {
            VStack(spacing: 0) {
                HStack {
                    Button { legacyOpen = false } label: {
                        Image(systemName: "chevron.left")
                            .font(.system(size: 15, weight: .bold))
                            .frame(width: 36, height: 36)
                            .background(.ultraThinMaterial, in: Circle())
                    }
                    .accessibilityIdentifier("grok-mobile-back")
                    Spacer()
                }
                .padding(.horizontal, 10)
                .padding(.vertical, 8)
                .background(.ultraThinMaterial)

                ContentView(model: model, messaging: messaging, appAgentSurface: appAgentSurface)
            }
        } else {
            home
                .task { await loadBots() }
                .task { await messaging.refresh() }
                .task(id: appAgentSurfaceFingerprint) { publishAppAgentSurface() }
        }
    }

    private var appAgentSurfaceFingerprint: String {
        [
            query,
            String(searchOpen),
            String(composeOpen),
            String(createBotOpen),
            botName,
            botDescription,
            String(botBusy),
            botError ?? "",
            bots.map { "\($0.id):\($0.name):\($0.miniAppId ?? "")" }.joined(separator: ","),
            messaging.conversations.map { "\($0.id):\($0.unreadCount):\($0.isArchived)" }.joined(separator: ","),
        ].joined(separator: "|")
    }

    @MainActor
    private func publishAppAgentSurface() {
        var elements: [FabushiAppAgentSurface.Element] = []
        var actions: [String: FabushiAppAgentSurface.Action] = [:]
        func add(
            _ id: String,
            role: String,
            name: String,
            enabled: Bool = true,
            action: FabushiAppAgentSurface.Action? = nil
        ) {
            let normalizedId = Self.semanticId(id)
            elements.append(.init(
                agentId: normalizedId,
                role: String(role.prefix(80)),
                name: String(name.prefix(240)),
                enabled: enabled
            ))
            if let action { actions[normalizedId] = action }
        }

        if createBotOpen {
            add("grok-create-bot", role: "dialog", name: "新建 Bot")
            add("new-bot-name", role: "textbox", name: "Bot 名称", action: .init(allowed: ["setValue"]) { value in botName = value ?? "" })
            add("new-bot-description", role: "textbox", name: "Bot 描述", action: .init(allowed: ["setValue"]) { value in botDescription = value ?? "" })
            add(
                "create-bot-submit",
                role: "button",
                name: botBusy ? "正在创建 Bot" : "创建 Bot",
                enabled: !botBusy && !botName.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
                action: .init(allowed: ["invoke"]) { _ in Task { await createBot() } }
            )
            add("create-bot-cancel", role: "button", name: "取消创建 Bot", action: .init(allowed: ["invoke"]) { _ in createBotOpen = false })
            if botError != nil { add("create-bot-error", role: "status", name: "Bot 创建失败") }
            try? appAgentSurface.publish(screen: "grok-create-bot", elements: elements, actions: actions)
            return
        }

        if composeOpen {
            add("grok-compose", role: "dialog", name: "创建")
            add("grok-compose-bot", role: "button", name: "新建 Bot", action: .init(allowed: ["invoke"]) { _ in composeOpen = false; createBotOpen = true })
            add("grok-compose-message", role: "button", name: "新消息", action: .init(allowed: ["invoke"]) { _ in composeOpen = false; legacyOpen = true })
            add("grok-compose-group", role: "button", name: "新建群组", action: .init(allowed: ["invoke"]) { _ in composeOpen = false; legacyOpen = true })
            add("grok-compose-channel", role: "button", name: "新建频道", action: .init(allowed: ["invoke"]) { _ in composeOpen = false; legacyOpen = true })
            add("grok-compose-cancel", role: "button", name: "取消", action: .init(allowed: ["invoke"]) { _ in composeOpen = false })
            try? appAgentSurface.publish(screen: "grok-compose", elements: elements, actions: actions)
            return
        }

        add("grok-mobile-home", role: "application", name: "Fabushi")
        add("grok-mobile-legacy", role: "button", name: "打开完整消息工作台", action: .init(allowed: ["invoke"]) { _ in legacyOpen = true })
        add("grok-mobile-search", role: "button", name: searchOpen ? "关闭搜索" : "打开搜索", action: .init(allowed: ["invoke"]) { _ in
            searchOpen.toggle()
            if !searchOpen { query = "" }
        })
        add("grok-mobile-search-field", role: "textbox", name: "搜索", action: .init(allowed: ["setValue"]) { value in
            searchOpen = true
            query = value ?? ""
        })
        add("grok-mobile-add", role: "button", name: "创建", action: .init(allowed: ["invoke"]) { _ in composeOpen = true })
        add("grok-bot-mahayana-assistant", role: "button", name: "Mahayana", action: .init(allowed: ["invoke"]) { _ in selectedBot = MobileBotSummary(id: "mahayana-assistant", name: "Mahayana", description: "Ready to help") })
        for bot in filteredBots.prefix(100) {
            add("grok-bot-\(bot.id)", role: "button", name: bot.name, action: .init(allowed: ["invoke"]) { _ in selectedBot = bot })
        }
        for conversation in filteredConversations.prefix(100) {
            add(
                "grok-conversation-\(conversation.id)",
                role: "button",
                name: conversation.title,
                action: .init(allowed: ["invoke"]) { _ in legacyOpen = true }
            )
        }
        try? appAgentSurface.publish(screen: "grok-home", elements: elements, actions: actions)
    }

    private static func semanticId(_ value: String) -> String {
        String(value.map { character in
            character.isASCII && (character.isLetter || character.isNumber || "._:/@-".contains(character)) ? character : "-"
        }.prefix(200))
    }

    private var home: some View {
        ZStack {
            Color(red: 0.985, green: 0.985, blue: 0.975).ignoresSafeArea()
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    HStack {
                        Button { legacyOpen = true } label: {
                            ZStack {
                                Circle().fill(Color(red: 1.0, green: 0.78, blue: 0.82))
                                Text(String(model.accountName.prefix(1)).uppercased()).font(.system(size: 14, weight: .bold)).foregroundStyle(.black)
                            }
                            .frame(width: 38, height: 38)
                            .overlay(Circle().stroke(.white, lineWidth: 3)).shadow(color: .black.opacity(0.08), radius: 6)
                        }
                        .accessibilityIdentifier("grok-mobile-legacy")
                        Spacer()
                        Button {
                            searchOpen.toggle()
                            if !searchOpen { query = "" }
                        } label: { Image(systemName: "magnifyingglass") }
                            .accessibilityIdentifier("grok-mobile-search")
                        Button { composeOpen = true } label: { Image(systemName: "plus") }
                            .accessibilityIdentifier("grok-mobile-add")
                    }
                    .font(.system(size: 19, weight: .semibold)).foregroundStyle(.black)
                    .buttonStyle(.plain)
                    .padding(.horizontal, 18).padding(.top, 10)

                    VStack(spacing: 7) {
                        ZStack {
                            ClothGhostAvatar(botId: "all-hands-green", size: 50).offset(x: -25, y: 7).rotationEffect(.degrees(-8))
                            ClothGhostAvatar(botId: "all-hands-violet", size: 50).offset(x: -1, y: 17).rotationEffect(.degrees(7))
                            ClothGhostAvatar(botId: "mahayana-assistant", size: 55).offset(x: 24, y: -1)
                            Text("+2").font(.system(size: 29, weight: .bold)).foregroundStyle(Color.black.opacity(0.34)).offset(x: 48, y: 28)
                        }.frame(width: 130, height: 82)
                        Text("All Hands").font(.system(size: 14)).foregroundStyle(.secondary)
                    }
                    .frame(maxWidth: .infinity).padding(.top, 38).padding(.bottom, 34)

                    if searchOpen {
                        TextField("Search", text: $query)
                            .textFieldStyle(.plain).padding(12).background(.white, in: RoundedRectangle(cornerRadius: 14))
                            .padding(.horizontal, 16).padding(.bottom, 14)
                            .accessibilityIdentifier("grok-mobile-search-field")
                    }

                    sectionTitle("Board")
                    botRow(MobileBotSummary(id: "mahayana-assistant", name: "Mahayana", description: "Ready to help"), subtitle: "that's the only new one.", badge: "Board")

                    if !bots.isEmpty {
                        sectionTitle("Bots  \(bots.count)")
                        ForEach(filteredBots) { bot in botRow(bot, subtitle: bot.description.isEmpty ? "Ready" : bot.description, badge: bot.miniAppId == nil ? "Bot" : "Mini App Bot") }
                    }

                    let projects = filteredConversations.filter { $0.kind == .group || $0.kind == .direct }
                    if !projects.isEmpty {
                        sectionTitle("Projects  \(projects.count)")
                        ForEach(projects.prefix(8)) { conversation in conversationRow(conversation) }
                    }
                    let channels = filteredConversations.filter { $0.kind == .channel }
                    if !channels.isEmpty {
                        sectionTitle("Channels  \(channels.count)")
                        ForEach(channels.prefix(8)) { conversation in conversationRow(conversation) }
                    }
                    Spacer(minLength: 40)
                }
            }
        }
        .confirmationDialog("Create", isPresented: $composeOpen, titleVisibility: .visible) {
            Button("New Bot") { createBotOpen = true }
            Button("New message") { legacyOpen = true }
            Button("New group") { legacyOpen = true }
            Button("New channel") { legacyOpen = true }
            Button("Cancel", role: .cancel) { }
        }
        .sheet(isPresented: $createBotOpen) { createBotSheet }
        .accessibilityIdentifier("grok-mobile-home")
    }

    private var filteredBots: [MobileBotSummary] {
        guard !query.isEmpty else { return bots }
        return bots.filter { $0.name.localizedCaseInsensitiveContains(query) || $0.description.localizedCaseInsensitiveContains(query) }
    }

    private var filteredConversations: [ConversationSummary] {
        let rows = messaging.conversations.filter { !$0.isArchived }
        guard !query.isEmpty else { return rows }
        return rows.filter { $0.title.localizedCaseInsensitiveContains(query) || $0.preview.localizedCaseInsensitiveContains(query) }
    }

    private func sectionTitle(_ text: String) -> some View {
        Text(text).font(.system(size: 16)).foregroundStyle(Color.black.opacity(0.42)).padding(.horizontal, 18).padding(.top, 13).padding(.bottom, 7)
    }

    private func botRow(_ bot: MobileBotSummary, subtitle: String, badge: String) -> some View {
        Button { selectedBot = bot } label: {
            HStack(spacing: 12) {
                ClothGhostAvatar(botId: bot.id, size: 47, badge: .green)
                VStack(alignment: .leading, spacing: 3) {
                    HStack(spacing: 7) {
                        Text(bot.name).font(.system(size: 17, weight: .semibold)).foregroundStyle(.black)
                        Text(badge).font(.caption).foregroundStyle(.secondary).padding(.horizontal, 7).padding(.vertical, 3).background(Color.black.opacity(0.045), in: Capsule())
                    }
                    Text(subtitle).font(.system(size: 14)).foregroundStyle(.secondary).lineLimit(1)
                }
                Spacer()
                Text("now").font(.caption).foregroundStyle(.secondary)
            }
            .padding(.horizontal, 18).padding(.vertical, 9)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    private func conversationRow(_ conversation: ConversationSummary) -> some View {
        Button { legacyOpen = true } label: {
            HStack(spacing: 12) {
                ClothGhostAvatar(botId: "conversation:\(conversation.id)", size: 45, badge: conversation.unreadCount > 0 ? .blue : nil)
                VStack(alignment: .leading, spacing: 3) {
                    HStack(spacing: 7) {
                        Text(conversation.title).font(.system(size: 17, weight: .semibold)).foregroundStyle(.black).lineLimit(1)
                        Text(conversation.kind == .channel ? "Channel" : "Engineering").font(.caption).foregroundStyle(.secondary).padding(.horizontal, 7).padding(.vertical, 3).background(Color.black.opacity(0.045), in: Capsule())
                    }
                    Text(conversation.preview.isEmpty ? "Ready" : conversation.preview).font(.system(size: 14)).foregroundStyle(.secondary).lineLimit(1)
                }
                Spacer()
                Text(conversation.time).font(.caption).foregroundStyle(.secondary)
            }
            .padding(.horizontal, 18).padding(.vertical, 9)
        }.buttonStyle(.plain)
    }

    private var createBotSheet: some View {
        NavigationStack {
            Form {
                Section {
                    HStack { Spacer(); ClothGhostAvatar(botId: botName.isEmpty ? "new-bot" : botName, size: 82, active: botBusy); Spacer() }
                }
                Section("Name") { TextField("Bot name", text: $botName).accessibilityIdentifier("new-bot-name") }
                Section("Description") { TextField("What does this Bot do?", text: $botDescription, axis: .vertical).lineLimit(2...5) }
                if let botError { Section { Text(botError).foregroundStyle(.red).font(.footnote) } }
            }
            .navigationTitle("New Bot")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { createBotOpen = false } }
                ToolbarItem(placement: .confirmationAction) {
                    Button(botBusy ? "Creating…" : "Create") { Task { await createBot() } }
                        .disabled(botBusy || botName.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                        .accessibilityIdentifier("create-bot-submit")
                }
            }
        }
    }

    @MainActor
    private func loadBots() async {
        let canonical = (try? await GlobalDharmaMiniAppBridge(host: host).installedMiniAppBots()) ?? []
        let installedBots = canonical.map {
            MobileBotSummary(
                id: $0.id,
                name: $0.name,
                description: $0.description,
                miniAppId: $0.miniAppId,
                menuButtonText: $0.menuButtonText
            )
        }
        let requestId = "ios-mobile-bot-list-\(UUID().uuidString.lowercased())"
        do {
            _ = try await host.request(method: "feature.execute", params: ["command": ["type": "bot.list", "requestId": requestId]])
            for _ in 0..<32 {
                let result = try await host.request(method: "feature.receive", params: ["timeoutMs": 80])
                guard let event = result.value as? [String: Any], let type = event["type"] as? String else { continue }
                if type == "bot.listed", let rows = event["bots"] as? [[String: Any]] {
                    let surfaceBots = rows.compactMap(Self.parseBot).filter { $0.id != "mahayana-assistant" }
                    bots = Self.mergeBots(installedBots, surfaceBots)
                    return
                }
            }
            bots = installedBots
        } catch {
            bots = installedBots
        }
    }

    @MainActor
    private func createBot() async {
        let name = botName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty, !botBusy else { return }
        botBusy = true
        botError = nil
        let requestId = "ios-mobile-bot-create-\(UUID().uuidString.lowercased())"
        do {
            _ = try await host.request(method: "feature.execute", params: ["command": ["type": "bot.create", "requestId": requestId, "name": String(name.prefix(72)), "description": String(botDescription.trimmingCharacters(in: .whitespacesAndNewlines).prefix(240))]])
            botName = ""
            botDescription = ""
            createBotOpen = false
            await loadBots()
            await messaging.refresh()
        } catch {
            botError = error.localizedDescription
        }
        botBusy = false
    }

    private static func mergeBots(_ installed: [MobileBotSummary], _ surface: [MobileBotSummary]) -> [MobileBotSummary] {
        var byId: [String: MobileBotSummary] = [:]
        for bot in surface { byId[bot.id] = bot }
        for bot in installed { byId[bot.id] = bot }
        return byId.values.sorted {
            if ($0.miniAppId != nil) != ($1.miniAppId != nil) { return $0.miniAppId != nil }
            return $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending
        }
    }

    private static func parseBot(_ row: [String: Any]) -> MobileBotSummary? {
        guard let id = row["id"] as? String, !id.isEmpty else { return nil }
        let explicitMiniAppId = (row["miniAppId"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines)
        let miniAppId = explicitMiniAppId?.isEmpty == false
            ? explicitMiniAppId
            : (id == "global-dharma-bot" ? GlobalDharmaMiniAppBridge.globalDharmaId : nil)
        let menuText = (row["menuButtonText"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines)
        return MobileBotSummary(
            id: id,
            name: (row["name"] as? String) ?? (row["displayName"] as? String) ?? id,
            description: row["description"] as? String ?? "",
            miniAppId: miniAppId,
            menuButtonText: menuText?.isEmpty == false ? menuText : (miniAppId == nil ? nil : "打开应用")
        )
    }
}
