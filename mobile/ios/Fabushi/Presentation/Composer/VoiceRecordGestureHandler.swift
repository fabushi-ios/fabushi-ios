//
//  VoiceRecordGestureHandler.swift
//  Fabushi
//
//  Production gesture-driven voice recording state machine & interactive controls matching Telegram iOS.
//  Coordinates hold-to-record (>150ms delay), slide-left-to-cancel (<= -100pt),
//  slide-up-to-lock (<= -80pt), live waveform meters, and haptic feedback.
//

import SwiftUI
#if canImport(UIKit)
import UIKit
#endif

public enum VoiceRecordingState: Equatable, Sendable {
    case idle
    case holding(duration: TimeInterval, dragOffset: CGSize)
    case locked(duration: TimeInterval, isPaused: Bool)
    case cancelled
}

@MainActor
public final class VoiceRecordGestureHandler: ObservableObject {
    public static let cancelThresholdX: CGFloat = -100.0 // Drag left >= 100pt
    public static let lockThresholdY: CGFloat = -80.0    // Drag up >= 80pt
    public static let holdTriggerDelay: TimeInterval = 0.150 // 150ms
    public static let minimumSendDuration: TimeInterval = 0.5 // 500ms

    @Published public private(set) var state: VoiceRecordingState = .idle
    @Published public private(set) var currentOffset: CGSize = .zero
    @Published public private(set) var liveSamples: [Double] = []
    @Published public var hasMicrophonePermission: Bool = true
    @Published public var showPermissionAlert: Bool = false
    @Published public var showTapHint: Bool = false

    #if canImport(UIKit)
    private var impactLight: UIImpactFeedbackGenerator?
    private var impactMedium: UIImpactFeedbackGenerator?
    private var impactHeavy: UIImpactFeedbackGenerator?
    private var notificationFeedback: UINotificationFeedbackGenerator?
    #endif

    public init() {
        #if canImport(UIKit)
        self.impactLight = UIImpactFeedbackGenerator(style: .light)
        self.impactMedium = UIImpactFeedbackGenerator(style: .medium)
        self.impactHeavy = UIImpactFeedbackGenerator(style: .heavy)
        self.notificationFeedback = UINotificationFeedbackGenerator()
        #endif
    }

    public var isHolding: Bool {
        if case .holding = state { return true }
        return false
    }

    public var isLocked: Bool {
        if case .locked = state { return true }
        return false
    }

    public var isRecording: Bool {
        isHolding || isLocked
    }

    public func onTouchBegan() {
        #if canImport(UIKit)
        impactLight?.prepare()
        impactMedium?.prepare()
        impactHeavy?.prepare()
        #endif
    }

    public func onLongPressTriggered() {
        guard hasMicrophonePermission else {
            showPermissionAlert = true
            return
        }
        #if canImport(UIKit)
        impactLight?.impactOccurred()
        #endif
        state = .holding(duration: 0.0, dragOffset: .zero)
        currentOffset = .zero
        liveSamples.removeAll()
    }

    public func onDragChanged(translation: CGSize) {
        guard case .holding(let dur, _) = state else { return }
        currentOffset = translation

        // Slide-to-cancel check: drag left <= -100pt
        if translation.width <= Self.cancelThresholdX {
            triggerCancel()
            return
        }

        // Slide-to-lock check: drag up <= -80pt
        if translation.height <= Self.lockThresholdY {
            triggerLock(duration: dur)
            return
        }

        state = .holding(duration: dur, dragOffset: translation)
    }

    public func onTouchEnded(duration: TimeInterval, completion: (Bool) -> Void) {
        switch state {
        case .holding(let dur, _):
            let effectiveDuration = max(duration, dur)
            if effectiveDuration >= Self.minimumSendDuration {
                #if canImport(UIKit)
                impactMedium?.impactOccurred()
                #endif
                state = .idle
                currentOffset = .zero
                completion(true) // Should send
            } else {
                state = .idle
                currentOffset = .zero
                // Short tap (<150ms or <0.5s) shows hint
                showTapHint = true
                completion(false) // Too short, discard
            }
        case .locked:
            // Hands-free lock remains active until explicit stop/send/trash
            break
        case .cancelled:
            state = .idle
            currentOffset = .zero
            completion(false)
        case .idle:
            state = .idle
            currentOffset = .zero
            showTapHint = true
            completion(false)
        }
    }

