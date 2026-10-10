import Foundation

struct AttachmentTooLargeError: LocalizedError, Equatable, Sendable {
    let limitBytes: Int
    var errorDescription: String? { "Attachment exceeds \(limitBytes) bytes." }
}

enum AttachmentLimits {
    static let attachmentByteLimit = 25 * 1024 * 1024
    static let videoByteLimit = 200 * 1024 * 1024
    static let bytesPerMB = 1024 * 1024

    static func nameLooksLikeVideo(_ name: String) -> Bool {
        SharedMediaExtensions.videoMimeFromExtension[SharedMediaExtensions.extensionOf(name)] != nil
    }
    static func attachmentByteLimit(forName name: String) -> Int {
        nameLooksLikeVideo(name) ? videoByteLimit : attachmentByteLimit
    }
    static func formatMegabytes(_ bytes: Int) -> String {
        "\(Int((Double(bytes) / Double(bytesPerMB)).rounded())) MB"
    }
    static func formatTooLargeNotice(filename: String) -> String {
        let video = nameLooksLikeVideo(filename)
        let limit = video ? videoByteLimit : attachmentByteLimit
        return "\"\(filename)\" is too large to attach (max \(formatMegabytes(limit))\(video ? " for video" : ""))."
    }
}
