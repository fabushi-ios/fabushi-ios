//
//  ChatStageView.swift
//  Fabushi
//
//  Full native SwiftUI 1:1 Telegram Conversation Stage.
//  Implements inverted scroll physics (.scaleEffect(y: -1)), pinned messages header,
//  floating hysteretic scroll-to-bottom button, CoreText message bubble rows,
//  swipe-to-reply, context menu, and secret chat theme overlays.
//

import SwiftUI
#if canImport(UIKit)
import UIKit
#endif

public struct ChatStageView: View {
    @StateObject public var viewModel: ConversationStageViewModel
    @Environment(\.colorScheme) private var colorScheme

    @StateObject private var attachmentViewModel = AttachmentDrawerViewModel()
    @StateObject private var stickerViewModel = StickerEmojiPickerViewModel()

    public init(viewModel: ConversationStageViewModel) {
        _viewModel = StateObject(wrappedValue: viewModel)
    }

    public var body: some View {
        ZStack(alignment: .bottomTrailing) {
            VStack(spacing: 0) {
                // Pinned Messages Carousel Bar
                if !viewModel.pinnedMessages.isEmpty {
                    PinnedMessagesBar(
                        pinnedMessages: viewModel.pinnedMessages,
                        activeIndex: viewModel.activePinnedIndex,
                        onTapBar: {
                            _ = viewModel.cyclePinnedMessages()
                        },
                        onOpenAllSheet: {},
                        onUnpin: { msgId in
                            viewModel.unpinMessage(id: msgId)
                        }
                    )
                }

                // Inverted ScrollView message stage
                ScrollViewReader { proxy in
                    ScrollView {
                        LazyVStack(spacing: 4) {
                            // Bottom sentinel for scroll distance tracking (in inverted coords, top of stack)
                            Color.clear
                                .frame(height: 1)
                                .id("chat-bottom-sentinel")

                            ForEach(Array(viewModel.messages.enumerated()), id: \.element.id) { index, message in
                                MessageBubbleRowView(
                                    message: message,
                                    clusterInfo: MessageClusterCalculator.calculate(for: viewModel.messages, at: index, isInverted: true),
                                    isHighlighted: viewModel.highlightedMessageId == message.id,
                                    isSecretChat: viewModel.isSecretChat,
                                    onSelectReaction: { emoji in
                                        viewModel.toggleReaction(messageId: message.id, emoji: emoji)
                                    },
                                    onMenuAction: { action in
                                        handleMenuAction(action, for: message)
                                    },
                                    onReply: { targetMsg in
                                        viewModel.activeReplyMessage = targetMsg
                                    },
                                    onExpireSecretMessage: {
                                        viewModel.deleteMessage(id: message.id)
                                    }
                                )
                                .id(message.id)
                                // Symmetrical inversion on each row
                                .scaleEffect(y: -1)
                            }
                        }
                        .padding(.horizontal, 10)
                        .padding(.vertical, 8)
                    }
                    // Symmetrical inversion on scroll view: physics locks offset 0 to physical bottom
                    .scaleEffect(y: -1)
                    .onChange(of: viewModel.highlightedMessageId) { _, targetId in
                        guard let targetId else { return }
                        withAnimation(.spring(response: 0.35, dampingFraction: 0.8)) {
                            proxy.scrollTo(targetId, anchor: .center)
                        }
                    }
                }

                // Native Telegram Auto-Expanding Composer Bar with Gestures
                TelegramComposerBar(
                    text: $viewModel.draftText,
                    replyMessage: $viewModel.activeReplyMessage,
                    placeholder: "Message",
                    maxCharacters: 4096,
                    isSecretChat: viewModel.isSecretChat,
                    onSendMessage: { text, replyTo in
                        Task {
                            await viewModel.sendTextMessage(text, replyTo: replyTo)
                        }
                    },
                    onSendVoice: { url, waveform, durationMs in
                        let voiceSnippet = "🎤 Voice Message (\(max(1, durationMs / 1000))s)"
                        Task {
                            await viewModel.sendTextMessage(voiceSnippet, replyTo: viewModel.activeReplyMessage?.id)
                        }
                    },
                    onAttachmentTap: {
                        viewModel.showAttachmentDrawer = true
                    },
                    onStickerTap: {
                        viewModel.showStickerPicker = true
                    },
                    onJumpToReply: { replyId in
                        viewModel.highlightedMessageId = replyId
                    },
                    onDismissReply: {
                        viewModel.activeReplyMessage = nil
                    }
                )
            }

            // Floating Scroll-To-Bottom Button
            ScrollToBottomButton(
                unreadCount: viewModel.unreadScrolledCount,
                isVisible: viewModel.isBottomButtonVisible,
                onTap: {
                    withAnimation(.spring(response: 0.35, dampingFraction: 0.8)) {
                        viewModel.scrollToBottomAction()
                    }
                }
            )
            .padding(.trailing, 16)
            .padding(.bottom, 60)
        }
        .navigationTitle(viewModel.isSecretChat ? "🔒 Secret Chat" : "Chat")
        #if os(iOS)
        .navigationBarTitleDisplayMode(.inline)
        #endif
        .toolbar {
            if viewModel.isSecretChat {
                #if os(iOS)
                ToolbarItem(placement: .topBarTrailing) {
                    Image(systemName: SecretChatVisualTheme.padlockIcon)
                        .foregroundStyle(SecretChatVisualTheme.accentGreen)
                }
                #else
                ToolbarItem(placement: .primaryAction) {
                    Image(systemName: SecretChatVisualTheme.padlockIcon)
                        .foregroundStyle(SecretChatVisualTheme.accentGreen)
                }
                #endif
            }
        }
        .task {
            await viewModel.loadInitialMessages()
        }
        .sheet(isPresented: $viewModel.showAttachmentDrawer) {
            AttachmentDrawerSheet(
                viewModel: attachmentViewModel,
                onSendMedia: { items, withoutCompression in
                    Task {
                        for item in items {
                            let label = item.isVideo ? "📹 Video" : "📷 Photo"
                            await viewModel.sendTextMessage(label)
                        }
                    }
                },
                onDismiss: { viewModel.showAttachmentDrawer = false }
            )
        }
        .sheet(isPresented: $viewModel.showStickerPicker) {
            StickerEmojiPickerSheet(
                viewModel: stickerViewModel,
                onSendSticker: { sticker in
                    Task {
                        await viewModel.sendTextMessage(sticker.emojiHint)
                    }
                },
                onSendEmoji: { emoji in
                    viewModel.draftText += emoji
                },
                onSendGIF: { gif in
                    Task {
                        await viewModel.sendTextMessage("GIF")
                    }
                },
                onDismiss: { viewModel.showStickerPicker = false }
            )
        }
        .overlay {
            if let mediaVM = viewModel.activeMediaViewerVM {
                TelegramMediaViewer(
                    viewModel: mediaVM,
                    onDismiss: {
                        withAnimation(.spring(response: 0.35, dampingFraction: 0.75)) {
                            viewModel.activeMediaViewerVM = nil
                        }
                    }
                )
                .transition(.opacity)
            }
        }
        .accessibilityIdentifier("chat-stage-view")
    }

