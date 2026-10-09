import SwiftUI
import UniformTypeIdentifiers
import UIKit

internal enum ForwardRecipientNavigationKey: Equatable {
    case arrowDown
    case arrowUp
    case pageDown
    case pageUp
    case home
    case end
}

internal enum ForwardRecipientNavigation {
    static func nextIndex(
        currentIndex: Int,
        recipientCount: Int,
        key: ForwardRecipientNavigationKey,
        pageSize: Int = 6
    ) -> Int? {
        guard recipientCount > 0 else { return nil }
        let last = recipientCount - 1
        let current = min(max(currentIndex, 0), last)
        let page = max(1, pageSize)
        switch key {
        case .arrowDown:
            return min(current + 1, last)
        case .arrowUp:
            return max(current - 1, 0)
        case .pageDown:
            return min(current + page, last)
        case .pageUp:
            return max(current - page, 0)
        case .home:
            return 0
        case .end:
            return last
        }
    }
}

private struct ForwardMessageSheet: View {
    let sourceConversationId: String
    let message: ChatMessage
    let messaging: MessagingModel
    let appAgentSurface: FabushiAppAgentSurface
    let onDismiss: () -> Void

    @State private var query = ""
    @State private var recipients: [ConversationSummary] = []
    @State private var selectedRecipients: [String: ConversationSummary] = [:]
    @State private var clientMessageIds: [String: String] = [:]
    @State private var settlements: [String: ForwardSettlement] = [:]
    @State private var dropSenderNames = false
    @State private var dropCaptions = false
    @State private var loading = false
    @State private var sending = false
    @State private var errorText: String?
    @State private var activeRecipientIndex = 0
    @FocusState private var recipientListFocused: Bool

    private var selectedInOrder: [ConversationSummary] {
        selectedRecipients.values.sorted {
            if $0.title == $1.title { return $0.id < $1.id }
            return $0.title.localizedStandardCompare($1.title) == .orderedAscending
        }
    }

    private var pendingRecipients: [ConversationSummary] {
        selectedInOrder.filter { settlements[$0.id]?.sent != true }
    }

    private var sentCount: Int {
        settlements.values.filter(\.sent).count
    }

    private var failedCount: Int {
        settlements.values.filter { !$0.sent }.count
    }

    private var optionsLocked: Bool {
        sentCount > 0
    }

