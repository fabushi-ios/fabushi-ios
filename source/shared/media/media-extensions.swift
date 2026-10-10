import Foundation

enum SharedMediaExtensions {
    static func extensionOf(_ name: String) -> String {
        let normalized = name.replacingOccurrences(of: "\\", with: "/")
        let base = normalized.split(separator: "/", omittingEmptySubsequences: false).last.map(String.init) ?? ""
        guard let dot = base.lastIndex(of: "."), dot != base.startIndex else { return "" }
        return String(base[dot...]).lowercased()
    }

    static let imageMimeFromExtension: [String: String] = [
        ".avif": "image/avif", ".bmp": "image/bmp", ".gif": "image/gif", ".ico": "image/x-icon",
        ".jpeg": "image/jpeg", ".jpg": "image/jpeg", ".png": "image/png", ".svg": "image/svg+xml", ".webp": "image/webp",
    ]
    static let clientNativeImageMimeFromExtension: [String: String] = [".heic": "image/heic", ".heif": "image/heif"]
    static let extensionFromImageMime: [String: String] = [
        "image/avif": ".avif", "image/bmp": ".bmp", "image/gif": ".gif", "image/jpeg": ".jpg",
        "image/png": ".png", "image/svg+xml": ".svg", "image/webp": ".webp", "image/x-icon": ".ico",
        "image/vnd.microsoft.icon": ".ico",
    ]
    static let videoMimeFromExtension: [String: String] = [
        ".m4v": "video/mp4", ".mov": "video/quicktime", ".mp4": "video/mp4", ".ogv": "video/ogg", ".webm": "video/webm",
    ]
    static let audioMimeFromExtension: [String: String] = [
        ".aac": "audio/aac", ".flac": "audio/flac", ".m4a": "audio/mp4", ".mp3": "audio/mpeg",
        ".oga": "audio/ogg", ".ogg": "audio/ogg", ".opus": "audio/ogg", ".wav": "audio/wav", ".weba": "audio/webm",
    ]
}
