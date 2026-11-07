//
//  MessageSwipeToReply.swift
//  Fabushi
//
//  Swipe-to-reply gesture modifier conforming to Telegram iOS physics.
//  Includes horizontal resistance curve capped at 60pt, circular reply badge morph,
//  50pt threshold medium haptic trigger, spring snapback, and vertical scroll dominance.
//

import SwiftUI
#if canImport(UIKit)
import UIKit
#endif

@MainActor
public final class SwipeToReplyCoordinator: ObservableObject {
    @Published public var dragOffset: CGFloat = 0.0
    @Published public var hasTriggeredHaptic: Bool = false
    @Published public var isVerticalDominant: Bool = false

    public init() {}
}

public struct MessageSwipeToReplyModifier: ViewModifier {
    public let message: TelegramStageMessage
    public let isEnabled: Bool
    public let onReply: (TelegramStageMessage) -> Void

    @StateObject private var coordinator = SwipeToReplyCoordinator()

    nonisolated public static let maxDisplacement: CGFloat = 60.0
    nonisolated public static let triggerThreshold: CGFloat = 50.0

    public init(
        message: TelegramStageMessage,
        isEnabled: Bool = true,
        onReply: @escaping (TelegramStageMessage) -> Void
    ) {
        self.message = message
        self.isEnabled = isEnabled
        self.onReply = onReply
    }

    /// Pure logarithmic resistance formula capped at 60pt
    nonisolated public static func computeDisplacement(dragDistance: CGFloat) -> CGFloat {
        guard dragDistance > 0 else { return 0 }
        let logValue = log1p(dragDistance / 25.0) * 35.0
        return min(maxDisplacement, logValue)
    }

    public func body(content: Content) -> some View {
        if !isEnabled {
            content
        } else {
            ZStack(alignment: .trailing) {
                // Circular reply badge appearing behind bubble
                let progress = min(1.0, max(0.0, coordinator.dragOffset / Self.triggerThreshold))
                let scale = 0.5 + 0.5 * progress

                Circle()
                    .fill(Color.accentColor)
                    .frame(width: 32, height: 32)
                    .overlay(
                        Image(systemName: "arrowshape.turn.up.left.fill")
                            .font(.system(size: 14, weight: .bold))
                            .foregroundStyle(Color.white)
                    )
                    .scaleEffect(scale)
                    .opacity(Double(progress))
                    .offset(x: -8 + (coordinator.dragOffset * 0.2))
                    .accessibilityIdentifier("swipe-to-reply-indicator")

                // Bubble row with horizontal drag offset
                content
                    .offset(x: -coordinator.dragOffset)
                    .gesture(
                        DragGesture(minimumDistance: 8, coordinateSpace: .local)
                            .onChanged { value in
                                let hDist = -value.translation.width // Dragging left produces positive hDist
                                let vDist = abs(value.translation.height)

                                // TC-T2-F15-02: Vertical scroll dominance
                                if vDist > abs(value.translation.width) && coordinator.dragOffset == 0 {
                                    coordinator.isVerticalDominant = true
                                    return
                                }
                                guard !coordinator.isVerticalDominant else { return }

                                if hDist > 0 {
                                    coordinator.dragOffset = Self.computeDisplacement(dragDistance: hDist)

                                    // TC-T1-F15-03: Trigger haptic once at threshold
                                    if coordinator.dragOffset >= Self.triggerThreshold && !coordinator.hasTriggeredHaptic {
                                        coordinator.hasTriggeredHaptic = true
                                        #if canImport(UIKit)
                                        let feedback = UIImpactFeedbackGenerator(style: .medium)
                                        feedback.prepare()
                                        feedback.impactOccurred()
                                        #endif
                                    } else if coordinator.dragOffset < Self.triggerThreshold {
                                        coordinator.hasTriggeredHaptic = false
                                    }
                                } else {
                                    coordinator.dragOffset = 0
                                }
                            }
                            .onEnded { _ in
                                guard !coordinator.isVerticalDominant else {
                                    coordinator.isVerticalDominant = false
                                    return
                                }

                                let shouldTrigger = coordinator.dragOffset >= Self.triggerThreshold
                                withAnimation(.spring(response: 0.32, dampingFraction: 0.68)) {
                                    coordinator.dragOffset = 0.0
                                }
                                coordinator.hasTriggeredHaptic = false

                                if shouldTrigger {
                                    onReply(message)
                                }
                            }
                    )
            }
        }
    }
}

public extension View {
    func messageSwipeToReply(
        message: TelegramStageMessage,
        isEnabled: Bool = true,
        onReply: @escaping (TelegramStageMessage) -> Void
    ) -> some View {
        modifier(MessageSwipeToReplyModifier(message: message, isEnabled: isEnabled, onReply: onReply))
    }
}
