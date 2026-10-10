import Foundation

struct AttachmentClassificationSource: Sendable {
    let mimeType: String?
    let fileName: String?
    let urlOrPath: String?
    init(mimeType: String? = nil, fileName: String? = nil, urlOrPath: String? = nil) {
        self.mimeType = mimeType; self.fileName = fileName; self.urlOrPath = urlOrPath
    }
}

enum AttachmentClassification {
    private static let jsonExtensions: Set<String> = ["json", "jsonc", "json5", "ndjson"]
    private static let archiveExtensions: Set<String> = ["zip", "tar", "gz", "tgz", "bz2", "tbz2", "xz", "txz", "zst", "7z", "rar"]
    private static let tableMimeTypes: Set<String> = [
        "text/csv", "text/tab-separated-values", "application/vnd.ms-excel",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    ]
    private static let documentMimeTypes: Set<String> = [
        "application/msword", "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    ]
    private static let archiveMimeTypes: Set<String> = [
        "application/zip", "application/x-zip-compressed", "application/gzip", "application/x-tar",
        "application/x-bzip2", "application/x-xz", "application/zstd", "application/x-7z-compressed",
        "application/x-rar-compressed", "application/vnd.rar",
    ]

    static func classifyMimeType(_ rawMimeType: String) -> SandAttachmentKind? {
        let mime = (rawMimeType.split(separator: ";", maxSplits: 1).first.map(String.init) ?? "")
            .trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        if mime.hasPrefix("image/") { return .image }
        if mime.hasPrefix("video/") { return .video }
        if mime.hasPrefix("audio/") { return .audio }
        if mime == "application/pdf" { return .pdf }
        if mime == "text/markdown" { return .markdown }
        if tableMimeTypes.contains(mime) { return .table }
        if mime == "application/json" || mime.hasSuffix("+json") { return .json }
        if documentMimeTypes.contains(mime) { return .document }
        if archiveMimeTypes.contains(mime) { return .archive }
        if mime.hasPrefix("text/") { return .text }
        return nil
    }

    static func extensionSubject(_ source: String) -> String {
        guard let components = URLComponents(string: source), components.scheme != nil else { return source }
        return components.percentEncodedPath.removingPercentEncoding ?? components.path
    }

    static func classifyPathLike(_ source: String) -> SandAttachmentKind? {
        let subject = extensionSubject(source)
        return switch FilePreviewPolicy.kind(for: subject) {
        case .image: .image
        case .video: .video
        case .audio: .audio
        case .pdf: .pdf
        case .markdown: .markdown
        case .table: .table
        case .docx: .document
        case .json: .json
        case .text:
            AttachmentOpenPolicy.attachmentExtension(subject).map(jsonExtensions.contains) == true ? .json : .text
        case .unknown:
            AttachmentOpenPolicy.attachmentExtension(subject).map(archiveExtensions.contains) == true ? .archive : nil
        }
    }

    static func classify(_ source: AttachmentClassificationSource) -> SandAttachmentKind {
        if let mime=source.mimeType, !mime.isEmpty, let kind=classifyMimeType(mime) { return kind }
        if let name=source.fileName, !name.isEmpty, let kind=classifyPathLike(name) { return kind }
        if let path=source.urlOrPath, !path.isEmpty, let kind=classifyPathLike(path) { return kind }
        return .file
    }

    static func countKinds(_ kinds: [SandAttachmentKind]) -> [KnownAttachmentKindCount] {
        var order: [SandAttachmentKind]=[]; var counts: [SandAttachmentKind:Int]=[:]
        for kind in kinds { if counts[kind] == nil { order.append(kind) }; counts[kind,default:0] += 1 }
        return order.compactMap { kind in counts[kind].map { .init(kind: kind, count: $0) } }
    }
}
