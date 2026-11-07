//
//  MessageContextMenu.swift
//  Fabushi
//
//  Telegram message long-press context menu and floating reaction pill bar.
//  Implements 1.05x zoom, ultraThinMaterial backdrop, 5-emoji reaction pill bar
//  (👍, ❤️, 🔥, 😂, 👏), edge auto-flip, action sheet, and double-tap heart burst.
//

import SwiftUI
#if canImport(UIKit)
import UIKit
#endif

public enum MessageMenuAction: String, CaseIterable, Sendable {
    case reply = "Reply"
    case copy = "Copy"
    case forward = "Forward"
    case pin = "Pin"
    case delete = "Delete"

    public var systemImage: String {
        switch self {
        case .reply: return "arrowshape.turn.up.left"
        case .copy: return "doc.on.doc"
        case .forward: return "arrowshape.turn.up.right"
        case .pin: return "pin"
        case .delete: return "trash"
        }
    }
}

// MARK: - Floating 5-Emoji Reaction Pill Bar
public struct FloatingReactionPillBar: View {
    public static let defaultEmojis = ["👍", "❤️", "🔥", "😂", "👏"]

    public let activeEmojis: [String]
    public let onSelectEmoji: (String) -> Void

    public init(
        activeEmojis: [String] = [],
        onSelectEmoji: @escaping (String) -> Void
    ) {
        self.activeEmojis = activeEmojis
        self.onSelectEmoji = onSelectEmoji
    }

    public var body: some View {
        HStack(spacing: 8) {
            ForEach(Self.defaultEmojis, id: \.self) { emoji in
                Button(action: {
                    #if canImport(UIKit)
                    let haptic = UIImpactFeedbackGenerator(style: .light)
                    haptic.impactOccurred()
                    #endif
                    onSelectEmoji(emoji)
                }) {
                    Text(emoji)
                        .font(.system(size: 24))
                        .padding(6)
                        .background(
                            activeEmojis.contains(emoji)
                                ? Circle().fill(Color.accentColor.opacity(0.2))
                                : Circle().fill(Color.clear)
                        )
                }
                .buttonStyle(.plain)
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
        .background(.ultraThinMaterial)
        .clipShape(Capsule())
        .shadow(color: Color.black.opacity(0.18), radius: 8, x: 0, y: 4)
        .accessibilityIdentifier("floating-reaction-pill-bar")
    }
}

// MARK: - Heart Burst Animation Overlay
@MainActor
public final class HeartBurstState: ObservableObject {
    @Published public var scale: CGFloat = 0.2
    @Published public var opacity: Double = 1.0
    @Published public var yOffset: CGFloat = 0.0
    public init() {}
}

public struct HeartBurstAnimationView: View {
    public let onFinished: () -> Void

    @StateObject private var state = HeartBurstState()

    public init(onFinished: @escaping () -> Void) {
        self.onFinished = onFinished
    }

    public var body: some View {
        Text("❤️")
            .font(.system(size: 48))
            .scaleEffect(state.scale)
            .opacity(state.opacity)
            .offset(y: state.yOffset)
            .onAppear {
                withAnimation(.spring(response: 0.35, dampingFraction: 0.6)) {
                    state.scale = 1.2
                    state.yOffset = -24
                }
                withAnimation(.easeOut(duration: 0.4).delay(0.25)) {
                    state.opacity = 0.0
                    state.yOffset = -48
                }
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.65) {
                    onFinished()
                }
            }
            .accessibilityIdentifier("heart-burst-particle")
    }
}

// MARK: - Message Context Menu State
@MainActor
public final class MessageContextMenuState: ObservableObject {
    @Published public var isMenuPresented: Bool = false
    @Published public var showHeartBurst: Bool = false
    @Published public var lastDoubleTapTime: TimeInterval = 0.0
    @Published public var bubbleFrame: CGRect = .zero
    public init() {}
}

// MARK: - Message Context Menu Modifier
public struct MessageContextMenuModifier: ViewModifier {
    // Static Layout & Gesture Metrics
    public static let dragThreshold: CGFloat = 10.0
    public static let pressDuration: TimeInterval = 0.35
    public static let longPressDuration: TimeInterval = 0.35
    public static let hapticIntensity: CGFloat = 0.8
    public static let doubleTapSpeedThreshold: TimeInterval = 0.25
    public static let doubleTapDebounce: TimeInterval = 0.15
    public static let topEdgeFlipThreshold: CGFloat = 120.0

    public let message: TelegramStageMessage
    public let onSelectReaction: (String) -> Void
    public let onAction: (MessageMenuAction) -> Void

    @StateObject private var coordinator = MessageContextMenuState()

    public init(
        message: TelegramStageMessage,
        onSelectReaction: @escaping (String) -> Void,
        onAction: @escaping (MessageMenuAction) -> Void
    ) {
        self.message = message
        self.onSelectReaction = onSelectReaction
        self.onAction = onAction
    }