    public func triggerCancel() {
        #if canImport(UIKit)
        impactHeavy?.impactOccurred()
        notificationFeedback?.notificationOccurred(.warning)
        #endif
        state = .cancelled
        currentOffset = .zero
        liveSamples.removeAll()

        Task { @MainActor in
            try? await Task.sleep(nanoseconds: 350_000_000)
            if self.state == .cancelled {
                self.state = .idle
            }
        }
    }

    public func triggerLock(duration: TimeInterval) {
        #if canImport(UIKit)
        impactMedium?.impactOccurred()
        #endif
        state = .locked(duration: duration, isPaused: false)
        currentOffset = .zero
    }

    public func togglePause() {
        if case .locked(let dur, let isPaused) = state {
            state = .locked(duration: dur, isPaused: !isPaused)
        }
    }

    public func appendLiveSample(_ sample: Double) {
        let clamped = min(1.0, max(0.0, sample))
        liveSamples.append(clamped)
        if liveSamples.count > 100 {
            liveSamples.removeFirst(liveSamples.count - 100)
        }
    }

    public func updateDuration(_ duration: TimeInterval) {
        switch state {
        case .holding(_, let offset):
            state = .holding(duration: duration, dragOffset: offset)
        case .locked(_, let isPaused):
            state = .locked(duration: duration, isPaused: isPaused)
        default:
            break
        }
    }

    public func reset() {
        state = .idle
        currentOffset = .zero
        liveSamples.removeAll()
        pressTimerTask?.cancel()
        pressTimerTask = nil
    }

    public var pressTimerTask: Task<Void, Never>? = nil
}

// MARK: - Voice Record Gesture Button View
public struct VoiceRecordGestureButton: View {
    @ObservedObject public var handler: VoiceRecordGestureHandler
    public let onSendVoice: () -> Void
    public let onCancelVoice: () -> Void

    public init(
        handler: VoiceRecordGestureHandler,
        onSendVoice: @escaping () -> Void,
        onCancelVoice: @escaping () -> Void
    ) {
        self.handler = handler
        self.onSendVoice = onSendVoice
        self.onCancelVoice = onCancelVoice
    }

    public var body: some View {
        ZStack {
            // Main Microphone Icon Button
            Image(systemName: handler.isRecording ? "mic.fill" : "mic.fill")
                .font(.system(size: 22, weight: .regular))
                .foregroundStyle(handler.isRecording ? Color.red : Color.secondary)
                .scaleEffect(handler.isRecording ? 1.25 : 1.0)
                .offset(handler.isHolding ? handler.currentOffset : .zero)
                .animation(.interactiveSpring(response: 0.25, dampingFraction: 0.8), value: handler.currentOffset)
        }
        .frame(width: 36, height: 36)
        .contentShape(Rectangle())
        .gesture(
            DragGesture(minimumDistance: 0)
                .onChanged { value in
                    if !handler.isRecording {
                        if handler.pressTimerTask == nil {
                            handler.onTouchBegan()
                            handler.pressTimerTask = Task { @MainActor in
                                try? await Task.sleep(nanoseconds: UInt64(VoiceRecordGestureHandler.holdTriggerDelay * 1_000_000_000))
                                if !Task.isCancelled {
                                    handler.onLongPressTriggered()
                                }
                            }
                        }
                    } else if handler.isHolding {
                        handler.onDragChanged(translation: value.translation)
                    }
                }
                .onEnded { value in
                    handler.pressTimerTask?.cancel()
                    handler.pressTimerTask = nil
                    let duration: TimeInterval
                    if case .holding(let dur, _) = handler.state {
                        duration = dur
                    } else {
                        duration = 0.0
                    }
                    handler.onTouchEnded(duration: duration) { shouldSend in
                        if shouldSend {
                            onSendVoice()
                        } else {
                            onCancelVoice()
                        }
                    }
                }
        )
    }
}
