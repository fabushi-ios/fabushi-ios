//
//  TelegramBubbleShape.swift
//  Fabushi
//
//  Custom SwiftUI Shape rendering Telegram speech bubbles with cubic Bezier tails.
//  Tails are drawn only on bottom-most clustered messages (.single, .bottom),
//  curving 6pt outward toward user avatar.
//

import SwiftUI

public struct TelegramBubbleShape: Shape, Sendable {
    public let isOutgoing: Bool
    public let radii: BubbleCornerRadii
    public let isRTL: Bool

    public init(
        isOutgoing: Bool,
        radii: BubbleCornerRadii,
        isRTL: Bool = false
    ) {
        self.isOutgoing = isOutgoing
        self.radii = radii
        self.isRTL = isRTL
    }

    public func path(in rect: CGRect) -> Path {
        var path = Path()
        let w = rect.width
        let h = rect.height
        let tw: CGFloat = 6.0  // Tail width extending horizontally
        let th: CGFloat = 16.0 // Tail height extending vertically

        let effectiveOutgoing = isRTL ? !isOutgoing : isOutgoing

        if effectiveOutgoing {
            // Outgoing bubble (tail anchored at bottom-right)
            let tl = radii.topLeft
            let tr = radii.topRight
            let bl = radii.bottomLeft
            let br = radii.bottomRight

            // Start at top-left after corner
            path.move(to: CGPoint(x: tl, y: 0))

            // Top edge to top-right corner
            path.addLine(to: CGPoint(x: w - tr, y: 0))
            path.addArc(
                center: CGPoint(x: w - tr, y: tr),
                radius: tr,
                startAngle: .degrees(-90),
                endAngle: .degrees(0),
                clockwise: false
            )

            // Right edge down to tail or bottom-right corner
            if radii.hasTail {
                path.addLine(to: CGPoint(x: w - tw, y: max(tr, h - th)))
                // Outward cubic Bezier scoop to tail tip at (w, h)
                path.addCurve(
                    to: CGPoint(x: w, y: h),
                    control1: CGPoint(x: w - tw + 1.0, y: h - 6.0),
                    control2: CGPoint(x: w - 2.0, y: h)
                )
                // Inward sweep along bottom edge
                path.addCurve(
                    to: CGPoint(x: w - tw - 12.0, y: h),
                    control1: CGPoint(x: w - 4.0, y: h),
                    control2: CGPoint(x: w - tw, y: h)
                )
            } else {
                path.addLine(to: CGPoint(x: w, y: max(tr, h - br)))
                path.addArc(
                    center: CGPoint(x: w - br, y: h - br),
                    radius: br,
                    startAngle: .degrees(0),
                    endAngle: .degrees(90),
                    clockwise: false
                )
            }

            // Bottom edge to bottom-left corner
            path.addLine(to: CGPoint(x: bl, y: h))
            path.addArc(
                center: CGPoint(x: bl, y: h - bl),
                radius: bl,
                startAngle: .degrees(90),
                endAngle: .degrees(180),
                clockwise: false
            )

            // Left edge up to top-left corner
            path.addLine(to: CGPoint(x: 0, y: tl))
            path.addArc(
                center: CGPoint(x: tl, y: tl),
                radius: tl,
                startAngle: .degrees(180),
                endAngle: .degrees(270),
                clockwise: false
            )
        } else {
            // Incoming bubble (tail anchored at bottom-left)
            let tl = radii.topLeft
            let tr = radii.topRight
            let bl = radii.bottomLeft
            let br = radii.bottomRight
            let leftInset: CGFloat = radii.hasTail ? tw : 0.0

            // Start after top-left corner
            path.move(to: CGPoint(x: leftInset + tl, y: 0))

            // Top edge to top-right
            path.addLine(to: CGPoint(x: w - tr, y: 0))
            path.addArc(
                center: CGPoint(x: w - tr, y: tr),
                radius: tr,
                startAngle: .degrees(-90),
                endAngle: .degrees(0),
                clockwise: false
            )

            // Right edge to bottom-right
            path.addLine(to: CGPoint(x: w, y: max(tr, h - br)))
            path.addArc(
                center: CGPoint(x: w - br, y: h - br),
                radius: br,
                startAngle: .degrees(0),
                endAngle: .degrees(90),
                clockwise: false
            )

            // Bottom edge toward bottom-left tail or corner
            if radii.hasTail {
                path.addLine(to: CGPoint(x: tw + 12.0, y: h))
                // Outward cubic Bezier scoop to tail tip at (0, h)
                path.addCurve(
                    to: CGPoint(x: 0, y: h),
                    control1: CGPoint(x: tw, y: h),
                    control2: CGPoint(x: 4.0, y: h)
                )
                // Inward sweep upward along left side
                path.addCurve(
                    to: CGPoint(x: tw, y: max(tl, h - th)),
                    control1: CGPoint(x: 2.0, y: h),
                    control2: CGPoint(x: tw - 1.0, y: h - 6.0)
                )
            } else {
                path.addLine(to: CGPoint(x: bl, y: h))
                path.addArc(
                    center: CGPoint(x: bl, y: h - bl),
                    radius: bl,
                    startAngle: .degrees(90),
                    endAngle: .degrees(180),
                    clockwise: false
                )
            }

            // Left edge up to top-left
            path.addLine(to: CGPoint(x: leftInset, y: tl))
            path.addArc(
                center: CGPoint(x: leftInset + tl, y: tl),
                radius: tl,
                startAngle: .degrees(180),
                endAngle: .degrees(270),
                clockwise: false
            )
        }

        path.closeSubpath()
        return path
    }
}
