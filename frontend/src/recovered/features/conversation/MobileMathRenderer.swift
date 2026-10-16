import Foundation
import SwiftMath
import SwiftUI
import UIKit

internal enum MobileAssistantMathSegmentKind: Equatable {
    case text
    case math
}

internal struct MobileAssistantMathSegment: Identifiable, Equatable {
    let id: Int
    let kind: MobileAssistantMathSegmentKind
    let text: String
    let displayMode: Bool
}

internal func splitMobileAssistantMath(_ text: String) -> [MobileAssistantMathSegment] {
    guard !text.isEmpty else {
        return [.init(id: 0, kind: .text, text: "", displayMode: false)]
    }
    let pattern = #"(\\\(([\s\S]*?)\\\)|\\\[([\s\S]*?)\\\]|\$\$([\s\S]*?)\$\$)"#
    guard let regex = try? NSRegularExpression(pattern: pattern) else {
        return [.init(id: 0, kind: .text, text: text, displayMode: false)]
    }

    let nsRange = NSRange(text.startIndex..<text.endIndex, in: text)
    let matches = regex.matches(in: text, range: nsRange)
    guard !matches.isEmpty else {
        return [.init(id: 0, kind: .text, text: text, displayMode: false)]
    }

    var segments: [MobileAssistantMathSegment] = []
    var cursor = text.startIndex
    var nextID = 0
    for match in matches {
        guard let fullRange = Range(match.range(at: 1), in: text) else { continue }
        if cursor < fullRange.lowerBound {
            segments.append(.init(
                id: nextID,
                kind: .text,
                text: String(text[cursor..<fullRange.lowerBound]),
                displayMode: false
            ))
            nextID += 1
        }

        let groups = [2, 3, 4]
        var expression = ""
        var displayMode = false
        for group in groups where match.range(at: group).location != NSNotFound {
            if let range = Range(match.range(at: group), in: text) {
                expression = String(text[range])
                displayMode = group != 2
                break
            }
        }
        segments.append(.init(
            id: nextID,
            kind: .math,
            text: expression,
            displayMode: displayMode
        ))
        nextID += 1
        cursor = fullRange.upperBound
    }
    if cursor < text.endIndex {
        segments.append(.init(
            id: nextID,
            kind: .text,
            text: String(text[cursor...]),
            displayMode: false
        ))
    }
    return segments.isEmpty
        ? [.init(id: 0, kind: .text, text: text, displayMode: false)]
        : segments
}

internal func mobileMathExpressionError(_ expression: String) -> String? {
    var error: NSError?
    let list = MTMathListBuilder.build(fromString: expression, error: &error)
    guard list != nil, error == nil else {
        return error?.localizedDescription ?? "Invalid math expression"
    }
    return nil
}

private struct MobileNativeMathLabel: UIViewRepresentable {
    let expression: String
    let displayMode: Bool

    func makeUIView(context: Context) -> MTMathUILabel {
        let label = MTMathUILabel()
        label.displayErrorInline = false
        label.textAlignment = .left
        label.setContentHuggingPriority(.required, for: .vertical)
        label.setContentCompressionResistancePriority(.required, for: .vertical)
        label.setContentHuggingPriority(.defaultLow, for: .horizontal)
        return label
    }

    func updateUIView(_ label: MTMathUILabel, context: Context) {
        label.displayErrorInline = false
        label.labelMode = displayMode ? .display : .text
        label.textAlignment = .left
        label.fontSize = displayMode ? 18 : 16
        label.textColor = UIColor.label
        label.latex = expression
        label.accessibilityLabel = expression
        label.isAccessibilityElement = true
        label.invalidateIntrinsicContentSize()
    }
}

internal struct MobileMathExpressionView: View {
    let expression: String
    let displayMode: Bool

    var body: some View {
        if let error = mobileMathExpressionError(expression) {
            Text(expression)
                .font(.system(size: 15, design: .monospaced))
                .foregroundStyle(.red)
                .textSelection(.enabled)
                .accessibilityLabel("Math expression unavailable: \(expression). \(error)")
        } else {
            ScrollView(.horizontal, showsIndicators: false) {
                MobileNativeMathLabel(
                    expression: expression,
                    displayMode: displayMode
                )
                .fixedSize()
                .padding(.vertical, displayMode ? 3 : 0)
            }
            .accessibilityLabel("Math expression \(expression)")
        }
    }
}

private struct MobileAssistantMathFragmentView: View {
    let text: String
    let streaming: Bool

    private var segments: [MobileAssistantMathSegment] {
        splitMobileAssistantMath(text)
    }

    private var containsMath: Bool {
        segments.contains { $0.kind == .math }
    }

    var body: some View {
        if !containsMath {
            Text(text)
                .overlay(alignment: .trailing) {
                    if streaming {
                        Text("▌").foregroundStyle(.black.opacity(0.65))
                    }
                }
                .font(.system(size: 16))
        } else {
            VStack(alignment: .leading, spacing: 4) {
                ForEach(segments) { segment in
                    switch segment.kind {
                    case .text:
                        if !segment.text.isEmpty {
                            Text(segment.text)
                                .font(.system(size: 16))
                        }
                    case .math:
                        MobileMathExpressionView(
                            expression: segment.text,
                            displayMode: segment.displayMode
                        )
                    }
                }
                if streaming {
                    Text("▌")
                        .font(.system(size: 16))
                        .foregroundStyle(.black.opacity(0.65))
                }
            }
        }
    }
}

internal struct MobileAssistantMathTextView: View {
    let text: String
    let streaming: Bool
    var renderScopeID: String = ""

    private var richSegments: [MobileAssistantRichSegment] {
        splitMobileAssistantMermaid(text)
    }

    private var containsMermaid: Bool {
        richSegments.contains { $0.kind == .mermaid }
    }

    var body: some View {
        if !containsMermaid {
            MobileAssistantMathFragmentView(text: text, streaming: streaming)
        } else {
            VStack(alignment: .leading, spacing: 8) {
                ForEach(Array(richSegments.enumerated()), id: \.element.id) { index, segment in
                    switch segment.kind {
                    case .text:
                        if !segment.text.isEmpty {
                            MobileAssistantMathFragmentView(
                                text: segment.text,
                                streaming: streaming && index == richSegments.count - 1
                            )
                        }
                    case .mermaid:
                        MobileMermaidDiagramView(
                            source: segment.text,
                            renderScopeID: "\(renderScopeID)\u{0}mermaid:\(segment.id)"
                        )
                    }
                }
                if streaming, richSegments.last?.kind == .mermaid {
                    Text("▌")
                        .font(.system(size: 16))
                        .foregroundStyle(.black.opacity(0.65))
                }
            }
        }
    }
}
