//
//  ChatListRowView.swift
//  Fabushi
//
//  Native iOS 1:1 Telegram Chat Row with Avatar Gradients, Online Indicator,
//  Spring Swipe Actions, Delivery Ticks, and Context Menu Peek Preview.
//

import SwiftUI
#if canImport(UIKit)
import UIKit
#endif

// MARK: - Row Metrics
public enum ChatListRowMetrics {
    public static let rowHeight: CGFloat = 76.0
    public static let avatarSize: CGFloat = 54.0
    public static let avatarLeadingPadding: CGFloat = 12.0
    public static let contentHorizontalSpacing: CGFloat = 10.0
    public static let trailingPadding: CGFloat = 14.0
}

// MARK: - ChatListRowView
struct ChatListRowView: View {
    let chat: TelegramChatSummary
    let typingActor: String?
    let onOpenChat: () -> Void
    let onTogglePin: () -> Void
    let onToggleMute: () -> Void
    let onToggleMarkRead: () -> Void
    let onArchive: () -> Void
    let onDelete: () -> Void

    #if canImport(UIKit)
    private let impactFeedback = UIImpactFeedbackGenerator(style: .medium)
    #endif

    init(
        chat: TelegramChatSummary,
        typingActor: String? = nil,
        onOpenChat: @escaping () -> Void,
        onTogglePin: @escaping () -> Void,
        onToggleMute: @escaping () -> Void,
        onToggleMarkRead: @escaping () -> Void,
        onArchive: @escaping () -> Void,
        onDelete: @escaping () -> Void
    ) {
        self.chat = chat
        self.typingActor = typingActor
        self.onOpenChat = onOpenChat
        self.onTogglePin = onTogglePin
        self.onToggleMute = onToggleMute
        self.onToggleMarkRead = onToggleMarkRead
        self.onArchive = onArchive
        self.onDelete = onDelete
    }

