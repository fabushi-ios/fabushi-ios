//
//  BubbleTimestampCollisionLayout.swift
//  Fabushi
//
//  CoreText typographic measurement for detecting and preventing collision
//  between message body text and inline trailing timestamps.
//

import CoreGraphics
import CoreText
import Foundation
#if canImport(UIKit)
import UIKit
#endif

public struct TextTimestampMetrics: Sendable, Equatable {
    public let causesCollision: Bool
    public let requiredHeight: CGFloat
    public let contentWidth: CGFloat
    public let lastLineWidth: CGFloat
    public let lineCount: Int

    public init(
        causesCollision: Bool,
        requiredHeight: CGFloat,
        contentWidth: CGFloat,
        lastLineWidth: CGFloat,
        lineCount: Int
    ) {
        self.causesCollision = causesCollision
        self.requiredHeight = requiredHeight
        self.contentWidth = contentWidth
        self.lastLineWidth = lastLineWidth
        self.lineCount = lineCount
    }

    /// Computes precise typographic metrics and detects timestamp collision.
    ///
    /// - Parameters:
    ///   - text: The message text to be measured.
    ///   - fontSize: Font point size (defaults to standard 16pt message bubble size).
    ///   - maxWidth: Available bubble inner width constraint.
    ///   - timestampWidth: Width of timestamp pill + delivery ticks (defaults to 46pt).
    ///   - timestampHeight: Height reserved for timestamp row (defaults to 18pt).
    public static func compute(
        text: String,
        fontSize: CGFloat = 16.0,
        maxWidth: CGFloat,
        timestampWidth: CGFloat = 46.0,
        timestampHeight: CGFloat = 18.0
    ) -> TextTimestampMetrics {
        let safeMaxWidth = max(44.0, maxWidth)
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)

        // Handle empty text safely without zero-dimension crashes (TC-T2-F12-01)
        if trimmed.isEmpty {
            let minWidth = max(44.0, timestampWidth + 12.0)
            return TextTimestampMetrics(
                causesCollision: false,
                requiredHeight: timestampHeight + 4.0,
                contentWidth: minWidth,
                lastLineWidth: 0,
                lineCount: 0
            )
        }

        // Setup CoreText typographic attributes
        let ctFont = CTFontCreateWithName("HelveticaNeue" as CFString, fontSize, nil)
        let paragraphStyle: CTParagraphStyle = {
            var lineBreak = CTLineBreakMode.byWordWrapping
            var alignment = CTTextAlignment.natural
            return withUnsafePointer(to: &lineBreak) { lbPtr in
                withUnsafePointer(to: &alignment) { alignPtr in
                    let settings = [
                        CTParagraphStyleSetting(spec: .lineBreakMode, valueSize: MemoryLayout<CTLineBreakMode>.size, value: lbPtr),
                        CTParagraphStyleSetting(spec: .alignment, valueSize: MemoryLayout<CTTextAlignment>.size, value: alignPtr)
                    ]
                    return CTParagraphStyleCreate(settings, settings.count)
                }
            }
        }()

        let attributes: [NSAttributedString.Key: Any] = [
            NSAttributedString.Key(kCTFontAttributeName as String): ctFont,
            NSAttributedString.Key(kCTParagraphStyleAttributeName as String): paragraphStyle
        ]

        let attrString = NSAttributedString(string: trimmed, attributes: attributes)
        let framesetter = CTFramesetterCreateWithAttributedString(attrString as CFAttributedString)

        // Create measurement path with finite width and unbounded height
        let path = CGPath(rect: CGRect(x: 0, y: 0, width: safeMaxWidth, height: CGFloat.greatestFiniteMagnitude), transform: nil)
        let frame = CTFramesetterCreateFrame(framesetter, CFRangeMake(0, 0), path, nil)

        guard let lines = CTFrameGetLines(frame) as? [CTLine], !lines.isEmpty else {
            return TextTimestampMetrics(
                causesCollision: false,
                requiredHeight: fontSize * 1.3 + timestampHeight,
                contentWidth: min(safeMaxWidth, timestampWidth + 16.0),
                lastLineWidth: 0,
                lineCount: 1
            )
        }

        let totalLines = lines.count
        var maxLineWidth: CGFloat = 0.0
        var totalAscent: CGFloat = 0.0
        var totalDescent: CGFloat = 0.0
        var totalLeading: CGFloat = 0.0

        for line in lines {
            var ascent: CGFloat = 0
            var descent: CGFloat = 0
            var leading: CGFloat = 0
            let lineWidth = CGFloat(CTLineGetTypographicBounds(line, &ascent, &descent, &leading))
            maxLineWidth = max(maxLineWidth, lineWidth)
            totalAscent += ascent
            totalDescent += descent
            totalLeading += leading
        }

        guard let lastLine = lines.last else {
            return TextTimestampMetrics(
                causesCollision: false,
                requiredHeight: 24.0,
                contentWidth: timestampWidth + 16.0,
                lastLineWidth: 0,
                lineCount: 1
            )
        }

        var lastAscent: CGFloat = 0
        var lastDescent: CGFloat = 0
        var lastLeading: CGFloat = 0
        let lastLineWidth = CGFloat(CTLineGetTypographicBounds(lastLine, &lastAscent, &lastDescent, &lastLeading))

        let lineHeight = fontSize * 1.3
        let textTotalHeight = CGFloat(totalLines) * lineHeight

        // Spacing between text and timestamp
        let horizontalPadding: CGFloat = 8.0

        if totalLines == 1 {
            // Single-line message: check if last line + timestamp + padding exceeds maxWidth
            let neededWidth = lastLineWidth + timestampWidth + horizontalPadding
            let collision = neededWidth > safeMaxWidth
            let finalWidth = collision ? safeMaxWidth : max(neededWidth, 44.0)
            let finalHeight = collision ? (textTotalHeight + timestampHeight) : max(textTotalHeight, 28.0)

            return TextTimestampMetrics(
                causesCollision: collision,
                requiredHeight: finalHeight,
                contentWidth: finalWidth,
                lastLineWidth: lastLineWidth,
                lineCount: 1
            )
        } else {
            // Multi-line message: check trailing space on the last line
            let trailingSpace = safeMaxWidth - lastLineWidth
            let collision = trailingSpace < (timestampWidth + horizontalPadding)
            let finalWidth = safeMaxWidth
            let finalHeight = collision ? (textTotalHeight + timestampHeight) : textTotalHeight

            return TextTimestampMetrics(
                causesCollision: collision,
                requiredHeight: finalHeight,
                contentWidth: finalWidth,
                lastLineWidth: lastLineWidth,
                lineCount: totalLines
            )
        }
    }
}
