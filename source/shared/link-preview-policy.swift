import Foundation

enum LinkPreviewPolicy {
    static let nonPublicHostnameSuffixes = [
        ".localhost", ".local", ".internal", ".lan", ".home", ".corp", ".cluster", ".svc", ".arpa", ".onion",
    ]
    static func hasNonPublicHostnameSuffix(_ hostname: String) -> Bool {
        let host = hostname.lowercased()
        return nonPublicHostnameSuffixes.contains { suffix in host == String(suffix.dropFirst()) || host.hasSuffix(suffix) }
    }
}
