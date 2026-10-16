import SwiftUI
import UniformTypeIdentifiers
import UIKit

private enum MobileDestination {
    case home
    case marketplace
    case remoteComputer
}

private enum MobileSection: String, CaseIterable, Identifiable {
    case chats, contacts, bots, groups, channels, calls, saved, archive, folders, miniapps, payments, settings
    var id: String { rawValue }
    var label: String {
        switch self {
        case .chats: "聊天"
        case .contacts: "联系人"
        case .bots: "Bots"
        case .groups: "群组"
        case .channels: "频道"
        case .calls: "通话"
        case .saved: "收藏"
        case .archive: "归档"
        case .folders: "文件夹"
        case .miniapps: "Mini Apps"
        case .payments: "支付"
        case .settings: "设置"
        }
    }
    var symbol: String {
        switch self {
        case .chats: "bubble.left.and.bubble.right.fill"
        case .contacts: "person.2.fill"
        case .bots: "sparkles"
        case .groups: "person.3.fill"
        case .channels: "megaphone.fill"
        case .calls: "phone.fill"
        case .saved: "bookmark.fill"
        case .archive: "archivebox.fill"
        case .folders: "folder.fill"
        case .miniapps: "square.grid.2x2.fill"
        case .payments: "wallet.bifold.fill"
        case .settings: "gearshape.fill"
        }
    }
}

private struct LoginBlob: View {
    let color: Color
    let width: CGFloat
    let height: CGFloat
    let rotation: Double

    var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: min(width, height) * 0.42, style: .continuous)
                .fill(color)
                .rotationEffect(.degrees(rotation))
            HStack(spacing: max(8, width * 0.08)) {
                Capsule().fill(.white).frame(width: max(8, width * 0.11), height: max(18, height * 0.25))
                Capsule().fill(.white).frame(width: max(8, width * 0.11), height: max(18, height * 0.25))
            }
            .rotationEffect(.degrees(rotation * 0.35))
        }
        .frame(width: width, height: height)
        .accessibilityHidden(true)
    }
}

struct ContentView: View {
    @Bindable var model: MarketplaceModel
    @Bindable var messaging: MessagingModel
    let appAgentSurface: FabushiAppAgentSurface
    let onShellBack: (() -> Void)?

    init(
        model: MarketplaceModel,
        messaging: MessagingModel,
        appAgentSurface: FabushiAppAgentSurface,
        onShellBack: (() -> Void)? = nil
    ) {
        self.model = model
        self.messaging = messaging
        self.appAgentSurface = appAgentSurface
        self.onShellBack = onShellBack
    }

    @State private var openedMiniApp: MarketplacePlugin?
    @State private var destination: MobileDestination = .home
    @State private var agentChatPresented = false
    @State private var isSearching = false
    @State private var homeQuery = ""
    @State private var selectedConversation: ConversationSummary?
    @State private var messageDraft = ""
    @State private var draftSyncTask: Task<Void, Never>?
    @State private var replyTarget: ChatMessage?
    @State private var editingMessage: ChatMessage?
    @State private var forwardMessage: ChatMessage?
    @State private var mediaViewerMessage: ChatMessage?
    @State private var conversationInfoPresented = false
    @State private var chatSearchPresented = false
    @State private var chatSearchQuery = ""
    @State private var attachmentPickerPresented = false
    @State private var locationSharePresented = false
    @State private var locationService = LocationService()
    @State private var voiceRecorder = VoiceRecorder()
    @State private var voicePlayback = VoicePlaybackController()
    @State private var contactSharePresented = false
    @State private var pollComposerPresented = false
    @State private var pollQuestion = ""
    @State private var pollOption1 = ""
    @State private var pollOption2 = ""
    @State private var pollOption3 = ""
    @State private var composeMenuPresented = false
    @State private var profileMenuPresented = false
    @State private var composeKind: ConversationKind?
    @State private var composeName = ""
    @State private var composeDescription = ""
    @State private var composeParticipantIds: Set<String> = []
    @State private var activeSection: MobileSection?
    @State private var contactGroupsPresented = false
    @State private var folderEditorPresented = false
    @State private var folderTitle = ""
    @State private var folderConversationIds: Set<String> = []
    @State private var folderIncludeGroups = false
    @State private var folderIncludeChannels = false

    var body: some View {
        Group {
            if model.onboardingStep < 3 {
                onboardingView
            } else if !model.authResolved {
                authLoadingView
            } else if !model.loggedIn {
                loginView
            } else {
                authenticatedContent
            }
        }
    }

    private var authenticatedContent: some View {
        Group {
            switch destination {
            case .home:
                homeView
            case .marketplace:
                marketplaceView
            case .remoteComputer:
                RemoteComputerSurface { destination = .home }
            }
        }
        .fullScreenCover(item: $openedMiniApp) { plugin in
            MiniAppWebMcpSurface(plugin: plugin, model: model)
        }
        .alert("插件权限", isPresented: Binding(
            get: { model.permissionRequest != nil },
            set: { if !$0 { model.denyPermissions() } }
        )) {
            Button("拒绝", role: .cancel) { model.denyPermissions() }
                .accessibilityIdentifier("permission-deny")
            Button("授权") { Task { await model.approvePermissions() } }
                .accessibilityIdentifier("permission-approve")
        } message: {
            if let request = model.permissionRequest {
                Text("\(request.pluginId) 请求以下权限：\n\(request.permissions.joined(separator: "\n"))")
            }
        }
        .task(id: appAgentSurfaceFingerprint) {
            publishAppAgentSurface()
        }
        .onDisappear {
            appAgentSurface.clear()
        }
    }

    private var appAgentSurfaceFingerprint: String {
        let pluginRevision = model.plugins.map { "\($0.pluginId):\($0.latestVersion ?? "")" }.joined(separator: ",")
        let destinationRevision: String = switch destination {
        case .home: "home"
        case .marketplace: "marketplace"
        case .remoteComputer: "remote-computer"
        }
        let conversationRevision = messaging.conversations.map {
            "\($0.id):\($0.unreadCount):\($0.isPinned):\($0.isMuted):\($0.isArchived):\($0.markedUnread):\($0.lastMessageId ?? "")"
        }.joined(separator: ",")
        let contactRevision = messaging.contacts.map { "\($0.id):\($0.kind):\($0.displayName)" }.joined(separator: ",")
        let folderRevision = messaging.folders.map { "\($0.id):\($0.title):\($0.conversationIds.count)" }.joined(separator: ",")
        let selectedMessagesRevision = (selectedConversation.flatMap { messaging.messagesByConversation[$0.id] } ?? []).map {
            "\($0.id):\($0.contentType):\($0.isOutgoing):\($0.isEdited):\($0.isPinned):\($0.deliveryState):\($0.reactions.count)"
        }.joined(separator: ",")
        let agentRevision = model.chatMessages.map { "\($0.id):\($0.kind.rawValue):\($0.role.rawValue):\($0.actionStatus ?? "")" }.joined(separator: ",")
        let fingerprintChunk1: [String] = [
    destinationRevision,
    String(isSearching),
    homeQuery,
    String(profileMenuPresented),
    String(composeMenuPresented),
    composeKind?.rawValue ?? "",
    composeName,
    composeDescription,
    composeParticipantIds.sorted().joined(separator: ","),
    activeSection?.rawValue ?? "",
    String(contactGroupsPresented),
    String(folderEditorPresented),
    folderTitle,
    folderConversationIds.sorted().joined(separator: ","),
    String(folderIncludeGroups),
    String(folderIncludeChannels),
    String(agentChatPresented),
]
let fingerprintChunk2: [String] = [
    model.chatDraft,
    String(model.chatBusy),
    agentRevision,
    selectedConversation?.id ?? "",
    messageDraft,
    replyTarget?.id ?? "",
    editingMessage?.id ?? "",
    forwardMessage?.id ?? "",
    mediaViewerMessage?.id ?? "",
    String(conversationInfoPresented),
    String(chatSearchPresented),
    chatSearchQuery,
    String(attachmentPickerPresented),
    String(locationSharePresented),
    String(contactSharePresented),
    String(pollComposerPresented),
    pollQuestion,
]
let fingerprintChunk3: [String] = [
    pollOption1,
    pollOption2,
    pollOption3,
    String(voiceRecorder.isRecording),
    model.query,
    model.message,
    String(model.loading),
    model.installingPluginId ?? "",
    model.permissionRequest?.pluginId ?? "",
    openedMiniApp?.pluginId ?? "",
    String(model.globalDharmaCommerce.accessAllowed),
    model.globalDharmaCommerce.accessReason,
    model.globalDharmaCommerce.message,
    String(model.globalDharmaCommerce.busy),
    model.globalDharmaCommerce.lastPaymentId ?? "",
    pluginRevision,
    conversationRevision,
    contactRevision,
    folderRevision,
    selectedMessagesRevision,
    String(messaging.loading),
    messaging.errorMessage ?? "",
]
var fingerprintParts: [String] = []
fingerprintParts.reserveCapacity(fingerprintChunk1.count + fingerprintChunk2.count + fingerprintChunk3.count)
fingerprintParts.append(contentsOf: fingerprintChunk1)
fingerprintParts.append(contentsOf: fingerprintChunk2)
fingerprintParts.append(contentsOf: fingerprintChunk3)
return fingerprintParts.joined(separator: "|")
    }