    var body: some View {
        Button(action: onOpenChat) {
            HStack(spacing: ChatListRowMetrics.contentHorizontalSpacing) {
                // Left: Avatar with Deterministic Gradient & Online Dot
                avatarView
                    .padding(.leading, ChatListRowMetrics.avatarLeadingPadding)

                // Center: Title, Delivery/Pin Icons, Message Preview / Typing Indicator
                VStack(alignment: .leading, spacing: 4) {
                    HStack(spacing: 4) {
                        Text(chat.title)
                            .font(.system(size: 16, weight: .semibold))
                            .foregroundStyle(Color.primary)
                            .lineLimit(1)

                        if chat.isMuted {
                            Image(systemName: "speaker.slash.fill")
                                .font(.system(size: 11, weight: .semibold))
                                .foregroundStyle(tertiaryLabelColor)
                        }

                        if chat.isPinned {
                            Image(systemName: "pin.fill")
                                .font(.system(size: 11, weight: .semibold))
                                .foregroundStyle(tertiaryLabelColor)
                        }

                        Spacer(minLength: 4)

                        // Delivery ticks (for outgoing messages)
                        deliveryStatusView

                        // Timestamp
                        Text(chat.formattedTime)
                            .font(.system(size: 13, weight: .regular))
                            .foregroundStyle(chat.unreadCount > 0 && !chat.isMuted ? Color.accentColor : secondaryLabelColor)
                    }

                    HStack(alignment: .center, spacing: 6) {
                        if let actor = typingActor, !actor.isEmpty {
                            // Real-time Typing Indicator
                            typingIndicatorView(actor: actor)
                        } else {
                            // Message Preview
                            Text(chat.displayPreview)
                                .font(.system(size: 15, weight: .regular))
                                .foregroundStyle(secondaryLabelColor)
                                .lineLimit(2)
                                .multilineTextAlignment(.leading)
                        }

                        Spacer(minLength: 8)

                        // Unread Badge Capsule
                        unreadBadgeView
                    }
                }
                .padding(.trailing, ChatListRowMetrics.trailingPadding)
            }
            .frame(height: ChatListRowMetrics.rowHeight)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        // MARK: - Spring Swipe Actions
        .swipeActions(edge: .trailing, allowsFullSwipe: true) {
            // Delete (Destructive)
            Button(role: .destructive) {
                #if canImport(UIKit)
                impactFeedback.impactOccurred()
                #endif
                onDelete()
            } label: {
                Label("Delete", systemImage: "trash.fill")
            }
            .tint(systemRedColor)

            // Archive (Purple/Slate)
            Button {
                #if canImport(UIKit)
                impactFeedback.impactOccurred()
                #endif
                onArchive()
            } label: {
                Label(chat.kind == "archived" ? "Unarchive" : "Archive", systemImage: "archivebox.fill")
            }
            .tint(Color(red: 0.35, green: 0.34, blue: 0.84))

            // Mute / Unmute
            Button {
                #if canImport(UIKit)
                impactFeedback.impactOccurred()
                #endif
                onToggleMute()
            } label: {
                Label(chat.isMuted ? "Unmute" : "Mute", systemImage: chat.isMuted ? "speaker.wave.2.fill" : "speaker.slash.fill")
            }
            .tint(chat.isMuted ? Color.accentColor : systemOrangeColor)
        }
        .swipeActions(edge: .leading, allowsFullSwipe: true) {
            // Pin / Unpin
            Button {
                #if canImport(UIKit)
                impactFeedback.impactOccurred()
                #endif
                onTogglePin()
            } label: {
                Label(chat.isPinned ? "Unpin" : "Pin", systemImage: chat.isPinned ? "pin.slash.fill" : "pin.fill")
            }
            .tint(Color.accentColor)

            // Mark Read / Unread
            Button {
                #if canImport(UIKit)
                impactFeedback.impactOccurred()
                #endif
                onToggleMarkRead()
            } label: {
                let isUnread = chat.unreadCount > 0
                Label(isUnread ? "Read" : "Unread", systemImage: isUnread ? "envelope.open.fill" : "envelope.badge.fill")
            }
            .tint(Color.accentColor)
        }
        // MARK: - Context Menu Peek & Pop
        .contextMenu(menuItems: {
            Button {
                onTogglePin()
            } label: {
                Label(chat.isPinned ? "Unpin" : "Pin", systemImage: chat.isPinned ? "pin.slash" : "pin")
            }

            Button {
                onToggleMute()
            } label: {
                Label(chat.isMuted ? "Unmute" : "Mute", systemImage: chat.isMuted ? "speaker.wave.2" : "speaker.slash")
            }

            Button {
                onToggleMarkRead()
            } label: {
                let isUnread = chat.unreadCount > 0
                Label(isUnread ? "Mark as Read" : "Mark as Unread", systemImage: isUnread ? "checkmark.message" : "envelope.badge")
            }

            Button {
                onArchive()
            } label: {
                Label(chat.kind == "archived" ? "Unarchive" : "Archive", systemImage: "archivebox")
            }

            Divider()

            Button(role: .destructive) {
                onDelete()
            } label: {
                Label("Delete Chat", systemImage: "trash")
            }
        }, preview: {
            ChatRowPeekPreviewView(chat: chat)
        })
        .accessibilityIdentifier("chat-row-\(chat.id)")
    }

    // MARK: - Subviews

    // 1. Avatar with Deterministic 7-Color Gradient & Online Dot
    private var avatarView: some View {
        ZStack(alignment: .bottomTrailing) {
            // Gradient circle avatar
            Circle()
                .fill(deterministicGradient(for: String(chat.id)))
                .frame(width: ChatListRowMetrics.avatarSize, height: ChatListRowMetrics.avatarSize)
                .overlay {
                    Text(avatarInitials(for: chat.title))
                        .font(.system(size: 20, weight: .semibold, design: .rounded))
                        .foregroundStyle(.white)
                }

            // Online status indicator dot (14x14 with 2.5pt background border)
            if chat.kind == "direct" {
                Circle()
                    .fill(Color(red: 0.20, green: 0.78, blue: 0.35)) // #34C759
                    .frame(width: 14, height: 14)
                    .overlay {
                        Circle()
                            .stroke(systemBackgroundColor, lineWidth: 2.5)
                    }
                    .offset(x: 2, y: 2)
            }
        }
    }

    // 2. Outgoing Delivery Status Ticks
    private var deliveryStatusView: some View {
        Group {
            if !chat.lastMessage.isEmpty {
                // Outgoing delivery status representation
                HStack(spacing: -6) {
                    Image(systemName: "checkmark")
                        .font(.system(size: 11, weight: .bold))
                    Image(systemName: "checkmark")
                        .font(.system(size: 11, weight: .bold))
                }
                .foregroundStyle(Color(red: 0.0, green: 0.48, blue: 1.0)) // #007AFF
            }
        }
    }

    // 3. Real-Time Animated Typing Indicator
    private func typingIndicatorView(actor: String) -> some View {
        HStack(spacing: 4) {
            TypingDotsPulseView()
            Text("\(actor) is typing...")
                .font(.system(size: 15, weight: .regular))
                .foregroundStyle(Color.accentColor)
                .lineLimit(1)
        }
    }

    // 4. Unread Capsule Badge
    private var unreadBadgeView: some View {
        Group {
            if chat.unreadCount > 0 {
                Text(chat.unreadCount > 999 ? "999+" : "\(chat.unreadCount)")
                    .font(.system(size: 12, weight: .bold, design: .rounded))
                    .foregroundStyle(.white)
                    .padding(.horizontal, max(5, chat.unreadCount >= 10 ? 6 : 4))
                    .frame(minWidth: 20, minHeight: 20)
                    .background(
                        Capsule()
                            .fill(chat.isMuted ? systemGrayColor : systemRedColor)
                    )
            }
        }
    }

    // MARK: - Dynamic Colors
    private var systemRedColor: Color {
        #if canImport(UIKit)
        return Color(uiColor: .systemRed)
        #else
        return Color(red: 1.0, green: 0.231, blue: 0.188)
        #endif
    }

    private var systemOrangeColor: Color {
        #if canImport(UIKit)
        return Color(uiColor: .systemOrange)
        #else
        return Color(red: 1.0, green: 0.584, blue: 0.0)
        #endif
    }

    private var systemGrayColor: Color {
        #if canImport(UIKit)
        return Color(uiColor: .systemGray)
        #else
        return Color(red: 0.557, green: 0.557, blue: 0.576)
        #endif
    }

    private var tertiaryLabelColor: Color {
        #if canImport(UIKit)
        return Color(uiColor: .tertiaryLabel)
        #else
        return Color.secondary.opacity(0.6)
        #endif
    }

    private var secondaryLabelColor: Color {
        #if canImport(UIKit)
        return Color(uiColor: .secondaryLabel)
        #else
        return Color.secondary
        #endif
    }

    private var systemBackgroundColor: Color {
        #if canImport(UIKit)
        return Color(uiColor: .systemBackground)
        #else
        return Color.black
        #endif
    }

    // MARK: - Helpers

    private func avatarInitials(for title: String) -> String {
        let clean = title.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !clean.isEmpty else { return "✦" }
        let components = clean.components(separatedBy: .whitespaces).filter { !$0.isEmpty }
        if components.count >= 2 {
            let first = String(components[0].prefix(1))
            let second = String(components[1].prefix(1))
            return "\(first)\(second)".uppercased()
        }
        return String(clean.prefix(1)).uppercased()
    }

    private func deterministicGradient(for id: String) -> LinearGradient {
        let palettes: [[Color]] = [
            [Color(hex: 0xFF512F), Color(hex: 0xDD2476)], // Red / Coral
            [Color(hex: 0xFF9966), Color(hex: 0xFF5E62)], // Orange / Peach
            [Color(hex: 0x8E2DE2), Color(hex: 0x4A00E0)], // Purple / Violet
            [Color(hex: 0x11998E), Color(hex: 0x38EF7D)], // Emerald / Green
            [Color(hex: 0x00C6FF), Color(hex: 0x0072FF)], // Sky / Azure
            [Color(hex: 0xF857A6), Color(hex: 0xFF5858)], // Pink / Rose
            [Color(hex: 0x4FACFE), Color(hex: 0x00F2FE)]  // Cyan / Deep Sky
        ]
        let index = abs(id.hashValue) % palettes.count
        let colors = palettes[index]
        return LinearGradient(
            colors: colors,
            startPoint: .topLeading,
            endPoint: .bottomTrailing
        )
    }
}

// MARK: - Animated Typing Waveform Dots
public struct TypingDotsPulseView: View {
    @State private var phase: CGFloat = 0.0

