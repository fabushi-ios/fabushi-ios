//
//  SecretChatOverlayView.swift
//  Fabushi
//
//  Visual themes, ephemeral countdown timer, screenshot observer,
//  and E2EE replay window for Telegram Secret Chats (Feature 19).
//

import SwiftUI
#if canImport(UIKit)
import UIKit
#endif

// MARK: - Secret Chat Visual Theme
public struct SecretChatVisualTheme: Sendable {
    public static let padlockIcon = "lock.fill"
    public static let accentGreen = Color(conversationHex: "#00C853")
    public static let outgoingBubbleLightGreen = Color(conversationHex: "#C8F7C5")
    public static let outgoingBubbleDarkGreen = Color(conversationHex: "#1E3F20")
}

@MainActor
public final class EphemeralCountdownController: ObservableObject {
    @Published public private(set) var progress: CGFloat = 1.0
    private var timerTask: Task<Void, Never>?

    public init() {}

    deinit {
        timerTask?.cancel()
    }

    public func start(expiresAtMs: Int64, ttlSeconds: Int, onExpire: @escaping () -> Void) {
        timerTask?.cancel()
        timerTask = Task { [weak self] in
            while !Task.isCancelled {
                let now = Int64(Date().timeIntervalSince1970 * 1000)
                let p = EphemeralTimerProgressBar.calculateProgress(nowMs: now, expiresAtMs: expiresAtMs, ttlSeconds: ttlSeconds)
                guard let self else { return }
                self.progress = p

                if p <= 0.0 {
                    onExpire()
                    break
                }
                try? await Task.sleep(nanoseconds: 100_000_000) // Update every 100ms
            }
        }
    }

    public func stop() {
        timerTask?.cancel()
    }
}

// MARK: - Ephemeral Countdown Progress Bar
public struct EphemeralTimerProgressBar: View {
    public let ttlSeconds: Int
    public let expiresAtMs: Int64
    public let onExpire: () -> Void

    @StateObject private var controller = EphemeralCountdownController()

    public init(
        ttlSeconds: Int,
        expiresAtMs: Int64,
        onExpire: @escaping () -> Void
    ) {
        self.ttlSeconds = ttlSeconds
        self.expiresAtMs = expiresAtMs
        self.onExpire = onExpire
    }

    nonisolated public static func calculateProgress(nowMs: Int64, expiresAtMs: Int64, ttlSeconds: Int) -> CGFloat {
        guard ttlSeconds > 0 else { return 1.0 } // TC-T2-F19-04: 0 means disabled
        let totalMs = CGFloat(ttlSeconds * 1000)
        let remainingMs = CGFloat(expiresAtMs - nowMs)
        let ratio = remainingMs / totalMs
        return max(0.0, min(1.0, ratio))
    }

    public var body: some View {
        if ttlSeconds <= 0 {
            EmptyView()
        } else {
            VStack(spacing: 2) {
                GeometryReader { geo in
                    ZStack(alignment: .leading) {
                        Capsule()
                            .fill(Color.primary.opacity(0.12))
                            .frame(height: 3)

                        Capsule()
                            .fill(SecretChatVisualTheme.accentGreen)
                            .frame(width: geo.size.width * controller.progress, height: 3)
                    }
                }
                .frame(height: 3)

                HStack {
                    Image(systemName: "flame.fill")
                        .font(.system(size: 9))
                        .foregroundStyle(SecretChatVisualTheme.accentGreen)
                    Text("\(ttlSeconds)s self-destruct")
                        .font(.system(size: 10, weight: .medium))
                        .foregroundStyle(Color.secondary)
                    Spacer()
                }
            }
            .padding(.top, 4)
            .onAppear {
                controller.start(expiresAtMs: expiresAtMs, ttlSeconds: ttlSeconds, onExpire: onExpire)
            }
            .onDisappear {
                controller.stop()
            }
            .accessibilityIdentifier("ephemeral-timer-progress-bar")
        }
    }
}

// MARK: - 128-Bit Sliding Replay Window
public struct SlidingReplayWindow: Sendable, Equatable {
    public static let windowBits: UInt64 = 128

    public private(set) var highest: UInt64
    // 128-bit bitmap represented by two UInt64 (high for bits 64..127, low for bits 0..63)
    public private(set) var bitmapLow: UInt64
    public private(set) var bitmapHigh: UInt64

    public init() {
        self.highest = 0
        self.bitmapLow = 0
        self.bitmapHigh = 0
    }

    /// Evaluates sequence counter and returns true if valid and not a duplicate.
    /// Returns false if duplicate replay or counter is too far behind the window.
    public mutating func checkAndUpdate(counter: UInt64) -> Bool {
        guard counter > 0 else { return false }

        if counter > highest {
            let shift = counter - highest
            if shift >= Self.windowBits {
                // Shifted beyond full window: reset bitmap to bit 0
                bitmapLow = 1
                bitmapHigh = 0
            } else if shift >= 64 {
                let highShift = shift - 64
                bitmapHigh = (bitmapLow << highShift)
                bitmapLow = 0
                bitmapLow |= (1 << 0)
            } else {
                bitmapHigh = (bitmapHigh << shift) | (bitmapLow >> (64 - shift))
                bitmapLow = (bitmapLow << shift) | 1
            }
            highest = counter
            return true
        }

        let distance = highest - counter
        if distance >= Self.windowBits {
            return false // Dropped outside the 128-bit window (TC-T2-F19-02)
        }

        if distance < 64 {
            let mask = UInt64(1) << distance
            if (bitmapLow & mask) != 0 {
                return false // Replay detected!
            }
            bitmapLow |= mask
            return true
        } else {
            let highDistance = distance - 64
            let mask = UInt64(1) << highDistance
            if (bitmapHigh & mask) != 0 {
                return false // Replay detected!
            }
            bitmapHigh |= mask
            return true
        }
    }
}

// MARK: - Screenshot Notification Observer
@MainActor
public final class ScreenshotDetector: ObservableObject {
    @Published public private(set) var screenshotTakenCount: Int = 0
    public var onScreenshotDetected: (() -> Void)?

    private var observer: NSObjectProtocol?

    public init(onScreenshotDetected: (() -> Void)? = nil) {
        self.onScreenshotDetected = onScreenshotDetected
        setupObserver()
    }

    isolated deinit {
        if let observer {
            NotificationCenter.default.removeObserver(observer)
        }
    }

    private func setupObserver() {
        #if canImport(UIKit)
        observer = NotificationCenter.default.addObserver(
            forName: UIApplication.userDidTakeScreenshotNotification,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            guard let self else { return }
            self.screenshotTakenCount += 1
            self.onScreenshotDetected?()
        }
        #endif
    }

    public func simulateScreenshotEvent() {
        screenshotTakenCount += 1
        onScreenshotDetected?()
    }
}
