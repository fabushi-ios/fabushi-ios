import AVKit
import SwiftUI
import UIKit

enum MobileTranscriptLoadErrorCopy {
    static let title = "Couldn't load conversation"
    static let detail = "Couldn't load this conversation. Check your connection and try again."
    static let retry = "Retry"
}


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

@discardableResult
internal func applyMobileOptimisticUserEcho(
    _ event: [String: Any],
    messages: inout [MobileChatMessage]
) -> Bool {
    guard event["type"] as? String == "chat.message",
          event["role"] as? String == "user",
          let rawMessageId = event["messageId"] as? String
    else { return false }
    let messageId = rawMessageId.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !messageId.isEmpty,
          let index = messages.firstIndex(where: {
              $0.role == .user && ($0.canonicalMessageId ?? $0.id) == messageId
          })
    else { return false }

    messages[index].canonicalMessageId = messageId
    messages[index].optimisticDeliveryPhase = nil
    messages[index].optimisticDeliveryError = nil
    if let replyTo = event["replyToMessageId"] as? String,
       !replyTo.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
        messages[index].replyToMessageId = replyTo
    }
    if let branched = event["branched"] as? Bool {
        messages[index].branched = branched
    }
    return true
}

internal func projectMobileTranscriptCardWithFallback(
    event: [String: Any],
    operationId: String?
) -> MobileChatMessage? {
    if let projected = projectMobileTranscriptCard(event: event, operationId: operationId) {
        return projected
    }
    guard
        event["card"] is [String: Any],
        let rawEntryId = event["entryId"] as? String
    else {
        return nil
    }
    let entryId = rawEntryId.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !entryId.isEmpty else { return nil }
    return MobileChatMessage(
        id: "transcript-card-fallback:\(entryId)",
        role: .assistant,
        text: "This message can’t be shown in this version of Fabushi",
        kind: .notice,
        operationId: operationId,
        canonicalMessageId: entryId
    )
}

