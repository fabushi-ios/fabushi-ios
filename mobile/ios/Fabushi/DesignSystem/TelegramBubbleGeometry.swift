import Foundation
import SwiftUI

/// Configurable Telegram bubble corner & tail geometry (`16pt/8pt/6pt` + Bubble-Free Page mode, `F46`).
public struct BubbleCornersConfig: Codable, Sendable, Equatable {
    public var mainRadius: CGFloat {
        didSet {
            let clamped = min(max(mainRadius, 4.0), 17.0)
            if mainRadius != clamped {
                mainRadius = clamped
            }
        }
    }
    public var auxiliaryRadius: CGFloat
    public var tailWidth: CGFloat
    public var hasTails: Bool
    public var isBubbleFreeMode: Bool

    public init(
        mainRadius: CGFloat = 16.0,
        auxiliaryRadius: CGFloat = 8.0,
        tailWidth: CGFloat = 6.0,
        hasTails: Bool = true,
        isBubbleFreeMode: Bool = false
    ) {
        self.mainRadius = min(max(mainRadius, 4.0), 17.0)
        self.auxiliaryRadius = auxiliaryRadius
        self.tailWidth = tailWidth
        self.hasTails = hasTails
        self.isBubbleFreeMode = isBubbleFreeMode
    }

    /// When `mainRadius < 10pt`, Telegram disables smaller merged inner corners and uses `mainRadius`.
    public var effectiveMergedRadius: CGFloat {
        mainRadius >= 10.0 ? auxiliaryRadius : mainRadius
    }

    public var mediaCornerRadius: CGFloat {
        max(2.0, mainRadius - 1.0)
    }

    public var mergedMediaCornerRadius: CGFloat {
        max(2.0, effectiveMergedRadius - 1.0)
    }

    public func makeCornerRadii(
        isOutgoing: Bool,
        mergeTop: Bool = false,
        mergeBottom: Bool = false,
        isLastInCluster: Bool = true
    ) -> BubbleCornerRadii {
        let merged = effectiveMergedRadius
        let showTail = hasTails && !isBubbleFreeMode && isLastInCluster && !mergeBottom
        if isOutgoing {
            return BubbleCornerRadii(
                topLeft: mainRadius,
                topRight: mergeTop ? merged : mainRadius,
                bottomLeft: mainRadius,
                bottomRight: mergeBottom ? merged : mainRadius,
                hasTail: showTail
            )
        } else {
            return BubbleCornerRadii(
                topLeft: mergeTop ? merged : mainRadius,
                topRight: mainRadius,
                bottomLeft: mergeBottom ? merged : mainRadius,
                bottomRight: mainRadius,
                hasTail: showTail
            )
        }
    }
}

public extension TelegramBubbleShape {
    init(
        isOutgoing: Bool,
        corners: BubbleCornersConfig,
        mergeTop: Bool = false,
        mergeBottom: Bool = false,
        isLastInCluster: Bool = true,
        isRTL: Bool = false
    ) {
        self.init(
            isOutgoing: isOutgoing,
            radii: corners.makeCornerRadii(
                isOutgoing: isOutgoing,
                mergeTop: mergeTop,
                mergeBottom: mergeBottom,
                isLastInCluster: isLastInCluster
            ),
            isRTL: isRTL
        )
    }
}

// MARK: - 1 / 2 / 3-Dash Reply & Quote Inline Stripe (`F24`, `F46`)

public enum MessageInlineStripeStyle: Int, Codable, Sendable, Equatable {
    case solid = 1
    case doubleDashed = 2
    case tripleDashed = 3
}

public struct MessageInlineStripeView: View {
    public let style: MessageInlineStripeStyle
    public let accentColor: Color
    public let isPageMode: Bool

    public init(
        style: MessageInlineStripeStyle = .solid,
        accentColor: Color = Color(tgHex: 0x007AFF),
        isPageMode: Bool = false
    ) {
        self.style = style
        self.accentColor = accentColor
        self.isPageMode = isPageMode
    }

    public var containerCornerRadius: CGFloat {
        isPageMode ? 10.0 : 4.0
    }

    public var dashCount: Int {
        style.rawValue
    }

    public var body: some View {
        Canvas { context, size in
            let width: CGFloat = 3.0
            let h = max(size.height, 12.0)
            switch style {
            case .solid:
                let rect = CGRect(x: 0, y: 0, width: width, height: h)
                context.fill(Path(roundedRect: rect, cornerRadius: 1.5), with: .color(accentColor))
            case .doubleDashed, .tripleDashed:
                let segments = CGFloat(style.rawValue)
                let gap: CGFloat = 3.0
                let segHeight = max(4.0, (h - gap * (segments - 1.0)) / segments)
                for idx in 0..<style.rawValue {
                    let y0 = CGFloat(idx) * (segHeight + gap)
                    var dashPath = Path()
                    dashPath.move(to: CGPoint(x: 0, y: y0 + 1.5))
                    dashPath.addLine(to: CGPoint(x: width, y: y0))
                    dashPath.addLine(to: CGPoint(x: width, y: y0 + segHeight - 1.5))
                    dashPath.addLine(to: CGPoint(x: 0, y: y0 + segHeight))
                    dashPath.closeSubpath()
                    context.fill(dashPath, with: .color(accentColor))
                }
            }
        }
        .frame(width: 3.0)
    }
}
