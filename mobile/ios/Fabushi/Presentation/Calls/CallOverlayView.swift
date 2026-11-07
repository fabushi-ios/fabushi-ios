//
//  CallOverlayView.swift
//  Fabushi
//
//  Feature 30: WebRTC Audio/Video Call Screen (1:1 Telegram iOS Native).
//  Fullscreen calling canvas, blurred avatar backdrop, decaying 3-ring concentric audio
//  pulse ripples, mute/speaker/flip/end call controls, and network reconnecting handling.
//

import SwiftUI
#if canImport(AVFoundation)
import AVFoundation
#endif

// MARK: - Call State

public enum CallState: Sendable, Equatable {
    case initiating
    case ringing
    case connecting
    case connected(duration: TimeInterval)
    case reconnecting
    case ended(reason: String)

    public var title: String {
        switch self {
        case .initiating: return "Calling..."
        case .ringing: return "Ringing..."
        case .connecting: return "Connecting..."
        case .connected(let duration):
            let mins = Int(duration) / 60
            let secs = Int(duration) % 60
            return String(format: "%02d:%02d", mins, secs)
        case .reconnecting: return "Reconnecting..."
        case .ended(let reason): return "Call Ended (\(reason))"
        }
    }

    public var isLive: Bool {
        switch self {
        case .initiating, .ringing, .connecting, .connected, .reconnecting:
            return true
        case .ended:
            return false
        }
    }
}

// MARK: - Audio Pulse Ring Calculator

public enum CallAudioPulseCalculator {
    /// Computes 3 concentric ripple radii given normalized audio energy E in [0.0, 1.0].
    public static func calculateRingRadii(baseRadius: CGFloat = 60.0, energy: Float) -> [CGFloat] {
        let clampedE = max(0.0, min(1.0, CGFloat(energy)))
        return [
            baseRadius * (1.0 + clampedE * 0.28),
            baseRadius * (1.0 + clampedE * 0.58),
            baseRadius * (1.0 + clampedE * 0.95)
        ]
    }

    public static func calculateRingOpacities(energy: Float) -> [Double] {
        let clampedE = Double(max(0.0, min(1.0, energy)))
        return [
            0.55 * clampedE,
            0.35 * clampedE,
            0.18 * clampedE
        ]
    }
}

// MARK: - View Model

@MainActor
public final class CallViewModel: ObservableObject {
    @Published public var state: CallState = .initiating
    @Published public var peerName: String
    @Published public var isVideo: Bool = false
    @Published public var audioEnergy: Float = 0.0 // 0.0 ... 1.0
    @Published public var isMuted: Bool = false
    @Published public var isSpeakerOn: Bool = false
    @Published public var isCameraFront: Bool = true
    @Published public var duration: TimeInterval = 0

    private var timerTask: Task<Void, Never>?

    public init(peerName: String = "Alice", isVideo: Bool = false) {
        self.peerName = peerName
        self.isVideo = isVideo
    }

    public func transitionToConnected() {
        state = .connected(duration: 0)
        startDurationTimer()
    }

    public func handlePacketLoss() {
        if case .connected = state {
            state = .reconnecting
        }
    }

    public func handlePacketRecovery() {
        if state == .reconnecting {
            state = .connected(duration: duration)
        }
    }

    public func toggleMute() {
        isMuted.toggle()
    }

    public func toggleSpeaker() {
        isSpeakerOn.toggle()
        #if os(iOS)
        try? AVAudioSession.sharedInstance().overrideOutputAudioPort(
            isSpeakerOn ? .speaker : .none
        )
        #endif
    }

    public func flipCamera() {
        isCameraFront.toggle()
    }

    public func endCall(reason: String = "User hangup") {
        timerTask?.cancel()
        timerTask = nil
        state = .ended(reason: reason)
    }

    public func injectAudioEnergy(_ energy: Float) {
        self.audioEnergy = max(0.0, min(1.0, energy))
    }

    private func startDurationTimer() {
        timerTask?.cancel()
        timerTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 1_000_000_000)
                guard let self = self else { break }
                if case .connected = self.state {
                    self.duration += 1
                    self.state = .connected(duration: self.duration)
                }
            }
        }
    }

    deinit {
        timerTask?.cancel()
    }
}

// MARK: - Presentation View

public struct CallOverlayView: View {
    @StateObject public var viewModel: CallViewModel
    public var onDismiss: (@Sendable () -> Void)?

    public init(
        viewModel: CallViewModel,
        onDismiss: (@Sendable () -> Void)? = nil
    ) {
        _viewModel = StateObject(wrappedValue: viewModel)
        self.onDismiss = onDismiss
    }

