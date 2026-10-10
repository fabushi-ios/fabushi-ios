import SwiftUI
import UIKit

private enum MobileGhostPalette {
    static let colors: [Color] = [
        Color(red: 0.00, green: 0.76, blue: 0.45),
        Color(red: 0.08, green: 0.49, blue: 0.98),
        Color(red: 0.53, green: 0.30, blue: 1.00),
        Color(red: 1.00, green: 0.31, blue: 0.10),
        Color(red: 0.94, green: 0.12, blue: 0.28),
        Color(red: 0.98, green: 0.62, blue: 0.04),
    ]

    static func color(for identity: String) -> Color {
        let value = identity.unicodeScalars.reduce(UInt64(1469598103934665603)) { partial, scalar in
            (partial ^ UInt64(scalar.value)) &* 1099511628211
        }
        return colors[Int(value % UInt64(colors.count))]
    }
}

private struct ClothGhostShape: Shape {
    var phase: CGFloat

    var animatableData: CGFloat {
        get { phase }
        set { phase = newValue }
    }

    func path(in rect: CGRect) -> Path {
        var path = Path()
        let w = rect.width
        let h = rect.height
        let top = h * 0.08
        let shoulder = h * 0.26
        let hem = h * 0.80
        let wave = h * 0.055
        let drift = sin(phase) * w * 0.018

        path.move(to: CGPoint(x: w * 0.16 + drift, y: hem))
        path.addLine(to: CGPoint(x: w * 0.16, y: shoulder))
        path.addCurve(
            to: CGPoint(x: w * 0.50, y: top),
            control1: CGPoint(x: w * 0.17, y: h * 0.13),
            control2: CGPoint(x: w * 0.34, y: top)
        )
        path.addCurve(
            to: CGPoint(x: w * 0.84, y: shoulder),
            control1: CGPoint(x: w * 0.66, y: top),
            control2: CGPoint(x: w * 0.83, y: h * 0.13)
        )
        path.addLine(to: CGPoint(x: w * 0.84 + drift, y: hem))

        let segment = w * 0.68 / 3.0
        for index in stride(from: 3, through: 1, by: -1) {
            let right = w * 0.16 + segment * CGFloat(index)
            let left = right - segment
            let local = phase + CGFloat(index) * 0.9
            let crest = hem + sin(local) * wave
            path.addQuadCurve(
                to: CGPoint(x: left, y: hem + sin(local + 0.7) * wave),
                control: CGPoint(x: (left + right) * 0.5, y: crest + h * 0.14)
            )
        }
        path.closeSubpath()
        return path
    }
}

/// Lightweight native avatar inspired by the reference video's cloth/ghost silhouette.
/// It intentionally uses one animated vector path instead of a physics/3D engine so it
/// stays inexpensive in dense mobile lists while retaining the soft floating motion.
internal struct ClothGhostAvatar: View {
    let botId: String
    var size: CGFloat = 44
    var active = false
    var badge: Color? = nil

    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        TimelineView(.animation(minimumInterval: 1.0 / 30.0, paused: reduceMotion)) { timeline in
            let seconds = timeline.date.timeIntervalSinceReferenceDate
            let phase = reduceMotion ? 0.0 : seconds * (active ? 3.0 : 1.55)
            let sway = reduceMotion ? 0.0 : sin(phase * 0.72) * 1.7
            let lift = reduceMotion ? 0.0 : sin(phase) * 1.2
            let gazeX = reduceMotion ? 0.0 : sin(phase * 0.48) * size * 0.026
            let gazeY = reduceMotion ? 0.0 : cos(phase * 0.39) * size * 0.017
            let shape = ClothGhostShape(phase: CGFloat(phase))
            let base = MobileGhostPalette.color(for: botId)

            ZStack(alignment: .topTrailing) {
                ZStack {
                    shape
                        .fill(
                            LinearGradient(
                                colors: [base.opacity(0.98), base.opacity(0.86), base.opacity(0.98)],
                                startPoint: .topLeading,
                                endPoint: .bottomTrailing
                            )
                        )
                    shape
                        .fill(
                            LinearGradient(
                                colors: [.white.opacity(0.24), .clear, .black.opacity(0.10)],
                                startPoint: .topLeading,
                                endPoint: .bottomTrailing
                            )
                        )
                        .blendMode(.softLight)
                    shape.stroke(.white.opacity(0.18), lineWidth: max(0.5, size * 0.012))

                    HStack(spacing: size * 0.115) {
                        Capsule().fill(.white).frame(width: size * 0.105, height: size * 0.23)
                        Capsule().fill(.white).frame(width: size * 0.105, height: size * 0.23)
                    }
                    .offset(x: gazeX, y: -size * 0.035 + gazeY)
                }
                .rotationEffect(.degrees(sway))
                .offset(y: lift)
                .scaleEffect(active && !reduceMotion ? 1.015 + sin(phase * 1.2) * 0.008 : 1)

                if let badge {
                    Circle()
                        .fill(badge)
                        .overlay(Circle().stroke(.white, lineWidth: 2))
                        .frame(width: size * 0.23, height: size * 0.23)
                        .offset(x: size * 0.02, y: size * 0.02)
                }
            }
            .frame(width: size, height: size)
        }
        .frame(width: size, height: size)
        .accessibilityLabel("Bot 头像")
        .accessibilityIdentifier("cloth-ghost-avatar")
    }
}


internal enum MobileAgentAvatarKind: Equatable {
    case photo
    case sharedRoom
    case group
    case persona
}

