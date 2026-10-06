import SwiftUI
import UniformTypeIdentifiers
import UIKit

extension ContentView {
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

    var authenticatedContent: some View {
        Group {
            switch destination {
            case .home:
                homeView
            case .marketplace:
                marketplaceView
            case .remoteComputer:
                RemoteComputerSurface(
                    bridge: bridge,
                    reconnectGeneration: reconnectGeneration
                ) {
                    destination = .home
                }
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

    var appAgentSurfaceFingerprint: String {
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
    String(signOutConfirmationPresented),
    String(model.accountUsageLoading),
    model.accountUsage.map { "\($0.windowStart):\($0.windowEnd):\($0.tokenLimit):\($0.remainingTokens):\($0.unlimited)" } ?? "",
    model.accountUsageError ?? "",
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
    func publishAppAgentSurface() {
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

        if signOutConfirmationPresented {
            add("mobile-logout-dialog", role: "alertdialog", name: "退出登录")
            add(
                "mobile-logout-cancel",
                role: "button",
                name: "取消退出登录",
                action: .init(allowed: ["invoke"]) { _ in
                    signOutConfirmationPresented = false
                }
            )
            add(
                "mobile-logout-confirm",
                role: "button",
                name: "确认退出登录",
                enabled: !model.loginBusy,
                action: .init(allowed: ["invoke"]) { _ in
                    Task { @MainActor in
                        await model.logout()
                        if !model.loggedIn {
                            signOutConfirmationPresented = false
                            profileMenuPresented = false
                        }
                    }
                }
            )
            if model.loggedIn, model.message.hasPrefix("退出登录失败") {
                add("mobile-logout-error", role: "status", name: String(model.message.prefix(200)))
            }
            publish("account-sign-out")
            return
        }

        if profileMenuPresented {
            add("profile-menu", role: "dialog", name: "导航")
            add("profile-account", role: "status", name: model.accountName)
            if let usage = model.accountUsage {
                add("profile-usage", role: "status", name: usage.semanticSummary)
            } else if model.accountUsageLoading {
                add("profile-usage-loading", role: "status", name: "正在加载账号用量")
            } else if model.accountUsageError != nil {
                add(
                    "profile-usage-refresh",
                    role: "button",
                    name: "重新加载账号用量",
                    action: .init(allowed: ["invoke"]) { _ in
                        Task { await model.refreshAccountUsage() }
                    }
                )
            }
            add(
                "mobile-logout",
                role: "button",
                name: "退出登录",
                action: .init(allowed: ["invoke"]) { _ in
                    signOutConfirmationPresented = true
                }
            )
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
}
