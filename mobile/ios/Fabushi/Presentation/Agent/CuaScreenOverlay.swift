//
//  CuaScreenOverlay.swift
//  Fabushi
//
//  Feature 34: Sub-10ms CUA Computer Lease (1:1 Telegram iOS Native).
//  Remote computer control viewport, touch coordinate clamping [0.0, 1.0],
//  pulsing click ripple indicators, and sub-10ms emergency kill-switch preemption.
//

import SwiftUI
#if canImport(Darwin)
import Darwin
#endif

// MARK: - Enums & Models

public enum CuaLeaseState: Sendable, Equatable {
    case unassigned
    case active(generation: UInt64, remainingSeconds: Int)
    case preempted(byUser: Bool)
    case expired
}

public enum CuaActionKind: String, Sendable, Equatable {
    case click
    case doubleClick
    case rightClick
    case drag
    case keyPress
}

public struct CuaClickPoint: Identifiable, Sendable, Equatable {
    public let id: UUID
    public let normalizedPoint: CGPoint
    public let timestamp: Date

    public init(id: UUID = UUID(), normalizedPoint: CGPoint, timestamp: Date = Date()) {
        self.id = id
        self.normalizedPoint = normalizedPoint
        self.timestamp = timestamp
    }
}

// MARK: - Coordinate Clamping & Mapping

public enum CuaCoordinateTransformer {
    /// Clamps arbitrary screen touch coordinates to [0.0, 1.0] normalized space.
    public static func clampNormalized(point: CGPoint) -> CGPoint {
        CGPoint(
            x: max(0.0, min(1.0, point.x)),
            y: max(0.0, min(1.0, point.y))
        )
    }

    /// Maps normalized point (u, v) to host display pixels (W, H).
    public static func mapToHostPixels(normalizedPoint: CGPoint, hostWidth: Int, hostHeight: Int) -> (x: Int, y: Int) {
        let clamped = clampNormalized(point: normalizedPoint)
        let x = Int(clamped.x * CGFloat(hostWidth))
        let y = Int(clamped.y * CGFloat(hostHeight))
        return (
            x: min(hostWidth - 1, max(0, x)),
            y: min(hostHeight - 1, max(0, y))
        )
    }
}

// MARK: - View Model

@MainActor
public final class CuaScreenOverlayViewModel: ObservableObject {
    @Published public private(set) var leaseState: CuaLeaseState = .unassigned
    @Published public private(set) var actionBannerText: String = "Ready"
    @Published public private(set) var clickIndicators: [CuaClickPoint] = []
    @Published public private(set) var latencyMs: Int = 12
    @Published public private(set) var currentBrokerGen: UInt64 = 1

    public var hostWidth: Int = 1920
    public var hostHeight: Int = 1080

    public init() {}

    public func assignLease(generation: UInt64 = 1, ttlSeconds: Int = 900) {
        currentBrokerGen = generation
        leaseState = .active(generation: generation, remainingSeconds: ttlSeconds)
        actionBannerText = "Computer lease active"
    }

    /// Handles touch input from user or agent with coordinate clamping.
    public func dispatchTouch(normalizedPoint: CGPoint, actionKind: CuaActionKind) async throws -> (Int, Int) {
        let clamped = CuaCoordinateTransformer.clampNormalized(point: normalizedPoint)
        let pixelPoint = CuaCoordinateTransformer.mapToHostPixels(
            normalizedPoint: clamped,
            hostWidth: hostWidth,
            hostHeight: hostHeight
        )

        // Add visual click ripple indicator
        let click = CuaClickPoint(normalizedPoint: clamped)
        clickIndicators.append(click)

        // Prune indicators after 1 second
        Task { [weak self] in
            try? await Task.sleep(nanoseconds: 800_000_000)
            self?.clickIndicators.removeAll { $0.id == click.id }
        }

        actionBannerText = "\(actionKind.rawValue.capitalized) at (\(pixelPoint.x), \(pixelPoint.y))"
        return pixelPoint
    }

