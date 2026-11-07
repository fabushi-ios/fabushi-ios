//
//  TalkModeOverlayView.swift
//  Fabushi
//
//  Feature 35: Full-Duplex Talk Mode Overlay (1:1 Telegram iOS Native).
//  Conversational dynamic energy orb, VAD RMS energy calculation,
//  sub-60ms speech barge-in interrupt, and 320-chunk audio buffer flush.
//

import SwiftUI

// MARK: - Session State & Audio Calculator

public enum TalkSessionState: String, Sendable, Equatable {
    case idle
    case listening
    case thinking
    case speaking
    case interrupted

    public var title: String {
        switch self {
        case .idle: return "Ready"
        case .listening: return "Listening..."
        case .thinking: return "Thinking..."
        case .speaking: return "Speaking..."
        case .interrupted: return "Interrupted"
        }
    }
}

public enum VadAudioCalculator {
    public static let defaultSpeechThreshold: Float = 300.0
    public static let consecutiveBargeInFramesRequired: Int = 3 // 3 * 20ms = 60ms

    /// Computes Root Mean Square (RMS) for a 20ms PCM-16 mono buffer (320 samples).
    public static func computeRms(samples: [Int16]) -> Float {
        guard !samples.isEmpty else { return 0.0 }
        var sumSquares: Double = 0.0
        for sample in samples {
            let val = Double(sample)
            sumSquares += val * val
        }
        let meanSquare = sumSquares / Double(samples.count)
        return Float(sqrt(meanSquare))
    }

    /// Computes dynamic energy orb radius scaling factor from RMS.
    public static func calculateOrbScale(rms: Float, baseRadius: CGFloat = 80.0) -> CGFloat {
        let normalized = min(1.0, max(0.0, rms / 500.0))
        return baseRadius * (1.0 + 0.35 * CGFloat(normalized))
    }
}

// MARK: - View Model

@MainActor
public final class TalkModeViewModel: ObservableObject {
    @Published public private(set) var sessionState: TalkSessionState = .idle
    @Published public private(set) var currentRms: Float = 0.0
    @Published public private(set) var normalizedEnergy: Float = 0.0
    @Published public private(set) var consecutiveSpeechFrames: Int = 0
    @Published public private(set) var queuedAudioBufferChunks: Int = 0
    @Published public private(set) var transcriptText: String = ""

    public var speechThreshold: Float = VadAudioCalculator.defaultSpeechThreshold
    public static let maxAudioBufferChunks: Int = 320 // 1 MiB boundary (320 * 3200 bytes)

    public init() {}

    public func startSession() {
        sessionState = .listening
        transcriptText = "Go ahead, I'm listening..."
    }

    public func setAgentSpeaking(withBufferedChunks chunks: Int = 320) {
        queuedAudioBufferChunks = min(Self.maxAudioBufferChunks, chunks)
        sessionState = .speaking
        transcriptText = "Assistant speaking..."
    }

    /// Feeds a 20ms PCM-16 audio frame (320 samples) into VAD engine.
    /// Returns true if barge-in speech interrupt was triggered.
    @discardableResult
    public func handlePcmFrame(samples: [Int16]) -> Bool {
        let rms = VadAudioCalculator.computeRms(samples: samples)
        self.currentRms = rms
        self.normalizedEnergy = min(1.0, rms / 600.0)

        if rms >= speechThreshold {
            consecutiveSpeechFrames += 1

            // Check sub-60ms barge-in threshold (3 frames * 20ms = 60ms)
            if consecutiveSpeechFrames >= VadAudioCalculator.consecutiveBargeInFramesRequired {
                if sessionState == .speaking || sessionState == .thinking {
                    // Barge-in interrupt triggered!
                    triggerBargeIn()
                    return true
                } else if sessionState != .listening {
                    sessionState = .listening
                }
            }
        } else {
            // Speech dropped below threshold
            consecutiveSpeechFrames = 0
            if sessionState == .interrupted {
                sessionState = .listening
            }
        }

        return false
    }