internal func mobileAgentAvatarKind(_ bot: MobileBotSummary) -> MobileAgentAvatarKind {
    if let dataURL = bot.avatarDataURL,
       !dataURL.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
        return .photo
    }
    if bot.isSharedRoom { return .sharedRoom }
    if bot.isGroup { return .group }
    return .persona
}

private func mobilePersonaHash(_ value: String) -> UInt32 {
    var hash: UInt32 = 2_166_136_261
    for unit in value.utf16 {
        hash = (hash ^ UInt32(unit)) &* 16_777_619
    }
    return hash
}

private func mobilePersonaRandom(_ seed: UInt32) -> Double {
    var value = seed
    value = value &+ 1_831_565_813
    var next = (value ^ (value >> 15)) &* (1 | value)
    next = (next &+ ((next ^ (next >> 7)) &* (61 | next))) ^ next
    next = next ^ (next >> 14)
    return Double(next) / 4_294_967_296.0
}

internal func mobileResolvePersonaColor(agentId: String, override: String?) -> String {
    let explicit = override?.trimmingCharacters(in: .whitespacesAndNewlines)
    if let explicit,
       AvatarImagePolicy.colors.contains(where: { $0.id == explicit }) {
        return explicit
    }
    let values = MobileOnboardingCharacterCatalog.colorIds
    let constant: UInt32 = 2_654_435_769
    let seed = mobilePersonaHash(agentId) ^ (1 &* constant)
    let index = Int(mobilePersonaRandom(seed ^ constant) * Double(values.count))
    return values.indices.contains(index) ? values[index] : "gray"
}

internal func mobileResolvePersonaShape(agentId: String, override: String?) -> String {
    let explicit = override?.trimmingCharacters(in: .whitespacesAndNewlines)
    if let explicit, MobileOnboardingCharacterCatalog.shapeIds.contains(explicit) {
        return explicit
    }
    var hash = mobilePersonaHash(agentId)
    hash = (hash ^ (hash >> 16)) &* 73_244_475
    hash = (hash ^ (hash >> 13)) &* 3_266_489_909
    hash = hash ^ (hash >> 16)
    let shapes = MobileOnboardingCharacterCatalog.shapeIds
    return shapes[Int(hash % UInt32(shapes.count))]
}

private func mobileAgentAvatarImage(_ dataURL: String?) -> UIImage? {
    guard let dataURL,
          let data = AvatarImagePolicy.data(fromImageDataURL: dataURL)
    else { return nil }
    return UIImage(data: data)
}

private func mobileCharacterState(_ state: MobileAgentAvatarState) -> MobileOnboardingCharacterState {
    switch state {
    case .idle: return .idle
    case .thinking: return .thinking
    case .searching: return .searching
    case .working: return .working
    case .loading: return .loading
    case .sending: return .sending
    case .orbit: return .orbit
    }
}

internal struct MobileAgentAvatar: View {
    let bot: MobileBotSummary
    var size: CGFloat = 44
    var activeOverride = false

    private var effectiveState: MobileAgentAvatarState {
        activeOverride && bot.avatarState == .idle ? .working : bot.avatarState
    }

    var body: some View {
        Group {
            if let image = mobileAgentAvatarImage(bot.avatarDataURL) {
                Image(uiImage: image)
                    .resizable()
                    .scaledToFill()
                    .frame(width: size, height: size)
                    .clipShape(Circle())
            } else if bot.isSharedRoom {
                ZStack {
                    Circle().fill(Color.accentColor.opacity(0.14))
                    Image(systemName: "globe.americas.fill")
                        .font(.system(size: size * 0.48, weight: .semibold))
                        .foregroundStyle(Color.accentColor)
                }
                .frame(width: size, height: size)
            } else if bot.isGroup {
                groupAvatar
            } else {
                personaAvatar(
                    agentId: bot.id,
                    shape: bot.avatarShape,
                    color: bot.avatarColor,
                    state: effectiveState,
                    size: size
                )
            }
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }

    @ViewBuilder
    private var groupAvatar: some View {
        let members = Array(bot.memberIds.prefix(bot.memberIds.count > 4 ? 3 : 4))
        let memberSize = size * 0.56
        ZStack {
            ForEach(Array(members.enumerated()), id: .offset) { index, memberId in
                personaAvatar(
                    agentId: memberId,
                    shape: nil,
                    color: nil,
                    state: .idle,
                    size: memberSize
                )
                .offset(
                    x: index % 2 == 0 ? -size * 0.18 : size * 0.18,
                    y: index < 2 ? -size * 0.18 : size * 0.18
                )
            }
            if bot.memberIds.count > 4 {
                Text("+\(bot.memberIds.count - 3)")
                    .font(.system(size: max(8, size * 0.19), weight: .bold))
                    .foregroundStyle(.primary)
                    .frame(width: memberSize, height: memberSize)
                    .background(.regularMaterial, in: Circle())
                    .offset(x: size * 0.18, y: size * 0.18)
            }
        }
        .frame(width: size, height: size)
    }

    @ViewBuilder
    private func personaAvatar(
        agentId: String,
        shape: String?,
        color: String?,
        state: MobileAgentAvatarState,
        size: CGFloat
    ) -> some View {
        MobileOnboardingCharacter(
            colorId: mobileResolvePersonaColor(agentId: agentId, override: color),
            shapeId: mobileResolvePersonaShape(agentId: agentId, override: shape),
            size: size,
            state: mobileCharacterState(state)
        )
        .frame(width: size, height: size)
    }
}