    var body: some View {
        NavigationStack {
            List {
                if let errorText {
                    Section {
                        Label(errorText, systemImage: "exclamationmark.triangle.fill")
                            .foregroundStyle(.red)
                    }
                }

                Section("选项") {
                    Toggle("隐藏原发送者", isOn: $dropSenderNames)
                        .disabled(optionsLocked || sending || dropCaptions)
                    Toggle("移除媒体说明文字", isOn: $dropCaptions)
                        .disabled(optionsLocked || sending)
                        .onChange(of: dropCaptions) { _, enabled in
                            if enabled { dropSenderNames = true }
                        }
                }

                Section("会话") {
                    if loading && recipients.isEmpty {
                        HStack {
                            Spacer()
                            ProgressView()
                            Spacer()
                        }
                    } else if recipients.isEmpty {
                        ContentUnavailableView(
                            query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                                ? "没有可转发的会话"
                                : "没有匹配的会话",
                            systemImage: "arrowshape.turn.up.right"
                        )
                    } else {
                        ForEach(Array(recipients.enumerated()), id: \.element.id) { index, recipient in
                            Button {
                                activeRecipientIndex = index
                                toggleRecipient(recipient)
                            } label: {
                                HStack(spacing: 10) {
                                    Image(
                                        systemName: selectedRecipients[recipient.id] == nil
                                            ? "circle"
                                            : "checkmark.circle.fill"
                                    )
                                    .foregroundStyle(
                                        selectedRecipients[recipient.id] == nil
                                            ? Color.secondary
                                            : Color.accentColor
                                    )
                                    VStack(alignment: .leading, spacing: 2) {
                                        Text(recipient.title)
                                            .foregroundStyle(.primary)
                                        Text(recipient.kind.label)
                                            .font(.caption)
                                            .foregroundStyle(.secondary)
                                    }
                                    Spacer()
                                    if let settlement = settlements[recipient.id] {
                                        if settlement.sent {
                                            Label("已发送", systemImage: "checkmark.circle.fill")
                                                .font(.caption)
                                                .foregroundStyle(.green)
                                        } else {
                                            Label("失败", systemImage: "exclamationmark.circle.fill")
                                                .font(.caption)
                                                .foregroundStyle(.red)
                                        }
                                    }
                                }
                            }
                            .buttonStyle(.plain)
                            .disabled(sending || settlements[recipient.id]?.sent == true)
                            .listRowBackground(
                                activeRecipientIndex == index
                                    ? Color.accentColor.opacity(0.14)
                                    : Color.clear
                            )
                        }
                    }
                }

                if sentCount > 0 || failedCount > 0 {
                    Section("结果") {
                        if sentCount > 0 {
                            Text("已成功转发到 \(sentCount) 个会话。")
                                .foregroundStyle(.secondary)
                        }
                        ForEach(
                            settlements.values
                                .filter { !$0.sent }
                                .sorted { $0.conversationId < $1.conversationId }
                        ) { settlement in
                            VStack(alignment: .leading, spacing: 3) {
                                Text(selectedRecipients[settlement.conversationId]?.title ?? settlement.conversationId)
                                    .fontWeight(.semibold)
                                Text(settlement.error ?? "转发失败")
                                    .font(.caption)
                                    .foregroundStyle(.red)
                            }
                        }
                    }
                }
            }
            .searchable(text: $query, prompt: "搜索会话")
            .focusable()
            .focused($recipientListFocused)
            .onKeyPress(
                keys: [.downArrow, .upArrow, .pageDown, .pageUp, .home, .end, .return, .space],
                phases: [.down, .repeat]
            ) { press in
                handleRecipientKeyPress(press)
            }
            .navigationTitle("转发到")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(sentCount > 0 && failedCount == 0 ? "完成" : "取消") {
                        onDismiss()
                    }
                    .disabled(sending)
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button(actionTitle) {
                        Task { await submit() }
                    }
                    .disabled(pendingRecipients.isEmpty || sending)
                }
            }
            .overlay {
                if sending {
                    ProgressView("正在转发…")
                        .padding()
                        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 14))
                }
            }
            .task(id: query) {
                await reloadRecipients()
            }
            .task(id: semanticFingerprint) {
                await MainActor.run {
                    publishSemanticSurface()
                }
            }
        }
    }

    private var semanticFingerprint: String {
        let recipientState = recipients.map { recipient in
            let settlement = settlements[recipient.id]
            return [
                recipient.id,
                recipient.title,
                selectedRecipients[recipient.id] == nil ? "0" : "1",
                settlement?.sent == true ? "sent" : settlement == nil ? "pending" : "failed",
                settlement?.error ?? "",
            ].joined(separator: ":")
        }.joined(separator: "|")
        return [
            sourceConversationId,
            message.id,
            query,
            recipientState,
            String(dropSenderNames),
            String(dropCaptions),
            String(loading),
            String(sending),
            String(sentCount),
            String(failedCount),
            errorText ?? "",
        ].joined(separator: "||")
    }

    @MainActor
    private func publishSemanticSurface() {
        var elements: [FabushiAppAgentSurface.Element] = []
        var actions: [String: FabushiAppAgentSurface.Action] = [:]

        func semanticId(_ value: String) -> String {
            String(value.map { character in
                character.isASCII && (character.isLetter || character.isNumber || "._:/@-".contains(character))
                    ? character
                    : "-"
            }.prefix(160))
        }

        func add(
            _ id: String,
            role: String,
            name: String,
            enabled: Bool = true,
            action: FabushiAppAgentSurface.Action? = nil
        ) {
            let normalizedId = semanticId(id)
            elements.append(.init(
                agentId: normalizedId,
                role: String(role.prefix(80)),
                name: String(name.prefix(240)),
                visible: true,
                enabled: enabled
            ))
            if let action {
                actions[normalizedId] = action
            }
        }

        add("forward-dialog", role: "dialog", name: "转发消息")
        add(
            "forward-search",
            role: "textbox",
            name: query.isEmpty ? "搜索可转发会话" : "搜索可转发会话：\(query)",
            enabled: !sending,
            action: .init(allowed: ["setValue"]) { value in
                query = value ?? ""
            }
        )
        if loading {
            add("forward-loading", role: "status", name: "正在加载可转发会话")
        }
        if let errorText {
            add("forward-error", role: "status", name: errorText)
        }
        if recipients.isEmpty && !loading {
            add(
                "forward-empty",
                role: "status",
                name: query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                    ? "没有可转发的会话"
                    : "没有匹配的可转发会话"
            )
        }

        for recipient in recipients.prefix(100) {
            let selected = selectedRecipients[recipient.id] != nil
            let settlement = settlements[recipient.id]
            let state = settlement?.sent == true ? "已发送" : settlement == nil ? (selected ? "已选择" : "未选择") : "失败"
            add(
                "forward-recipient-\(recipient.id)",
                role: "checkbox",
                name: "\(recipient.title) · \(state)",
                enabled: !sending && settlement?.sent != true,
                action: .init(allowed: ["toggle", "invoke"]) { _ in
                    toggleRecipient(recipient)
                }
            )
        }

        add(
            "forward-hide-sender",
            role: "checkbox",
            name: dropSenderNames ? "隐藏原发送者：已开启" : "隐藏原发送者：已关闭",
            enabled: !optionsLocked && !sending && !dropCaptions,
            action: .init(allowed: ["toggle", "invoke"]) { _ in
                guard !optionsLocked, !sending, !dropCaptions else { return }
                dropSenderNames.toggle()
            }
        )
        add(
            "forward-drop-captions",
            role: "checkbox",
            name: dropCaptions ? "移除媒体说明文字：已开启" : "移除媒体说明文字：已关闭",
            enabled: !optionsLocked && !sending,
            action: .init(allowed: ["toggle", "invoke"]) { _ in
                guard !optionsLocked, !sending else { return }
                dropCaptions.toggle()
                if dropCaptions {
                    dropSenderNames = true
                }
            }
        )
        add(
            "forward-selection-status",
            role: "status",
            name: "\(selectedRecipients.count) 个会话已选择，\(sentCount) 个已发送，\(failedCount) 个失败"
        )
        for settlement in settlements.values.filter({ !$0.sent }).prefix(100) {
            let title = selectedRecipients[settlement.conversationId]?.title ?? settlement.conversationId
            add(
                "forward-failure-\(settlement.conversationId)",
                role: "status",
                name: "\(title)：\(settlement.error ?? "转发失败")"
            )
        }
        add(
            "forward-cancel",
            role: "button",
            name: sentCount > 0 && failedCount == 0 ? "完成" : "取消",
            enabled: !sending,
            action: .init(allowed: ["invoke"]) { _ in
                guard !sending else { return }
                onDismiss()
            }
        )
        add(
            "forward-submit",
            role: "button",
            name: actionTitle,
            enabled: !pendingRecipients.isEmpty && !sending,
            action: .init(allowed: ["invoke"]) { _ in
                guard !pendingRecipients.isEmpty, !sending else { return }
                Task { await submit() }
            }
        )

        try? appAgentSurface.publish(
            screen: "forward-message",
            elements: elements,
            actions: actions
        )
    }

    private func handleRecipientKeyPress(_ press: KeyPress) -> KeyPress.Result {
        guard !sending, !recipients.isEmpty else { return .ignored }

        let navigationKey: ForwardRecipientNavigationKey?
        switch press.key {
        case .downArrow:
            navigationKey = .arrowDown
        case .upArrow:
            navigationKey = .arrowUp
        case .pageDown:
            navigationKey = .pageDown
        case .pageUp:
            navigationKey = .pageUp
        case .home:
            navigationKey = .home
        case .end:
            navigationKey = .end
        default:
            navigationKey = nil
        }

        if let navigationKey,
           let next = ForwardRecipientNavigation.nextIndex(
               currentIndex: activeRecipientIndex,
               recipientCount: recipients.count,
               key: navigationKey
           )
        {
            activeRecipientIndex = next
            return .handled
        }

        if press.key == .return, press.modifiers.contains(.command) {
            guard !pendingRecipients.isEmpty else { return .ignored }
            Task { await submit() }
            return .handled
        }

        if press.key == .return || press.key == .space {
            guard recipients.indices.contains(activeRecipientIndex) else { return .ignored }
            let recipient = recipients[activeRecipientIndex]
            guard settlements[recipient.id]?.sent != true else { return .handled }
            toggleRecipient(recipient)
            return .handled
        }

        return .ignored
    }

    private var actionTitle: String {
        if sending { return "发送中" }
        if failedCount > 0 { return "重试失败项" }
        if sentCount > 0 && pendingRecipients.isEmpty { return "已完成" }
        return selectedRecipients.count > 1 ? "转发（\(selectedRecipients.count)）" : "转发"
    }

    private func toggleRecipient(_ recipient: ConversationSummary) {
        guard settlements[recipient.id]?.sent != true else { return }
        if selectedRecipients[recipient.id] == nil {
            selectedRecipients[recipient.id] = recipient
            if clientMessageIds[recipient.id] == nil {
                clientMessageIds[recipient.id] = "ios-forward:\(UUID().uuidString.lowercased())"
            }
        } else {
            selectedRecipients.removeValue(forKey: recipient.id)
            settlements.removeValue(forKey: recipient.id)
        }
    }

    private func reloadRecipients() async {
        do {
            try await Task.sleep(for: .milliseconds(200))
            guard !Task.isCancelled else { return }
            loading = true
            defer { loading = false }
            recipients = try await messaging.searchForwardRecipients(
                sourceConversationId: sourceConversationId,
                messageId: message.id,
                query: query,
                limit: 100
            )
            if recipients.isEmpty {
                activeRecipientIndex = 0
            } else {
                activeRecipientIndex = min(activeRecipientIndex, recipients.count - 1)
            }
            errorText = nil
        } catch is CancellationError {
            return
        } catch {
            loading = false
            errorText = "无法加载可转发会话：\(error.localizedDescription)"
        }
    }

    private func submit() async {
        let targets = pendingRecipients
        guard !targets.isEmpty else { return }
        sending = true
        errorText = nil
        for target in targets where clientMessageIds[target.id] == nil {
            clientMessageIds[target.id] = "ios-forward:\(UUID().uuidString.lowercased())"
        }
        let requests = targets.compactMap { target -> ForwardDestinationRequest? in
            guard let clientMessageId = clientMessageIds[target.id] else { return nil }
            return ForwardDestinationRequest(
                conversationId: target.id,
                clientMessageId: clientMessageId
            )
        }
        let results = await messaging.forwardMessageBatch(
            sourceConversationId: sourceConversationId,
            messageId: message.id,
            destinations: requests,
            dropSenderNames: dropSenderNames,
            dropCaptions: dropCaptions
        )
        for settlement in results {
            settlements[settlement.conversationId] = settlement
        }
        sending = false
        if results.isEmpty {
            errorText = "没有可发送的目标会话。"
        } else if results.allSatisfy(\.sent) {
            onDismiss()
        }
    }
}