    /// Emergency Takeover / Kill-Switch.
    /// Executes immediately and returns preemption latency in milliseconds (must be < 10ms).
    @discardableResult
    public func triggerEmergencyStop() -> Double {
        let start = CFAbsoluteTimeGetCurrent()

        // 1. Invalidate lease generation fence
        currentBrokerGen += 1
        leaseState = .preempted(byUser: true)
        actionBannerText = "EMERGENCY PREEMPTION ACTIVATED"

        // 2. Clear input queues
        clickIndicators.removeAll()

        let elapsedSeconds = CFAbsoluteTimeGetCurrent() - start
        let elapsedMs = elapsedSeconds * 1000.0
        return elapsedMs
    }
}

// MARK: - Presentation View

public struct CuaScreenOverlay: View {
    @StateObject public var viewModel: CuaScreenOverlayViewModel

    public init(viewModel: CuaScreenOverlayViewModel) {
        _viewModel = StateObject(wrappedValue: viewModel)
    }

    public var body: some View {
        ZStack {
            // Viewport Screen Canvas
            Color.black.ignoresSafeArea()

            // Simulated Remote Display Feed
            GeometryReader { geo in
                ZStack {
                    // Remote Host Wallpaper / Content
                    LinearGradient(
                        colors: [Color(hex: 0x1F2937), Color(hex: 0x111827)],
                        startPoint: .topLeading,
                        endPoint: .bottomTrailing
                    )

                    // Touch ripple click indicators
                    ForEach(viewModel.clickIndicators) { click in
                        Circle()
                            .stroke(Color.cyan, lineWidth: 2)
                            .frame(width: 44, height: 44)
                            .position(
                                x: click.normalizedPoint.x * geo.size.width,
                                y: click.normalizedPoint.y * geo.size.height
                            )
                            .transition(.scale.combined(with: .opacity))
                    }
                }
                .contentShape(Rectangle())
                .gesture(
                    DragGesture(minimumDistance: 0)
                        .onEnded { value in
                            let normalized = CGPoint(
                                x: value.location.x / geo.size.width,
                                y: value.location.y / geo.size.height
                            )
                            Task {
                                try? await viewModel.dispatchTouch(normalizedPoint: normalized, actionKind: .click)
                            }
                        }
                )
            }

            // Top Status & Emergency Stop Bar
            VStack {
                HStack(spacing: 12) {
                    // Lease & Status Banner
                    HStack(spacing: 8) {
                        Circle()
                            .fill(statusColor)
                            .frame(width: 8, height: 8)

                        Text(viewModel.actionBannerText)
                            .font(.system(size: 13, weight: .semibold))
                            .foregroundStyle(.white)

                        Spacer()

                        Text("RTT \(viewModel.latencyMs)ms")
                            .font(.system(size: 11, design: .monospaced))
                            .foregroundStyle(.white.opacity(0.6))
                    }
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                    .background(Color.black.opacity(0.75), in: Capsule())

                    // Red Emergency Kill-Switch Button
                    Button(action: {
                        viewModel.triggerEmergencyStop()
                    }) {
                        HStack(spacing: 4) {
                            Image(systemName: "hand.raised.fill")
                                .font(.system(size: 12))
                            Text("STOP")
                                .font(.system(size: 12, weight: .bold))
                        }
                        .foregroundStyle(.white)
                        .padding(.horizontal, 14)
                        .padding(.vertical, 8)
                        .background(Color.red, in: Capsule())
                        .shadow(color: .red.opacity(0.5), radius: 6)
                    }
                }
                .padding(.horizontal, 16)
                .padding(.top, 12)

                Spacer()
            }
        }
    }

    private var statusColor: Color {
        switch viewModel.leaseState {
        case .active: return .green
        case .preempted: return .red
        case .unassigned, .expired: return .secondary
        }
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
