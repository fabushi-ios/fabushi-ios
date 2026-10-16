import Foundation
import UniformTypeIdentifiers

enum IOSMimeTypes {
    static func lookup(path: String) -> String? {
        let pathExtension = (path as NSString).pathExtension
        guard !pathExtension.isEmpty else { return nil }
        return UTType(filenameExtension: pathExtension)?.preferredMIMEType
    }
}
