import Foundation

enum AttachmentOpenPolicy {
    static func attachmentExtension(_ nameOrPath: String) -> String? {
        let normalized = nameOrPath.replacingOccurrences(of: "\\", with: "/")
        let base = normalized.split(separator: "/", omittingEmptySubsequences: false).last.map(String.init) ?? ""
        guard let dot = base.lastIndex(of: "."),
              dot != base.startIndex,
              base.index(after: dot) < base.endIndex
        else { return nil }
        return String(base[base.index(after: dot)...]).lowercased()
    }
}
