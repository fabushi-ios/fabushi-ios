import SwiftUI

enum MobileOnboardingCharacterState: Equatable, Sendable {
    case idle
    case happy
    case working
}

private struct MobileOnboardingCharacterShape: Shape {
    let shapeId: String
    var phase: CGFloat

    var animatableData: CGFloat {
        get { phase }
        set { phase = newValue }
    }

    func path(in rect: CGRect) -> Path {
        let w = rect.width
        let h = rect.height
        let drift = sin(phase) * w * 0.012
        var path = Path()

        switch shapeId {
        case "pebble":
            path.addRoundedRect(
                in: CGRect(x: w * 0.10 + drift, y: h * 0.13, width: w * 0.80, height: h * 0.74),
                cornerSize: CGSize(width: w * 0.38, height: h * 0.30)
            )
        case "squircle":
            path.addRoundedRect(
                in: CGRect(x: w * 0.08 + drift, y: h * 0.08, width: w * 0.84, height: h * 0.84),
                cornerSize: CGSize(width: w * 0.25, height: h * 0.25)
            )
        case "tablet":
            path.addRoundedRect(
                in: CGRect(x: w * 0.14 + drift, y: h * 0.07, width: w * 0.72, height: h * 0.86),
                cornerSize: CGSize(width: w * 0.30, height: h * 0.18)
            )
        case "wedge":
            path.move(to: CGPoint(x: w * 0.18 + drift, y: h * 0.18))
            path.addQuadCurve(
                to: CGPoint(x: w * 0.88 + drift, y: h * 0.50),
                control: CGPoint(x: w * 0.68 + drift, y: h * 0.02)
            )
            path.addQuadCurve(
                to: CGPoint(x: w * 0.18 + drift, y: h * 0.82),
                control: CGPoint(x: w * 0.68 + drift, y: h * 0.98)
            )
            path.addQuadCurve(
                to: CGPoint(x: w * 0.18 + drift, y: h * 0.18),
                control: CGPoint(x: w * 0.05 + drift, y: h * 0.50)
            )
        case "hex":
            let points = [
                CGPoint(x: w * 0.50 + drift, y: h * 0.05),
                CGPoint(x: w * 0.88 + drift, y: h * 0.27),
                CGPoint(x: w * 0.88 + drift, y: h * 0.73),
                CGPoint(x: w * 0.50 + drift, y: h * 0.95),
                CGPoint(x: w * 0.12 + drift, y: h * 0.73),
                CGPoint(x: w * 0.12 + drift, y: h * 0.27),
            ]
            path.move(to: points[0])
            for point in points.dropFirst() { path.addLine(to: point) }
            path.closeSubpath()
        case "cloud":
            path.move(to: CGPoint(x: w * 0.16 + drift, y: h * 0.66))
            path.addCurve(
                to: CGPoint(x: w * 0.34 + drift, y: h * 0.28),
                control1: CGPoint(x: w * 0.04 + drift, y: h * 0.52),
                control2: CGPoint(x: w * 0.16 + drift, y: h * 0.30)
            )
            path.addCurve(
                to: CGPoint(x: w * 0.68 + drift, y: h * 0.24),
                control1: CGPoint(x: w * 0.45 + drift, y: h * 0.05),
                control2: CGPoint(x: w * 0.63 + drift, y: h * 0.08)
            )
            path.addCurve(
                to: CGPoint(x: w * 0.88 + drift, y: h * 0.67),
                control1: CGPoint(x: w * 0.92 + drift, y: h * 0.24),
                control2: CGPoint(x: w * 0.98 + drift, y: h * 0.52)
            )
            path.addQuadCurve(
                to: CGPoint(x: w * 0.16 + drift, y: h * 0.66),
                control: CGPoint(x: w * 0.52 + drift, y: h * 1.00)
            )
        case "teardrop":
            path.move(to: CGPoint(x: w * 0.50 + drift, y: h * 0.04))
            path.addCurve(
                to: CGPoint(x: w * 0.50 + drift, y: h * 0.96),
                control1: CGPoint(x: w * 0.92 + drift, y: h * 0.42),
                control2: CGPoint(x: w * 0.92 + drift, y: h * 0.82)
            )
            path.addCurve(
                to: CGPoint(x: w * 0.50 + drift, y: h * 0.04),
                control1: CGPoint(x: w * 0.08 + drift, y: h * 0.82),
                control2: CGPoint(x: w * 0.08 + drift, y: h * 0.42)
            )
        default:
            path.move(to: CGPoint(x: w * 0.18 + drift, y: h * 0.20))
            path.addCurve(
                to: CGPoint(x: w * 0.82 + drift, y: h * 0.18),
                control1: CGPoint(x: w * 0.36 + drift, y: h * 0.02),
                control2: CGPoint(x: w * 0.67 + drift, y: h * 0.02)
            )
            path.addCurve(
                to: CGPoint(x: w * 0.86 + drift, y: h * 0.72),
                control1: CGPoint(x: w * 0.99 + drift, y: h * 0.33),
                control2: CGPoint(x: w * 0.98 + drift, y: h * 0.58)
            )
            path.addCurve(
                to: CGPoint(x: w * 0.18 + drift, y: h * 0.80),
                control1: CGPoint(x: w * 0.67 + drift, y: h * 0.98),
                control2: CGPoint(x: w * 0.34 + drift, y: h * 0.99)
            )
            path.addCurve(
                to: CGPoint(x: w * 0.18 + drift, y: h * 0.20),
                control1: CGPoint(x: w * 0.02 + drift, y: h * 0.64),
                control2: CGPoint(x: w * 0.01 + drift, y: h * 0.36)
            )
        }
        return path
    }
}