    @MainActor
    private func publishAppAgentSurface() {
        var elements: [FabushiAppAgentSurface.Element] = []
        var actions: [String: FabushiAppAgentSurface.Action] = [:]
        func semanticId(_ value: String) -> String {
            String(value.map { character in
                character.isASCII && (character.isLetter || character.isNumber || "._:/@-".contains(character))
                    ? character
                    : "-"
            }.prefix(200))
        }
        func add(
            _ id: String,
            role: String,
            name: String,
            enabled: Bool = true,
            visible: Bool = true,
            action: FabushiAppAgentSurface.Action? = nil
        ) {
            let normalizedId = semanticId(id)
            elements.append(.init(
                agentId: normalizedId,
                role: String(role.prefix(80)),
                name: String(name.prefix(240)),
                visible: visible,
                enabled: enabled
            ))
            if let action { actions[normalizedId] = action }
        }
        func publish(_ screen: String) {
            if model.permissionRequest != nil {
                add(
                    "permission-approve",
                    role: "button",
                    name: "授权插件权限",
                    action: .init(allowed: ["invoke"]) { _ in Task { await model.approvePermissions() } }
                )
                add(
                    "permission-deny",
                    role: "button",
                    name: "拒绝插件权限",
                    action: .init(allowed: ["invoke"]) { _ in model.denyPermissions() }
                )
            }
            try? appAgentSurface.publish(
                screen: model.permissionRequest == nil ? screen : "permission-dialog",
                elements: elements,
                actions: actions
            )
        }
        func openConversation(_ conversation: ConversationSummary) {
            selectedConversation = conversation
            Task { await messaging.markRead(conversation.id) }
        }
        func currentConversation(_ fallback: ConversationSummary) -> ConversationSummary {
            messaging.conversations.first(where: { $0.id == fallback.id }) ?? fallback
        }

        if let onShellBack {
            add(
                "grok-mobile-back",
                role: "button",
                name: "返回消息首页",
                action: .init(allowed: ["invoke"]) { _ in onShellBack() }
            )
        }

        if let openedMiniApp {
            add("miniapp-\(openedMiniApp.pluginId)", role: "application", name: openedMiniApp.displayName)
            add("miniapp-close", role: "button", name: "关闭 MiniApp", action: .init(allowed: ["invoke"]) { _ in self.openedMiniApp = nil })
            if openedMiniApp.pluginId == GlobalDharmaCommerceModel.miniAppId {
                let commerce = model.globalDharmaCommerce
                add("global-dharma-entitlement-status", role: "status", name: commerce.message)
                add(
                    "global-dharma-local-prayer-wheel-access",
                    role: "status",
                    name: commerce.accessAllowed ? "local.prayer-wheel.start allowed" : "local.prayer-wheel.start denied"
                )
                add(
                    "global-dharma-buy-lifetime",
                    role: "button",
                    name: "\(commerce.lifetimePriceLabel) 买断本地转经轮",
                    enabled: commerce.canBuyLifetime,
                    action: .init(allowed: ["invoke"]) { _ in Task { await commerce.purchaseLifetime() } }
                )
                add(
                    "global-dharma-restore-purchase",
                    role: "button",
                    name: "恢复购买",
                    enabled: !commerce.busy,
                    action: .init(allowed: ["invoke"]) { _ in Task { await commerce.restoreLifetime() } }
                )
            }
            publish("miniapp")
            return
        }

        if let mediaViewerMessage {
            add("media-viewer", role: "application", name: "媒体查看器")
            add("media-viewer-kind", role: "status", name: mediaViewerMessage.contentType)
            add("media-viewer-close", role: "button", name: "关闭媒体", action: .init(allowed: ["invoke"]) { _ in self.mediaViewerMessage = nil })
            publish("media-viewer")
            return
        }

        if let selected = selectedConversation {
            let conversation = currentConversation(selected)
            let messages = messaging.messagesByConversation[conversation.id] ?? []

            if let forwarding = forwardMessage {
                add("forward-dialog", role: "dialog", name: "转发消息")
                for destination in messaging.conversations.filter({ $0.id != conversation.id && !$0.isArchived }).prefix(100) {
                    add(
                        "forward-destination-\(destination.id)",
                        role: "button",
                        name: destination.title,
                        action: .init(allowed: ["invoke"]) { _ in
                            Task { await messaging.forwardMessage(sourceConversationId: conversation.id, messageId: forwarding.id, destinationConversationId: destination.id) }
                            forwardMessage = nil
                        }
                    )
                }
                add("forward-cancel", role: "button", name: "取消转发", action: .init(allowed: ["invoke"]) { _ in forwardMessage = nil })
                publish("forward-message")
                return
            }

            if pollComposerPresented {
                add("poll-compose", role: "dialog", name: "新建投票")
                add("poll-question", role: "textbox", name: "问题", action: .init(allowed: ["setValue"]) { value in pollQuestion = value ?? "" })
                add("poll-option-1", role: "textbox", name: "选项 1", action: .init(allowed: ["setValue"]) { value in pollOption1 = value ?? "" })
                add("poll-option-2", role: "textbox", name: "选项 2", action: .init(allowed: ["setValue"]) { value in pollOption2 = value ?? "" })
                add("poll-option-3", role: "textbox", name: "选项 3", action: .init(allowed: ["setValue"]) { value in pollOption3 = value ?? "" })
                let canSendPoll = !pollQuestion.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                    && !pollOption1.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                    && !pollOption2.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                add(
                    "poll-send",
                    role: "button",
                    name: "发送投票",
                    enabled: canSendPoll,
                    action: .init(allowed: ["invoke"]) { _ in
                        let question = pollQuestion
                        let options = [pollOption1, pollOption2, pollOption3]
                        Task {
                            do { try await messaging.sendPoll(conversationId: conversation.id, question: question, options: options); pollComposerPresented = false }
                            catch { model.message = "投票发送失败：\(error.localizedDescription)" }
                        }
                    }
                )
                add("poll-cancel", role: "button", name: "取消投票", action: .init(allowed: ["invoke"]) { _ in pollComposerPresented = false })
                publish("poll-compose")
                return
            }

            if contactSharePresented {
                add("contact-share", role: "dialog", name: "发送联系人")
                for contact in messaging.contacts.prefix(100) {
                    add(
                        "contact-share-\(contact.id)",
                        role: "button",
                        name: contact.displayName,
                        action: .init(allowed: ["invoke"]) { _ in
                            Task {
                                do { try await messaging.sendContact(conversationId: conversation.id, contact: contact); contactSharePresented = false }
                                catch { model.message = "联系人发送失败：\(error.localizedDescription)" }
                            }
                        }
                    )
                }
                add("contact-share-cancel", role: "button", name: "取消发送联系人", action: .init(allowed: ["invoke"]) { _ in contactSharePresented = false })
                publish("contact-share")
                return
            }

            if locationSharePresented {
                add("location-share", role: "dialog", name: "发送位置")
                if let coordinate = locationService.coordinate {
                    add(
                        "location-send",
                        role: "button",
                        name: "发送当前位置",
                        action: .init(allowed: ["invoke"]) { _ in
                            Task {
                                do { try await messaging.sendLocation(conversationId: conversation.id, latitude: coordinate.latitude, longitude: coordinate.longitude); locationSharePresented = false }
                                catch { model.message = "位置发送失败：\(error.localizedDescription)" }
                            }
                        }
                    )
                } else {
                    add("location-retry", role: "button", name: "重新获取位置", action: .init(allowed: ["invoke"]) { _ in locationService.requestLocation() })
                }
                add("location-cancel", role: "button", name: "取消发送位置", action: .init(allowed: ["invoke"]) { _ in locationSharePresented = false })
                publish("location-share")
                return
            }

            if conversationInfoPresented {
                add("conversation-info", role: "dialog", name: conversation.title)
                add("conversation-info-close", role: "button", name: "关闭会话信息", action: .init(allowed: ["invoke"]) { _ in conversationInfoPresented = false })
                publish("conversation-info")
                return
            }

            add("chat-\(conversation.id)", role: "application", name: conversation.title)
            add("chat-back", role: "button", name: "返回聊天列表", action: .init(allowed: ["invoke"]) { _ in chatSearchPresented = false; chatSearchQuery = ""; selectedConversation = nil })
            add("chat-info", role: "button", name: "会话信息", action: .init(allowed: ["invoke"]) { _ in conversationInfoPresented = true })
            add(
                "chat-search-toggle",
                role: "button",
                name: chatSearchPresented ? "关闭聊天搜索" : "搜索聊天",
                action: .init(allowed: ["invoke"]) { _ in chatSearchPresented.toggle(); if !chatSearchPresented { chatSearchQuery = "" } }
            )
            if chatSearchPresented {
                add("chat-search-field", role: "textbox", name: "搜索此聊天", action: .init(allowed: ["setValue"]) { value in chatSearchQuery = value ?? "" })
            }
            add(
                "chat-mute",
                role: "button",
                name: conversation.isMuted ? "取消静音" : "静音",
                action: .init(allowed: ["invoke"]) { _ in Task { await messaging.setMuted(conversation.id, muted: !conversation.isMuted) } }
            )
            add(
                "chat-pin",
                role: "button",
                name: conversation.isPinned ? "取消置顶" : "置顶",
                action: .init(allowed: ["invoke"]) { _ in Task { await messaging.setPinned(conversation.id, pinned: !conversation.isPinned) } }
            )
            add("chat-mark-unread", role: "button", name: "标为未读", action: .init(allowed: ["invoke"]) { _ in Task { await messaging.setMarkedUnread(conversation.id, markedUnread: true) }; selectedConversation = nil })
            add("chat-archive", role: "button", name: "归档", action: .init(allowed: ["invoke"]) { _ in Task { await messaging.setArchived(conversation.id, archived: true) }; selectedConversation = nil })
            add("chat-sync", role: "button", name: "同步消息", enabled: !messaging.loading, action: .init(allowed: ["invoke"]) { _ in Task { await messaging.refresh() } })

            if let editingMessage {
                add("chat-editing", role: "status", name: "正在编辑消息")
                add("chat-edit-cancel", role: "button", name: "取消编辑", action: .init(allowed: ["invoke"]) { _ in self.editingMessage = nil; messageDraft = "" })
            } else if replyTarget != nil {
                add("chat-replying", role: "status", name: "正在回复消息")
                add("chat-reply-cancel", role: "button", name: "取消回复", action: .init(allowed: ["invoke"]) { _ in self.replyTarget = nil })
            }

            add("chat-draft", role: "textbox", name: "消息", action: .init(allowed: ["setValue"]) { value in messageDraft = value ?? "" })
            let draftReady = !messageDraft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            add("chat-send", role: "button", name: editingMessage == nil ? "发送消息" : "保存编辑", enabled: draftReady, action: .init(allowed: ["invoke"]) { _ in sendMessage(in: conversation) })
            add("chat-attach-file", role: "button", name: "选择照片视频或文件", action: .init(allowed: ["invoke"]) { _ in attachmentPickerPresented = true })
            add("chat-attach-location", role: "button", name: "发送位置", action: .init(allowed: ["invoke"]) { _ in locationSharePresented = true; locationService.requestLocation() })
            add("chat-attach-contact", role: "button", name: "发送联系人", action: .init(allowed: ["invoke"]) { _ in contactSharePresented = true })
            add("chat-attach-poll", role: "button", name: "新建投票", action: .init(allowed: ["invoke"]) { _ in pollQuestion = ""; pollOption1 = ""; pollOption2 = ""; pollOption3 = ""; pollComposerPresented = true })

            let filteredMessages = messages.filter { message in
                chatSearchQuery.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || message.text.localizedCaseInsensitiveContains(chatSearchQuery)
            }
            for message in filteredMessages.suffix(100) {
                let messageId = semanticId(message.id)
                let direction = message.isOutgoing ? "已发送" : "已接收"
                add("message-\(messageId)", role: "article", name: "\(direction) \(message.contentType)")
                add("message-reply-\(messageId)", role: "button", name: "回复消息", action: .init(allowed: ["invoke"]) { _ in replyTarget = message; editingMessage = nil })
                add("message-forward-\(messageId)", role: "button", name: "转发消息", action: .init(allowed: ["invoke"]) { _ in forwardMessage = message })
                add("message-react-\(messageId)", role: "button", name: "点赞消息", action: .init(allowed: ["invoke"]) { _ in Task { await messaging.setReaction(conversationId: conversation.id, messageId: message.id, reaction: "👍", enabled: true) } })
                if message.isOutgoing {
                    add("message-edit-\(messageId)", role: "button", name: "编辑消息", action: .init(allowed: ["invoke"]) { _ in editingMessage = message; replyTarget = nil; messageDraft = message.text })
                }
                add("message-pin-\(messageId)", role: "button", name: message.isPinned ? "取消置顶消息" : "置顶消息", action: .init(allowed: ["invoke"]) { _ in Task { await messaging.setMessagePinned(conversationId: conversation.id, messageId: message.id, pinned: !message.isPinned) } })
                add("message-delete-\(messageId)", role: "button", name: "删除消息", action: .init(allowed: ["invoke"]) { _ in Task { await messaging.deleteMessage(conversationId: conversation.id, messageId: message.id) } })
                if ["photo", "video", "document"].contains(message.contentType) {
                    add("message-open-media-\(messageId)", role: "button", name: "打开媒体", action: .init(allowed: ["invoke"]) { _ in mediaViewerMessage = message })
                }
                if message.contentType == "voice" {
                    add("message-play-voice-\(messageId)", role: "button", name: "播放语音", action: .init(allowed: ["invoke"]) { _ in Task { await voicePlayback.toggle(message: message, messaging: messaging) } })
                }
                if message.contentType == "poll" {
                    for option in message.pollOptions.prefix(20) {
                        add(
                            "poll-vote-\(messageId)-\(option.id)",
                            role: "button",
                            name: option.text,
                            action: .init(allowed: ["invoke"]) { _ in
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
                            }
                        )
                    }
                }
            }
            if let errorMessage = messaging.errorMessage { add("chat-error", role: "status", name: String(errorMessage.prefix(200))) }
            publish("chat")
            return
        }

        if agentChatPresented {
            add("mahayana-agent-chat", role: "application", name: "大乘助手")
            add("mahayana-close", role: "button", name: "关闭大乘助手", action: .init(allowed: ["invoke"]) { _ in agentChatPresented = false })
            add("mahayana-draft", role: "textbox", name: "消息大乘助手", action: .init(allowed: ["setValue"]) { value in model.chatDraft = value ?? "" })
            let sendId = model.chatBusy ? "mahayana-stop" : "mahayana-send"
            add(
                sendId,
                role: "button",
                name: model.chatBusy ? "停止大乘助手" : "发送给大乘助手",
                enabled: model.chatBusy || !model.chatDraft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
                action: .init(allowed: ["invoke"]) { _ in if model.chatBusy { Task { await model.stopChat() } } else { Task { await model.sendChat() } } }
            )
            for entry in model.chatMessages.suffix(100) {
                let id = semanticId(entry.id)
                let label = entry.kind == .thinking ? "大乘助手思考" : entry.kind == .action ? "大乘助手动作" : entry.role == .user ? "用户消息" : "大乘助手消息"
                add("mahayana-entry-\(id)", role: "log", name: label)
            }
            publish("agent-chat")
            return
        }

        if let kind = composeKind {
            add("compose-\(kind.rawValue)", role: "dialog", name: "新建\(kind.label)")
            add("compose-name", role: "textbox", name: kind == .direct ? "联系人名称" : "名称", action: .init(allowed: ["setValue"]) { value in composeName = value ?? "" })
            if kind == .channel {
                add("compose-description", role: "textbox", name: "描述", action: .init(allowed: ["setValue"]) { value in composeDescription = value ?? "" })
            }
            if kind == .group {
                for contact in messaging.contacts.prefix(100) {
                    add(
                        "compose-participant-\(contact.id)",
                        role: "checkbox",
                        name: contact.displayName,
                        action: .init(allowed: ["toggle", "invoke"]) { _ in
                            if composeParticipantIds.contains(contact.id) { composeParticipantIds.remove(contact.id) } else { composeParticipantIds.insert(contact.id) }
                        }
                    )
                }
            }
            let canCreate = !composeName.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && (kind != .group || !composeParticipantIds.isEmpty)
            add("compose-create", role: "button", name: "创建", enabled: canCreate, action: .init(allowed: ["invoke"]) { _ in createConversation(kind: kind) })
            add("compose-cancel", role: "button", name: "取消", action: .init(allowed: ["invoke"]) { _ in composeKind = nil })
            publish("compose-\(kind.rawValue)")
            return
        }

        if folderEditorPresented {
            add("folder-editor", role: "dialog", name: "新建文件夹")
            add("folder-title", role: "textbox", name: "文件夹名称", action: .init(allowed: ["setValue"]) { value in folderTitle = value ?? "" })
            add("folder-include-groups", role: "checkbox", name: "自动包含群组", action: .init(allowed: ["toggle", "invoke"]) { _ in folderIncludeGroups.toggle() })
            add("folder-include-channels", role: "checkbox", name: "自动包含频道", action: .init(allowed: ["toggle", "invoke"]) { _ in folderIncludeChannels.toggle() })
            for conversation in messaging.conversations.filter({ !$0.isArchived }).prefix(100) {
                add("folder-conversation-\(conversation.id)", role: "checkbox", name: conversation.title, action: .init(allowed: ["toggle", "invoke"]) { _ in if folderConversationIds.contains(conversation.id) { folderConversationIds.remove(conversation.id) } else { folderConversationIds.insert(conversation.id) } })
            }
            add(
                "folder-create",
                role: "button",
                name: "创建文件夹",
                enabled: !folderTitle.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
                action: .init(allowed: ["invoke"]) { _ in
                    let folder = MessagingFolder(
                        id: "folder-\(UUID().uuidString.lowercased())",
                        title: folderTitle.trimmingCharacters(in: .whitespacesAndNewlines),
                        icon: "folder",
                        conversationIds: Array(folderConversationIds),
                        includeContacts: false,
                        includeBots: false,
                        includeGroups: folderIncludeGroups,
                        includeChannels: folderIncludeChannels,
                        excludeMuted: false,
                        excludeRead: false,
                        excludeArchived: true
                    )
                    Task { await messaging.upsertFolder(folder) }
                    folderEditorPresented = false
                }
            )
            add("folder-cancel", role: "button", name: "取消", action: .init(allowed: ["invoke"]) { _ in folderEditorPresented = false })
            publish("folder-editor")
            return
        }

        if contactGroupsPresented {
            add("contact-groups", role: "dialog", name: "联系人分组")
            add("contact-groups-close", role: "button", name: "完成", action: .init(allowed: ["invoke"]) { _ in contactGroupsPresented = false })
            publish("contact-groups")
            return
        }

        if let section = activeSection {
            add("section-\(section.rawValue)", role: "application", name: section.label)
            add("section-close", role: "button", name: "完成", action: .init(allowed: ["invoke"]) { _ in activeSection = nil })
            switch section {
            case .contacts, .bots:
                let rows = section == .bots ? messaging.contacts.filter { $0.kind == "bot" || $0.kind == "assistant" } : messaging.contacts
                for contact in rows.prefix(100) {
                    add(
                        "contact-\(contact.id)",
                        role: "button",
                        name: contact.displayName,
                        action: .init(allowed: ["invoke"]) { _ in
                            Task {
                                do {
                                    if let conversation = try await messaging.createDirect(contact: contact) {
                                        activeSection = nil
                                        selectedConversation = conversation
                                    }
                                } catch { model.message = "创建私聊失败：\(error.localizedDescription)" }
                            }
                        }
                    )
                }
            case .groups, .channels, .archive, .saved:
                let rows = messaging.conversations.filter { conversation in
                    switch section {
                    case .groups: conversation.kind == .group && !conversation.isArchived
                    case .channels: conversation.kind == .channel && !conversation.isArchived
                    case .archive: conversation.isArchived
                    case .saved: conversation.kind == .savedMessages
                    default: false
                    }
                }
                for conversation in rows.prefix(100) {
                    add("section-conversation-\(conversation.id)", role: "button", name: conversation.title, action: .init(allowed: ["invoke"]) { _ in activeSection = nil; openConversation(conversation) })
                    if section == .archive {
                        add("section-restore-\(conversation.id)", role: "button", name: "恢复 \(conversation.title)", action: .init(allowed: ["invoke"]) { _ in Task { await messaging.setArchived(conversation.id, archived: false) } })
                    }
                }
            case .folders:
                add("folder-new", role: "button", name: "新建文件夹", action: .init(allowed: ["invoke"]) { _ in folderTitle = ""; folderConversationIds = []; folderIncludeGroups = false; folderIncludeChannels = false; folderEditorPresented = true })
                for folder in messaging.folders.prefix(100) {
                    add("folder-\(folder.id)", role: "group", name: folder.title)
                    add("folder-delete-\(folder.id)", role: "button", name: "删除 \(folder.title)", action: .init(allowed: ["invoke"]) { _ in Task { await messaging.deleteFolder(folder.id) } })
                }
            case .calls, .payments, .settings:
                add("section-unavailable", role: "status", name: "此入口当前为移动端占位页")
            case .chats, .miniapps:
                break
            }
            publish("section-\(section.rawValue)")
            return
        }

        if profileMenuPresented {
            add("profile-menu", role: "dialog", name: "导航")
            add("profile-account", role: "status", name: model.accountName)
            add("mobile-logout", role: "button", name: "退出登录", action: .init(allowed: ["invoke"]) { _ in profileMenuPresented = false; Task { await model.logout() } })
            add("remote-computer-entry", role: "button", name: "我的电脑", action: .init(allowed: ["invoke"]) { _ in profileMenuPresented = false; destination = .remoteComputer })
            add("marketplace-entry", role: "button", name: "插件市场", action: .init(allowed: ["invoke"]) { _ in profileMenuPresented = false; destination = .marketplace })
            for section in MobileSection.allCases {
                add("profile-section-\(section.rawValue)", role: "button", name: section.label, action: .init(allowed: ["invoke"]) { _ in profileMenuPresented = false; handleSection(section) })
            }
            add("profile-menu-close", role: "button", name: "取消", action: .init(allowed: ["invoke"]) { _ in profileMenuPresented = false })
            publish("profile-menu")
            return
        }

        if composeMenuPresented {
            add("compose-menu", role: "dialog", name: "新建")
            add("compose-new-message", role: "button", name: "新消息", action: .init(allowed: ["invoke"]) { _ in composeMenuPresented = false; activeSection = .contacts })
            add("compose-new-group", role: "button", name: "新建群组", action: .init(allowed: ["invoke"]) { _ in composeMenuPresented = false; startCompose(.group) })
            add("compose-new-channel", role: "button", name: "新建频道", action: .init(allowed: ["invoke"]) { _ in composeMenuPresented = false; startCompose(.channel) })
            add("compose-contact-groups", role: "button", name: "联系人分组", action: .init(allowed: ["invoke"]) { _ in composeMenuPresented = false; contactGroupsPresented = true })
            add("compose-menu-cancel", role: "button", name: "取消", action: .init(allowed: ["invoke"]) { _ in composeMenuPresented = false })
            publish("compose-menu")
            return
        }

        switch destination {
        case .home:
            add("app-shell", role: "application", name: "Fabushi")
            add("home-sync", role: "button", name: "同步消息", enabled: !messaging.loading, action: .init(allowed: ["invoke"]) { _ in Task { await messaging.refresh() } })
            add("profile-avatar", role: "button", name: "个人菜单", action: .init(allowed: ["invoke"]) { _ in profileMenuPresented = true })
            add("home-search-button", role: "button", name: isSearching ? "关闭搜索" : "搜索对话", action: .init(allowed: ["invoke"]) { _ in isSearching.toggle(); if !isSearching { homeQuery = "" } })
            if isSearching {
                add("home-search-field", role: "textbox", name: "搜索消息", action: .init(allowed: ["setValue"]) { value in homeQuery = value ?? "" })
            }
            add("home-add-button", role: "button", name: "新建", action: .init(allowed: ["invoke"]) { _ in composeMenuPresented = true })
            add("mahayana-agent-entry", role: "button", name: "大乘助手", action: .init(allowed: ["invoke"]) { _ in agentChatPresented = true })
            if !archivedConversations.isEmpty && homeQuery.isEmpty {
                add("archive-entry", role: "button", name: "已归档", action: .init(allowed: ["invoke"]) { _ in activeSection = .archive })
            }
            for conversation in filteredConversations.prefix(100) {
                add("conversation-\(conversation.id)", role: "button", name: conversation.title, action: .init(allowed: ["invoke"]) { _ in openConversation(conversation) })
                add("conversation-pin-\(conversation.id)", role: "button", name: conversation.isPinned ? "取消置顶 \(conversation.title)" : "置顶 \(conversation.title)", action: .init(allowed: ["invoke"]) { _ in Task { await messaging.setPinned(conversation.id, pinned: !conversation.isPinned) } })
                add("conversation-mute-\(conversation.id)", role: "button", name: conversation.isMuted ? "取消静音 \(conversation.title)" : "静音 \(conversation.title)", action: .init(allowed: ["invoke"]) { _ in Task { await messaging.setMuted(conversation.id, muted: !conversation.isMuted) } })
                add("conversation-unread-\(conversation.id)", role: "button", name: conversation.markedUnread ? "取消标为未读 \(conversation.title)" : "标为未读 \(conversation.title)", action: .init(allowed: ["invoke"]) { _ in Task { await messaging.setMarkedUnread(conversation.id, markedUnread: !conversation.markedUnread) } })
                add("conversation-archive-\(conversation.id)", role: "button", name: "归档 \(conversation.title)", action: .init(allowed: ["invoke"]) { _ in Task { await messaging.setArchived(conversation.id, archived: true) } })
            }
            if let errorMessage = messaging.errorMessage { add("home-error", role: "status", name: String(errorMessage.prefix(200))) }
            publish("home")
        case .marketplace:
            add("marketplace-back", role: "button", name: "消息", action: .init(allowed: ["invoke"]) { _ in destination = .home })
            add("marketplace-search", role: "textbox", name: "搜索插件", action: .init(allowed: ["setValue"]) { value in model.query = value ?? "" })
            add("marketplace-search-submit", role: "button", name: "搜索", enabled: !model.loading, action: .init(allowed: ["invoke"]) { _ in Task { await model.refresh() } })
            add("host-status", role: "status", name: model.message)
            for plugin in model.plugins.prefix(100) {
                add("plugin-\(plugin.pluginId)", role: "group", name: plugin.displayName)
                add("open-\(plugin.pluginId)", role: "button", name: "打开 \(plugin.displayName)", action: .init(allowed: ["invoke"]) { _ in openedMiniApp = plugin })
                add("install-\(plugin.pluginId)", role: "button", name: "安装 \(plugin.displayName)", enabled: plugin.latestVersion != nil && model.installingPluginId == nil, action: .init(allowed: ["invoke"]) { _ in Task { await model.install(plugin) } })
            }
            publish("marketplace")
        case .remoteComputer:
            add("remote-computer-surface", role: "application", name: "远程控制我的电脑")
            add("remote-computer-close", role: "button", name: "关闭远程控制", action: .init(allowed: ["invoke"]) { _ in destination = .home })
            publish("remote-computer")
        }
    }

