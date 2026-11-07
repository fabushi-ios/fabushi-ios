//
//  MessageFormattingView.swift
//  Fabushi
//
//  Rich text entity parser and renderer for Telegram messages.
//  Conforms to MTProto UTF-16 code unit offset/length indexing,
//  overlapping formatting traits, code block copy capsules, and link handlers.
//

import SwiftUI
#if canImport(UIKit)
import UIKit
#endif

// MARK: - Formatted Text Builder
public struct FormattedTextBuilder: Sendable {

    /// Constructs a styled SwiftUI AttributedString adhering to UTF-16 entity ranges.
    public static func buildAttributedString(
        text: String,
        entities: [MessageEntity],
        revealedSpoilers: Set<String> = []
    ) -> AttributedString {
        let utf16 = text.utf16
        let utf16Count = utf16.count
        guard utf16Count > 0 else {
            return AttributedString("")
        }

        var attributed = AttributedString(text)

        // Filter and sanitize entities
        for entity in entities {
            // TC-T2-F13-04: Ignore zero-length entities
            guard entity.length > 0 else { continue }

            // TC-T2-F13-02: Clamp entity offsets to valid UTF-16 range
            let clampedOffset = max(0, min(utf16Count, entity.offset))
            let clampedLength = max(0, min(utf16Count - clampedOffset, entity.length))
            guard clampedLength > 0 else { continue }

            // Convert UTF-16 code unit offset/length to Swift String.Index range safely
            guard let utf16Start = utf16.index(utf16.startIndex, offsetBy: clampedOffset, limitedBy: utf16.endIndex),
                  let utf16End = utf16.index(utf16Start, offsetBy: clampedLength, limitedBy: utf16.endIndex),
                  let strStart = String.Index(utf16Start, within: text),
                  let strEnd = String.Index(utf16End, within: text)
            else {
                continue
            }

            // Convert String.Index range to AttributedString range
            guard let attrStart = AttributedString.Index(strStart, within: attributed),
                  let attrEnd = AttributedString.Index(strEnd, within: attributed),
                  attrStart < attrEnd
            else {
                continue
            }

            let range = attrStart..<attrEnd

            // Apply style traits with support for overlapping ranges (TC-T2-F13-01)
            switch entity.type {
            case .bold:
                attributed[range].font = .system(size: 16, weight: .bold)

            case .italic:
                attributed[range].font = .system(size: 16).italic()

            case .underline:
                attributed[range].underlineStyle = .single

            case .strikethrough:
                attributed[range].strikethroughStyle = .single

            case .code:
                attributed[range].font = .system(size: 15, design: .monospaced)
                attributed[range].backgroundColor = Color.primary.opacity(0.08)

            case .pre:
                attributed[range].font = .system(size: 14, design: .monospaced)

            case .url, .textUrl:
                let targetUrlString = entity.url ?? String(text[strStart..<strEnd])
                if let url = URL(string: targetUrlString) {
                    attributed[range].link = url
                    attributed[range].foregroundColor = Color.accentColor
                    attributed[range].underlineStyle = .single
                }

            case .mention:
                let mentionText = String(text[strStart..<strEnd])
                if let url = URL(string: "tg-mention://\(mentionText)") {
                    attributed[range].link = url
                    attributed[range].foregroundColor = Color.accentColor
                }

            case .spoiler:
                let spoilerKey = entity.id
                if !revealedSpoilers.contains(spoilerKey) {
                    attributed[range].backgroundColor = Color.primary.opacity(0.18)
                    attributed[range].foregroundColor = Color.clear
                }
            }
        }

        return attributed
    }
}

// MARK: - Preformatted Code Block View
@MainActor
public final class PreCodeBlockState: ObservableObject {
    @Published public var copied: Bool = false
    public init() {}
}

public struct MessagePreCodeBlockView: View {
    public let code: String
    public let language: String?

    @StateObject private var state = PreCodeBlockState()