    private func handleMenuAction(_ action: MessageMenuAction, for message: TelegramStageMessage) {
        switch action {
        case .reply:
            viewModel.activeReplyMessage = message
        case .copy:
            #if canImport(UIKit)
            UIPasteboard.general.string = message.text
            #endif
        case .pin:
            viewModel.pinMessage(id: message.id)
        case .delete:
            viewModel.deleteMessage(id: message.id)
        case .forward:
            break
        }
    }
}

// MARK: - Individual Message Bubble Row View
public struct MessageBubbleRowView: View {
    public let message: TelegramStageMessage
    public let clusterInfo: (MessageClusterPosition, BubbleCornerRadii)
    public let isHighlighted: Bool
    public let isSecretChat: Bool
    public let onSelectReaction: (String) -> Void
    public let onMenuAction: (MessageMenuAction) -> Void
    public let onReply: (TelegramStageMessage) -> Void
    public let onExpireSecretMessage: () -> Void

    @Environment(\.colorScheme) private var colorScheme

    private var bubbleBackgroundColor: Color {
        if message.isOutgoing {
            if isSecretChat {
                return colorScheme == .dark
                    ? SecretChatVisualTheme.outgoingBubbleDarkGreen
                    : SecretChatVisualTheme.outgoingBubbleLightGreen
            }
            return colorScheme == .dark
                ? Color(conversationHex: "#2B5278")
                : Color(conversationHex: "#E1FFC7")
        } else {
            return colorScheme == .dark
                ? Color(conversationHex: "#182533")
                : Color(conversationHex: "#EFEFEF")
        }
    }