    /// Triggers immediate speech barge-in, flushing audio playback queue.
    public func triggerBargeIn() {
        _ = flushAudioBuffer()
        sessionState = .interrupted
        transcriptText = "Listening to you..."
    }

    /// Flushes all queued chunks (up to 320 chunks) and returns count of flushed chunks.
    public func flushAudioBuffer() -> Int {
        let flushed = queuedAudioBufferChunks
        queuedAudioBufferChunks = 0
        return flushed
    }

    public func endSession() {
        sessionState = .idle
        queuedAudioBufferChunks = 0
    }
}

// MARK: - Presentation View

public struct TalkModeOverlayView: View {
    @StateObject public var viewModel: TalkModeViewModel
    public var onDismiss: (@Sendable () -> Void)?

    public init(
        viewModel: TalkModeViewModel,
        onDismiss: (@Sendable () -> Void)? = nil
    ) {
        _viewModel = StateObject(wrappedValue: viewModel)
        self.onDismiss = onDismiss
    }

    public var body: some View {
        ZStack {
            // Semi-transparent backdrop
            Color.black.opacity(0.88)
                .ignoresSafeArea()

            VStack(spacing: 36) {
                // Header status
                HStack {
                    Text(viewModel.sessionState.title)
                        .font(.system(size: 16, weight: .semibold))
                        .foregroundStyle(orbGradientColors.first ?? .cyan)

                    Spacer()

                    Button {
                        viewModel.endSession()
                        onDismiss?()
                    } label: {
                        Image(systemName: "xmark.circle.fill")
                            .font(.system(size: 24))
                            .foregroundStyle(.white.opacity(0.6))
                    }
                }
                .padding(.horizontal, 24)
                .padding(.top, 24)

                Spacer()

                // Dynamic Energy Orb Stage
                ZStack {
                    let radius = VadAudioCalculator.calculateOrbScale(rms: viewModel.currentRms, baseRadius: 85)

                    // Outer ambient glow
                    Circle()
                        .fill(
                            RadialGradient(
                                colors: [orbGradientColors.first?.opacity(0.4) ?? .cyan.opacity(0.4), .clear],
                                center: .center,
                                startRadius: radius * 0.4,
                                endRadius: radius * 1.6
                            )
                        )
                        .frame(width: radius * 3.2, height: radius * 3.2)

                    // Main Energy Orb
                    Circle()
                        .fill(
                            LinearGradient(
                                colors: orbGradientColors,
                                startPoint: .topLeading,
                                endPoint: .bottomTrailing
                            )
                        )
                        .frame(width: radius * 2, height: radius * 2)
                        .shadow(color: orbGradientColors.first?.opacity(0.6) ?? .clear, radius: 24)
                        .animation(.spring(response: 0.15, dampingFraction: 0.7), value: viewModel.currentRms)
                }
                .frame(height: 280)

                Spacer()

                // Floating Transcript & Prompt
                VStack(spacing: 12) {
                    Text(viewModel.transcriptText)
                        .font(.system(size: 18, weight: .medium))
                        .foregroundStyle(.white)
                        .multilineTextAlignment(.center)
                        .padding(.horizontal, 24)

                    if viewModel.queuedAudioBufferChunks > 0 {
                        Text("Buffer: \(viewModel.queuedAudioBufferChunks) chunks queued")
                            .font(.system(size: 12, design: .monospaced))
                            .foregroundStyle(.white.opacity(0.5))
                    }
                }
                .padding(.bottom, 48)
            }
        }
    }

    private var orbGradientColors: [Color] {
        switch viewModel.sessionState {
        case .idle, .listening:
            return [Color.cyan, Color.blue]
        case .thinking:
            return [Color.purple, Color.indigo]
        case .speaking:
            return [Color.teal, Color.green]
        case .interrupted:
            return [Color.orange, Color.red]
        }
    }
}