extension ContentView {
    func chatView(_ conversation: ConversationSummary) -> some View {
        let messages = messaging.messagesByConversation[conversation.id] ?? []
        let searchMatches = chatSearchMatches(
            messages.map {
                ChatSearchEntry(
                    id: $0.id,
                    text: chatSearchText(
                        for: $0,
                        author: messaging.searchAuthorByMessageId[$0.id]
                    )
                )
            },
            query: chatSearchQuery
        )
        return NavigationStack {
            ZStack {
                Color(red: 0.055, green: 0.06, blue: 0.07).ignoresSafeArea()
                VStack(spacing: 0) {
                    if chatSearchPresented {
                        HStack(spacing: 8) {
                            Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                            TextField("搜索此聊天", text: $chatSearchQuery)
                                .textInputAutocapitalization(.never)
                                .autocorrectionDisabled()
                                .onChange(of: chatSearchQuery) { _, _ in
                                    chatSearchMatchIndex = nil
                                    chatSearchTargetID = nil
                                }
                            if !chatSearchQuery.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                                Text(searchMatches.isEmpty ? "0 / 0" : "\((chatSearchMatchIndex ?? -1) + 1) / \(searchMatches.count)")
                                    .font(.caption.monospacedDigit())
                                    .foregroundStyle(.secondary)
                                Button { navigateChatSearch(matches: searchMatches, delta: -1) } label: { Image(systemName: "chevron.up") }
                                    .disabled(searchMatches.isEmpty)
                                Button { navigateChatSearch(matches: searchMatches, delta: 1) } label: { Image(systemName: "chevron.down") }
                                    .disabled(searchMatches.isEmpty)
                                Button { chatSearchQuery = "" } label: { Image(systemName: "xmark.circle.fill") }
                            }
                        }
                        .padding(.horizontal, 12).frame(height: 40).background(Color.white.opacity(0.08), in: RoundedRectangle(cornerRadius: 12)).padding(8)
                    }
                    if let pinnedId = conversation.pinnedMessageIds.last, let pinned = messaging.messagesByConversation[conversation.id]?.first(where: { $0.id == pinnedId }) {
                        HStack(spacing: 9) {
                            Rectangle().fill(Color.accentColor).frame(width: 3, height: 34)
                            VStack(alignment: .leading, spacing: 2) { Text("置顶消息").font(.caption.bold()).foregroundStyle(Color.accentColor); Text(pinned.text).font(.caption).lineLimit(1) }
                            Spacer()
                            Button { Task { await messaging.setMessagePinned(conversationId: conversation.id, messageId: pinned.id, pinned: false) } } label: { Image(systemName: "xmark") }
                        }.padding(.horizontal, 12).padding(.vertical, 6).background(.ultraThinMaterial)
                    }
                    if let typingName = messaging.typingActorByConversation[conversation.id] {
                        Text("\(typingName) 正在输入…")
                            .font(.caption).foregroundStyle(.secondary)
                            .frame(maxWidth: .infinity).padding(.vertical, 4)
                    }
                    ScrollViewReader { proxy in
                        ScrollView {
                            LazyVStack(spacing: 8) {
                                ForEach(messages) { message in
                                    HStack {
                                        if message.isOutgoing { Spacer(minLength: 56) }
                                        VStack(alignment: .trailing, spacing: 4) {
                                            if let origin = message.forwardOrigin {
                                                HStack(spacing: 4) { Image(systemName: "arrowshape.turn.up.right.fill"); Text("转发自 \(origin)") }
                                                    .font(.caption2.bold()).foregroundStyle(Color.accentColor).frame(maxWidth: .infinity, alignment: .leading)
                                            }
                                            if let replyId = message.replyToMessageId, let replied = messaging.messagesByConversation[conversation.id]?.first(where: { $0.id == replyId }) {
                                                VStack(alignment: .leading, spacing: 2) { Text("回复").font(.caption2.bold()); Text(replied.text).font(.caption).lineLimit(2) }
                                                    .frame(maxWidth: .infinity, alignment: .leading).padding(.leading, 8).overlay(alignment: .leading) { Rectangle().fill(Color.accentColor).frame(width: 2) }
                                            }
                                            switch message.contentType {
                                            case "contact":
                                                HStack(spacing: 10) {
                                                    ZStack { Circle().fill(Color.accentColor); Text(String((message.contactName ?? "联").prefix(1))).foregroundStyle(.white).fontWeight(.bold) }.frame(width: 40, height: 40)
                                                    VStack(alignment: .leading, spacing: 2) { Text(message.contactName ?? "联系人").fontWeight(.semibold); Text("联系人").font(.caption).foregroundStyle(.secondary) }
                                                    Spacer()
                                                }.frame(maxWidth: .infinity)
                                            case "location":
                                                VStack(alignment: .leading, spacing: 6) {
                                                    HStack { Image(systemName: "map.fill").foregroundStyle(Color.accentColor); Text("位置").fontWeight(.semibold) }
                                                    if let latitude = message.latitude, let longitude = message.longitude { Text("\(latitude, specifier: "%.6f"), \(longitude, specifier: "%.6f")").font(.caption).foregroundStyle(.secondary) }
                                                }.frame(maxWidth: .infinity, alignment: .leading)
                                            case "poll":
                                                VStack(alignment: .leading, spacing: 7) {
                                                    Text(message.pollQuestion ?? "投票").fontWeight(.semibold)
                                                    ForEach(message.pollOptions) { option in
                                                        Button {
                                                            let chosenIds = Set(message.pollOptions.filter(\.chosen).map(\.id))
                                                            let next: [String]
                                                            if message.pollMultipleAnswers {
                                                                var values = chosenIds
                                                                if option.chosen { values.remove(option.id) } else { values.insert(option.id) }
                                                                next = Array(values)
                                                            } else {
                                                                next = option.chosen ? [] : [option.id]
                                                            }
                                                            Task { await messaging.votePoll(conversationId: conversation.id, messageId: message.id, optionIds: next) }
                                                        } label: {
                                                            HStack(spacing: 7) {
                                                                Image(systemName: option.chosen ? "checkmark.circle.fill" : "circle").foregroundStyle(option.chosen ? Color.accentColor : .secondary)
                                                                Text(option.text).foregroundStyle(.primary)
                                                                Spacer()
                                                                Text("\(option.voterCount)").font(.caption2).foregroundStyle(.secondary)
                                                            }.padding(.vertical, 3)
                                                        }.buttonStyle(.plain)
                                                    }
                                                }.frame(maxWidth: .infinity, alignment: .leading)
                                            case "voice":
                                                Button { Task { await voicePlayback.toggle(message: message, messaging: messaging) } } label: {
                                                    HStack(spacing: 10) {
                                                        Image(systemName: voicePlayback.playingMessageId == message.id ? "stop.circle.fill" : "play.circle.fill").font(.title2).foregroundStyle(Color.accentColor)
                                                        VStack(alignment: .leading) { Text("语音消息").fontWeight(.medium).foregroundStyle(.primary); Text(voicePlayback.playingMessageId == message.id ? "正在播放" : (message.mediaFileName ?? "录音")).font(.caption).foregroundStyle(.secondary) }
                                                        Spacer()
                                                    }
                                                }.buttonStyle(.plain)
                                            case "audio":
                                                HStack(spacing: 10) { Image(systemName: "music.note").font(.title2).foregroundStyle(Color.accentColor); Text(message.mediaFileName ?? "音频"); Spacer() }
                                            case "photo", "video", "document":
                                                Button { mediaViewerMessage = message } label: {
                                                    HStack(spacing: 9) {
                                                        Image(systemName: message.contentType == "photo" ? "photo.fill" : message.contentType == "video" ? "video.fill" : "doc.fill").font(.title2).foregroundStyle(Color.accentColor)
                                                        VStack(alignment: .leading) { Text(message.mediaFileName ?? message.text).fontWeight(.medium).foregroundStyle(.primary); Text(message.contentType == "photo" ? "图片 · 点击查看" : message.contentType == "video" ? "视频 · 点击播放" : "文件 · 点击打开").font(.caption).foregroundStyle(.secondary) }
                                                        Spacer()
                                                    }.frame(maxWidth: .infinity)
                                                }.buttonStyle(.plain)
                                            default:
                                                Text(message.text).foregroundStyle(.primary).frame(maxWidth: .infinity, alignment: .leading)
                                            }
                                            if !message.reactions.isEmpty {
                                                HStack(spacing: 5) {
                                                    ForEach(Array(message.reactions.enumerated()), id: \.offset) { _, reaction in
                                                        Button {
                                                            Task {
                                                                await messaging.setReaction(
                                                                    conversationId: conversation.id,
                                                                    messageId: message.id,
                                                                    reaction: reaction.reaction,
                                                                    enabled: !reaction.chosenByMe
                                                                )
                                                            }
                                                        } label: {
                                                            Text("\(reaction.reaction) \(reaction.count)")
                                                                .font(.caption2)
                                                                .padding(.horizontal, 7)
                                                                .padding(.vertical, 3)
                                                                .background(
                                                                    reaction.chosenByMe
                                                                        ? Color.accentColor.opacity(0.25)
                                                                        : Color.white.opacity(0.08),
                                                                    in: Capsule()
                                                                )
                                                        }
                                                        .buttonStyle(.plain)
                                                        .accessibilityLabel(
                                                            reaction.chosenByMe
                                                                ? "取消表情 \(reaction.reaction)"
                                                                : "添加表情 \(reaction.reaction)"
                                                        )
                                                    }
                                                }.frame(maxWidth: .infinity, alignment: .leading)
                                            }
                                            HStack(spacing: 3) {
                                                if message.isEdited { Text("已编辑").font(.caption2).foregroundStyle(.secondary) }
                                                Text(message.time).font(.caption2).foregroundStyle(.secondary)
                                                if message.isOutgoing {
                                                    Image(systemName: message.deliveryState.lowercased().contains("read") ? "checkmark.checkmark" : message.deliveryState.lowercased().contains("deliver") ? "checkmark.checkmark" : "checkmark")
                                                        .font(.caption2).foregroundStyle(message.deliveryState.lowercased().contains("read") ? .blue : .secondary)
                                                }
                                            }
                                        }
                                        .padding(.horizontal, 11).padding(.vertical, 7)
                                        .background(message.isOutgoing ? Color.accentColor.opacity(0.20) : Color.white.opacity(0.10), in: RoundedRectangle(cornerRadius: 16))
                                        .overlay {
                                            if chatSearchTargetID == message.id {
                                                RoundedRectangle(cornerRadius: 16).stroke(Color.accentColor, lineWidth: 2)
                                            }
                                        }
                                        .simultaneousGesture(
                                            DragGesture(minimumDistance: 18)
                                                .onEnded { value in
                                                    guard value.translation.width > 58, abs(value.translation.height) < 70 else { return }
                                                    replyTarget = message
                                                    editingMessage = nil
                                                    UIImpactFeedbackGenerator(style: .light).impactOccurred()
                                                }
                                        )
                                        .contextMenu {
                                            Button("回复", systemImage: "arrowshape.turn.up.left") { replyTarget = message; editingMessage = nil }
                                            Button("转发", systemImage: "arrowshape.turn.up.right") { forwardMessage = message }
                                            let hasOwnThumbsUp = message.reactions.contains {
                                                $0.reaction == "👍" && $0.chosenByMe
                                            }
                                            Button(
                                                hasOwnThumbsUp ? "取消 👍" : "👍",
                                                systemImage: hasOwnThumbsUp ? "hand.thumbsup.slash" : "hand.thumbsup"
                                            ) {
                                                Task {
                                                    await messaging.setReaction(
                                                        conversationId: conversation.id,
                                                        messageId: message.id,
                                                        reaction: "👍",
                                                        enabled: !hasOwnThumbsUp
                                                    )
                                                }
                                            }
                                            if message.isOutgoing {
                                                Button("编辑", systemImage: "pencil") { editingMessage = message; replyTarget = nil; messageDraft = message.text }
                                            }
                                            Button(message.isPinned ? "取消置顶消息" : "置顶消息", systemImage: "pin") { Task { await messaging.setMessagePinned(conversationId: conversation.id, messageId: message.id, pinned: !message.isPinned) } }
                                            Button("删除", systemImage: "trash", role: .destructive) { Task { await messaging.deleteMessage(conversationId: conversation.id, messageId: message.id) } }
                                        }
                                        if !message.isOutgoing { Spacer(minLength: 56) }
                                    }.padding(.horizontal, 10).id(message.id)
                                }
                            }.padding(.vertical, 12)
                        }
                        .onAppear {
                            guard let target = pendingInitialMessageID,
                                  messaging.messagesByConversation[conversation.id]?.contains(where: { $0.id == target }) == true
                            else { return }
                            pendingInitialMessageID = nil
                            DispatchQueue.main.async { withAnimation { proxy.scrollTo(target, anchor: .center) } }
                        }
                        .onChange(of: chatSearchTargetID) { _, target in
                            guard let target else { return }
                            withAnimation { proxy.scrollTo(target, anchor: .center) }
                        }
                        .onChange(of: messaging.messagesByConversation[conversation.id]?.count ?? 0) { _, _ in
                            if let target = pendingInitialMessageID,
                               messaging.messagesByConversation[conversation.id]?.contains(where: { $0.id == target }) == true {
                                pendingInitialMessageID = nil
                                withAnimation { proxy.scrollTo(target, anchor: .center) }
                            } else if pendingInitialMessageID == nil,
                                      let id = messaging.messagesByConversation[conversation.id]?.last?.id {
                                withAnimation { proxy.scrollTo(id, anchor: .bottom) }
                            }
                        }
                    }
                    if let editingMessage {
                        HStack {
                            Image(systemName: "pencil")
                            VStack(alignment: .leading, spacing: 2) { Text("编辑消息").font(.caption.bold()); Text(editingMessage.text).font(.caption).lineLimit(1) }
                            Spacer()
                            Button { self.editingMessage = nil; messageDraft = "" } label: { Image(systemName: "xmark.circle.fill") }
                        }.padding(.horizontal, 12).padding(.vertical, 6).background(.ultraThinMaterial)
                    } else if let replyTarget {
                        HStack {
                            Image(systemName: "arrowshape.turn.up.left")
                            VStack(alignment: .leading, spacing: 2) { Text("回复").font(.caption.bold()); Text(replyTarget.text).font(.caption).lineLimit(1) }
                            Spacer()
                            Button { self.replyTarget = nil } label: { Image(systemName: "xmark.circle.fill") }
                        }.padding(.horizontal, 12).padding(.vertical, 6).background(.ultraThinMaterial)
                    }
                    if humanHandoffBusy || humanHandoffError != nil {
                        HStack(spacing: 8) {
                            if humanHandoffBusy {
                                ProgressView()
                                Text("Agent 正在从这段 Human 会话继续…")
                            } else if let humanHandoffError {
                                Image(systemName: "exclamationmark.triangle.fill")
                                    .foregroundStyle(.orange)
                                Text(humanHandoffError)
                            }
                            Spacer()
                            if humanHandoffError != nil {
                                Button {
                                    self.humanHandoffError = nil
                                } label: {
                                    Image(systemName: "xmark.circle.fill")
                                }
                            }
                        }
                        .font(.caption)
                        .padding(.horizontal, 12)
                        .padding(.vertical, 6)
                        .background(.ultraThinMaterial)
                    }
                    if voiceRecorder.isRecording {
                        HStack(spacing: 10) {
                            Circle().fill(Color.red).frame(width: 9, height: 9)
                            Text("正在录音 \(voiceRecorder.elapsedSeconds / 60):\(String(format: "%02d", voiceRecorder.elapsedSeconds % 60))").font(.subheadline).fontWeight(.semibold)
                            Spacer()
                            Button("取消", role: .destructive) { voiceRecorder.cancel() }
                        }.padding(.horizontal, 12).padding(.vertical, 7).background(.ultraThinMaterial)
                    }
                    HStack(alignment: .bottom, spacing: 8) {
                        Menu {
                            Button("照片或视频", systemImage: "photo") { attachmentPickerPresented = true }
                            Button("文件", systemImage: "doc") { attachmentPickerPresented = true }
                            Button("位置", systemImage: "location") { locationSharePresented = true; locationService.requestLocation() }
                            Button("联系人", systemImage: "person.crop.circle") { contactSharePresented = true }
                            Button("投票", systemImage: "chart.bar.fill") { pollQuestion = ""; pollOption1 = ""; pollOption2 = ""; pollOption3 = ""; pollComposerPresented = true }
                        } label: { Image(systemName: "paperclip").font(.title3).frame(width: 36, height: 36) }
                        TextField("消息", text: $messageDraft, axis: .vertical).lineLimit(1...5)
                            .onChange(of: messageDraft) { _, value in
                                Task {
                                    if value.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { await messaging.stopTyping(conversation.id) }
                                    else { await messaging.startTyping(conversation.id) }
                                }
                                scheduleDraftSync(conversationId: conversation.id)
                            }
                            .onChange(of: replyTarget?.id) { _, _ in scheduleDraftSync(conversationId: conversation.id) }
                            .padding(.horizontal, 12).padding(.vertical, 9).background(Color.white.opacity(0.10), in: RoundedRectangle(cornerRadius: 18))
                        Button {
                            if messageDraft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                                if voiceRecorder.isRecording {
                                    if let recording = voiceRecorder.stop() {
                                        Task {
                                            do { try await messaging.sendVoice(conversationId: conversation.id, fileName: recording.url.lastPathComponent, mimeType: "audio/mp4", data: recording.data) }
                                            catch { model.message = "语音发送失败：\(error.localizedDescription)" }
                                        }
                                    }
                                } else {
                                    Task { await voiceRecorder.start() }
                                }
                            } else {
                                sendMessage(in: conversation)
                            }
                        } label: {
                            Image(systemName: messageDraft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? (voiceRecorder.isRecording ? "stop.fill" : "mic.fill") : "arrow.up")
                                .font(.system(size: 18, weight: .bold)).foregroundStyle(.white).frame(width: 38, height: 38).background(voiceRecorder.isRecording ? Color.red : Color.accentColor, in: Circle())
                        }
                        .contextMenu {
                            if !messageDraft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                                Button("静默发送", systemImage: "bell.slash.fill") { sendMessage(in: conversation, silent: true) }
                                Button("1 小时后发送", systemImage: "clock.fill") { sendMessage(in: conversation, scheduledAtMs: Int64(Date().addingTimeInterval(3600).timeIntervalSince1970 * 1000)) }
                                Button("明天上午 9:00", systemImage: "calendar") {
                                    let calendar = Calendar.current
                                    let tomorrow = calendar.date(byAdding: .day, value: 1, to: Date()) ?? Date().addingTimeInterval(86400)
                                    let scheduled = calendar.date(bySettingHour: 9, minute: 0, second: 0, of: tomorrow) ?? tomorrow
                                    sendMessage(in: conversation, scheduledAtMs: Int64(scheduled.timeIntervalSince1970 * 1000))
                                }
                            }
                        }
                    }.padding(.horizontal, 8).padding(.vertical, 7).background(.ultraThinMaterial)
                }
            }
            .navigationTitle("")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .principal) {
                    Button { conversationInfoPresented = true } label: {
                        VStack(spacing: 1) { Text(conversation.title).font(.headline); Text("\(conversation.participants.count) 位成员").font(.caption2).foregroundStyle(.secondary) }
                    }.buttonStyle(.plain)
                }
                ToolbarItem(placement: .topBarLeading) { Button { chatSearchPresented = false; chatSearchQuery = ""; chatSearchMatchIndex = nil; chatSearchTargetID = nil; selectedConversation = nil } label: { Image(systemName: "chevron.left") } }
                ToolbarItem(placement: .topBarTrailing) {
                    Menu {
                        Button(chatSearchPresented ? "关闭搜索" : "搜索", systemImage: "magnifyingglass") {
                            chatSearchPresented.toggle()
                            if !chatSearchPresented {
                                chatSearchQuery = ""
                                chatSearchMatchIndex = nil
                                chatSearchTargetID = nil
                            }
                        }
                        Menu {
                            if humanHandoffAgents.isEmpty {
                                Text("没有可用的 Agent")
                            } else {
                                ForEach(humanHandoffAgents) { agent in
                                    Button(agent.name, systemImage: "sparkles") {
                                        startHumanHandoff(
                                            to: agent,
                                            conversation: conversation,
                                            messages: messages
                                        )
                                    }
                                    .disabled(humanHandoffBusy)
                                }
                            }
                        } label: {
                            Label(
                                humanHandoffBusy ? "Agent 正在接手…" : "Ask Agent",
                                systemImage: "sparkles"
                            )
                        }
                        .disabled(humanHandoffBusy || bridge == nil)
                        Button(conversation.isMuted ? "取消静音" : "静音", systemImage: "speaker.slash") { Task { await messaging.setMuted(conversation.id, muted: !conversation.isMuted) } }
                        Button(conversation.isPinned ? "取消置顶" : "置顶", systemImage: "pin") { Task { await messaging.setPinned(conversation.id, pinned: !conversation.isPinned) } }
                        Button("标为未读", systemImage: "circle.fill") { Task { await messaging.setMarkedUnread(conversation.id, markedUnread: true) }; selectedConversation = nil }
                        Button("归档", systemImage: "archivebox") { Task { await messaging.setArchived(conversation.id, archived: true) }; selectedConversation = nil }
                    } label: { Image(systemName: "ellipsis.circle") }
                }
            }
        }
        .task(id: conversation.id) {
            chatSearchMatchIndex = nil
            chatSearchTargetID = nil
            humanHandoffConversationId = conversation.id
            humanHandoffAgents = []
            humanHandoffBusy = false
            humanHandoffError = nil
            if let bridge {
                do {
                    let agents = try await GrokMobileBotService(bridge: bridge).loadOnboardingAgents()
                    guard !Task.isCancelled, humanHandoffConversationId == conversation.id else { return }
                    humanHandoffAgents = agents
                } catch is CancellationError {
                    return
                } catch {
                    guard !Task.isCancelled, humanHandoffConversationId == conversation.id else { return }
                    humanHandoffError = "无法加载 Agent：\(error.localizedDescription)"
                }
            }
            let draft = messaging.draftsByConversation[conversation.id]
            messageDraft = draft?.text ?? ""
            replyTarget = draft?.replyToMessageId.flatMap { replyId in messaging.messagesByConversation[conversation.id]?.first(where: { $0.id == replyId }) }
        }
        .sheet(isPresented: $conversationInfoPresented) {
            ConversationInfoView(conversationId: conversation.id, messaging: messaging) { conversationInfoPresented = false }
        }
        .fullScreenCover(item: $mediaViewerMessage) { message in
            MediaViewer(message: message, messaging: messaging) { mediaViewerMessage = nil }
        }
        .sheet(item: $forwardMessage) { message in
            ForwardMessageSheet(
                sourceConversationId: conversation.id,
                message: message,
                messaging: messaging,
                appAgentSurface: appAgentSurface
            ) {
                forwardMessage = nil
            }
        }
        .fileImporter(isPresented: $attachmentPickerPresented, allowedContentTypes: [.item], allowsMultipleSelection: false) { result in
            guard case let .success(urls) = result, let url = urls.first else { return }
            let accessed = url.startAccessingSecurityScopedResource()
            defer { if accessed { url.stopAccessingSecurityScopedResource() } }
            do {
                let data = try Data(contentsOf: url)
                let values = try? url.resourceValues(forKeys: [.contentTypeKey])
                let mime = values?.contentType?.preferredMIMEType ?? "application/octet-stream"
                Task {
                    do { try await messaging.sendAttachment(conversationId: conversation.id, fileName: url.lastPathComponent, mimeType: mime, data: data) }
                    catch { model.message = "附件发送失败：\(error.localizedDescription)" }
                }
            } catch { model.message = "读取附件失败：\(error.localizedDescription)" }
        }
        .sheet(isPresented: $locationSharePresented) {
            NavigationStack {
                VStack(spacing: 18) {
                    if locationService.loading { ProgressView("正在获取位置…") }
                    else if let coordinate = locationService.coordinate {
                        Image(systemName: "location.circle.fill").font(.system(size: 52)).foregroundStyle(Color.accentColor)
                        Text("纬度 \(coordinate.latitude, specifier: "%.6f")")
                        Text("经度 \(coordinate.longitude, specifier: "%.6f")")
                        Button("发送此位置") {
                            Task {
                                do { try await messaging.sendLocation(conversationId: conversation.id, latitude: coordinate.latitude, longitude: coordinate.longitude); locationSharePresented = false }
                                catch { model.message = "位置发送失败：\(error.localizedDescription)" }
                            }
                        }.buttonStyle(.borderedProminent)
                    } else {
                        ContentUnavailableView("无法获取位置", systemImage: "location.slash", description: Text(locationService.errorMessage ?? "请检查位置权限"))
                        Button("重试") { locationService.requestLocation() }
                    }
                }.padding().navigationTitle("发送位置")
                .toolbar { ToolbarItem(placement: .cancellationAction) { Button("取消") { locationSharePresented = false } } }
            }
        }
        .sheet(isPresented: $contactSharePresented) {
            NavigationStack {
                List(messaging.contacts) { contact in
                    Button {
                        Task {
                            do { try await messaging.sendContact(conversationId: conversation.id, contact: contact); contactSharePresented = false }
                            catch { model.message = "联系人发送失败：\(error.localizedDescription)" }
                        }
                    } label: { Text(contact.displayName) }
                }
                .navigationTitle("发送联系人")
                .toolbar { ToolbarItem(placement: .cancellationAction) { Button("取消") { contactSharePresented = false } } }
            }
        }
        .sheet(isPresented: $pollComposerPresented) {
            NavigationStack {
                Form {
                    Section("问题") { TextField("输入问题", text: $pollQuestion) }
                    Section("选项") {
                        TextField("选项 1", text: $pollOption1)
                        TextField("选项 2", text: $pollOption2)
                        TextField("选项 3（可选）", text: $pollOption3)
                    }
                }
                .navigationTitle("新建投票")
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) { Button("取消") { pollComposerPresented = false } }
                    ToolbarItem(placement: .confirmationAction) {
                        Button("发送") {
                            let question = pollQuestion; let options = [pollOption1, pollOption2, pollOption3]
                            Task {
                                do { try await messaging.sendPoll(conversationId: conversation.id, question: question, options: options); pollComposerPresented = false }
                                catch { model.message = "投票发送失败：\(error.localizedDescription)" }
                            }
                        }.disabled(pollQuestion.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || pollOption1.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || pollOption2.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                    }
                }
            }
        }
    }

    func startHumanHandoff(
        to agent: MobileBotSummary,
        conversation: ConversationSummary,
        messages: [ChatMessage]
    ) {
        guard let bridge, !humanHandoffBusy else { return }
        let transcriptLines = messages.compactMap { message -> String? in
            let text = message.text.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !text.isEmpty else { return nil }
            let author = message.isOutgoing
                ? "You"
                : (messaging.searchAuthorByMessageId[message.id] ?? conversation.title)
            return "\(author): \(text)"
        }
        guard let prompt = GrokMobileBotService.humanHandoffPrompt(
            conversationTitle: conversation.title,
            transcriptLines: transcriptLines
        ) else {
            humanHandoffError = "这段 Human 会话还没有可交给 Agent 的消息。"
            return
        }

        humanHandoffBusy = true
        humanHandoffError = nil
        humanHandoffConversationId = conversation.id
        Task {
            do {
                try await GrokMobileBotService(bridge: bridge).handoffHumanConversation(
                    agentId: agent.id,
                    humanConversationId: conversation.id,
                    prompt: prompt
                )
                guard !Task.isCancelled, humanHandoffConversationId == conversation.id else { return }
                await messaging.refresh()
                guard !Task.isCancelled, humanHandoffConversationId == conversation.id else { return }
                humanHandoffBusy = false
            } catch is CancellationError {
                if humanHandoffConversationId == conversation.id {
                    humanHandoffBusy = false
                }
            } catch {
                guard humanHandoffConversationId == conversation.id else { return }
                humanHandoffBusy = false
                humanHandoffError = "Agent 接手失败：\(error.localizedDescription)"
            }
        }
    }

    func navigateChatSearch(matches: [ChatSearchMatch], delta: Int) {
        guard let next = nextChatSearchIndex(current: chatSearchMatchIndex, count: matches.count, delta: delta) else {
            chatSearchMatchIndex = nil
            chatSearchTargetID = nil
            return
        }
        chatSearchMatchIndex = next
        chatSearchTargetID = matches[next].entryId
    }

    func sendMessage(in conversation: ConversationSummary, silent: Bool = false, scheduledAtMs: Int64? = nil) {
        let text = messageDraft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        let edit = editingMessage
        let reply = replyTarget
        draftSyncTask?.cancel()
        messageDraft = ""
        Task { await messaging.stopTyping(conversation.id); await messaging.setDraft(conversationId: conversation.id, text: "", replyToMessageId: nil) }
        editingMessage = nil
        replyTarget = nil
        Task {
            do {
                if let edit { try await messaging.editText(conversationId: conversation.id, messageId: edit.id, text: text) }
                else { try await messaging.sendText(conversationId: conversation.id, text: text, replyToMessageId: reply?.id, silent: silent, scheduledAtMs: scheduledAtMs) }
            } catch { model.message = "发送失败：\(error.localizedDescription)" }
        }
    }

    func scheduleDraftSync(conversationId: String) {
        guard editingMessage == nil else { return }
        draftSyncTask?.cancel()
        let text = messageDraft
        let replyId = replyTarget?.id
        draftSyncTask = Task {
            try? await Task.sleep(for: .milliseconds(350))
            guard !Task.isCancelled else { return }
            await messaging.setDraft(conversationId: conversationId, text: text, replyToMessageId: replyId)
        }
    }

    var avatar: some View {
        ZStack {
            Circle().fill(Color.white.opacity(0.08))
            Text("✦").font(.system(size: 22, weight: .bold)).foregroundStyle(.orange)
        }
    }

    var marketplaceView: some View {
        NavigationStack {
            List {
                Section {
                    VStack(alignment: .leading, spacing: 8) {
                        Text("MAHAYANA RUST HOST")
                            .font(.caption.bold())
                            .foregroundStyle(.secondary)
                        HStack {
                            Text("全球法布施").font(.largeTitle.bold())
                            Spacer()
                            Text("SwiftUI · Rust")
                                .font(.caption.bold())
                                .accessibilityIdentifier("runtime-badge")
                        }
                        .accessibilityElement(children: .contain)
                    }
                    .accessibilityElement(children: .contain)
                }


                Section {
                    Picker("插件视图", selection: $model.pluginBrowserTab) {
                        ForEach(MarketplaceBrowserTab.allCases) { tab in
                            Text(tab.rawValue).tag(tab)
                        }
                    }
                    .pickerStyle(.segmented)
                    .accessibilityIdentifier("plugin-browser-tabs")
                }

                if model.pluginBrowserTab == .marketplace {
                Section("本地插件市场") {
                    Text("iOS 主壳使用 SwiftUI；MiniApp 使用受控 WebMCP Surface；代码从 GitHub 固定版本拉取并由共享 Mahayana Rust Host 校验、安装、更新。")
                    TextField("搜索插件", text: $model.query)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .accessibilityIdentifier("marketplace-search")
                    Button("搜索") {
                        Task { await model.refresh() }
                    }
                    .disabled(model.loading)
                    .accessibilityIdentifier("marketplace-search-submit")
                }
                }

                Section("Host 状态") {
                    HStack(spacing: 12) {
                        if model.loading { ProgressView() }
                        Text(model.message)
                    }
                    .accessibilityElement(children: .contain)
                    .accessibilityIdentifier("host-status")

                    if let featureHostSmokeStatus = model.featureHostSmokeStatus {
                        Text(featureHostSmokeStatus)
                            .accessibilityIdentifier("feature-host-smoke")
                    }
                }

                if model.pluginBrowserTab == .marketplace, let permission = model.permissionRequest {
                    Section("插件权限") {
                        VStack(alignment: .leading, spacing: 8) {
                            Text(permission.pluginId)
                                .font(.headline)
                            Text("此插件请求以下权限。只有明确批准后，Fabushi 才会把这些权限授予已安装插件。")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                            ForEach(permission.permissions, id: \.self) { capability in
                                Label(capability, systemImage: "lock.shield")
                                    .font(.caption)
                            }
                            HStack(spacing: 8) {
                                Button("拒绝") {
                                    model.denyPermissions()
                                }
                                .buttonStyle(.bordered)
                                .accessibilityIdentifier("plugin-permission-deny-\(permission.pluginId)")

                                Button("批准并继续") {
                                    Task { await model.approvePermissions() }
                                }
                                .buttonStyle(.borderedProminent)
                                .accessibilityIdentifier("plugin-permission-approve-\(permission.pluginId)")
                            }
                        }
                        .accessibilityElement(children: .contain)
                        .accessibilityIdentifier("plugin-permission-request-\(permission.pluginId)")
                    }
                }


                if model.pluginBrowserTab == .yours {
                    Section("我的 Skills") {
                        if !model.hasPrivateSkillAgentScope {
                            Label(
                                "请从某个 Agent 的“设置 → Skills”进入 Yours；这里不会默认绑定主助手。",
                                systemImage: "person.crop.circle.badge.exclamationmark"
                            )
                            .foregroundStyle(.secondary)
                            .accessibilityIdentifier("plugin-yours-agent-scope-required")
                        }
                        HStack(spacing: 8) {
                            TextField("搜索 Yours", text: $model.privateSkillQuery)
                                .textInputAutocapitalization(.never)
                                .autocorrectionDisabled()
                                .accessibilityIdentifier("plugin-yours-search")
                            Button("刷新") {
                                Task { await model.refreshPrivateSkills() }
                            }
                            .disabled(model.privateSkillsLoading || !model.hasPrivateSkillAgentScope)
                            .accessibilityIdentifier("plugin-yours-refresh")
                        }

                        if model.hasPrivateSkillAgentScope {
                        Picker("来源", selection: $model.privateSkillOwnershipFilter) {
                            ForEach(MarketplaceSkillOwnershipFilter.allCases) { filter in
                                Text(filter.rawValue).tag(filter)
                            }
                        }
                        .pickerStyle(.segmented)
                        .accessibilityIdentifier("plugin-yours-ownership-filter")

                        if model.hasPrivateSkillAgentScope {
                            HStack(spacing: 8) {
                                if model.skillPublishTargetsLoading {
                                    ProgressView()
                                }
                                if !model.skillPublishTargets.isEmpty {
                                    Picker(
                                        "发布到",
                                        selection: Binding(
                                            get: {
                                                model.selectedSkillPublishTeamId
                                                    ?? model.skillPublishTargets.first?.teamId
                                                    ?? 0
                                            },
                                            set: { model.selectedSkillPublishTeamId = $0 }
                                        )
                                    ) {
                                        ForEach(model.skillPublishTargets) { target in
                                            Text(target.name).tag(target.teamId)
                                        }
                                    }
                                    .accessibilityIdentifier("plugin-yours-publish-target")
                                }
                                Spacer()
                                Button(
                                    model.skillPublishTargets.isEmpty ? "加载发布团队" : "刷新发布团队"
                                ) {
                                    Task { await model.refreshSkillPublishTargets() }
                                }
                                .disabled(
                                    model.skillPublishTargetsLoading
                                        || model.privateSkillPublishingId != nil
                                )
                                .accessibilityIdentifier("plugin-yours-publish-target-refresh")
                            }
                        }

                        if model.privateSkillsLoading {
                            ProgressView("正在读取 authoritative workflow state…")
                        }
                        if let error = model.privateSkillError {
                            Text(error)
                                .font(.caption)
                                .foregroundStyle(.red)
                                .accessibilityIdentifier("plugin-yours-error")
                        }
                        if model.visiblePrivateSkills.isEmpty && !model.privateSkillsLoading {
                            Text("当前没有匹配的 Skills。")
                                .foregroundStyle(.secondary)
                        }

                        ForEach(model.visiblePrivateSkills) { skill in
                            DisclosureGroup {
                                VStack(alignment: .leading, spacing: 8) {
                                    TextField(
                                        "名称",
                                        text: Binding(
                                            get: { model.privateSkillNameDrafts[skill.id] ?? skill.name },
                                            set: { model.privateSkillNameDrafts[skill.id] = $0 }
                                        )
                                    )
                                    .disabled(!skill.canEdit)

                                    TextField(
                                        "Description / Use when",
                                        text: Binding(
                                            get: { model.privateSkillDescriptionDrafts[skill.id] ?? skill.description },
                                            set: { model.privateSkillDescriptionDrafts[skill.id] = $0 }
                                        ),
                                        axis: .vertical
                                    )
                                    .disabled(!skill.canEdit)

                                    TextEditor(
                                        text: Binding(
                                            get: { model.privateSkillBodyDrafts[skill.id] ?? skill.body },
                                            set: { model.privateSkillBodyDrafts[skill.id] = $0 }
                                        )
                                    )
                                    .frame(minHeight: 88)
                                    .disabled(!skill.canEdit)
                                    .accessibilityIdentifier("plugin-skill-body-\(skill.id)")

                                    if skill.canEdit {
                                        HStack(spacing: 8) {
                                            Button("保存") {
                                                Task { await model.savePrivateSkill(skill) }
                                            }
                                            .disabled(
                                                model.privateSkillMutatingId != nil
                                                    || model.privateSkillPublishingId != nil
                                            )
                                            .accessibilityIdentifier("plugin-skill-save-\(skill.id)")

                                            Button("发布到团队") {
                                                Task { await model.publishPrivateSkill(skill) }
                                            }
                                            .disabled(
                                                model.privateSkillMutatingId != nil
                                                    || model.privateSkillPublishingId != nil
                                            )
                                            .accessibilityIdentifier("plugin-skill-publish-\(skill.id)")

                                            Button("删除", role: .destructive) {
                                                Task { await model.deletePrivateSkill(skill) }
                                            }
                                            .disabled(
                                                model.privateSkillMutatingId != nil
                                                    || model.privateSkillPublishingId != nil
                                            )
                                            .accessibilityIdentifier("plugin-skill-delete-\(skill.id)")
                                        }
                                    }

                                    if skill.source == "plugin", skill.publishedByCurrentUser {
                                        HStack(spacing: 8) {
                                            Button("同步更新") {
                                                Task { await model.resyncPublishedSkill(skill) }
                                            }
                                            .disabled(model.privateSkillPublishingId != nil)
                                            .accessibilityIdentifier("plugin-skill-sync-\(skill.id)")

                                            Button("取消发布", role: .destructive) {
                                                Task { await model.unpublishPublishedSkill(skill) }
                                            }
                                            .disabled(model.privateSkillPublishingId != nil)
                                            .accessibilityIdentifier("plugin-skill-unpublish-\(skill.id)")
                                        }
                                    }

                                    if model.privateSkillPublishingId == skill.id {
                                        ProgressView("正在核对 authoritative 发布状态…")
                                            .font(.caption)
                                    }

                                    if skill.source == "plugin", let pluginId = skill.pluginId {
                                        Text("Plugin: \(pluginId)")
                                            .font(.caption2.monospaced())
                                            .foregroundStyle(.secondary)
                                    }
                                    if skill.publishedByCurrentUser {
                                        Text("由当前账号发布")
                                            .font(.caption)
                                            .foregroundStyle(.secondary)
                                    }
                                }
                            } label: {
                                HStack(spacing: 10) {
                                    VStack(alignment: .leading, spacing: 2) {
                                        Text(skill.name).font(.headline)
                                        Text(skill.sourceLabel)
                                            .font(.caption)
                                            .foregroundStyle(.secondary)
                                    }
                                    Spacer()
                                    if skill.canToggle {
                                        Toggle(
                                            "启用",
                                            isOn: Binding(
                                                get: { skill.isEnabledForAgent },
                                                set: { enabled in
                                                    Task { await model.setPrivateSkillEnabled(skill, enabled: enabled) }
                                                }
                                            )
                                        )
                                        .labelsHidden()
                                        .disabled(model.privateSkillMutatingId != nil)
                                        .accessibilityIdentifier("plugin-skill-enabled-\(skill.id)")
                                    }
                                }
                            }
                            .accessibilityIdentifier("plugin-skill-\(skill.id)")
                        }

                        Text("Publish / Sync / Unpublish 由 Host-owned lifecycle 管理：发布只有在 authoritative pluginId + commit SHA 确认后才移除 private copy；取消发布会先恢复 private copy，再执行远端 unpublish。")
                            .font(.caption2)
                            .foregroundStyle(.secondary)
                            .accessibilityIdentifier("plugin-yours-publish-lifecycle-note")
                        }
                    }

                Section("MCP 连接器") {
                    if !model.loggedIn {
                        Text("登录 Fabushi 后可管理当前账号的 MCP 连接器与工具。")
                            .foregroundStyle(.secondary)
                    } else {
                        HStack(spacing: 10) {
                            if model.mcpBackendLoggedIn {
                                VStack(alignment: .leading, spacing: 2) {
                                    Text("MCP 账号后端已连接")
                                        .font(.subheadline.weight(.medium))
                                    if !model.mcpBackendEmail.isEmpty {
                                        Text(model.mcpBackendEmail)
                                            .font(.caption)
                                            .foregroundStyle(.secondary)
                                    }
                                }
                                Spacer()
                                Button("退出") {
                                    Task { await model.logoutMcpBackend() }
                                }
                                .disabled(model.mcpBackendBusy)
                                .accessibilityIdentifier("mcp-backend-logout")
                            } else {
                                Text("连接账号后可使用 Desktop 同源的多账号 OAuth、重命名与移除。")
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                                Spacer()
                                Button(model.mcpBackendBusy ? "连接中…" : "连接") {
                                    Task { await model.beginMcpBackendLogin() }
                                }
                                .disabled(model.mcpBackendBusy)
                                .accessibilityIdentifier("mcp-backend-login")
                            }
                        }

                        HStack {
                            Text("服务器与工具状态由同一 Coordinator / SandMcpManager 管理。")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                            Spacer()
                            if model.mcpLoading { ProgressView() }
                            Button("刷新") {
                                Task {
                                    await model.refreshMcpBackendStatus()
                                    await model.refreshMcpServers()
                                }
                            }
                            .disabled(model.mcpLoading)
                            .accessibilityIdentifier("mcp-refresh")
                        }

                        if let error = model.mcpError {
                            Text(error)
                                .font(.caption)
                                .foregroundStyle(.red)
                                .accessibilityIdentifier("mcp-error")
                        }

                        if model.mcpServers.isEmpty && !model.mcpLoading {
                            Text("当前账号没有可管理的 MCP 服务器。")
                                .foregroundStyle(.secondary)
                        }

                        ForEach(model.mcpServers) { server in
                            VStack(alignment: .leading, spacing: 8) {
                                HStack {
                                    VStack(alignment: .leading, spacing: 2) {
                                        Text(server.name).font(.headline)
                                        Text("\(server.transport.uppercased()) · \(server.status) · \(server.toolCount) 个启用工具")
                                            .font(.caption)
                                            .foregroundStyle(.secondary)
                                        Text("账号：\(server.accountKey)")
                                            .font(.caption2)
                                            .foregroundStyle(.secondary)
                                        HStack(spacing: 6) {
                                            if server.isTeamServer {
                                                Label("团队", systemImage: "person.3.fill")
                                            }
                                            if server.isRequired {
                                                Label("团队必需", systemImage: "lock.fill")
                                            }
                                            if server.managedByTeamPluginPolicy {
                                                Label("团队策略管理", systemImage: "building.2.fill")
                                            }
                                            if server.isDisabledByTeamAdminPolicy {
                                                Label("管理员已禁用", systemImage: "nosign")
                                            }
                                        }
                                        .font(.caption2)
                                        .foregroundStyle(.secondary)
                                        if let detail = server.statusDetail, !detail.isEmpty {
                                            Text(detail).font(.caption2).foregroundStyle(.secondary)
                                        }
                                    }
                                    Spacer()
                                    Button(model.mcpLoadingServerId == server.serverId ? "读取中…" : "工具") {
                                        Task { await model.loadMcpTools(serverId: server.serverId) }
                                    }
                                    .disabled(
                                        model.mcpLoadingServerId == server.serverId
                                            || server.isDisabledByTeamAdminPolicy
                                    )
                                }
                                .accessibilityElement(children: .contain)
                                .accessibilityIdentifier("mcp-server-\(server.id)")

                                if model.mcpBackendLoggedIn && !server.isDisabledByTeamAdminPolicy {
                                    HStack(spacing: 8) {
                                        if server.status == "needsAuth" {
                                            Button("连接此账号") {
                                                Task {
                                                    await model.authenticateMcpServer(
                                                        serverId: server.serverId,
                                                        accountKey: server.accountKey
                                                    )
                                                }
                                            }
                                            .accessibilityIdentifier("mcp-account-auth-\(server.id)")
                                        } else {
                                            Button("重新认证") {
                                                Task {
                                                    await model.authenticateMcpServer(
                                                        serverId: server.serverId,
                                                        accountKey: server.accountKey,
                                                        forceReauth: true
                                                    )
                                                }
                                            }
                                            Button("退出账号") {
                                                Task {
                                                    await model.logoutMcpAccount(
                                                        serverId: server.serverId,
                                                        accountKey: server.accountKey
                                                    )
                                                }
                                            }
                                        }
                                        Button("移除账号", role: .destructive) {
                                            Task {
                                                await model.removeMcpAccount(
                                                    serverId: server.serverId,
                                                    accountKey: server.accountKey
                                                )
                                            }
                                        }
                                    }
                                    .buttonStyle(.borderless)

                                    if server.accountKey != DEFAULT_MCP_ACCOUNT_KEY {
                                        HStack(spacing: 8) {
                                            TextField(
                                                "重命名账号",
                                                text: Binding(
                                                    get: {
                                                        model.mcpRenameDraftByIdentity[server.id]
                                                            ?? server.accountKey
                                                    },
                                                    set: {
                                                        model.mcpRenameDraftByIdentity[server.id] = $0
                                                    }
                                                )
                                            )
                                            .textInputAutocapitalization(.never)
                                            .autocorrectionDisabled()
                                            Button("重命名") {
                                                let next = model.mcpRenameDraftByIdentity[server.id]
                                                    ?? server.accountKey
                                                Task {
                                                    await model.renameMcpAccount(
                                                        serverId: server.serverId,
                                                        accountKey: server.accountKey,
                                                        newAccountKey: next
                                                    )
                                                }
                                            }
                                        }
                                    }

                                    HStack(spacing: 8) {
                                        TextField(
                                            "新账号标签",
                                            text: Binding(
                                                get: {
                                                    model.mcpNewAccountDraftByServerId[server.serverId]
                                                        ?? ""
                                                },
                                                set: {
                                                    model.mcpNewAccountDraftByServerId[server.serverId] = $0
                                                }
                                            )
                                        )
                                        .textInputAutocapitalization(.never)
                                        .autocorrectionDisabled()
                                        Button("添加账号") {
                                            let key = model.mcpNewAccountDraftByServerId[server.serverId]
                                                ?? ""
                                            Task {
                                                await model.authenticateMcpServer(
                                                    serverId: server.serverId,
                                                    accountKey: key
                                                )
                                            }
                                        }
                                    }
                                }

                                if let tools = model.mcpToolsByServerId[server.serverId] {
                                    if tools.isEmpty {
                                        Text("此服务器没有报告可管理工具。")
                                            .font(.caption)
                                            .foregroundStyle(.secondary)
                                    }
                                    ForEach(tools) { tool in
                                        HStack(alignment: .top, spacing: 10) {
                                            VStack(alignment: .leading, spacing: 2) {
                                                Text(tool.title ?? tool.name)
                                                    .font(.subheadline.weight(.medium))
                                                if let description = tool.description, !description.isEmpty {
                                                    Text(description)
                                                        .font(.caption)
                                                        .foregroundStyle(.secondary)
                                                }
                                            }
                                            Spacer()
                                            Toggle(
                                                "启用",
                                                isOn: Binding(
                                                    get: { !tool.isDisabled },
                                                    set: { enabled in
                                                        Task {
                                                            await model.setMcpToolEnabled(
                                                                serverId: server.serverId,
                                                                toolName: tool.name,
                                                                enabled: enabled
                                                            )
                                                        }
                                                    }
                                                )
                                            )
                                            .labelsHidden()
                                            .disabled(
                                                server.isDisabledByTeamAdminPolicy
                                                    || model.mcpMutatingToolKey
                                                        == "\(server.serverId):\(tool.name)"
                                            )
                                            .accessibilityIdentifier(
                                                "mcp-tool-\(server.id)-\(tool.name)"
                                            )
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                }

                if model.pluginBrowserTab == .marketplace {
                Section("插件") {
                    if model.plugins.isEmpty && !model.loading {
                        Text("没有匹配的 iOS 插件。")
                    }
                    ForEach(model.plugins) { plugin in
                        VStack(alignment: .leading, spacing: 8) {
                            Group {
                                Text(plugin.pluginId).font(.caption.monospaced()).foregroundStyle(.secondary)
                                Text(plugin.displayName).font(.headline)
                                Text(plugin.description).foregroundStyle(.secondary)
                                if let version = plugin.latestVersion {
                                    Text("\(version) · GitHub \(plugin.sourceRef?.prefix(9) ?? "待确认")")
                                        .font(.caption)
                                }
                            }
                            .accessibilityElement(children: .combine)
                            .accessibilityIdentifier("plugin-\(plugin.pluginId)")

                            HStack(spacing: 8) {
                                Button("打开 WebMCP") {
                                    openedMiniApp = plugin
                                }
                                .accessibilityIdentifier("open-\(plugin.pluginId)")

                                Button(model.installingPluginId == plugin.pluginId ? "处理中…" : "安装 / 更新") {
                                    Task { await model.install(plugin) }
                                }
                                .disabled(plugin.latestVersion == nil || model.installingPluginId != nil)
                                .accessibilityIdentifier("install-\(plugin.pluginId)")
                            }
                        }
                    }
                }
                }
            }
            .navigationTitle("法布施")
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("消息") { destination = .home }
                        .accessibilityIdentifier("marketplace-back")
                }
            }
            .refreshable {
                if model.pluginBrowserTab == .marketplace {
                    await model.refresh()
                } else {
                    if model.hasPrivateSkillAgentScope {
                        await model.refreshPrivateSkills()
                    }
                    if model.loggedIn {
                        await model.refreshMcpServers()
                    }
                }
            }
            .task {
                if model.pluginBrowserTab == .marketplace {
                    if model.plugins.isEmpty {
                        await model.refresh()
                    }
                } else {
                    if model.hasPrivateSkillAgentScope && model.privateSkills.isEmpty {
                        await model.refreshPrivateSkills()
                    }
                    if model.loggedIn && model.mcpServers.isEmpty {
                        await model.refreshMcpServers()
                    }
                }
            }
            .onChange(of: model.pluginBrowserTab) { _, tab in
                Task {
                    if tab == .marketplace {
                        if model.plugins.isEmpty { await model.refresh() }
                    } else {
                        if model.hasPrivateSkillAgentScope {
                            await model.refreshPrivateSkills()
                        }
                        if model.loggedIn && model.mcpServers.isEmpty {
                            await model.refreshMcpServers()
                        }
                    }
                }
            }
        }
    }
}
