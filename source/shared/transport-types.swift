import Foundation

func isValidAttachmentUrl(_ rawUrl: String) -> Bool {
    guard let url = URL(string: rawUrl), let scheme = url.scheme?.lowercased() else { return false }
    return scheme == "file" || scheme == "https"
}