internal func projectMobileConversationWindowMessage(_ row: [String: Any]) -> MobileChatMessage? {
    guard
        let id = (row["id"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines),
        !id.isEmpty,
        let roleRaw = row["role"] as? String,
        let role = MobileChatRole(rawValue: roleRaw),
        let text = row["text"] as? String,
        let createdAtMs = GrokMobileBotService.int64Value(row["createdAtMs"])
    else {
        return nil
    }
    var replyToMessageId: String?
    if let rawReplyTo = row["replyToMessageId"] {
        guard let value = rawReplyTo as? String else { return nil }
        let normalized = value.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !normalized.isEmpty else { return nil }
        replyToMessageId = normalized
    }
    if row["branched"] != nil, row["branched"] is Bool == false { return nil }
    var message = MobileChatMessage(
        id: "history:\(id)",
        role: role,
        text: text,
        canonicalMessageId: id,
        replyToMessageId: replyToMessageId,
        reactions: projectMobileTranscriptReactions(row["reactions"]),
        branched: row["branched"] as? Bool ?? false
    )
    message.createdAt = Date(timeIntervalSince1970: TimeInterval(createdAtMs) / 1_000)
    return message
}

internal func projectMobileConversationWindowEntries(
    _ row: [String: Any]
) -> [MobileChatMessage]? {
    guard var message = projectMobileConversationWindowMessage(row) else { return nil }
    var projected: [MobileChatMessage] = []
    let cards: [[String: Any]]
    if let rawCards = row["cards"] {
        guard let typedCards = rawCards as? [[String: Any]] else { return nil }
        cards = typedCards
    } else {
        cards = []
    }
    if !message.text.isEmpty || cards.isEmpty {
        projected.append(message)
    }
    let sourceMessageId = message.canonicalMessageId ?? message.id
    for (index, card) in cards.enumerated() {
        let entryId = "\(sourceMessageId)-card-\(index)"
        guard var cardMessage = projectMobileTranscriptCardWithFallback(
            event: [
                "type": "transcript.card",
                "entryId": entryId,
                "card": card,
            ],
            operationId: nil
        ) else { return nil }
        cardMessage.createdAt = message.createdAt
        projected.append(cardMessage)
    }
    return projected
}

internal func mobileTranscriptCanonicalId(_ message: MobileChatMessage) -> String {
    message.canonicalMessageId ?? message.id
}

internal func mobileMainTranscriptEntries(_ entries: [MobileChatMessage]) -> [MobileChatMessage] {
    let topology = entries.map {
        TranscriptEntry(
            kind: "message",
            id: mobileTranscriptCanonicalId($0),
            replyTo: $0.replyToMessageId,
            branched: $0.branched
        )
    }
    let mainIds = Set(getMainTranscriptEntries(topology).compactMap(\.id))
    return entries.filter { mainIds.contains(mobileTranscriptCanonicalId($0)) }
}

internal func mobileThreadEntries(
    _ entries: [MobileChatMessage],
    rootId: String
) -> [MobileChatMessage] {
    let topology = entries.map {
        TranscriptEntry(
            kind: "message",
            id: mobileTranscriptCanonicalId($0),
            replyTo: $0.replyToMessageId,
            branched: $0.branched
        )
    }
    let threadIds = Set(getThreadTranscriptEntries(topology, rootId: rootId).compactMap(\.id))
    return entries.filter { threadIds.contains(mobileTranscriptCanonicalId($0)) }
}

internal func mobileThreadReplyCounts(_ entries: [MobileChatMessage]) -> [String: Int] {
    let branched = entries.compactMap { message -> BranchedTranscriptEntry? in
        guard message.branched, let replyTo = message.replyToMessageId else { return nil }
        return BranchedTranscriptEntry(id: mobileTranscriptCanonicalId(message), replyTo: replyTo)
    }
    return branchReplyCounts(branched)
}

internal func mobileTranscriptCopyText(_ entry: MobileChatMessage) -> String? {
    if let projection = entry.sendMessageTextProjection {
        guard case .text = projection.presentation else { return nil }
        return projection.content.isEmpty ? nil : projection.content
    }
    guard entry.kind == .message, !entry.text.isEmpty else { return nil }
    return entry.text
}

internal func mergeMobileConversationHistory(
    current: [MobileChatMessage],
    fetched: [MobileChatMessage]
) -> [MobileChatMessage] {
    func historyIdentity(_ message: MobileChatMessage) -> String {
        message.kind == .message ? mobileTranscriptCanonicalId(message) : message.id
    }
    var entriesById: [String: MobileChatMessage] = [:]
    for message in current {
        entriesById[historyIdentity(message)] = message
    }
    for message in fetched {
        entriesById[historyIdentity(message)] = message
    }
    return Array(entriesById.values).sorted {
        if $0.createdAt != $1.createdAt { return $0.createdAt < $1.createdAt }
        return $0.id < $1.id
    }
}

internal func reconcileMobileConversationBaseline(
    baseline: [MobileChatMessage],
    current: [MobileChatMessage],
    identitiesAtRequestStart: Set<String>
) -> [MobileChatMessage] {
    func identity(_ message: MobileChatMessage) -> String {
        message.canonicalMessageId ?? message.id
    }

    var merged = baseline
    var seen = Set(baseline.map(identity))
    for message in current {
        let key = identity(message)
        guard !seen.contains(key) else { continue }

        let arrivedAfterRequestStarted = !identitiesAtRequestStart.contains(key)
        let unresolvedOptimistic = message.id.hasPrefix("ios-mobile-bot-chat-")
            || (message.kind == .message && message.canonicalMessageId == message.id)
        let activeEphemeral = message.kind != .message || message.streaming
        guard arrivedAfterRequestStarted || unresolvedOptimistic || activeEphemeral else { continue }

        merged.append(message)
        seen.insert(key)
    }
    return merged.sorted {
        if $0.createdAt != $1.createdAt { return $0.createdAt < $1.createdAt }
        return $0.id < $1.id
    }
}

private struct MobileLinkMetadataCard: View {
    let url: String
    let model: MarketplaceModel
    var isGroupStart = false

    @State private var metadata: MobileLinkMetadata?
    @State private var loading = false

    var body: some View {
        Link(destination: URL(string: url)!) {
            VStack(alignment: .leading, spacing: 5) {
                if let imageURL = metadata?.imageURL,
                   let image = URL(string: imageURL)
                {
                    AsyncImage(url: image) { phase in
                        switch phase {
                        case .success(let imageView):
                            imageView
                                .resizable()
                                .scaledToFill()
                                .frame(maxWidth: .infinity)
                                .frame(height: 96)
                                .clipped()
                                .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
                        case .empty:
                            ProgressView().controlSize(.small)
                        case .failure:
                            EmptyView()
                        @unknown default:
                            EmptyView()
                        }
                    }
                }
                Text(metadata?.displayTitle ?? url)
                    .font(.system(size: 14, weight: .semibold))
                    .lineLimit(2)
                    .multilineTextAlignment(.leading)
                if let description = metadata?.description {
                    Text(description)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(2)
                        .multilineTextAlignment(.leading)
                }
                if let hostname = metadata?.hostname {
                    Text(hostname)
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                } else if loading {
                    ProgressView().controlSize(.mini)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(10)
            .background(
                Color.black.opacity(0.045),
                in: RoundedRectangle(cornerRadius: 12, style: .continuous)
            )
        }
        .buttonStyle(.plain)
        .accessibilityLabel(metadata?.displayTitle ?? url)
        .accessibilityIdentifier("mobile-bot-link-card")
        .task(id: url) {
            metadata = nil
            loading = true
            defer { loading = false }
            metadata = try? await model.linkMetadata(for: url)
        }
    }
}

private struct MobileTranscriptMediaAttachmentView: View {
    let rawURL: String
    let alt: String?

    @State private var player: AVPlayer?
    @State private var isPlaying = false

    private var destination: URL? {
        let trimmed = rawURL.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return nil }
        if let url = URL(string: trimmed), url.scheme != nil { return url }
        if trimmed.hasPrefix("/") { return URL(fileURLWithPath: trimmed) }
        return nil
    }

    var body: some View {
        Group {
            if let destination {
                switch mobileAttachmentMediaPresentation(rawURL) {
                case .image:
                    if destination.isFileURL,
                       let image = UIImage(contentsOfFile: destination.path) {
                        Image(uiImage: image)
                            .resizable()
                            .scaledToFit()
                            .frame(maxHeight: 260)
                            .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
                            .accessibilityLabel(alt ?? "Image attachment")
                    } else {
                        AsyncImage(url: destination) { phase in
                            switch phase {
                            case let .success(image):
                                image
                                    .resizable()
                                    .scaledToFit()
                                    .frame(maxHeight: 260)
                                    .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
                            case .empty:
                                ProgressView().controlSize(.small)
                            default:
                                Link(alt ?? "Open image", destination: destination)
                            }
                        }
                        .accessibilityLabel(alt ?? "Image attachment")
                    }

                case .video:
                    if let player {
                        VideoPlayer(player: player)
                            .frame(minHeight: 180, maxHeight: 280)
                            .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
                            .accessibilityLabel(alt ?? "Video attachment")
                    } else {
                        ProgressView().controlSize(.small)
                    }

                case .audio:
                    HStack(spacing: 10) {
                        Button {
                            guard let player else { return }
                            if isPlaying {
                                player.pause()
                            } else {
                                player.play()
                            }
                            isPlaying.toggle()
                        } label: {
                            Image(systemName: isPlaying ? "pause.fill" : "play.fill")
                        }
                        .buttonStyle(.bordered)
                        Text(
                            alt
                                ?? (destination.lastPathComponent.isEmpty
                                    ? "Audio attachment"
                                    : destination.lastPathComponent)
                        )
                        .font(.caption)
                        .lineLimit(1)
                        Spacer()
                        Link(destination: destination) {
                            Image(systemName: "arrow.up.right.square")
                        }
                    }
                    .accessibilityElement(children: .contain)

                case .file:
                    Link(alt ?? "Open attachment", destination: destination)
                }
            } else {
                Label(alt ?? "Attachment unavailable", systemImage: "paperclip")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        .task(id: destination?.absoluteString) {
            guard let destination,
                  [.video, .audio].contains(mobileAttachmentMediaPresentation(rawURL))
            else {
                player?.pause()
                player = nil
                isPlaying = false
                return
            }
            player?.pause()
            player = AVPlayer(url: destination)
            isPlaying = false
        }
        .onDisappear {
            player?.pause()
            isPlaying = false
        }
    }
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
    @State private var asyncTasksPresented = false
    @State private var replyTargetId: String?
    @State private var replyIsFork = false
    @State private var threadRootId: String?
    @State private var threadLoadGeneration = 0
    @State private var threadLoadingRootId: String?
    @State private var threadLoadError: String?
    @State private var voiceRecorder = VoiceRecorder()
    @State private var voiceTranscriber = OfflineSpeechTranscriber()
    @State private var transcribingVoice = false
    @State private var voiceInputGeneration = 0
    @State private var reactionGeneration = 0
    @State private var reactionPickerPresented = false
    @State private var reactionPickerTargetId: String?
    @State private var reactionPickerDraft = ""
    @State private var approvalGeneration = 0
    @State private var transcriptBaselineGeneration = 0
    @State private var transcriptBaselineError: String?
    @State private var widgetGeneration = 0
    @State private var widgetPendingEntryIds: Set<String> = []
    @State private var widgetCustomAnswers: [String: String] = [:]
    @State private var widgetErrors: [String: String] = [:]
    @State private var transcriptDraftRecipients: [String: String] = [:]
    @State private var transcriptDraftSubjects: [String: String] = [:]
    @State private var transcriptDraftBodies: [String: String] = [:]
    @State private var transcriptDraftPendingEntryIds: Set<String> = []
    @State private var transcriptDraftStatuses: [String: String] = [:]
    @State private var transcriptDraftErrors: [String: String] = [:]
    @State private var secretDrafts: [String: String] = [:]
    @State private var secretPendingEntryIds: Set<String> = []
    @State private var secretProvidedEntryIds: Set<String> = []
    @State private var secretErrors: [String: String] = [:]

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
        .task(id: "\(bot.id):\(bot.conversationId ?? "")") {
            await loadInitialConversationTail()
        }
        .task(id: listenerScopeFingerprint) {
            await pollVisibleListenerIntegrations()
        }
        .onChange(of: bot.id) { _, _ in
            cancelVoiceInput()
            approvalGeneration &+= 1
            transcriptBaselineGeneration &+= 1
            transcriptBaselineError = nil
            widgetGeneration &+= 1
            widgetPendingEntryIds.removeAll()
            widgetCustomAnswers.removeAll()
            widgetErrors.removeAll()
            threadLoadGeneration &+= 1
            threadLoadingRootId = nil
            threadLoadError = nil
            threadRootId = nil
            resetTranscriptDraftUI()
            resetSecretRequestUI()
        }
        .onDisappear {
            cancelVoiceInput()
            approvalGeneration &+= 1
            transcriptBaselineGeneration &+= 1
            widgetGeneration &+= 1
            widgetPendingEntryIds.removeAll()
            threadLoadGeneration &+= 1
            threadLoadingRootId = nil
        }
        .fullScreenCover(isPresented: $openedMiniApp) {
            miniAppCover
        }
        .sheet(isPresented: $asyncTasksPresented) {
            MobileAsyncTasksPanel(
                agentId: bot.id,
                agentName: bot.name,
                bridge: bridge,
                onClose: { asyncTasksPresented = false }
            )
        }
        .sheet(isPresented: $reactionPickerPresented) {
            reactionPickerSheet
        }
        .sheet(
            isPresented: Binding(
                get: { threadRootId != nil },
                set: { presented in if !presented { threadRootId = nil } }
            )
        ) {
            threadSheet
        }
    }

    private var visibleListenerPlatforms: [String] {
        Array(Set(entries.compactMap { $0.listenerPlatform })).sorted()
    }

    private var listenerScopeFingerprint: String {
        "\(bot.id)|\(model.accountEmail)|\(visibleListenerPlatforms.joined(separator: ","))"
    }

    @MainActor
    private func pollVisibleListenerIntegrations() async {
        guard !visibleListenerPlatforms.isEmpty else { return }
        while !Task.isCancelled {
            await model.refreshListenerIntegrations()
            do {
                try await Task.sleep(for: .seconds(5))
            } catch {
                return
            }
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
            Button {
                asyncTasksPresented = true
            } label: {
                Image(systemName: "clock.arrow.circlepath")
                    .font(.system(size: 16, weight: .medium))
                    .frame(width: 38, height: 38)
                    .background(Color.black.opacity(0.045), in: Circle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Async tasks")
            .accessibilityIdentifier("mobile-bot-async-tasks")
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

                    ForEach(mobileMainTranscriptEntries(entries)) { entry in
                        transcript(entry)
                            .id(entry.id)
                    }
                    if let transcriptBaselineError {
                        VStack(spacing: 8) {
                            Text("Couldn't load conversation")
                                .font(.headline)
                            Text("Couldn't load this conversation. Check your connection and try again.")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                                .multilineTextAlignment(.center)
                            Button(MobileTranscriptLoadErrorCopy.retry) {
                                Task { await loadInitialConversationTail() }
                            }
                            .buttonStyle(.borderedProminent)
                        }
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 16)
                        .accessibilityElement(children: .contain)
                        .accessibilityLabel("Couldn't load conversation. \(transcriptBaselineError)")
                        .accessibilityIdentifier("mobile-bot-transcript-load-error")
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
                if let last = mobileMainTranscriptEntries(entries).last {
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
                Image(systemName: replyIsFork ? "bubble.left.and.bubble.right" : "arrowshape.turn.up.left")
                Text(replyIsFork ? "Thread reply · \(replyTargetId)" : "Replying · \(replyTargetId)")
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
            let reactionFingerprint = entry.reactions
                .map { "\($0.emoji):\($0.by)" }
                .joined(separator: "|")
            return "\(entryId):\(entryKind):\(entryRole):\(reactionFingerprint)"
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
            .init(agentId: "mobile-bot-async-tasks", role: "button", name: "Async tasks"),
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
        for entry in entries where entry.kind == .permissionRequest && entry.actionStatus == "pending" {
            let approveId = Self.semanticId("mobile-bot-approval-once-\(entry.id)")
            let denyId = Self.semanticId("mobile-bot-approval-deny-\(entry.id)")
            elements.append(.init(agentId: approveId, role: "button", name: "Allow once"))
            elements.append(.init(agentId: denyId, role: "button", name: "Deny"))
            if let proposed = entry.approvalProposedRule,
               !proposed.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                let alwaysId = Self.semanticId("mobile-bot-approval-always-\(entry.id)")
                elements.append(.init(agentId: alwaysId, role: "button", name: "Always allow"))
            }
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
            "mobile-bot-async-tasks": .init(allowed: ["invoke"]) { _ in asyncTasksPresented = true },
            "mobile-bot-draft": .init(allowed: ["setValue"]) { value in draft = value ?? "" },
        ]
        actions[sendId] = .init(allowed: ["invoke"]) { _ in
            if busy { Task { await stop() } } else { Task { await send() } }
        }
        if bot.miniAppId == GlobalDharmaMiniAppBridge.globalDharmaId {
            actions["mobile-bot-open-miniapp"] = .init(allowed: ["invoke"]) { _ in openedMiniApp = true }
        }
        for entry in entries where entry.kind == .permissionRequest && entry.actionStatus == "pending" {
            let approveId = Self.semanticId("mobile-bot-approval-once-\(entry.id)")
            let denyId = Self.semanticId("mobile-bot-approval-deny-\(entry.id)")
            actions[approveId] = .init(allowed: ["invoke"]) { _ in
                Task { await resolveApproval(entry, resolution: .approved) }
            }
            actions[denyId] = .init(allowed: ["invoke"]) { _ in
                Task { await resolveApproval(entry, resolution: .denied) }
            }
            if let proposed = entry.approvalProposedRule,
               !proposed.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                let alwaysId = Self.semanticId("mobile-bot-approval-always-\(entry.id)")
                actions[alwaysId] = .init(allowed: ["invoke"]) { _ in
                    Task { await resolveApproval(entry, resolution: .always) }
                }
            }
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
            VStack(alignment: .leading, spacing: 10) {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Image(systemName: "lock.shield").foregroundStyle(.secondary)
                    Text(entry.text).font(.caption)
                    Spacer(minLength: 8)
                    Text(entry.createdAt, style: .time).font(.caption2).foregroundStyle(.tertiary)
                }
                if let proposed = entry.approvalProposedRule,
                   !proposed.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                    Text(redactSandAutoReviewInlineSecrets(proposed))
                        .font(.caption2.monospaced()).foregroundStyle(.secondary).lineLimit(4)
                }
                if entry.actionStatus == "pending" {
                    HStack(spacing: 8) {
                        Button("Allow once") { Task { await resolveApproval(entry, resolution: .approved) } }
                            .buttonStyle(.borderedProminent)
                            .accessibilityIdentifier(Self.semanticId("mobile-bot-approval-once-\(entry.id)"))
                        if let proposed = entry.approvalProposedRule,
                           !proposed.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                            Button("Always allow") { Task { await resolveApproval(entry, resolution: .always) } }
                                .buttonStyle(.bordered)
                                .accessibilityIdentifier(Self.semanticId("mobile-bot-approval-always-\(entry.id)"))
                        }
                        Button("Deny", role: .destructive) { Task { await resolveApproval(entry, resolution: .denied) } }
                            .buttonStyle(.bordered)
                            .accessibilityIdentifier(Self.semanticId("mobile-bot-approval-deny-\(entry.id)"))
                    }
                } else if entry.actionStatus == "submitting" {
                    ProgressView("Applying decision…").controlSize(.small)
                } else if let status = entry.actionStatus {
                    Text(status == "always" ? "Always allowed"
                        : status == "approved" ? "Allowed once"
                        : status == "denied" ? "Denied"
                        : status == "stale" ? "This request is no longer pending."
                        : "Approval failed. You can retry from the next request.")
                        .font(.caption).foregroundStyle(status == "failed" ? .red : .secondary)
                }
            }
            .padding(10)
            .background(Color.black.opacity(0.035), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
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
            if let widget = mobileTranscriptWidgetProjection(entry) {
                transcriptWidgetCard(entry, projection: widget)
            } else if let draft = mobileTranscriptDraftProjection(entry.canonicalTranscriptCard) {
                transcriptDraftCard(entry, draft: draft)
            } else if let secret = mobileSecretRequestProjection(entry.canonicalTranscriptCard) {
                secretRequestCard(entry, secret: secret)
            } else if let platform = entry.listenerPlatform {
                listenerIntegrationCard(entry, platform: platform)
            } else {
                HStack(spacing: 7) {
                    Circle().fill(entry.actionStatus == "failed" ? Color.red : Color.orange).frame(width: 7, height: 7)
                    Text(entry.actionTitle ?? "Working").font(.caption.weight(.medium))
                    if let detail = entry.actionDetail, !detail.isEmpty { Text(detail).font(.caption).foregroundStyle(.secondary).lineLimit(1) }
                }
                .padding(.vertical, 2)
            }
        } else if entry.role == .user {
            HStack {
                Spacer(minLength: 54)
                VStack(alignment: .leading, spacing: 7) {
                    messageTextContent(entry)
                    attachmentContent(entry)
                    reactionPills(entry)
                    threadAffordance(entry)
                    if let phase = entry.optimisticDeliveryPhase {
                        HStack(spacing: 5) {
                            if phase == .pending || phase == .acceptedAwaitingEcho {
                                ProgressView().controlSize(.mini).tint(.white)
                            } else {
                                Image(systemName: "exclamationmark.circle.fill")
                            }
                            Text(
                                phase == .pending ? "Sending…"
                                    : phase == .acceptedAwaitingEcho ? "Waiting for sync…"
                                    : "Failed to send"
                            )
                            .font(.caption2.weight(.semibold))
                        }
                        .accessibilityIdentifier(Self.semanticId("mobile-bot-send-state-\(entry.id)"))
                        if phase == .failed,
                           let detail = entry.optimisticDeliveryError,
                           !detail.isEmpty {
                            Text(detail)
                                .font(.caption2)
                                .lineLimit(2)
                                .opacity(0.8)
                        }
                    }
                }
                .foregroundStyle(.white)
                .tint(.white)
                .padding(.horizontal, 15).padding(.vertical, 10)
                .background(.black, in: RoundedRectangle(cornerRadius: 18, style: .continuous))
                .contextMenu {
                    Button("Reply") { beginReply(to: entry, inThread: threadRootId != nil) }
                    Button(threadRootId == nil ? "Start Thread" : "Reply in Thread") { beginReply(to: entry, inThread: true) }
                    if let copyText = mobileTranscriptCopyText(entry) {
                        Button("Copy") { UIPasteboard.general.string = copyText }
                    }
                    reactionMenu(entry)
                }
            }
        } else {
            VStack(alignment: .leading, spacing: 3) {
                Text(bot.name).font(.caption).foregroundStyle(.secondary).padding(.leading, 12)
                HStack(alignment: .bottom, spacing: 7) {
                    ClothGhostAvatar(botId: bot.id, size: 20)
                    VStack(alignment: .leading, spacing: 7) {
                        messageTextContent(entry)
                            .foregroundStyle(.black)
                        attachmentContent(entry)
                        reactionPills(entry)
                        threadAffordance(entry)
                    }
                    .padding(.horizontal, 15).padding(.vertical, 10)
                    .background(Color.black.opacity(0.055), in: RoundedRectangle(cornerRadius: 18, style: .continuous))
                    .contextMenu {
                        Button("Reply") { beginReply(to: entry, inThread: threadRootId != nil) }
                        Button(threadRootId == nil ? "Start Thread" : "Reply in Thread") { beginReply(to: entry, inThread: true) }
                    if let copyText = mobileTranscriptCopyText(entry) {
                        Button("Copy") { UIPasteboard.general.string = copyText }
                    }
                        reactionMenu(entry)
                    }
                    Spacer(minLength: 30)
                }
            }
        }
    }

    @MainActor
    private func beginReply(to entry: MobileChatMessage, inThread: Bool) {
        replyTargetId = mobileTranscriptCanonicalId(entry)
        replyIsFork = inThread
        if threadRootId != nil { threadRootId = nil }
    }

    @ViewBuilder
    private func threadAffordance(_ entry: MobileChatMessage) -> some View {
        if threadRootId == nil {
            let rootId = mobileTranscriptCanonicalId(entry)
            let count = mobileThreadReplyCounts(entries)[rootId] ?? 0
            if count > 0 {
                Button {
                    openThread(rootId: rootId)
                } label: {
                    HStack(spacing: 5) {
                        Text("View thread")
                        Text(count == 1 ? "1 reply" : "\(count) replies")
                            .foregroundStyle(.secondary)
                    }
                    .font(.caption2.weight(.semibold))
                }
                .buttonStyle(.plain)
                .accessibilityLabel("View thread, \(count == 1 ? "1 reply" : "\(count) replies")")
                .accessibilityIdentifier(Self.semanticId("mobile-bot-view-thread-\(rootId)"))
            }
        }
    }

    @ViewBuilder
    private var threadSheet: some View {
        if let rootId = threadRootId {
            NavigationStack {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 9) {
                        if threadLoadingRootId == rootId {
                            ProgressView("Loading full thread…")
                                .controlSize(.small)
                        }
                        if let threadLoadError, !threadLoadError.isEmpty {
                            Text(threadLoadError)
                                .font(.caption)
                                .foregroundStyle(.red)
                        }
                        ForEach(mobileThreadEntries(entries, rootId: rootId)) { entry in
                            transcript(entry)
                                .id("thread:\(entry.id)")
                        }
                    }
                    .padding(16)
                }
                .navigationTitle("Thread")
                .navigationBarTitleDisplayMode(.inline)
                .safeAreaInset(edge: .bottom) {
                    Button {
                        replyTargetId = rootId
                        replyIsFork = true
                        threadRootId = nil
                    } label: {
                        Label("Reply in thread", systemImage: "arrowshape.turn.up.left")
                            .frame(maxWidth: .infinity)
                    }
                    .buttonStyle(.borderedProminent)
                    .padding()
                    .background(.ultraThinMaterial)
                    .accessibilityIdentifier(Self.semanticId("mobile-bot-thread-reply-\(rootId)"))
                }
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) {
                        Button("Close") { threadRootId = nil }
                    }
                }
            }
            .accessibilityIdentifier(Self.semanticId("mobile-bot-thread-\(rootId)"))
        }
    }

    @MainActor
    private func openThread(rootId: String) {
        threadRootId = rootId
        threadLoadError = nil
        Task { await loadCompleteThread(rootId: rootId) }
    }

    @MainActor
    private func loadCompleteThread(rootId: String) async {
        guard bot.miniAppId == nil,
              let conversationId = bot.conversationId?
                .trimmingCharacters(in: .whitespacesAndNewlines),
              !conversationId.isEmpty
        else { return }

        threadLoadGeneration &+= 1
        let generation = threadLoadGeneration
        let ownedBotId = bot.id
        threadLoadingRootId = rootId
        defer {
            if generation == threadLoadGeneration, threadLoadingRootId == rootId {
                threadLoadingRootId = nil
            }
        }

        var beforeMessageId: String?
        var fetched: [MobileChatMessage] = []
        var seenAnchors: Set<String> = []
        do {
            for page in 0..<50 {
                guard generation == threadLoadGeneration,
                      bot.id == ownedBotId,
                      threadRootId == rootId
                else { return }

                let requestId = "ios-mobile-thread-\(page)-\(UUID().uuidString.lowercased())"
                var command: [String: Any] = [
                    "type": "conversation.openWindowed",
                    "requestId": requestId,
                    "conversationId": conversationId,
                    "limit": 200,
                ]
                if let beforeMessageId {
                    command["beforeMessageId"] = beforeMessageId
                }
                _ = try await bridge.request(
                    method: "feature.execute",
                    params: ["command": command]
                )
                let result = try await bridge.receiveFeatureEvent(
                    deadlineMilliseconds: 8_000
                ) { event in
                    event["type"] as? String == "conversation.windowOpened"
                        && event["requestId"] as? String == requestId
                        && event["conversationId"] as? String == conversationId
                }
                guard generation == threadLoadGeneration,
                      bot.id == ownedBotId,
                      threadRootId == rootId,
                      let event = result.value as? [String: Any],
                      let rows = event["messages"] as? [[String: Any]]
                else { return }

                var pageEntries: [MobileChatMessage] = []
                for row in rows {
                    guard let projected = projectMobileConversationWindowEntries(row) else {
                        throw NSError(
                            domain: "Fabushi.MobileBotChat",
                            code: 42,
                            userInfo: [NSLocalizedDescriptionKey: "Host returned malformed thread history"]
                        )
                    }
                    pageEntries.append(contentsOf: projected)
                }
                fetched.append(contentsOf: pageEntries)
                if pageEntries.contains(where: {
                    $0.kind == .message && mobileTranscriptCanonicalId($0) == rootId
                }) {
                    entries = mergeMobileConversationHistory(current: entries, fetched: fetched)
                    threadLoadError = nil
                    return
                }

                guard let next = event["nextBeforeMessageId"] as? String,
                      !next.isEmpty
                else {
                    throw NSError(
                        domain: "Fabushi.MobileBotChat",
                        code: 43,
                        userInfo: [NSLocalizedDescriptionKey: "Thread root is no longer available in conversation history"]
                    )
                }
                guard seenAnchors.insert(next).inserted else {
                    throw NSError(
                        domain: "Fabushi.MobileBotChat",
                        code: 44,
                        userInfo: [NSLocalizedDescriptionKey: "Conversation history pagination repeated an anchor"]
                    )
                }
                beforeMessageId = next
            }
            throw NSError(
                domain: "Fabushi.MobileBotChat",
                code: 45,
                userInfo: [NSLocalizedDescriptionKey: "Thread history exceeded the bounded 10,000-message lookup"]
            )
        } catch is CancellationError {
            return
        } catch {
            guard generation == threadLoadGeneration,
                  bot.id == ownedBotId,
                  threadRootId == rootId
            else { return }
            threadLoadError = error.localizedDescription
        }
    }

    @MainActor
    private func resolveTranscriptWidget(
        entry: MobileChatMessage,
        value: String?,
        dismiss: Bool
    ) async {
        guard !widgetPendingEntryIds.contains(entry.id),
              let conversationId = bot.conversationId?
                .trimmingCharacters(in: .whitespacesAndNewlines),
              !conversationId.isEmpty
        else { return }

        let answer = value?.trimmingCharacters(in: .whitespacesAndNewlines)
        if !dismiss, answer?.isEmpty != false { return }

        let ownedGeneration = widgetGeneration
        let ownedBotId = bot.id
        widgetPendingEntryIds.insert(entry.id)
        widgetErrors.removeValue(forKey: entry.id)
        defer {
            if widgetGeneration == ownedGeneration {
                widgetPendingEntryIds.remove(entry.id)
            }
        }

        do {
            let requestId = "ios-widget-\(UUID().uuidString.lowercased())"
            var command: [String: Any] = [
                "type": dismiss ? "widget.dismiss" : "widget.respond",
                "requestId": requestId,
                "conversationId": conversationId,
                "entryId": entry.id,
                "agentId": bot.id,
            ]
            if let answer { command["value"] = answer }
            let result = try await bridge.request(
                method: "feature.execute",
                params: ["command": command]
            )
            guard widgetGeneration == ownedGeneration, bot.id == ownedBotId else { return }
            let accepted = result.value as? [String: Any]
            let operationId = accepted?["operationId"] as? String

            let changed = try await bridge.receiveFeatureEvent(
                deadlineMilliseconds: 8_000
            ) { event in
                event["type"] as? String == "transcript.card"
                    && event["entryId"] as? String == entry.id
            }
            guard widgetGeneration == ownedGeneration, bot.id == ownedBotId,
                  let event = changed.value as? [String: Any],
                  let projected = projectMobileTranscriptCard(
                    event: event,
                    operationId: operationId
                  )
            else { return }
            if let index = entries.firstIndex(where: { $0.id == entry.id }) {
                entries[index] = projected
            }
            widgetCustomAnswers[entry.id] = ""

            guard let operationId, !operationId.isEmpty else { return }
            busy = true
            activeOperationId = operationId
            entries.append(MobileChatMessage(
                id: "thinking:\(operationId)",
                role: .assistant,
                text: "",
                kind: .thinking,
                operationId: operationId,
                actionTitle: "Continuing from your answer",
                actionStatus: "running"
            ))
            await pump(operationId: operationId)
            if widgetGeneration == ownedGeneration, bot.id == ownedBotId {
                activeOperationId = nil
                busy = false
            }
        } catch {
            guard widgetGeneration == ownedGeneration, bot.id == ownedBotId else { return }
            widgetErrors[entry.id] = error.localizedDescription
            activeOperationId = nil
            busy = false
        }
    }

    @ViewBuilder
    private func transcriptWidgetCard(
        _ entry: MobileChatMessage,
        projection: MobileTranscriptWidgetProjection
    ) -> some View {
        let widget = projection.widget
        let pending = widgetPendingEntryIds.contains(entry.id)
        let settled = projection.respondedValue != nil || projection.dismissed
        let options = widget.options
        VStack(alignment: .leading, spacing: 10) {
            Text(widget.prompt)
                .font(.body.weight(.semibold))
            if let help = widget.helpText, !help.isEmpty {
                Text(help)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            if let respondedValue = projection.respondedValue {
                Label(
                    getWidgetAnswerLabel(widget: widget, answerValue: respondedValue),
                    systemImage: "checkmark.circle.fill"
                )
                .font(.caption.weight(.semibold))
                .foregroundStyle(.green)
            } else if projection.dismissed {
                Label("Dismissed", systemImage: "xmark.circle")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(.secondary)
            } else {
                ForEach(Array(options.enumerated()), id: \.offset) { index, option in
                    Button {
                        Task {
                            await resolveTranscriptWidget(
                                entry: entry,
                                value: option.value ?? option.label,
                                dismiss: false
                            )
                        }
                    } label: {
                        HStack(alignment: .top, spacing: 9) {
                            Text(String(UnicodeScalar(65 + index)!))
                                .font(.caption.monospaced().weight(.bold))
                                .foregroundStyle(.secondary)
                            VStack(alignment: .leading, spacing: 2) {
                                Text(option.label)
                                    .font(.caption.weight(.semibold))
                                if let description = option.description, !description.isEmpty {
                                    Text(description)
                                        .font(.caption2)
                                        .foregroundStyle(.secondary)
                                }
                            }
                            Spacer()
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    .buttonStyle(.bordered)
                    .disabled(pending)
                    .keyboardShortcut(
                        KeyEquivalent(Character(String(UnicodeScalar(97 + index)!))),
                        modifiers: []
                    )
                }

                if widget.allowCustom == true {
                    HStack(spacing: 8) {
                        TextField(
                            "Other answer",
                            text: Binding(
                                get: { widgetCustomAnswers[entry.id] ?? "" },
                                set: { widgetCustomAnswers[entry.id] = $0 }
                            )
                        )
                        .disabled(pending)
                        Button("Submit") {
                            Task {
                                await resolveTranscriptWidget(
                                    entry: entry,
                                    value: widgetCustomAnswers[entry.id],
                                    dismiss: false
                                )
                            }
                        }
                        .buttonStyle(.borderedProminent)
                        .disabled(
                            pending
                                || (widgetCustomAnswers[entry.id] ?? "")
                                    .trimmingCharacters(in: .whitespacesAndNewlines)
                                    .isEmpty
                        )
                    }
                }

                Button("Dismiss", role: .cancel) {
                    Task {
                        await resolveTranscriptWidget(
                            entry: entry,
                            value: nil,
                            dismiss: true
                        )
                    }
                }
                .buttonStyle(.bordered)
                .disabled(pending)
            }

            if pending {
                ProgressView("Saving response…")
                    .controlSize(.small)
            }
            if let error = widgetErrors[entry.id], !error.isEmpty {
                Text(error)
                    .font(.caption2)
                    .foregroundStyle(.red)
            }
        }
        .padding(10)
        .background(Color.black.opacity(0.035), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .accessibilityIdentifier(Self.semanticId("mobile-bot-widget-\(entry.id)"))
        .accessibilityValue(settled ? "settled" : "waiting")
    }

    @MainActor
    private func resetTranscriptDraftUI() {
        transcriptDraftRecipients.removeAll()
        transcriptDraftSubjects.removeAll()
        transcriptDraftBodies.removeAll()
        transcriptDraftPendingEntryIds.removeAll()
        transcriptDraftStatuses.removeAll()
        transcriptDraftErrors.removeAll()
    }

    @MainActor
    private func resolveTranscriptDraft(
        entry: MobileChatMessage,
        draft: MobileTranscriptDraftProjection,
        action: String
    ) async {
        guard let payload = entry.canonicalTranscriptCard,
              !transcriptDraftPendingEntryIds.contains(entry.id)
        else { return }

        var overrides: [String: Any] = [:]
        if action == "send" {
            switch draft {
            case let .email(email):
                let recipientsValue = transcriptDraftRecipients[entry.id] ?? email.to.joined(separator: ", ")
                guard let recipients = mobileEmailRecipients(recipientsValue) else { return }
                let subject = transcriptDraftSubjects[entry.id] ?? email.subject
                let body = transcriptDraftBodies[entry.id] ?? email.body
                guard !body.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
                overrides["to"] = recipients
                overrides["subject"] = subject
                overrides["body"] = body
            case let .slack(slack):
                let body = transcriptDraftBodies[entry.id] ?? slack.body
                guard !body.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
                overrides["body"] = body
            }
        }

        transcriptDraftPendingEntryIds.insert(entry.id)
        transcriptDraftStatuses[entry.id] = action == "send" ? "sending" : "discarding"
        transcriptDraftErrors.removeValue(forKey: entry.id)
        defer { transcriptDraftPendingEntryIds.remove(entry.id) }

        do {
            let result = try await model.resolveTranscriptDraft(
                payload,
                overrides: overrides,
                action: action
            )
            transcriptDraftStatuses[entry.id] = result.status
            if let error = result.error, !error.isEmpty {
                transcriptDraftErrors[entry.id] = error
            }
        } catch {
            transcriptDraftStatuses[entry.id] = "failed"
            transcriptDraftErrors[entry.id] = error.localizedDescription
        }
    }

    @ViewBuilder
    private func transcriptDraftCard(
        _ entry: MobileChatMessage,
        draft: MobileTranscriptDraftProjection
    ) -> some View {
        let pending = transcriptDraftPendingEntryIds.contains(entry.id)
        switch draft {
        case let .email(email):
            let status = transcriptDraftStatuses[entry.id] ?? email.status
            let terminal = ["sent", "discarded"].contains(status)
            let recipients = transcriptDraftRecipients[entry.id] ?? email.to.joined(separator: ", ")
            let body = transcriptDraftBodies[entry.id] ?? email.body
            VStack(alignment: .leading, spacing: 9) {
                HStack {
                    Label("New email", systemImage: "envelope")
                        .font(.caption.weight(.semibold))
                    Spacer()
                    Text(status == "sending" ? "Sending…" : status.capitalized)
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                }
                if let from = email.from, !from.isEmpty {
                    LabeledContent("From", value: from).font(.caption2)
                }
                if !terminal {
                    TextField(
                        "name@example.com",
                        text: Binding(
                            get: { transcriptDraftRecipients[entry.id] ?? email.to.joined(separator: ", ") },
                            set: { transcriptDraftRecipients[entry.id] = $0 }
                        )
                    )
                    .textContentType(.emailAddress)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    TextField(
                        "Subject",
                        text: Binding(
                            get: { transcriptDraftSubjects[entry.id] ?? email.subject },
                            set: { transcriptDraftSubjects[entry.id] = $0 }
                        )
                    )
                    TextField(
                        "Write a message",
                        text: Binding(
                            get: { transcriptDraftBodies[entry.id] ?? email.body },
                            set: { transcriptDraftBodies[entry.id] = $0 }
                        ),
                        axis: .vertical
                    )
                    .lineLimit(4...10)
                    HStack {
                        Button("Send email") {
                            Task { await resolveTranscriptDraft(entry: entry, draft: draft, action: "send") }
                        }
                        .buttonStyle(.borderedProminent)
                        .disabled(pending || mobileEmailRecipients(recipients) == nil || body.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                        Button("Discard", role: .destructive) {
                            Task { await resolveTranscriptDraft(entry: entry, draft: draft, action: "discard") }
                        }
                        .buttonStyle(.bordered)
                        .disabled(pending)
                    }
                } else {
                    Text(status == "sent" ? "Sent to \(email.to.first ?? "") — “\(email.subject)”" : "Draft discarded")
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                }
                if let error = transcriptDraftErrors[entry.id] ?? email.error, !error.isEmpty {
                    Text(error).font(.caption2).foregroundStyle(.red)
                }
            }
            .padding(10)
            .background(Color.black.opacity(0.035), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
            .accessibilityIdentifier(Self.semanticId("mobile-bot-email-draft-\(entry.id)"))

        case let .slack(slack):
            let status = transcriptDraftStatuses[entry.id] ?? slack.status
            let terminal = ["sent", "discarded"].contains(status)
            let body = transcriptDraftBodies[entry.id] ?? slack.body
            VStack(alignment: .leading, spacing: 9) {
                HStack {
                    Label("Slack message", systemImage: "message")
                        .font(.caption.weight(.semibold))
                    Spacer()
                    Text(status == "sending" ? "Sending…" : status.capitalized)
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                }
                if let workspace = slack.workspace, !workspace.isEmpty {
                    LabeledContent("Workspace", value: workspace).font(.caption2)
                }
                LabeledContent("To", value: slack.target).font(.caption2)
                LabeledContent("Thread", value: slack.thread ?? "New message").font(.caption2)
                if !terminal {
                    TextField(
                        "Write a message",
                        text: Binding(
                            get: { transcriptDraftBodies[entry.id] ?? slack.body },
                            set: { transcriptDraftBodies[entry.id] = $0 }
                        ),
                        axis: .vertical
                    )
                    .lineLimit(4...10)
                    HStack {
                        Button("Send message") {
                            Task { await resolveTranscriptDraft(entry: entry, draft: draft, action: "send") }
                        }
                        .buttonStyle(.borderedProminent)
                        .disabled(pending || body.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                        Button("Discard", role: .destructive) {
                            Task { await resolveTranscriptDraft(entry: entry, draft: draft, action: "discard") }
                        }
                        .buttonStyle(.bordered)
                        .disabled(pending)
                    }
                } else {
                    Text(status == "sent" ? "Sent to \(slack.target)" : "Draft discarded")
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                }
                if let error = transcriptDraftErrors[entry.id] ?? slack.error, !error.isEmpty {
                    Text(error).font(.caption2).foregroundStyle(.red)
                }
            }
            .padding(10)
            .background(Color.black.opacity(0.035), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
            .accessibilityIdentifier(Self.semanticId("mobile-bot-slack-draft-\(entry.id)"))
        }
    }

    @MainActor
    private func resetSecretRequestUI() {
        secretDrafts.removeAll()
        secretPendingEntryIds.removeAll()
        secretProvidedEntryIds.removeAll()
        secretErrors.removeAll()
    }

    @MainActor
    private func submitSecretRequest(
        entry: MobileChatMessage,
        secret: MobileSecretRequestProjection
    ) async {
        let value = (secretDrafts[entry.id] ?? "")
            .trimmingCharacters(in: .whitespacesAndNewlines)
        guard !value.isEmpty,
              !secret.provided,
              !secretProvidedEntryIds.contains(entry.id),
              !secretPendingEntryIds.contains(entry.id)
        else { return }

        secretPendingEntryIds.insert(entry.id)
        secretErrors.removeValue(forKey: entry.id)
        defer { secretPendingEntryIds.remove(entry.id) }
        do {
            try await model.provideTranscriptSecret(
                secretRequestId: secret.requestId,
                value: value
            )
            secretDrafts[entry.id] = ""
            secretProvidedEntryIds.insert(entry.id)
        } catch {
            secretErrors[entry.id] = error.localizedDescription
        }
    }

    @ViewBuilder
    private func secretRequestCard(
        _ entry: MobileChatMessage,
        secret: MobileSecretRequestProjection
    ) -> some View {
        let provided = secret.provided || secretProvidedEntryIds.contains(entry.id)
        let pending = secretPendingEntryIds.contains(entry.id)
        VStack(alignment: .leading, spacing: 9) {
            HStack(spacing: 8) {
                Image(systemName: provided ? "checkmark.shield.fill" : "key.fill")
                    .foregroundStyle(provided ? .green : .secondary)
                Text(secret.label)
                    .font(.caption.weight(.semibold))
                Spacer()
                if provided {
                    Text("Provided")
                        .font(.caption2.weight(.semibold))
                        .foregroundStyle(.green)
                }
            }
            if let description = secret.description,
               !description.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                Text(description)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
            if !provided {
                SecureField(
                    "Enter secret",
                    text: Binding(
                        get: { secretDrafts[entry.id] ?? "" },
                        set: { secretDrafts[entry.id] = $0 }
                    )
                )
                .textContentType(.password)
                .disabled(pending)
                .accessibilityIdentifier(Self.semanticId("mobile-bot-secret-input-\(entry.id)"))

                Button(pending ? "Submitting…" : "Submit") {
                    Task { await submitSecretRequest(entry: entry, secret: secret) }
                }
                .buttonStyle(.borderedProminent)
                .controlSize(.small)
                .disabled(
                    pending
                        || (secretDrafts[entry.id] ?? "")
                            .trimmingCharacters(in: .whitespacesAndNewlines)
                            .isEmpty
                )
                .accessibilityIdentifier(Self.semanticId("mobile-bot-secret-submit-\(entry.id)"))
            }
            if let error = secretErrors[entry.id], !error.isEmpty {
                Text(error)
                    .font(.caption2)
                    .foregroundStyle(.red)
            }
        }
        .padding(10)
        .background(Color.black.opacity(0.035), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .accessibilityIdentifier(Self.semanticId("mobile-bot-secret-request-\(entry.id)"))
    }

    @ViewBuilder
    private func listenerIntegrationCard(
        _ entry: MobileChatMessage,
        platform rawPlatform: String
    ) -> some View {
        let platform = rawPlatform.lowercased()
        let integration = model.listenerIntegrationState(for: platform)
        let connected = integration?.isConnected ?? (entry.actionStatus == "completed")
        let connecting = model.listenerConnectingPlatform == platform
        let authorizing = model.listenerAuthorizingPlatform == platform
        let busy = connecting || authorizing
        let title = integration?.displayName
            ?? entry.actionTitle?.replacingOccurrences(of: "连接 ", with: "")
            ?? platform.capitalized
        VStack(alignment: .leading, spacing: 7) {
            HStack(spacing: 8) {
                Image(systemName: connected ? "checkmark.circle.fill" : "bolt.horizontal.circle")
                    .foregroundStyle(connected ? .green : .secondary)
                Text(connected ? "\(title) 已连接" : "连接 \(title)")
                    .font(.caption.weight(.semibold))
                Spacer(minLength: 8)
                if connected {
                    Text("Connected")
                        .font(.caption2.weight(.semibold))
                        .foregroundStyle(.green)
                } else {
                    Button(authorizing ? "授权中…" : (connecting ? "连接中…" : "连接")) {
                        Task { await model.connectListenerIntegration(platform: platform) }
                    }
                    .buttonStyle(.borderedProminent)
                    .controlSize(.small)
                    .disabled(busy)
                    .accessibilityIdentifier(Self.semanticId("mobile-bot-listener-connect-\(platform)"))
                }
            }
            if !connected {
                let detail = integration?.blurb ?? entry.actionDetail
                if let detail, !detail.isEmpty {
                    Text(detail)
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                }
            }
            if let error = integration?.error ?? model.listenerIntegrationErrors[platform],
               !error.isEmpty {
                Text(error)
                    .font(.caption2)
                    .foregroundStyle(.red)
            }
        }
        .padding(.vertical, 4)
        .accessibilityIdentifier(Self.semanticId("mobile-bot-listener-card-\(platform)"))
    }

    private static let quickReactionEmojis = ["👍", "👎", "❤️", "😂", "🎉", "😮"]

    @ViewBuilder
    private func reactionMenu(_ entry: MobileChatMessage) -> some View {
        if entry.kind == .message, !entry.streaming {
            Menu("React") {
                ForEach(Self.quickReactionEmojis, id: \.self) { emoji in
                    Button(emoji) { toggleReaction(entry, emoji: emoji) }
                }
                Divider()
                Button("More Reactions…") { openReactionPicker(entry) }
            }
        }
    }

    @ViewBuilder
    private func reactionPills(_ entry: MobileChatMessage) -> some View {
        let pills = projectMobileReactionPills(entry.reactions)
        if !pills.isEmpty {
            HStack(spacing: 5) {
                ForEach(pills) { pill in
                    Button {
                        toggleReaction(entry, emoji: pill.emoji)
                    } label: {
                        HStack(spacing: 3) {
                            Text(pill.emoji)
                            if pill.count > 1 { Text("\(pill.count)").font(.caption2) }
                        }
                        .padding(.horizontal, 7)
                        .padding(.vertical, 3)
                        .background(
                            pill.chosenByMe
                                ? Color.accentColor.opacity(0.16)
                                : Color.black.opacity(0.06),
                            in: Capsule()
                        )
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(
                        "\(pill.emoji), \(pill.count) reaction\(pill.count == 1 ? "" : "s")"
                            + (pill.chosenByMe ? ", selected by you" : "")
                    )
                    .accessibilityHint(
                        pill.reactors.isEmpty
                            ? "Toggle reaction"
                            : "Reactors: \(pill.reactors.joined(separator: ", "))"
                    )
                    .accessibilityIdentifier(
                        Self.semanticId("mobile-bot-reaction-\(entry.id)-\(pill.emoji)")
                    )
                }
            }
        }
    }

    private var reactionPickerSheet: some View {
        NavigationStack {
            Form {
                Section("Quick reactions") {
                    LazyVGrid(
                        columns: Array(repeating: GridItem(.flexible()), count: 3),
                        spacing: 12
                    ) {
                        ForEach(Self.quickReactionEmojis, id: \.self) { emoji in
                            Button(emoji) {
                                reactionPickerDraft = emoji
                                submitReactionPicker()
                            }
                            .font(.title2)
                        }
                    }
                    .padding(.vertical, 6)
                }
                Section("Custom reaction") {
                    TextField("Emoji", text: $reactionPickerDraft)
                        .accessibilityIdentifier("mobile-bot-reaction-picker-input")
                }
            }
            .navigationTitle("React")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { reactionPickerPresented = false }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { submitReactionPicker() }
                        .disabled(normalizeMobileReactionInput(reactionPickerDraft) == nil)
                }
            }
        }
        .presentationDetents([.medium])
        .accessibilityIdentifier("mobile-bot-reaction-picker")
    }

    @MainActor
    private func openReactionPicker(_ entry: MobileChatMessage) {
        reactionPickerTargetId = entry.id
        reactionPickerDraft = ""
        reactionPickerPresented = true
    }

    @MainActor
    private func submitReactionPicker() {
        guard let targetId = reactionPickerTargetId,
              let emoji = normalizeMobileReactionInput(reactionPickerDraft),
              let entry = entries.first(where: { $0.id == targetId })
        else { return }
        reactionPickerPresented = false
        reactionPickerTargetId = nil
        reactionPickerDraft = ""
        toggleReaction(entry, emoji: emoji)
    }

    @MainActor
    private func toggleReaction(_ entry: MobileChatMessage, emoji rawEmoji: String) {
        guard let emoji = normalizeMobileReactionInput(rawEmoji),
              entry.kind == .message, !entry.streaming,
              let index = entries.firstIndex(where: { $0.id == entry.id })
        else { return }
        let entryId = entry.canonicalMessageId ?? entry.id
        let had = entries[index].myReactions.contains(emoji)
        if had {
            entries[index].reactions.removeAll { $0.emoji == emoji && $0.by == "me" }
            entries[index].myReactions.remove(emoji)
        } else {
            entries[index].reactions.append(.init(emoji: emoji, by: "me"))
            entries[index].myReactions.insert(emoji)
        }

        reactionGeneration &+= 1
        let generation = reactionGeneration
        let agentId = bot.id
        Task { @MainActor in
            do {
                let response = try await bridge.request(
                    method: "reactToMessage",
                    params: [
                        "entryId": entryId,
                        "emoji": emoji,
                        "agentId": agentId,
                    ]
                )
                guard generation == reactionGeneration,
                      agentId == bot.id,
                      let object = response.value as? [String: Any],
                      object["applied"] as? Bool == true,
                      let currentIndex = entries.firstIndex(where: { $0.id == entry.id })
                else { return }
                let canonical = projectMobileTranscriptReactions(object["reactions"])
                entries[currentIndex].reactions = canonical
                entries[currentIndex].myReactions = Set(
                    canonical.filter { $0.by == "me" }.map(\.emoji)
                )
            } catch {
                // Desktop keeps the optimistic value until the authoritative transcript
                // reconciles. Preserve that behavior rather than inventing a second error owner.
            }
        }
    }

    @ViewBuilder
    private func messageTextContent(_ entry: MobileChatMessage) -> some View {
        if let projection = entry.sendMessageTextProjection {
            switch projection.presentation {
            case .urlCard(let rawURL):
                if URL(string: rawURL) != nil {
                    MobileLinkMetadataCard(url: rawURL, model: model)
                        .accessibilityIdentifier(Self.semanticId("mobile-bot-url-card-\(projection.id)"))
                } else if !projection.content.isEmpty {
                    Text(projection.content)
                        .font(.system(size: 16))
                }
            case .text:
                if !projection.content.isEmpty {
                    Text(projection.content)
                        .overlay(alignment: .trailing) {
                            if projection.streaming {
                                Text("▌").foregroundStyle(.black.opacity(0.65))
                            }
                        }
                        .font(.system(size: 16))
                }
            }
            ForEach(Array(projection.images.enumerated()), id: \.offset) { index, image in
                if let url = URL(string: image.url),
                   let scheme = url.scheme?.lowercased(),
                   scheme == "http" || scheme == "https"
                {
                    AsyncImage(url: url) { phase in
                        switch phase {
                        case .success(let imageView):
                            imageView
                                .resizable()
                                .scaledToFit()
                                .frame(maxHeight: 220)
                                .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
                        case .failure:
                            Label(image.alt ?? "Image unavailable", systemImage: "photo")
                                .font(.caption)
                        case .empty:
                            ProgressView().controlSize(.small)
                        @unknown default:
                            EmptyView()
                        }
                    }
                    .accessibilityLabel(image.alt ?? "Attached image")
                    .accessibilityIdentifier(Self.semanticId("mobile-bot-text-image-\(projection.id)-\(index)"))
                } else {
                    Label(image.alt ?? "Attached image", systemImage: "photo")
                        .font(.caption)
                        .accessibilityIdentifier(Self.semanticId("mobile-bot-text-image-\(projection.id)-\(index)"))
                }
            }
        } else if !entry.text.isEmpty {
            Text(entry.text)
                .overlay(alignment: .trailing) {
                    if entry.streaming {
                        Text("▌").foregroundStyle(.black.opacity(0.65))
                    }
                }
                .font(.system(size: 16))
        }
    }

    @ViewBuilder
    private func attachmentContent(_ entry: MobileChatMessage) -> some View {
        if let attachment = entry.attachmentProjection {
            switch attachment.kind {
            case .box:
                VStack(alignment: .leading, spacing: 4) {
                    Label(
                        attachment.instruction ?? attachment.request ?? "Computer attachment",
                        systemImage: "desktopcomputer"
                    )
                    .font(.caption.weight(.medium))
                    if attachment.screenshotDataURL != nil {
                        Text("Computer snapshot attached")
                            .font(.caption2)
                            .foregroundStyle(.secondary)
                    }
                }
                .accessibilityIdentifier(Self.semanticId("mobile-bot-attachment-box-\(attachment.id)"))
            case .legacyLink:
                MobileLinkMetadataCard(url: attachment.url, model: model)
                    .accessibilityIdentifier(Self.semanticId("mobile-bot-attachment-\(attachment.id)"))
            case .media:
                MobileTranscriptMediaAttachmentView(
                    rawURL: attachment.url,
                    alt: attachment.alt ?? attachment.name
                )
                .accessibilityIdentifier(Self.semanticId("mobile-bot-attachment-\(attachment.id)"))
            case .file:
                let label = attachment.name ?? attachment.alt ?? "Open attachment"
                if let destination = attachmentDestinationURL(attachment.url) {
                    Link(destination: destination) {
                        Label(label, systemImage: "paperclip")
                            .font(.caption.weight(.medium))
                    }
                    .accessibilityIdentifier(Self.semanticId("mobile-bot-attachment-\(attachment.id)"))
                } else {
                    Label(label, systemImage: "paperclip")
                        .font(.caption.weight(.medium))
                        .accessibilityIdentifier(Self.semanticId("mobile-bot-attachment-\(attachment.id)"))
                }
            }
        } else if let rawURL = entry.attachmentURL,
                  let destination = attachmentDestinationURL(rawURL)
        {
            Link(destination: destination) {
                Label(
                    entry.attachmentFileName ?? entry.attachmentAlt ?? "Open attachment",
                    systemImage: "paperclip"
                )
                .font(.caption.weight(.medium))
            }
        }
    }

    private func attachmentDestinationURL(_ raw: String) -> URL? {
        let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return nil }
        if let url = URL(string: trimmed), url.scheme != nil {
            return url
        }
        if trimmed.hasPrefix("/") {
            return URL(fileURLWithPath: trimmed)
        }
        return nil
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
    private func loadInitialConversationTail() async {
        guard bot.miniAppId == nil,
              let conversationId = bot.conversationId?.trimmingCharacters(in: .whitespacesAndNewlines),
              !conversationId.isEmpty
        else { return }

        transcriptBaselineGeneration &+= 1
        transcriptBaselineError = nil
        let generation = transcriptBaselineGeneration
        let ownedBotID = bot.id
        let identitiesAtRequestStart = Set(entries.map { $0.canonicalMessageId ?? $0.id })
        let requestId = "ios-mobile-conversation-tail-\(UUID().uuidString.lowercased())"

        do {
            _ = try await bridge.request(
                method: "feature.execute",
                params: [
                    "command": [
                        "type": "conversation.openTail",
                        "requestId": requestId,
                        "conversationId": conversationId,
                        "limit": 200,
                    ],
                ]
            )
            let result = try await bridge.receiveFeatureEvent(
                deadlineMilliseconds: 8_000
            ) { event in
                event["type"] as? String == "conversation.windowOpened"
                    && event["requestId"] as? String == requestId
                    && event["conversationId"] as? String == conversationId
            }
            guard
                generation == transcriptBaselineGeneration,
                bot.id == ownedBotID,
                let event = result.value as? [String: Any],
                let rows = event["messages"] as? [[String: Any]]
            else { return }

            var baseline: [MobileChatMessage] = []
            for row in rows {
                guard let projected = projectMobileConversationWindowEntries(row) else {
                    throw NSError(
                        domain: "Fabushi.MobileBotChat",
                        code: 41,
                        userInfo: [NSLocalizedDescriptionKey: "Host returned a malformed conversation baseline"]
                    )
                }
                baseline.append(contentsOf: projected)
            }
            entries = reconcileMobileConversationBaseline(
                baseline: baseline,
                current: entries,
                identitiesAtRequestStart: identitiesAtRequestStart
            )
            transcriptBaselineError = nil
        } catch is CancellationError {
            return
        } catch {
            guard generation == transcriptBaselineGeneration, bot.id == ownedBotID else { return }
            transcriptBaselineError = error.localizedDescription
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
        let replyTarget = replyTargetId
        let sendAsFork = replyIsFork
        replyTargetId = nil
        replyIsFork = false
        entries.append(MobileChatMessage(
            id: requestId,
            role: .user,
            text: text,
            canonicalMessageId: requestId,
            replyToMessageId: replyTarget,
            branched: sendAsFork,
            optimisticDeliveryPhase: .pending
        ))

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
            if let index = entries.firstIndex(where: { $0.id == requestId && $0.role == .user }) {
                entries[index].optimisticDeliveryPhase = .acceptedAwaitingEcho
                entries[index].optimisticDeliveryError = nil
            }
            activeOperationId = operationId
            entries.append(MobileChatMessage(id: "thinking:\(operationId)", role: .assistant, text: "", kind: .thinking, operationId: operationId, actionTitle: "Thinking", actionStatus: "running"))
            await pump(operationId: operationId)
        } catch {
            let message = error.localizedDescription
            if let index = entries.firstIndex(where: { $0.id == requestId && $0.role == .user }) {
                entries[index].optimisticDeliveryPhase = .failed
                entries[index].optimisticDeliveryError = message
            }
            errorText = message
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
    private func resolveApproval(_ entry: MobileChatMessage, resolution: MobileAutoReviewResolution) async {
        guard let approvalId = entry.approvalId,
              let initialIndex = entries.firstIndex(where: { $0.approvalId == approvalId }),
              entries[initialIndex].actionStatus == "pending" else { return }

        let ownedBotId = bot.id
        let ownedGeneration = approvalGeneration
        entries[initialIndex].actionStatus = "submitting"

        if resolution == .always, let proposedRule = entry.approvalProposedRule {
            do {
                let currentResult = try await bridge.request(method: "getAutoReviewInstructions", params: [:])
                let current = try decodeMobileAutoReviewInstructions(currentResult.value)
                if let next = appendMobileAutoReviewAllowRule(current, proposedRule: proposedRule) {
                    _ = try await bridge.request(method: "setAutoReviewInstructions", params: [
                        "isEnabled": next.isEnabled,
                        "allowInstructions": next.allowInstructions,
                        "blockInstructions": next.blockInstructions,
                    ])
                }
            } catch {
                // Desktop parity: durable-rule failure degrades to one-time allow.
            }
        }

        do {
            _ = try await bridge.request(method: "feature.approval.resolve", params: [
                "resolution": ["approvalId": approvalId, "decision": resolution.hostDecision],
            ])
            guard approvalGeneration == ownedGeneration, bot.id == ownedBotId,
                  let index = entries.firstIndex(where: { $0.approvalId == approvalId }) else { return }
            entries[index].actionStatus = resolution.rawValue
        } catch {
            guard approvalGeneration == ownedGeneration, bot.id == ownedBotId,
                  let index = entries.firstIndex(where: { $0.approvalId == approvalId }) else { return }
            entries[index].actionStatus = error.localizedDescription.localizedCaseInsensitiveContains("unknown approval")
                ? "stale" : "failed"
        }
    }

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
                    if type == "approval.requested" {
                        return event["operationId"] as? String == operationId
                    }
                    if type == "host.transport" {
                        guard event["channel"] as? String == "transcript.reaction",
                              let payload = event["payload"] as? [String: Any]
                        else { return false }
                        return payload["agentId"] as? String == bot.id
                    }
                    let acceptedTypes: Set<String> = [
                        "box.handoff.requested",
                        "approval.requested",
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
                case "host.transport":
                    _ = applyMobileTranscriptReactionEvent(
                        event,
                        agentId: bot.id,
                        messages: &entries
                    )
                case "approval.requested":
                    guard let row = projectMobileApprovalRequest(event, operationId: operationId) else { continue }
                    if let index = entries.firstIndex(where: { $0.approvalId == row.approvalId }) {
                        entries[index] = row
                    } else {
                        entries.append(row)
                    }
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
                    if event["role"] as? String == "user" {
                        _ = applyMobileOptimisticUserEcho(event, messages: &entries)
                        continue
                    }
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
                        let canonicalMessageId = event["messageId"] as? String
                        let attachmentBatchId = event["attachmentBatchId"] as? String
                        entries[index].canonicalMessageId = canonicalMessageId
                        entries[index].replyToMessageId = event["replyToMessageId"] as? String
                        entries[index].attachmentBatchId = attachmentBatchId
                        if let rawAttachment = event["attachment"] as? [String: Any],
                           let attachment = projectMobileChatMessageAttachment(
                                id: canonicalMessageId ?? "assistant:\(operationId)",
                                raw: rawAttachment,
                                batchId: attachmentBatchId,
                                timestampMs: event["timestampMs"]
                           )
                        {
                            entries[index].attachmentProjection = attachment
                            entries[index].attachmentURL = attachment.url
                            entries[index].attachmentFileName = attachment.name
                            entries[index].attachmentAlt = attachment.alt
                        } else {
                            entries[index].attachmentProjection = nil
                            entries[index].attachmentURL = nil
                            entries[index].attachmentFileName = nil
                            entries[index].attachmentAlt = nil
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
                    guard let row = projectMobileTranscriptCardWithFallback(
                        event: event,
                        operationId: eventOperationId
                    ) else { continue }
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