    private var homeView: some View {
        NavigationStack {
            ZStack(alignment: .bottomTrailing) {
                Color(red: 0.043, green: 0.043, blue: 0.047).ignoresSafeArea()
                ScrollView {
                    LazyVStack(spacing: 0) {
                        if isSearching {
                            HStack(spacing: 8) {
                                Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                                TextField("搜索", text: $homeQuery)
                                    .textInputAutocapitalization(.never)
                                    .autocorrectionDisabled()
                                    .foregroundStyle(.white)
                                if !homeQuery.isEmpty {
                                    Button { homeQuery = "" } label: { Image(systemName: "xmark.circle.fill") }
                                        .foregroundStyle(.secondary)
                                }
                            }
                            .padding(.horizontal, 12)
                            .frame(height: 40)
                            .background(Color.white.opacity(0.08), in: RoundedRectangle(cornerRadius: 12))
                            .padding(.horizontal, 12)
                            .padding(.vertical, 8)
                            .accessibilityIdentifier("home-search-field")
                        }

                        if !archivedConversations.isEmpty && homeQuery.isEmpty {
                            Button { activeSection = .archive } label: {
                                HStack(spacing: 12) {
                                    Image(systemName: "archivebox.fill").foregroundStyle(.secondary)
                                    Text("已归档").font(.headline).foregroundStyle(.primary)
                                    Spacer()
                                    Text("\(archivedConversations.count)").foregroundStyle(.secondary)
                                }
                                .padding(.horizontal, 16).frame(height: 48)
                            }.buttonStyle(.plain)
                        }

                        if homeQuery.isEmpty {
                            Button { agentChatPresented = true } label: {
                                HStack(spacing: 12) {
                                    avatar.frame(width: 54, height: 54)
                                    VStack(alignment: .leading, spacing: 4) {
                                        Text("大乘助手").font(.system(size: 17, weight: .semibold)).foregroundStyle(.primary)
                                        Text("Mahayana 多步骤智能体 · 实时工作流").font(.system(size: 15)).foregroundStyle(.secondary).lineLimit(1)
                                    }
                                    Spacer()
                                    Image(systemName: "chevron.right").font(.caption).foregroundStyle(.secondary)
                                }
                                .padding(.horizontal, 14).padding(.vertical, 8)
                            }
                            .buttonStyle(.plain)
                            .accessibilityIdentifier("mahayana-agent-entry")
                        }

                        ForEach(filteredConversations) { conversation in
                            conversationRow(conversation)
                                .swipeActions(edge: .leading, allowsFullSwipe: true) {
                                    Button {
                                        Task { await messaging.setPinned(conversation.id, pinned: !conversation.isPinned) }
                                    } label: { Label(conversation.isPinned ? "取消置顶" : "置顶", systemImage: "pin.fill") }
                                    .tint(.orange)
                                }
                                .swipeActions(edge: .trailing, allowsFullSwipe: false) {
                                    Button {
                                        Task { await messaging.setArchived(conversation.id, archived: true) }
                                    } label: { Label("归档", systemImage: "archivebox.fill") }
                                    .tint(.blue)
                                    Button {
                                        Task { await messaging.setMuted(conversation.id, muted: !conversation.isMuted) }
                                    } label: { Label(conversation.isMuted ? "取消静音" : "静音", systemImage: "speaker.slash.fill") }
                                    .tint(.gray)
                                }
                        }

                        if filteredConversations.isEmpty {
                            VStack(spacing: 10) {
                                Image(systemName: homeQuery.isEmpty ? "bubble.left.and.bubble.right" : "magnifyingglass")
                                    .font(.system(size: 34))
                                Text(homeQuery.isEmpty ? "还没有对话" : "没有找到结果").font(.headline)
                                Text(homeQuery.isEmpty ? "点击写消息按钮开始聊天" : "尝试其他关键词").font(.subheadline)
                            }
                            .foregroundStyle(.secondary)
                            .frame(maxWidth: .infinity)
                            .padding(.top, 90)
                        }

                        if let featureHostSmokeStatus = model.featureHostSmokeStatus {
                            Text(featureHostSmokeStatus).font(.caption2).foregroundStyle(.clear)
                                .accessibilityIdentifier("feature-host-smoke")
                        }
                    }
                }
                .accessibilityIdentifier("conversation-list")

            }
            .navigationTitle("聊天")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button { profileMenuPresented = true } label: {
                        avatar.frame(width: 34, height: 34)
                    }
                    .accessibilityLabel("个人菜单")
                    .accessibilityIdentifier("profile-avatar")
                }
                ToolbarItemGroup(placement: .topBarTrailing) {
                    Button {
                        withAnimation(.easeInOut(duration: 0.18)) { isSearching.toggle() }
                        if !isSearching { homeQuery = "" }
                    } label: { Image(systemName: isSearching ? "xmark" : "magnifyingglass") }
                    .accessibilityIdentifier("home-search-button")
                    Button { composeMenuPresented = true } label: { Image(systemName: "square.and.pencil") }
                        .accessibilityIdentifier("home-add-button")
                }
            }
        }
        .confirmationDialog("新建", isPresented: $composeMenuPresented, titleVisibility: .visible) {
            Button("新消息") { activeSection = .contacts }
            Button("新建群组") { startCompose(.group) }
            Button("新建频道") { startCompose(.channel) }
            Button("联系人分组") { contactGroupsPresented = true }
            Button("取消", role: .cancel) {}
        }
        .sheet(isPresented: $profileMenuPresented) {
            NavigationStack {
                List {
                    Section("账号") {
                        HStack(spacing: 12) {
                            avatar.frame(width: 42, height: 42)
                            VStack(alignment: .leading, spacing: 3) {
                                Text(model.accountName).font(.headline)
                                if !model.accountEmail.isEmpty { Text(model.accountEmail).font(.caption).foregroundStyle(.secondary) }
                            }
                        }
                        Button("退出登录", role: .destructive) {
                            profileMenuPresented = false
                            Task { await model.logout() }
                        }
                        .accessibilityIdentifier("mobile-logout")
                    }
                    Section("工作台") {
                        Button {
                            profileMenuPresented = false
                            destination = .remoteComputer
                        } label: {
                            Label("我的电脑", systemImage: "desktopcomputer")
                        }
                        .accessibilityIdentifier("remote-computer-entry")
                        Button {
                            profileMenuPresented = false
                            destination = .marketplace
                        } label: {
                            Label("插件市场", systemImage: "puzzlepiece.extension")
                        }
                        .accessibilityIdentifier("marketplace-entry")
                    }
                    Section("导航") {
                        ForEach(MobileSection.allCases) { section in
                            Button {
                                profileMenuPresented = false
                                handleSection(section)
                            } label: {
                                Label(section.label, systemImage: section.symbol)
                            }
                            .accessibilityIdentifier("profile-section-\(section.rawValue)")
                        }
                    }
                }
                .navigationTitle("导航")
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) {
                        Button("取消") { profileMenuPresented = false }
                    }
                }
            }
        }
        .sheet(item: $composeKind) { kind in composeSheet(kind) }
        .sheet(isPresented: $contactGroupsPresented) { simpleSectionSheet(title: "联系人分组", symbol: "folder.fill") }
        .sheet(item: $activeSection) { section in sectionSheet(section) }
        .fullScreenCover(item: $selectedConversation) { conversation in chatView(conversation) }
        .fullScreenCover(isPresented: $agentChatPresented) { agentChatView }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("app-shell")
        .task { await messaging.refresh() }
    }

    private var onboardingView: some View {
        ZStack {
            Color(red: 0.043, green: 0.043, blue: 0.047).ignoresSafeArea()
            VStack(spacing: 22) {
                Spacer()
                avatar.frame(width: 92, height: 92)
                Text("欢迎来到法布施").font(.largeTitle.bold()).foregroundStyle(.white)
                Text("统一的聊天、插件和 Mahayana 多步骤智能体工作台。每一步工作都会像消息一样实时出现。")
                    .multilineTextAlignment(.center).foregroundStyle(.white.opacity(0.72)).padding(.horizontal, 32)
                HStack(spacing: 7) {
                    ForEach(0..<3, id: \.self) { index in
                        Capsule().fill(index <= model.onboardingStep ? Color.accentColor : Color.white.opacity(0.18)).frame(width: 24, height: 5)
                    }
                }
                Spacer()
                Button(model.onboardingStep >= 2 ? "开始使用" : "继续") { model.advanceOnboarding() }
                    .buttonStyle(.borderedProminent).controlSize(.large).accessibilityIdentifier("mobile-onboarding-continue")
                Button("跳过介绍") { model.onboardingStep = 3; UserDefaults.standard.set(true, forKey: "fabushi.mobile.onboarding-complete.v1") }
                    .font(.footnote).foregroundStyle(.secondary).accessibilityIdentifier("mobile-onboarding-skip")
                if let featureHostSmokeStatus = model.featureHostSmokeStatus {
                    Text(featureHostSmokeStatus).font(.caption2).foregroundStyle(.clear).accessibilityIdentifier("feature-host-smoke")
                }
            }
            .padding(24)
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("mobile-onboarding")
    }

    private var authLoadingView: some View {
        ZStack {
            Color(red: 0.985, green: 0.985, blue: 0.978).ignoresSafeArea()
            VStack(spacing: 16) {
                avatar.frame(width: 70, height: 70)
                ProgressView().tint(.black)
                Text("正在连接 Fabushi…").foregroundStyle(Color.black.opacity(0.68))
                if let featureHostSmokeStatus = model.featureHostSmokeStatus {
                    Text(featureHostSmokeStatus).font(.caption2).foregroundStyle(.clear).accessibilityIdentifier("feature-host-smoke")
                }
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("mobile-auth-loading")
    }

    private var loginView: some View {
        ZStack {
            Color(red: 0.985, green: 0.985, blue: 0.978).ignoresSafeArea()
            GeometryReader { proxy in
                let width = proxy.size.width
                let height = proxy.size.height
                Group {
                    LoginBlob(color: Color(red: 1.00, green: 0.57, blue: 0.04), width: 92, height: 92, rotation: -8)
                        .position(x: width * 0.25, y: height * 0.16)
                    LoginBlob(color: Color(red: 0.53, green: 0.30, blue: 1.00), width: 58, height: 66, rotation: 12)
                        .position(x: width * 0.69, y: height * 0.17)
                    LoginBlob(color: Color(red: 0.00, green: 0.78, blue: 0.45), width: 82, height: 68, rotation: 5)
                        .position(x: width * 1.00, y: height * 0.32)
                    LoginBlob(color: Color(red: 0.08, green: 0.49, blue: 0.98), width: 84, height: 66, rotation: 7)
                        .position(x: width * 0.00, y: height * 0.37)
                    LoginBlob(color: Color(red: 1.00, green: 0.14, blue: 0.26), width: 92, height: 82, rotation: 8)
                        .position(x: width * 0.56, y: height * 0.79)
                    LoginBlob(color: Color(red: 0.00, green: 0.72, blue: 0.65), width: 70, height: 70, rotation: -9)
                        .position(x: width * 0.18, y: height * 0.76)
                    LoginBlob(color: Color(red: 0.64, green: 0.40, blue: 0.20), width: 62, height: 62, rotation: 17)
                        .position(x: width * 0.91, y: height * 0.68)
                }
            }
            .ignoresSafeArea()

            VStack(spacing: 0) {
                Spacer()
                VStack(spacing: 17) {
                    Text("Fabushi")
                        .font(.system(size: 48, weight: .bold, design: .rounded))
                        .tracking(-1.5)
                        .foregroundStyle(.black)
                    Text("你的常驻智能体团队，持续完成工作。")
                        .font(.system(size: 18, weight: .semibold))
                        .foregroundStyle(Color.black.opacity(0.42))
                        .multilineTextAlignment(.center)
                        .padding(.horizontal, 40)
                }
                .padding(.bottom, 74)
                Spacer()

                if model.loginError != nil {
                    Text("登录暂时不可用，请稍后重试。")
                        .font(.footnote)
                        .foregroundStyle(Color.red.opacity(0.82))
                        .multilineTextAlignment(.center)
                        .padding(.horizontal, 24)
                        .padding(.bottom, 12)
                }

                if model.browserLoginAttemptId != nil {
                    Button("继续登录") { Task { await model.reopenBrowserLogin() } }
                        .font(.system(size: 18, weight: .bold))
                        .foregroundStyle(.white)
                        .frame(maxWidth: .infinity, minHeight: 58)
                        .background(.black, in: Capsule())
                        .accessibilityIdentifier("mobile-login-reopen")
                    Button("取消登录") { Task { await model.cancelBrowserLogin() } }
                        .font(.footnote.weight(.semibold))
                        .foregroundStyle(Color.black.opacity(0.46))
                        .padding(.top, 10)
                        .accessibilityIdentifier("mobile-login-cancel")
                } else {
                    Button { Task { await model.beginBrowserLogin() } } label: {
                        HStack(spacing: 10) {
                            if model.loginBusy { ProgressView().tint(.white) }
                            Text(model.loginBusy ? "正在准备…" : "登录")
                        }
                        .font(.system(size: 18, weight: .bold))
                        .foregroundStyle(.white)
                        .frame(maxWidth: .infinity, minHeight: 58)
                        .background(.black, in: Capsule())
                    }
                    .disabled(model.loginBusy)
                    .accessibilityIdentifier("mobile-login-browser")
                }

                if let featureHostSmokeStatus = model.featureHostSmokeStatus {
                    Text(featureHostSmokeStatus).font(.caption2).foregroundStyle(.clear).accessibilityIdentifier("feature-host-smoke")
                }
            }
            .padding(.horizontal, 28)
            .padding(.bottom, 18)
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("mobile-login")
    }

    private var visibleConversations: [ConversationSummary] {
        messaging.conversations.filter { !$0.isArchived }
            .sorted { lhs, rhs in lhs.isPinned != rhs.isPinned ? lhs.isPinned : lhs.time > rhs.time }
    }

    private var archivedConversations: [ConversationSummary] { messaging.conversations.filter(\.isArchived) }

    private var filteredConversations: [ConversationSummary] {
        let query = homeQuery.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !query.isEmpty else { return visibleConversations }
        return visibleConversations.filter { $0.title.localizedCaseInsensitiveContains(query) || $0.preview.localizedCaseInsensitiveContains(query) }
    }

    private func startCompose(_ kind: ConversationKind) {
        composeName = ""
        composeDescription = ""
        composeParticipantIds = []
        composeKind = kind
    }

    @ViewBuilder
    private func composeSheet(_ kind: ConversationKind) -> some View {
        NavigationStack {
            Form {
                Section {
                    TextField(kind == .direct ? "联系人名称" : "名称", text: $composeName)
                        .accessibilityIdentifier("compose-name")
                    if kind == .channel { TextField("描述", text: $composeDescription, axis: .vertical) }
                }
                if kind == .group {
                    Section("添加成员") {
                        if messaging.contacts.isEmpty {
                            Text("暂无可用联系人").foregroundStyle(.secondary)
                        } else {
                            ForEach(messaging.contacts) { contact in
                                Button {
                                    if composeParticipantIds.contains(contact.id) { composeParticipantIds.remove(contact.id) } else { composeParticipantIds.insert(contact.id) }
                                } label: {
                                    HStack {
                                        VStack(alignment: .leading) { Text(contact.displayName).foregroundStyle(.primary); Text(contact.username.map { "@\($0)" } ?? contact.kind).font(.caption).foregroundStyle(.secondary) }
                                        Spacer()
                                        if composeParticipantIds.contains(contact.id) { Image(systemName: "checkmark.circle.fill").foregroundStyle(Color.accentColor) }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            .navigationTitle("新建\(kind.label)")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("取消") { composeKind = nil } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("创建") { createConversation(kind: kind) }.accessibilityIdentifier("compose-create").disabled(composeName.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || (kind == .group && composeParticipantIds.isEmpty))
                }
            }
        }
    }

    private func createConversation(kind: ConversationKind) {
        let title = composeName.trimmingCharacters(in: .whitespacesAndNewlines)
        let description = composeDescription
        let participantIds = Array(composeParticipantIds)
        Task {
            do {
                let created = try await messaging.createConversation(kind: kind, title: title, description: description, participantActorIds: participantIds)
                composeKind = nil
                if let created { selectedConversation = created }
            } catch {
                model.message = "创建会话失败：\(error.localizedDescription)"
            }
        }
    }

    private func handleSection(_ section: MobileSection) {
        switch section {
        case .chats: activeSection = nil
        case .miniapps: destination = .marketplace
        default: activeSection = section
        }
    }

    @ViewBuilder
    private func sectionSheet(_ section: MobileSection) -> some View {
        if section == .folders {
            NavigationStack {
                List {
                    Section {
                        Button { folderTitle = ""; folderConversationIds = []; folderIncludeGroups = false; folderIncludeChannels = false; folderEditorPresented = true } label: { Label("新建文件夹", systemImage: "folder.badge.plus") }
                    }
                    Section("我的文件夹") {
                        if messaging.folders.isEmpty { Text("暂无会话文件夹").foregroundStyle(.secondary) }
                        ForEach(messaging.folders) { folder in
                            NavigationLink {
                                List {
                                    ForEach(folderConversations(folder)) { conversation in conversationRow(conversation) }
                                }.navigationTitle(folder.title)
                            } label: {
                                HStack { Image(systemName: "folder.fill").foregroundStyle(Color.accentColor); Text(folder.title); Spacer(); Text("\(folderConversations(folder).count)").foregroundStyle(.secondary) }
                            }
                            .swipeActions { Button("删除", role: .destructive) { Task { await messaging.deleteFolder(folder.id) } } }
                        }
                    }
                }
                .navigationTitle("文件夹")
                .toolbar { ToolbarItem(placement: .topBarLeading) { Button("完成") { activeSection = nil } } }
                .sheet(isPresented: $folderEditorPresented) { folderEditorSheet }
            }
        } else if section == .contacts || section == .bots {
            NavigationStack {
                List {
                    let rows = section == .bots ? messaging.contacts.filter { $0.kind == "bot" || $0.kind == "assistant" } : messaging.contacts
                    if rows.isEmpty {
                        ContentUnavailableView(section.label, systemImage: section.symbol, description: Text("暂无可用联系人"))
                    } else {
                        ForEach(rows) { contact in
                            Button {
                                Task {
                                    do {
                                        if let conversation = try await messaging.createDirect(contact: contact) {
                                            activeSection = nil
                                            selectedConversation = conversation
                                        }
                                    } catch {
                                        model.message = "创建私聊失败：\(error.localizedDescription)"
                                    }
                                }
                            } label: {
                                HStack(spacing: 12) {
                                    ZStack {
                                        Circle().fill(Color.accentColor)
                                        Text(String(contact.displayName.prefix(1)).uppercased()).foregroundStyle(.white).fontWeight(.bold)
                                    }.frame(width: 42, height: 42)
                                    VStack(alignment: .leading) {
                                        Text(contact.displayName).foregroundStyle(.primary)
                                        Text(contact.username.map { "@\($0)" } ?? contact.kind).font(.caption).foregroundStyle(.secondary)
                                    }
                                }
                            }
                        }
                    }
                }
                .navigationTitle(section.label)
                .toolbar { ToolbarItem(placement: .topBarLeading) { Button("完成") { activeSection = nil } } }
            }
        } else if section == .groups || section == .channels || section == .archive || section == .saved {
            NavigationStack {
                List {
                    let rows = messaging.conversations.filter { conversation in
                        switch section {
                        case .groups: conversation.kind == .group && !conversation.isArchived
                        case .channels: conversation.kind == .channel && !conversation.isArchived
                        case .archive: conversation.isArchived
                        case .saved: conversation.kind == .savedMessages
                        default: false
                        }
                    }
                    if rows.isEmpty { ContentUnavailableView(section.label, systemImage: section.symbol) }
                    ForEach(rows) { conversation in
                        conversationRow(conversation)
                            .swipeActions(edge: .trailing, allowsFullSwipe: true) {
                                if section == .archive {
                                    Button("恢复") { Task { await messaging.setArchived(conversation.id, archived: false) } }.tint(.blue)
                                }
                            }
                    }
                }
                .navigationTitle(section.label)
                .toolbar { ToolbarItem(placement: .topBarLeading) { Button("完成") { activeSection = nil } } }
            }
        } else {
            simpleSectionSheet(title: section.label, symbol: section.symbol)
        }
    }

    private func folderConversations(_ folder: MessagingFolder) -> [ConversationSummary] {
        messaging.conversations.filter { conversation in
            guard !conversation.isArchived || !folder.excludeArchived else { return false }
            guard !conversation.isMuted || !folder.excludeMuted else { return false }
            guard conversation.unreadCount > 0 || !folder.excludeRead else { return false }
            return folder.conversationIds.contains(conversation.id) || (folder.includeGroups && conversation.kind == .group) || (folder.includeChannels && conversation.kind == .channel)
        }
    }

    private var folderEditorSheet: some View {
        NavigationStack {
            Form {
                Section("名称") { TextField("文件夹名称", text: $folderTitle) }
                Section("自动包含") {
                    Toggle("群组", isOn: $folderIncludeGroups)
                    Toggle("频道", isOn: $folderIncludeChannels)
                }
                Section("选择会话") {
                    ForEach(messaging.conversations.filter { !$0.isArchived }) { conversation in
                        Button {
                            if folderConversationIds.contains(conversation.id) { folderConversationIds.remove(conversation.id) } else { folderConversationIds.insert(conversation.id) }
                        } label: {
                            HStack { Text(conversation.title).foregroundStyle(.primary); Spacer(); if folderConversationIds.contains(conversation.id) { Image(systemName: "checkmark.circle.fill").foregroundStyle(Color.accentColor) } }
                        }
                    }
                }
            }
            .navigationTitle("新建文件夹")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("取消") { folderEditorPresented = false } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("创建") {
                        let folder = MessagingFolder(
                            id: "folder-\(UUID().uuidString.lowercased())", title: folderTitle.trimmingCharacters(in: .whitespacesAndNewlines), icon: "folder",
                            conversationIds: Array(folderConversationIds), includeContacts: false, includeBots: false, includeGroups: folderIncludeGroups, includeChannels: folderIncludeChannels,
                            excludeMuted: false, excludeRead: false, excludeArchived: true
                        )
                        Task { await messaging.upsertFolder(folder) }
                        folderEditorPresented = false
                    }.disabled(folderTitle.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                }
            }
        }
    }

    @ViewBuilder
    private func simpleSectionSheet(title: String, symbol: String) -> some View {
        NavigationStack {
            ContentUnavailableView(title, systemImage: symbol, description: Text("此入口与桌面端共用同一业务能力；移动端采用 Telegram 式单栈导航。"))
                .navigationTitle(title)
                .toolbar { ToolbarItem(placement: .topBarLeading) { Button("完成") { activeSection = nil; contactGroupsPresented = false } } }
        }
    }

    private func conversationRow(_ conversation: ConversationSummary) -> some View {
        Button {
            selectedConversation = conversation
            Task { await messaging.markRead(conversation.id) }
        } label: {
            HStack(spacing: 12) {
                ZStack {
                    Circle().fill(Color.accentColor.opacity(0.85))
                    Text(conversation.badge.isEmpty ? "✦" : conversation.badge).font(.headline).foregroundStyle(.white)
                }.frame(width: 54, height: 54)
                VStack(alignment: .leading, spacing: 5) {
                    HStack(spacing: 5) {
                        Text(conversation.title).font(.system(size: 17, weight: .semibold)).foregroundStyle(.primary).lineLimit(1)
                        if conversation.isMuted { Image(systemName: "speaker.slash.fill").font(.caption2).foregroundStyle(.secondary) }
                        if conversation.isPinned { Image(systemName: "pin.fill").font(.caption2).foregroundStyle(.secondary) }
                    }
                    Text(conversation.preview).font(.system(size: 15)).foregroundStyle(.secondary).lineLimit(1)
                }
                Spacer(minLength: 8)
                VStack(alignment: .trailing, spacing: 6) {
                    Text(conversation.time).font(.caption).foregroundStyle(.secondary)
                    if conversation.unreadCount > 0 {
                        Text("\(conversation.unreadCount)").font(.caption2.bold()).foregroundStyle(.white)
                            .padding(.horizontal, 6).frame(minWidth: 20, minHeight: 20).background(Color.accentColor, in: Capsule())
                    } else if conversation.markedUnread {
                        Circle().fill(Color.accentColor).frame(width: 10, height: 10)
                    }
                }
            }
            .padding(.horizontal, 14).padding(.vertical, 8)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .contextMenu {
            Button(conversation.isPinned ? "取消置顶" : "置顶", systemImage: "pin") { Task { await messaging.setPinned(conversation.id, pinned: !conversation.isPinned) } }
            Button(conversation.isMuted ? "取消静音" : "静音", systemImage: "speaker.slash") { Task { await messaging.setMuted(conversation.id, muted: !conversation.isMuted) } }
            Button(conversation.markedUnread ? "取消标为未读" : "标为未读", systemImage: "circle.fill") { Task { await messaging.setMarkedUnread(conversation.id, markedUnread: !conversation.markedUnread) } }
            Button(conversation.isArchived ? "恢复" : "归档", systemImage: "archivebox") { Task { await messaging.setArchived(conversation.id, archived: !conversation.isArchived) } }
        }
        .accessibilityIdentifier("conversation-\(conversation.id)")
    }

    private var agentChatView: some View {
        NavigationStack {
            ZStack {
                Color(red: 0.055, green: 0.06, blue: 0.07).ignoresSafeArea()
                VStack(spacing: 0) {
                    ScrollViewReader { proxy in
                        ScrollView {
                            LazyVStack(alignment: .leading, spacing: 10) {
                                if model.chatMessages.isEmpty {
                                    VStack(spacing: 10) {
                                        avatar.frame(width: 68, height: 68)
                                        Text("大乘助手").font(.title2.bold()).foregroundStyle(.white)
                                        Text("这是 Mahayana 多步骤智能体。真实的模型路由、工具调用和每一步工作会逐条显示在这里。")
                                            .multilineTextAlignment(.center).font(.subheadline).foregroundStyle(.white.opacity(0.65))
                                    }
                                    .frame(maxWidth: .infinity).padding(.top, 90).padding(.horizontal, 26)
                                }
                                ForEach(model.chatMessages) { entry in
                                    agentChatEntry(entry)
                                        .id(entry.id)
                                }
                            }
                            .padding(.horizontal, 14).padding(.vertical, 18)
                        }
                        .onChange(of: model.chatMessages.count) { _, _ in
                            if let last = model.chatMessages.last { withAnimation(.easeOut(duration: 0.18)) { proxy.scrollTo(last.id, anchor: .bottom) } }
                        }
                    }

                    HStack(spacing: 9) {
                        TextField("消息大乘助手", text: $model.chatDraft, axis: .vertical)
                            .lineLimit(1...5).textFieldStyle(.plain).foregroundStyle(.white)
                            .padding(.horizontal, 13).padding(.vertical, 10)
                            .background(Color.white.opacity(0.10), in: RoundedRectangle(cornerRadius: 18))
                            .onSubmit { if !model.chatBusy { Task { await model.sendChat() } } }
                        if model.chatBusy {
                            Button { Task { await model.stopChat() } } label: {
                                Image(systemName: "stop.fill").font(.system(size: 15, weight: .bold)).foregroundStyle(.white)
                                    .frame(width: 38, height: 38).background(Color.red, in: Circle())
                            }.accessibilityIdentifier("mahayana-stop")
                        } else {
                            Button { Task { await model.sendChat() } } label: {
                                Image(systemName: "arrow.up").font(.system(size: 18, weight: .bold)).foregroundStyle(.white)
                                    .frame(width: 38, height: 38).background(Color.accentColor, in: Circle())
                            }.disabled(model.chatDraft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                                .accessibilityIdentifier("mahayana-send")
                        }
                    }
                    .padding(.horizontal, 9).padding(.vertical, 8).background(.ultraThinMaterial)
                }
            }
            .navigationTitle("大乘助手")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("关闭") { agentChatPresented = false }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Text(model.chatBusy ? "正在工作" : "Mahayana").font(.caption).foregroundStyle(model.chatBusy ? .orange : .secondary)
                }
            }
        }
        .accessibilityIdentifier("mahayana-agent-chat")
    }

    @ViewBuilder
    private func agentChatEntry(_ entry: MobileChatMessage) -> some View {
        if entry.kind == .thinking {
            HStack(spacing: 9) {
                avatar.frame(width: 30, height: 30)
                VStack(alignment: .leading, spacing: 2) {
                    Text(entry.actionTitle ?? "正在思考").font(.subheadline.weight(.semibold)).foregroundStyle(.white)
                    HStack(spacing: 5) { ProgressView().controlSize(.mini).tint(.orange); Text("Mahayana 正在处理…").font(.caption).foregroundStyle(.secondary) }
                }
                Spacer()
            }
            .padding(10).background(Color.orange.opacity(0.10), in: RoundedRectangle(cornerRadius: 13))
            .overlay(RoundedRectangle(cornerRadius: 13).stroke(Color.orange.opacity(0.24)))
            .accessibilityIdentifier("mahayana-thinking")
        } else if entry.kind == .action {
            HStack(spacing: 8) {
                avatar.frame(width: 25, height: 25)
                VStack(alignment: .leading, spacing: 2) {
                    Text(entry.actionTitle ?? "助手动作").font(.caption.weight(.semibold)).foregroundStyle(.white)
                    if let detail = entry.actionDetail, !detail.isEmpty { Text(detail).font(.caption2).foregroundStyle(.secondary).lineLimit(2) }
                }
                Spacer()
                Text(entry.actionStatus == "failed" ? "失败" : entry.actionStatus == "running" ? "进行中" : "完成")
                    .font(.caption2).foregroundStyle(entry.actionStatus == "failed" ? .red : entry.actionStatus == "running" ? .orange : .green)
            }
            .padding(.horizontal, 10).padding(.vertical, 8).background(Color.white.opacity(0.07), in: RoundedRectangle(cornerRadius: 11))
            .accessibilityIdentifier("mahayana-step")
        } else if entry.role == .user {
            HStack { Spacer(minLength: 44); Text(entry.text).foregroundStyle(.white).padding(.horizontal, 13).padding(.vertical, 10).background(Color.black, in: RoundedRectangle(cornerRadius: 16)) }
        } else {
            HStack(alignment: .top, spacing: 8) {
                avatar.frame(width: 28, height: 28)
                Text(entry.text).foregroundStyle(.white).padding(.horizontal, 13).padding(.vertical, 10).background(Color.white.opacity(0.12), in: RoundedRectangle(cornerRadius: 16))
                Spacer(minLength: 28)
            }
        }
    }

    @ViewBuilder
    private func chatView(_ conversation: ConversationSummary) -> some View {
        NavigationStack {
            ZStack {
                Color(red: 0.055, green: 0.06, blue: 0.07).ignoresSafeArea()
                VStack(spacing: 0) {
                    if chatSearchPresented {
                        HStack(spacing: 8) {
                            Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                            TextField("搜索此聊天", text: $chatSearchQuery).textInputAutocapitalization(.never).autocorrectionDisabled()
                            if !chatSearchQuery.isEmpty { Button { chatSearchQuery = "" } label: { Image(systemName: "xmark.circle.fill") } }
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
                                ForEach((messaging.messagesByConversation[conversation.id] ?? []).filter { message in
                                    chatSearchQuery.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || message.text.localizedCaseInsensitiveContains(chatSearchQuery)
                                }) { message in
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
                                                        Text("\(reaction.reaction) \(reaction.count)").font(.caption2).padding(.horizontal, 7).padding(.vertical, 3)
                                                            .background(reaction.chosenByMe ? Color.accentColor.opacity(0.25) : Color.white.opacity(0.08), in: Capsule())
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
                                            Button("👍", systemImage: "hand.thumbsup") { Task { await messaging.setReaction(conversationId: conversation.id, messageId: message.id, reaction: "👍", enabled: true) } }
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
                        .onChange(of: messaging.messagesByConversation[conversation.id]?.count ?? 0) { _, _ in
                            if let id = messaging.messagesByConversation[conversation.id]?.last?.id { withAnimation { proxy.scrollTo(id, anchor: .bottom) } }
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
                ToolbarItem(placement: .topBarLeading) { Button { chatSearchPresented = false; chatSearchQuery = ""; selectedConversation = nil } label: { Image(systemName: "chevron.left") } }
                ToolbarItem(placement: .topBarTrailing) {
                    Menu {
                        Button(chatSearchPresented ? "关闭搜索" : "搜索", systemImage: "magnifyingglass") { chatSearchPresented.toggle(); if !chatSearchPresented { chatSearchQuery = "" } }
                        Button(conversation.isMuted ? "取消静音" : "静音", systemImage: "speaker.slash") { Task { await messaging.setMuted(conversation.id, muted: !conversation.isMuted) } }
                        Button(conversation.isPinned ? "取消置顶" : "置顶", systemImage: "pin") { Task { await messaging.setPinned(conversation.id, pinned: !conversation.isPinned) } }
                        Button("标为未读", systemImage: "circle.fill") { Task { await messaging.setMarkedUnread(conversation.id, markedUnread: true) }; selectedConversation = nil }
                        Button("归档", systemImage: "archivebox") { Task { await messaging.setArchived(conversation.id, archived: true) }; selectedConversation = nil }
                    } label: { Image(systemName: "ellipsis.circle") }
                }
            }
        }
        .task(id: conversation.id) {
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
            NavigationStack {
                List(messaging.conversations.filter { $0.id != conversation.id && !$0.isArchived }) { destination in
                    Button {
                        Task { await messaging.forwardMessage(sourceConversationId: conversation.id, messageId: message.id, destinationConversationId: destination.id) }
                        forwardMessage = nil
                    } label: { Text(destination.title) }
                }
                .navigationTitle("转发到")
                .toolbar { ToolbarItem(placement: .cancellationAction) { Button("取消") { forwardMessage = nil } } }
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

    private func sendMessage(in conversation: ConversationSummary, silent: Bool = false, scheduledAtMs: Int64? = nil) {
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

    private func scheduleDraftSync(conversationId: String) {
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

    private var avatar: some View {
        ZStack {
            Circle().fill(Color.white.opacity(0.08))
            Text("✦").font(.system(size: 22, weight: .bold)).foregroundStyle(.orange)
        }
    }

    private var marketplaceView: some View {
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
            .navigationTitle("法布施")
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("消息") { destination = .home }
                }
            }
            .refreshable { await model.refresh() }
            .task {
                if model.plugins.isEmpty {
                    await model.refresh()
                }
            }
        }
    }
}
