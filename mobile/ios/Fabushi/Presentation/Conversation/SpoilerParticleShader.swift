//
//  SpoilerParticleShader.swift
//  Fabushi
//
//  Continuous dynamic sparkle particle noise engine via SwiftUI Canvas.
//  Conceals spoiler text with animated shimmering particle noise
//  and provides tap-to-reveal dissolve animation with scroll persistence.
//

import SwiftUI

@MainActor
public final class SpoilerParticleState: ObservableObject {
    @Published public var dissolveProgress: CGFloat = 0.0
    public init() {}
}

public struct SpoilerParticleView: View {
    public let size: CGSize
    public let isRevealed: Bool
    public let onReveal: () -> Void

    @StateObject private var state = SpoilerParticleState()

    public init(
        size: CGSize,
        isRevealed: Bool,
        onReveal: @escaping () -> Void
    ) {
        self.size = size
        self.isRevealed = isRevealed
        self.onReveal = onReveal
    }

    public var body: some View {
        if isRevealed && state.dissolveProgress >= 1.0 {
            Color.clear.frame(width: size.width, height: size.height)
        } else {
            TimelineView(.animation) { timeline in
                let time = timeline.date.timeIntervalSinceReferenceDate
                Canvas { context, canvasSize in
                    let w = canvasSize.width
                    let h = canvasSize.height
                    let particleCount = max(20, Int(w * h / 120.0))

                    // Base background veil
                    let baseAlpha = (1.0 - state.dissolveProgress) * 0.95
                    context.fill(
                        Path(CGRect(origin: .zero, size: canvasSize)),
                        with: .color(Color.primary.opacity(baseAlpha * 0.15))
                    )

                    // Draw shimmering noise particles
                    for i in 0..<particleCount {
                        let seed = Double(i) * 137.5
                        let xNorm = (sin(seed * 0.1) * 0.5 + 0.5)
                        let yNorm = (cos(seed * 0.23) * 0.5 + 0.5)
                        let px = CGFloat(xNorm) * w
                        let py = CGFloat(yNorm) * h

                        // Oscillating alpha and size
                        let shimmer = sin(time * 3.5 + seed) * 0.5 + 0.5
                        let alpha = (1.0 - state.dissolveProgress) * (0.3 + 0.7 * shimmer)
                        let radius = CGFloat(1.0 + 1.8 * (sin(time * 2.0 + seed * 0.5) * 0.5 + 0.5))

                        let particleRect = CGRect(
                            x: px - radius,
                            y: py - radius,
                            width: radius * 2.0,
                            height: radius * 2.0
                        )

                        context.fill(
                            Path(ellipseIn: particleRect),
                            with: .color(Color.primary.opacity(alpha * 0.7))
                        )
                    }
                }
                .frame(width: size.width, height: size.height)
                .clipShape(RoundedRectangle(cornerRadius: 4))
                .contentShape(Rectangle())
                .onTapGesture {
                    guard !isRevealed else { return }
                    withAnimation(.easeOut(duration: 0.35)) {
                        state.dissolveProgress = 1.0
                    }
                    onReveal()
                }
                .accessibilityIdentifier("spoiler-particle-overlay")
            }
        }
    }
}

// MARK: - Spoiler Text Container Modifier
public struct SpoilerContainerModifier: ViewModifier {
    public let isRevealed: Bool
    public let onReveal: () -> Void

    public func body(content: Content) -> some View {
        content
            .opacity(isRevealed ? 1.0 : 0.05)
            .overlay(
                GeometryReader { geo in
                    SpoilerParticleView(
                        size: geo.size,
                        isRevealed: isRevealed,
                        onReveal: onReveal
                    )
                }
            )
    }
}

public extension View {
    func telegramSpoiler(isRevealed: Bool, onReveal: @escaping () -> Void) -> some View {
        modifier(SpoilerContainerModifier(isRevealed: isRevealed, onReveal: onReveal))
    }
}