    public func body(content: Content) -> some View {
        ZStack {
            content
                .scaleEffect(coordinator.isMenuPresented ? 1.05 : 1.0)
                .animation(.spring(response: 0.3, dampingFraction: 0.75), value: coordinator.isMenuPresented)
                .overlay(
                    GeometryReader { geo in
                        Color.clear
                            .preference(key: BubbleFramePreferenceKey.self, value: geo.frame(in: .global))
                    }
                )
                .onPreferenceChange(BubbleFramePreferenceKey.self) { frame in
                    coordinator.bubbleFrame = frame
                }
                // Double tap quick reaction
                .onTapGesture(count: 2) {
                    handleDoubleTap()
                }
                // Long press to reveal custom context menu & floating reactions
                .onLongPressGesture(minimumDuration: Self.longPressDuration) {
                    #if canImport(UIKit)
                    let haptic = UIImpactFeedbackGenerator(style: .medium)
                    haptic.impactOccurred()
                    #endif
                    withAnimation(.spring(response: 0.3, dampingFraction: 0.75)) {
                        coordinator.isMenuPresented = true
                    }
                }

            if coordinator.showHeartBurst {
                HeartBurstAnimationView {
                    coordinator.showHeartBurst = false
                }
            }
        }
        #if os(iOS)
        .fullScreenCover(isPresented: Binding(
            get: { coordinator.isMenuPresented },
            set: { coordinator.isMenuPresented = $0 }
        )) {
            menuOverlay(content: content)
        }
        #else
        .sheet(isPresented: Binding(
            get: { coordinator.isMenuPresented },
            set: { coordinator.isMenuPresented = $0 }
        )) {
            menuOverlay(content: content)
        }
        #endif
    }

    @ViewBuilder
    private func menuOverlay(content: Content) -> some View {
        ZStack {
            // Dimmed and blurred backdrop
            Color.black.opacity(0.35)
                .background(.ultraThinMaterial)
                    .ignoresSafeArea()
                    .onTapGesture {
                        withAnimation { coordinator.isMenuPresented = false }
                    }

                VStack(spacing: 12) {
                    // TC-T2-F16-01: Auto-flip reaction bar if too close to screen top (< 120pt)
                    let flipBelow = coordinator.bubbleFrame.minY < Self.topEdgeFlipThreshold

                    if !flipBelow {
                        FloatingReactionPillBar(
                            activeEmojis: message.reactions.filter { $0.chosenByMe }.map { $0.emoji },
                            onSelectEmoji: { emoji in
                                onSelectReaction(emoji)
                                withAnimation { coordinator.isMenuPresented = false }
                            }
                        )
                    }

                    // Scaled message clone in menu overlay
                    content
                        .scaleEffect(1.05)
                        .padding(.horizontal, 16)

                    if flipBelow {
                        FloatingReactionPillBar(
                            activeEmojis: message.reactions.filter { $0.chosenByMe }.map { $0.emoji },
                            onSelectEmoji: { emoji in
                                onSelectReaction(emoji)
                                withAnimation { coordinator.isMenuPresented = false }
                            }
                        )
                    }

                    // Context Menu Action List
                    VStack(spacing: 0) {
                        ForEach(MessageMenuAction.allCases, id: \.self) { action in
                            Button(action: {
                                withAnimation { coordinator.isMenuPresented = false }
                                onAction(action)
                            }) {
                                HStack {
                                    Text(action.rawValue)
                                        .font(.system(size: 16))
                                        .foregroundStyle(action == .delete ? Color.red : Color.primary)
                                    Spacer()
                                    Image(systemName: action.systemImage)
                                        .font(.system(size: 16))
                                        .foregroundStyle(action == .delete ? Color.red : Color.secondary)
                                }
                                .padding(.horizontal, 16)
                                .padding(.vertical, 12)
                            }
                            .buttonStyle(.plain)

                            if action != MessageMenuAction.allCases.last {
                                Divider()
                            }
                        }
                    }
                    .background(.regularMaterial)
                    .clipShape(RoundedRectangle(cornerRadius: 14))
                    .padding(.horizontal, 32)
                    .shadow(color: Color.black.opacity(0.12), radius: 10, x: 0, y: 5)
                }
            }
            .transaction { $0.animation = nil }
        }

    private func handleDoubleTap() {
        let now = Date().timeIntervalSince1970
        // TC-T2-F16-03: Debounce rapid 100 taps (150ms debounce)
        guard now - coordinator.lastDoubleTapTime > 0.15 else { return }
        coordinator.lastDoubleTapTime = now

        #if canImport(UIKit)
        let feedback = UIImpactFeedbackGenerator(style: .medium)
        feedback.impactOccurred()
        #endif

        coordinator.showHeartBurst = true
        onSelectReaction("❤️")
    }
}

// Preference key for tracking bubble frame
private struct BubbleFramePreferenceKey: PreferenceKey {
    static let defaultValue: CGRect = .zero
    static func reduce(value: inout CGRect, nextValue: () -> CGRect) {
        value = nextValue()
    }
}

public extension View {
    func messageContextMenu(
        message: TelegramStageMessage,
        onSelectReaction: @escaping (String) -> Void,
        onAction: @escaping (MessageMenuAction) -> Void
    ) -> some View {
        modifier(MessageContextMenuModifier(message: message, onSelectReaction: onSelectReaction, onAction: onAction))
    }
}
