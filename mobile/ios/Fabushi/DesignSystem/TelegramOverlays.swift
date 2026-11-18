import Foundation
import SwiftUI

// MARK: - SheetContainer (`40%` Dim Backdrop, `5pt` Drag Indicator, `24.88%` Top Inset)

public struct SheetContainer<SheetContent: View>: View {
    public static let backdropDimOpacity: Double = 0.40
    public static let attachmentTopInsetFraction: CGFloat = 0.2488

    @Binding public var isPresented: Bool
    public let topInsetFraction: CGFloat
    private let content: () -> SheetContent

    public init(
        isPresented: Binding<Bool>,
        topInsetFraction: CGFloat = SheetContainer.attachmentTopInsetFraction,
        @ViewBuilder content: @escaping () -> SheetContent
    ) {
        self._isPresented = isPresented
        self.topInsetFraction = topInsetFraction
        self.content = content
    }

    public var body: some View {
        if isPresented {
            GeometryReader { geo in
                ZStack(alignment: .bottom) {
                    Color.black.opacity(Self.backdropDimOpacity)
                        .ignoresSafeArea()
                        .onTapGesture {
                            withAnimation(.spring(response: 0.32, dampingFraction: 0.86)) {
                                isPresented = false
                            }
                        }

                    VStack(spacing: 0) {
                        Capsule()
                            .fill(Color.secondary.opacity(0.45))
                            .frame(width: 36, height: 5)
                            .padding(.vertical, 8)

                        content()
                    }
                    .frame(maxWidth: .infinity)
                    .frame(height: max(220, geo.size.height * (1.0 - topInsetFraction)))
                    .background(.regularMaterial)
                    .clipShape(RoundedRectangle(cornerRadius: 26, style: .continuous))
                    .accessibilityElement(children: .contain)
                    .accessibilityIdentifier("fabushi.overlay.sheet")
                }
            }
            .transition(.opacity.combined(with: .move(edge: .bottom)))
        }
    }
}

// MARK: - GlassAlertPresenter (`20%` Dim, `1.15 -> 1.0` `0.4s` Spring, Continuation-Safe Outside Dismiss)

public struct GlassAlertSpec: Sendable, Equatable, Identifiable {
    public let id: UUID
    public let title: String
    public let message: String
    public let primaryTitle: String
    public let cancelTitle: String?
    public let allowsOutsideDismissal: Bool

    public init(
        id: UUID = UUID(),
        title: String,
        message: String,
        primaryTitle: String = "OK",
        cancelTitle: String? = "Cancel",
        allowsOutsideDismissal: Bool = true
    ) {
        self.id = id
        self.title = title
        self.message = message
        self.primaryTitle = primaryTitle
        self.cancelTitle = cancelTitle
        self.allowsOutsideDismissal = allowsOutsideDismissal
    }
}

public enum GlassAlertDecision: String, Sendable, Equatable {
    case primary
    case cancel
    case dismissedOutside
}

public struct GlassAlertPresenter: View {
    public static let backdropDimOpacity: Double = 0.20
    public static let initialScale: CGFloat = 1.15

    public let alert: GlassAlertSpec?
    public let onDecision: @Sendable (GlassAlertDecision) -> Void

    @State private var appeared = false

    public init(
        alert: GlassAlertSpec?,
        onDecision: @escaping @Sendable (GlassAlertDecision) -> Void
    ) {
        self.alert = alert
        self.onDecision = onDecision
    }

    public var body: some View {
        if let alert {
            ZStack {
                Color.black.opacity(Self.backdropDimOpacity)
                    .ignoresSafeArea()
                    .onTapGesture {
                        if alert.allowsOutsideDismissal {
                            onDecision(.dismissedOutside)
                        }
                    }

                VStack(spacing: 14) {
                    Text(alert.title)
                        .font(.system(size: 17, weight: .semibold))
                        .multilineTextAlignment(.center)

                    Text(alert.message)
                        .font(.system(size: 13))
                        .foregroundStyle(.secondary)
                        .multilineTextAlignment(.center)

                    Divider()

                    HStack(spacing: 12) {
                        if let cancelTitle = alert.cancelTitle {
                            Button(cancelTitle) {
                                onDecision(.cancel)
                            }
                            .frame(maxWidth: .infinity)
                            .accessibilityIdentifier("fabushi.overlay.alert.cancelButton")
                        }

                        Button(alert.primaryTitle) {
                            onDecision(.primary)
                        }
                        .font(.system(size: 17, weight: .semibold))
                        .frame(maxWidth: .infinity)
                        .accessibilityIdentifier("fabushi.overlay.alert.primaryButton")
                    }
                }
                .padding(20)
                .frame(maxWidth: 290)
                .background(GlassBackgroundView(cornerRadius: 22))
                .scaleEffect(appeared ? 1.0 : Self.initialScale)
                .opacity(appeared ? 1.0 : 0.0)
                .accessibilityElement(children: .contain)
                .accessibilityIdentifier("fabushi.overlay.alert")
                .onAppear {
                    withAnimation(.spring(response: 0.40, dampingFraction: 0.82)) {
                        appeared = true
                    }
                }
                .onDisappear {
                    appeared = false
                }
            }
        }
    }
}

// MARK: - UndoToastBanner (`#474747` Dark Floating Toast, `5.0s` Radial Countdown + `"Undo"`)

public struct UndoToastBanner: View {
    public static let toastHex: UInt32 = 0x474747
    public static let defaultDurationSeconds: Double = 5.0

    public let message: String
    public let undoTitle: String
    public let remainingSeconds: Int
    public let totalSeconds: Double
    public let onUndo: () -> Void

    public init(
        message: String,
        undoTitle: String = "Undo",
        remainingSeconds: Int = 5,
        totalSeconds: Double = UndoToastBanner.defaultDurationSeconds,
        onUndo: @escaping () -> Void
    ) {
        self.message = message
        self.undoTitle = undoTitle
        self.remainingSeconds = max(0, remainingSeconds)
        self.totalSeconds = max(1.0, totalSeconds)
        self.onUndo = onUndo
    }

    public var progressFraction: CGFloat {
        CGFloat(min(1.0, max(0.0, Double(remainingSeconds) / totalSeconds)))
    }

    public var body: some View {
        HStack(spacing: 12) {
            ZStack {
                Circle()
                    .stroke(Color.white.opacity(0.25), lineWidth: 2)
                    .frame(width: 22, height: 22)

                Circle()
                    .trim(from: 0, to: progressFraction)
                    .stroke(Color.white, style: StrokeStyle(lineWidth: 2, lineCap: .round))
                    .rotationEffect(.degrees(-90))
                    .frame(width: 22, height: 22)

                Text("\(remainingSeconds)")
                    .font(.system(size: 11, weight: .semibold).monospacedDigit())
                    .foregroundStyle(.white)
            }
            .accessibilityIdentifier("fabushi.overlay.undoToast.timer")

            Text(message)
                .font(.system(size: 14))
                .foregroundStyle(.white)
                .lineLimit(2)

            Spacer(minLength: 8)

            Button(action: onUndo) {
                Text(undoTitle)
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundStyle(Color(tgHex: 0x5AC8FA))
            }
            .accessibilityIdentifier("fabushi.overlay.undoToast.undoButton")
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 12)
        .background(
            RoundedRectangle(cornerRadius: 14, style: .continuous)
                .fill(Color(tgHex: Self.toastHex, alpha: 0.96))
        )
        .padding(.horizontal, 12)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("fabushi.overlay.undoToast")
    }
}
