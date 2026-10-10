import Foundation

enum SharedImageMime {
    static func imageMime(fromPath path: String) -> String? {
        SharedMediaExtensions.imageMimeFromExtension[SharedMediaExtensions.extensionOf(path)]
    }
    static func servableImageMime(fromPath path: String) -> String? {
        let ext = SharedMediaExtensions.extensionOf(path)
        return SharedMediaExtensions.imageMimeFromExtension[ext]
            ?? SharedMediaExtensions.clientNativeImageMimeFromExtension[ext]
    }
    static func extensionFromImageMime(_ mime: String) -> String? {
        SharedMediaExtensions.extensionFromImageMime[mime.lowercased()]
    }
    static func videoMime(fromPath path: String) -> String? {
        SharedMediaExtensions.videoMimeFromExtension[SharedMediaExtensions.extensionOf(path)]
    }
    static func audioMime(fromPath path: String) -> String? {
        SharedMediaExtensions.audioMimeFromExtension[SharedMediaExtensions.extensionOf(path)]
    }
}
