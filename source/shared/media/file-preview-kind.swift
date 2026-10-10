import Foundation

enum FilePreviewKind: String, Equatable, Sendable {
    case image, video, audio, pdf, table, json, markdown, docx, text, unknown
}
enum FilePreviewPolicy {
    private static let imageExtensions = Set(SharedMediaExtensions.imageMimeFromExtension.keys.map { String($0.dropFirst()) })
    private static let videoExtensions = Set(SharedMediaExtensions.videoMimeFromExtension.keys.map { String($0.dropFirst()) })
    private static let audioExtensions = Set(SharedMediaExtensions.audioMimeFromExtension.keys.map { String($0.dropFirst()) })
    private static let markdownExtensions: Set<String> = ["md", "markdown", "mdx"]
    private static let jsonExtensions: Set<String> = ["json"]
    private static let tableExtensions: Set<String> = ["csv", "tsv", "xlsx", "xls"]

    static func kind(for nameOrPath: String) -> FilePreviewKind {
        guard let ext = AttachmentOpenPolicy.attachmentExtension(nameOrPath) else { return .unknown }
        if imageExtensions.contains(ext) { return .image }
        if videoExtensions.contains(ext) { return .video }
        if audioExtensions.contains(ext) { return .audio }
        if ext == "pdf" { return .pdf }
        if tableExtensions.contains(ext) { return .table }
        if jsonExtensions.contains(ext) { return .json }
        if markdownExtensions.contains(ext) { return .markdown }
        if ext == "docx" { return .docx }
        if AttachmentPreviewPolicy.isTextPreviewableName(nameOrPath) { return .text }
        return .unknown
    }
    static func needsBytes(_ kind: FilePreviewKind) -> Bool {
        switch kind {
        case .pdf, .table, .docx, .text, .json, .markdown: true
        case .image, .video, .audio, .unknown: false
        }
    }
}