    public init() {}

    public var body: some View {
        HStack(spacing: 2.5) {
            ForEach(0..<3) { index in
                Circle()
                    .fill(Color.accentColor)
                    .frame(width: 4, height: 4)
                    .scaleEffect(scale(for: index))
                    .opacity(opacity(for: index))
            }
        }
        .onAppear {
            withAnimation(.easeInOut(duration: 0.8).repeatForever(autoreverses: true)) {
                phase = 1.0
            }
        }
    }

    private func scale(for index: Int) -> CGFloat {
        let progress = (phase + CGFloat(index) * 0.3).truncatingRemainder(dividingBy: 1.0)
        return 0.6 + 0.6 * sin(progress * .pi)
    }

    private func opacity(for index: Int) -> Double {
        let progress = (phase + CGFloat(index) * 0.3).truncatingRemainder(dividingBy: 1.0)
        return 0.4 + 0.6 * sin(progress * .pi)
    }
}

// MARK: - Peek Preview Card (Peek & Pop)
struct ChatRowPeekPreviewView: View {
    let chat: TelegramChatSummary

    init(chat: TelegramChatSummary) {
        self.chat = chat
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            // Header
            HStack(spacing: 12) {
                Circle()
                    .fill(Color.accentColor)
                    .frame(width: 40, height: 40)
                    .overlay {
                        Text(String(chat.title.prefix(1)).uppercased())
                            .font(.headline)
                            .foregroundStyle(.white)
                    }

                VStack(alignment: .leading, spacing: 2) {
                    Text(chat.title)
                        .font(.system(size: 16, weight: .semibold))
                        .foregroundStyle(.primary)

                    Text(chat.kind.capitalized)
                        .font(.system(size: 12, weight: .regular))
                        .foregroundStyle(.secondary)
                }
                Spacer()
            }
            .padding(14)
            .background(.ultraThinMaterial)

            Divider()

            // Preview message snippet stream
            VStack(alignment: .leading, spacing: 10) {
                HStack {
                    Text(chat.displayPreview)
                        .font(.system(size: 14))
                        .padding(10)
                        .background(secondarySystemBackgroundColor, in: RoundedRectangle(cornerRadius: 12))
                    Spacer()
                }
            }
            .padding(14)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(systemBackgroundColor)
        }
        .frame(width: 320, height: 180)
        .clipShape(RoundedRectangle(cornerRadius: 16))
    }

    private var secondarySystemBackgroundColor: Color {
        #if canImport(UIKit)
        return Color(uiColor: .secondarySystemBackground)
        #else
        return Color.gray.opacity(0.15)
        #endif
    }

    private var systemBackgroundColor: Color {
        #if canImport(UIKit)
        return Color(uiColor: .systemBackground)
        #else
        return Color.black
        #endif
    }
}

// Color hex helper
private extension Color {
    init(hex: UInt32, alpha: Double = 1.0) {
        let red = Double((hex >> 16) & 0xFF) / 255.0
        let green = Double((hex >> 8) & 0xFF) / 255.0
        let blue = Double(hex & 0xFF) / 255.0
        self.init(.sRGB, red: red, green: green, blue: blue, opacity: alpha)
    }
}