    public var body: some View {
        HStack(alignment: .bottom, spacing: 0) {
            if message.isOutgoing {
                Spacer(minLength: 40)
            }

            VStack(alignment: message.isOutgoing ? .trailing : .leading, spacing: 2) {
                // Group Chat Sender Name Header
                if !message.isOutgoing && (clusterInfo.0 == .single || clusterInfo.0 == .top) {
                    let senderTitle = message.senderName ?? "User \(message.senderId)"
                    Text(senderTitle)
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundStyle(TelegramPeerColorPalette.color(for: message.senderId))
                        .padding(.horizontal, 10)
                        .padding(.top, 2)
                        .accessibilityIdentifier("sender-name-header")
                }

                // Bubble Body with CoreText Typography & Collision Layout
                GeometryReader { geo in
                    let maxAvailable = geo.size.width * 0.75
                    let metrics = TextTimestampMetrics.compute(
                        text: message.text,
                        fontSize: 16,
                        maxWidth: maxAvailable
                    )

                    ZStack(alignment: .bottomTrailing) {
                        // Main text formatting
                        MessageFormattingView(
                            text: message.text,
                            entities: message.entities
                        )
                        .padding(.horizontal, 12)
                        .padding(.top, 8)
                        .padding(.bottom, metrics.causesCollision ? 24 : 8)

                        // Bottom-right inline timestamp + delivery status
                        MessageDeliveryTicksView(
                            timestamp: message.timestamp,
                            status: message.status,
                            isOutgoing: message.isOutgoing,
                            isEdited: message.isEdited
                        )
                    }
                    .frame(
                        width: max(44, min(maxAvailable, metrics.contentWidth + 24)),
                        height: max(36, metrics.requiredHeight + 16),
                        alignment: message.isOutgoing ? .trailing : .leading
                    )
                    .background(bubbleBackgroundColor)
                    .clipShape(
                        TelegramBubbleShape(
                            isOutgoing: message.isOutgoing,
                            radii: clusterInfo.1
                        )
                    )
                    .overlay(
                        // Flash highlight overlay for pinned jump (TC-T1-F17-02)
                        TelegramBubbleShape(isOutgoing: message.isOutgoing, radii: clusterInfo.1)
                            .fill(Color.accentColor.opacity(isHighlighted ? 0.35 : 0.0))
                            .animation(.easeInOut(duration: 0.3), value: isHighlighted)
                    )
                }
                .frame(minHeight: 44)

                // Ephemeral countdown progress bar for Secret Chats
                if isSecretChat, let ttl = message.ttlSeconds, let expiresAt = message.expiresAtMs {
                    EphemeralTimerProgressBar(
                        ttlSeconds: ttl,
                        expiresAtMs: expiresAt,
                        onExpire: onExpireSecretMessage
                    )
                    .frame(maxWidth: 180)
                }

                // Attached Reaction Pills Bar
                if !message.reactions.isEmpty {
                    HStack(spacing: 4) {
                        ForEach(message.reactions) { rx in
                            HStack(spacing: 2) {
                                Text(rx.emoji)
                                    .font(.system(size: 12))
                                if rx.count > 1 {
                                    Text("\(rx.count)")
                                        .font(.system(size: 11, weight: .bold))
                                        .foregroundStyle(rx.chosenByMe ? Color.accentColor : Color.secondary)
                                }
                            }
                            .padding(.horizontal, 6)
                            .padding(.vertical, 2)
                            .background(
                                Capsule()
                                    .fill(rx.chosenByMe ? Color.accentColor.opacity(0.15) : Color.secondary.opacity(0.12))
                            )
                            .overlay(
                                Capsule()
                                    .stroke(rx.chosenByMe ? Color.accentColor : Color.clear, lineWidth: 1)
                            )
                            .contentShape(Capsule())
                            .onTapGesture {
                                onSelectReaction(rx.emoji)
                            }
                        }
                    }
                    .padding(.horizontal, 6)
                }
            }
            .messageSwipeToReply(message: message, onReply: onReply)
            .messageContextMenu(
                message: message,
                onSelectReaction: onSelectReaction,
                onAction: onMenuAction
            )

            if !message.isOutgoing {
                Spacer(minLength: 40)
            }
        }
        .accessibilityIdentifier("message-bubble-row-\(message.id)")
    }
}
