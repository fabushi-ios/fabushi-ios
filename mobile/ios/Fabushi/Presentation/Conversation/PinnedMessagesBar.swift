//
//  PinnedMessagesBar.swift
//  Fabushi
//
//  Translucent pinned message header bar conforming to Telegram iOS 1:1 specs.
//  Includes 2.5pt vertical accent indicator, tap-to-scroll jump with 0.8s highlight flash,
//  carousel cycling, media snippet fallback, and full pinned sheet list.
//

import SwiftUI

public struct PinnedMessagesBar: View {
    public let pinnedMessages: [TelegramStageMessage]
    public let activeIndex: Int
    public let onTapBar: () -> Void
    public let onOpenAllSheet: () -> Void
    public let onUnpin: (Int64) -> Void

    public init(
        pinnedMessages: [TelegramStageMessage],
        activeIndex: Int = 0,
        onTapBar: @escaping () -> Void,
        onOpenAllSheet: @escaping () -> Void,
        onUnpin: @escaping (Int64) -> Void
    ) {
        self.pinnedMessages = pinnedMessages
        self.activeIndex = activeIndex
        self.onTapBar = onTapBar
        self.onOpenAllSheet = onOpenAllSheet
        self.onUnpin = onUnpin
    }

    public var currentMessage: TelegramStageMessage? {
        guard !pinnedMessages.isEmpty, activeIndex >= 0 && activeIndex < pinnedMessages.count else {
            return nil
        }
        return pinnedMessages[activeIndex]
    }

    /// Formats the snippet text with media fallback (TC-T2-F17-05)
    nonisolated public static func snippetText(for message: TelegramStageMessage) -> String {
        let trimmed = message.text.trimmingCharacters(in: .whitespacesAndNewlines)
        if !trimmed.isEmpty {
            return trimmed
        }
        if let mediaType = message.mediaType, !mediaType.isEmpty {
            return mediaType.capitalized
        }
        return "Photo"
    }

    public var body: some View {
        if let current = currentMessage {
            HStack(spacing: 10) {
                // 2.5pt vertical accent line
                RoundedRectangle(cornerRadius: 1.5)
                    .fill(Color.accentColor)
                    .frame(width: 2.5, height: 32)
                    .accessibilityIdentifier("pinned-bar-accent-line")

                // Snippet & Title
                VStack(alignment: .leading, spacing: 2) {
                    let headerTitle = pinnedMessages.count > 1
                        ? "Pinned Message #\(activeIndex + 1)"
                        : "Pinned Message"

                    Text(headerTitle)
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(Color.accentColor)

                    Text(Self.snippetText(for: current))
                        .font(.system(size: 13, weight: .regular))
                        .foregroundStyle(Color.primary)
                        .lineLimit(1)
                }
                .contentShape(Rectangle())
                .onTapGesture {
                    onTapBar()
                }

                Spacer(minLength: 4)

                // List icon button to view all pinned messages
                Button(action: onOpenAllSheet) {
                    Image(systemName: "pin.fill")
                        .font(.system(size: 14))
                        .foregroundStyle(Color.accentColor)
                        .padding(6)
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("pinned-bar-list-button")
            }
            .padding(.horizontal, 12)
            .frame(height: 46)
            .background(.ultraThinMaterial)
            .overlay(Divider(), alignment: .bottom)
            .contextMenu {
                Button(role: .destructive) {
                    onUnpin(current.id)
                } label: {
                    Label("Unpin", systemImage: "pin.slash")
                }
            }
            .transition(.move(edge: .top).combined(with: .opacity))
            .accessibilityIdentifier("pinned-messages-bar")
        }
    }
}

// MARK: - All Pinned Messages Sheet
public struct AllPinnedMessagesSheet: View {
    public let pinnedMessages: [TelegramStageMessage]
    public let onSelectMessage: (TelegramStageMessage) -> Void
    public let onUnpin: (Int64) -> Void

    @Environment(\.dismiss) private var dismiss

    public var body: some View {
        NavigationStack {
            List {
                ForEach(pinnedMessages) { message in
                    HStack(spacing: 12) {
                        Image(systemName: "pin.fill")
                            .font(.system(size: 14))
                            .foregroundStyle(Color.accentColor)

                        VStack(alignment: .leading, spacing: 3) {
                            Text(PinnedMessagesBar.snippetText(for: message))
                                .font(.system(size: 15))
                                .lineLimit(2)

                            let date = Date(timeIntervalSince1970: TimeInterval(message.timestamp) / 1000.0)
                            Text(date.formatted(date: .abbreviated, time: .shortened))
                                .font(.system(size: 12))
                                .foregroundStyle(Color.secondary)
                        }

                        Spacer()
                    }
                    .contentShape(Rectangle())
                    .onTapGesture {
                        onSelectMessage(message)
                    }
                    .swipeActions(edge: .trailing) {
                        Button(role: .destructive) {
                            onUnpin(message.id)
                        } label: {
                            Label("Unpin", systemImage: "pin.slash")
                        }
                    }
                }
            }
            .navigationTitle("Pinned Messages (\(pinnedMessages.count))")
            #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
            #endif
            .toolbar {
                #if os(iOS)
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Done") {
                        dismiss()
                    }
                }
                #else
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") {
                        dismiss()
                    }
                }
                #endif
            }
            .accessibilityIdentifier("all-pinned-messages-sheet")
        }
    }
}
