import Foundation

enum SandAttachmentKind: String, CaseIterable, Sendable {
    case image, video, audio, pdf, markdown, table, json, text, document, archive, file
}
struct AttachmentKindCount: Equatable, Sendable { let kind: String; let count: Int }
struct KnownAttachmentKindCount: Equatable, Sendable { let kind: SandAttachmentKind; let count: Int }

enum AttachmentSummaryText {
    private static let labels: [SandAttachmentKind: (String, String)] = [
        .image: ("image", "images"), .video: ("video", "videos"), .audio: ("audio file", "audio files"),
        .pdf: ("PDF", "PDFs"), .markdown: ("Markdown file", "Markdown files"), .table: ("spreadsheet", "spreadsheets"),
        .json: ("JSON file", "JSON files"), .text: ("text file", "text files"), .document: ("document", "documents"),
        .archive: ("archive", "archives"), .file: ("file", "files"),
    ]

    static func kindPhrase(_ kind: SandAttachmentKind, count: Int) -> String {
        let label = labels[kind] ?? ("file", "files")
        return "\(count) \(count == 1 ? label.0 : label.1)"
    }

    static func mergeKindCounts(_ kinds: [AttachmentKindCount]?) -> [KnownAttachmentKindCount] {
        guard let kinds, !kinds.isEmpty else { return [] }
        var order: [SandAttachmentKind] = []
        var counts: [SandAttachmentKind: Int] = [:]
        for entry in kinds where entry.count > 0 {
            let kind = SandAttachmentKind(rawValue: entry.kind) ?? .file
            if counts[kind] == nil { order.append(kind) }
            counts[kind, default: 0] += entry.count
        }
        return order.compactMap { kind in counts[kind].map { .init(kind: kind, count: $0) } }
    }

    static func formatSentSummary(count: Int, kinds: [AttachmentKindCount]? = nil) -> String {
        let total = max(1, count)
        let merged = mergeKindCounts(kinds)
        if merged.isEmpty { return "Sent \(kindPhrase(.file, count: total))" }
        if merged.count == 1, let first = merged.first { return "Sent \(kindPhrase(first.kind, count: total))" }
        let breakdown = merged.map { kindPhrase($0.kind, count: $0.count) }.joined(separator: ", ")
        return "Sent \(kindPhrase(.file, count: total)) · \(breakdown)"
    }
}