struct MobileOnboardingCharacter: View {
    let colorId: String
    let shapeId: String
    var size: CGFloat = 80
    var state: MobileOnboardingCharacterState = .idle

    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private var color: Color {
        let hex = MobileOnboardingCharacterCatalog.colors
            .first(where: { $0.id == colorId })?.value ?? "#4786f5"
        let cleaned = hex.trimmingCharacters(in: CharacterSet(charactersIn: "#"))
        var parsed: UInt64 = 0
        Scanner(string: cleaned).scanHexInt64(&parsed)
        return Color(
            red: Double((parsed >> 16) & 0xff) / 255,
            green: Double((parsed >> 8) & 0xff) / 255,
            blue: Double(parsed & 0xff) / 255
        )
    }

    var body: some View {
        TimelineView(.animation(minimumInterval: 1.0 / 30.0, paused: reduceMotion)) { timeline in
            let seconds = timeline.date.timeIntervalSinceReferenceDate
            let speed = state == .working ? 3.2 : state == .happy ? 2.4 : 1.4
            let phase = reduceMotion ? 0 : seconds * speed
            let lift: CGFloat = reduceMotion ? 0 : CGFloat(sin(phase)) * (state == .happy ? 2.8 : 1.6)
            let scale: CGFloat = reduceMotion ? 1 : 1 + CGFloat(sin(phase * 0.72)) * (state == .happy ? 0.035 : 0.018)
            let gazeX: CGFloat = reduceMotion ? 0 : CGFloat(sin(phase * 0.55)) * size * 0.025
            let gazeY: CGFloat = reduceMotion ? 0 : CGFloat(cos(phase * 0.42)) * size * 0.018
            let shape = MobileOnboardingCharacterShape(
                shapeId: MobileOnboardingCharacterCatalog.shapeIds.contains(shapeId) ? shapeId : "blob",
                phase: CGFloat(phase)
            )

            ZStack {
                shape
                    .fill(
                        LinearGradient(
                            colors: [color.opacity(0.98), color.opacity(0.82), color.opacity(0.98)],
                            startPoint: .topLeading,
                            endPoint: .bottomTrailing
                        )
                    )
                shape
                    .fill(
                        LinearGradient(
                            colors: [.white.opacity(0.28), .clear, .black.opacity(0.10)],
                            startPoint: .topLeading,
                            endPoint: .bottomTrailing
                        )
                    )
                    .blendMode(.softLight)

                HStack(spacing: size * 0.13) {
                    Circle()
                        .fill(.black.opacity(0.72))
                        .frame(width: size * 0.08, height: size * 0.08)
                    Circle()
                        .fill(.black.opacity(0.72))
                        .frame(width: size * 0.08, height: size * 0.08)
                }
                .offset(x: gazeX, y: gazeY - size * 0.02)

                if state == .happy {
                    Capsule()
                        .fill(.black.opacity(0.64))
                        .frame(width: size * 0.18, height: size * 0.055)
                        .offset(y: size * 0.16)
                }
            }
            .frame(width: size, height: size)
            .scaleEffect(scale)
            .offset(y: lift)
            .accessibilityHidden(true)
        }
    }
}