    public var body: some View {
        ZStack {
            // Blurred Backdrop
            LinearGradient(
                colors: [Color(hex: 0x1A212B), Color(hex: 0x0E141B)],
                startPoint: .top,
                endPoint: .bottom
            )
            .ignoresSafeArea()

            VStack(spacing: 32) {
                // Top Info Section
                VStack(spacing: 8) {
                    Text(viewModel.peerName)
                        .font(.system(size: 28, weight: .bold))
                        .foregroundStyle(.white)

                    HStack(spacing: 6) {
                        if viewModel.state == .reconnecting {
                            Circle()
                                .fill(Color.orange)
                                .frame(width: 8, height: 8)
                        }
                        Text(viewModel.state.title)
                            .font(.system(size: 16, weight: .medium))
                            .foregroundStyle(viewModel.state == .reconnecting ? .orange : .white.opacity(0.8))
                    }
                }
                .padding(.top, 40)

                Spacer()

                // Center Calling Stage with Audio Pulse Rings
                ZStack {
                    let radii = CallAudioPulseCalculator.calculateRingRadii(baseRadius: 70, energy: viewModel.audioEnergy)
                    let opacities = CallAudioPulseCalculator.calculateRingOpacities(energy: viewModel.audioEnergy)

                    // Outer rings
                    ForEach(0..<3, id: \.self) { i in
                        Circle()
                            .stroke(Color.cyan.opacity(opacities[i]), lineWidth: 2)
                            .frame(width: radii[i] * 2, height: radii[i] * 2)
                            .animation(.easeOut(duration: 0.15), value: viewModel.audioEnergy)
                    }

                    // Avatar Circle
                    Circle()
                        .fill(LinearGradient(colors: [Color.blue, Color.cyan], startPoint: .topLeading, endPoint: .bottomTrailing))
                        .frame(width: 140, height: 140)
                        .overlay(
                            Text(String(viewModel.peerName.prefix(1)))
                                .font(.system(size: 54, weight: .bold))
                                .foregroundStyle(.white)
                        )
                        .shadow(color: .cyan.opacity(Double(viewModel.audioEnergy) * 0.4), radius: 20)
                }
                .frame(height: 280)

                Spacer()

                // Bottom Call Controls Dock
                HStack(spacing: 28) {
                    // Mute
                    CallControlButton(
                        icon: viewModel.isMuted ? "mic.slash.fill" : "mic.fill",
                        title: viewModel.isMuted ? "Unmute" : "Mute",
                        isActive: viewModel.isMuted
                    ) {
                        viewModel.toggleMute()
                    }

                    // Speaker
                    CallControlButton(
                        icon: viewModel.isSpeakerOn ? "speaker.wave.3.fill" : "speaker.fill",
                        title: "Speaker",
                        isActive: viewModel.isSpeakerOn
                    ) {
                        viewModel.toggleSpeaker()
                    }

                    // Video / Camera Flip
                    if viewModel.isVideo {
                        CallControlButton(
                            icon: "camera.rotate.fill",
                            title: "Flip",
                            isActive: false
                        ) {
                            viewModel.flipCamera()
                        }
                    }

                    // End Call
                    CallControlButton(
                        icon: "phone.down.fill",
                        title: "End",
                        backgroundColor: .red,
                        foregroundColor: .white
                    ) {
                        viewModel.endCall()
                        onDismiss?()
                    }
                }
                .padding(.horizontal, 24)
                .padding(.bottom, 48)
            }
        }
    }
}

// MARK: - Call Control Button
private struct CallControlButton: View {
    let icon: String
    let title: String
    var isActive: Bool = false
    var backgroundColor: Color? = nil
    var foregroundColor: Color? = nil
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            VStack(spacing: 8) {
                Circle()
                    .fill(backgroundColor ?? (isActive ? Color.white : Color.white.opacity(0.18)))
                    .frame(width: 64, height: 64)
                    .overlay(
                        Image(systemName: icon)
                            .font(.system(size: 24, weight: .semibold))
                            .foregroundStyle(foregroundColor ?? (isActive ? Color.black : Color.white))
                    )

                Text(title)
                    .font(.system(size: 12, weight: .medium))
                    .foregroundStyle(.white.opacity(0.8))
            }
        }
        .buttonStyle(.plain)
    }
}

private extension Color {
    init(hex: UInt32, alpha: Double = 1.0) {
        let red = Double((hex >> 16) & 0xFF) / 255.0
        let green = Double((hex >> 8) & 0xFF) / 255.0
        let blue = Double(hex & 0xFF) / 255.0
        self.init(.sRGB, red: red, green: green, blue: blue, opacity: alpha)
    }
}