    public init(code: String, language: String? = nil) {
        self.code = code
        self.language = language
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                if let language, !language.isEmpty {
                    Text(language.uppercased())
                        .font(.system(size: 11, weight: .bold, design: .monospaced))
                        .foregroundStyle(Color.gray)
                } else {
                    Text("CODE")
                        .font(.system(size: 11, weight: .bold, design: .monospaced))
                        .foregroundStyle(Color.gray)
                }
                Spacer()
                Button(action: copyToClipboard) {
                    HStack(spacing: 4) {
                        Image(systemName: state.copied ? "checkmark" : "doc.on.doc")
                            .font(.system(size: 12))
                        Text(state.copied ? "Copied" : "Copy")
                            .font(.system(size: 11, weight: .medium))
                    }
                    .foregroundStyle(state.copied ? Color.green : Color.white.opacity(0.8))
                    .padding(.horizontal, 8)
                    .padding(.vertical, 3)
                    .background(Color.white.opacity(0.12))
                    .clipShape(Capsule())
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("code-block-copy-button")
            }

            ScrollView(.horizontal, showsIndicators: false) {
                Text(code)
                    .font(.system(size: 13, weight: .regular, design: .monospaced))
                    .foregroundStyle(Color.white)
                    .textSelection(.enabled)
                    .padding(.vertical, 4)
            }
        }
        .padding(10)
        .background(Color(conversationHex: "#1E1E1E"))
        .clipShape(RoundedRectangle(cornerRadius: 8))
        .accessibilityIdentifier("message-pre-code-block")
    }

    private func copyToClipboard() {
        #if canImport(UIKit)
        UIPasteboard.general.string = code
        let generator = UINotificationFeedbackGenerator()
        generator.notificationOccurred(.success)
        #endif
        withAnimation(.spring(response: 0.25, dampingFraction: 0.7)) {
            state.copied = true
        }
        Task {
            try? await Task.sleep(nanoseconds: 2_000_000_000)
            withAnimation {
                state.copied = false
            }
        }
    }
}

// MARK: - Message Formatting View
public struct MessageFormattingView: View {
    public let text: String
    public let entities: [MessageEntity]
    public let revealedSpoilers: Set<String>
    public let onRevealSpoiler: ((String) -> Void)?
    public let onTapUrl: ((URL) -> Void)?

    public init(
        text: String,
        entities: [MessageEntity] = [],
        revealedSpoilers: Set<String> = [],
        onRevealSpoiler: ((String) -> Void)? = nil,
        onTapUrl: ((URL) -> Void)? = nil
    ) {
        self.text = text
        self.entities = entities
        self.revealedSpoilers = revealedSpoilers
        self.onRevealSpoiler = onRevealSpoiler
        self.onTapUrl = onTapUrl
    }

    public var body: some View {
        let preEntities = entities.filter { $0.type == .pre }

        if preEntities.isEmpty {
            // Standard formatted inline message
            let attr = FormattedTextBuilder.buildAttributedString(
                text: text,
                entities: entities,
                revealedSpoilers: revealedSpoilers
            )

            Text(attr)
                .font(.system(size: 16))
                .environment(\.openURL, OpenURLAction { url in
                    if let onTapUrl {
                        onTapUrl(url)
                        return .handled
                    }
                    return .systemAction
                })
                .textSelection(.enabled)
        } else {
            // Message contains preformatted code block(s)
            VStack(alignment: .leading, spacing: 8) {
                let nonPreEntities = entities.filter { $0.type != .pre }
                let attr = FormattedTextBuilder.buildAttributedString(
                    text: text,
                    entities: nonPreEntities,
                    revealedSpoilers: revealedSpoilers
                )

                if !text.isEmpty {
                    Text(attr)
                        .font(.system(size: 16))
                        .environment(\.openURL, OpenURLAction { url in
                            if let onTapUrl {
                                onTapUrl(url)
                                return .handled
                            }
                            return .systemAction
                        })
                }

                ForEach(preEntities) { pre in
                    let utf16 = text.utf16
                    if let start = utf16.index(utf16.startIndex, offsetBy: max(0, min(utf16.count, pre.offset)), limitedBy: utf16.endIndex),
                       let end = utf16.index(start, offsetBy: max(0, min(utf16.count - pre.offset, pre.length)), limitedBy: utf16.endIndex),
                       let s = String.Index(start, within: text),
                       let e = String.Index(end, within: text) {
                        let codeSnippet = String(text[s..<e])
                        MessagePreCodeBlockView(code: codeSnippet, language: pre.language)
                    }
                }
            }
        }
    }
}
