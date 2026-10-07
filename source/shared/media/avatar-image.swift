import Foundation
import UIKit

struct MobileAvatarCrop: Equatable, Sendable {
    var zoom: Double
    var centerX: Double
    var centerY: Double
}

enum AvatarImagePolicyError: LocalizedError, Equatable {
    case sourceTooLarge
    case invalidImage
    case exportFailed

    var errorDescription: String? {
        switch self {
        case .sourceTooLarge: return "Choose an image smaller than 25 MB."
        case .invalidImage: return "That image could not be loaded."
        case .exportFailed: return "Could not export the avatar."
        }
    }
}

enum AvatarImagePolicy {
    static let sourceMaxBytes = 25 * 1024 * 1024
    static let sourceMaxDimension = 1_024
    static let outputSize = 256
    static let stageSize = 260.0
    static let minZoom = 1.0
    static let maxZoom = 5.0

    static let colors: [(id: String, label: String, value: String)] = [
        ("black", "Black", "#000000"),
        ("brown", "Brown", "#936439"),
        ("red", "Red", "#FF263C"),
        ("orange", "Orange", "#FF6700"),
        ("yellow", "Yellow", "#FF9800"),
        ("green", "Green", "#00C972"),
        ("cyan", "Cyan", "#00BCA6"),
        ("blue", "Blue", "#1084FE"),
        ("violet", "Violet", "#9159FE"),
        ("magenta", "Magenta", "#FF309B"),
        ("gray", "Gray", "#777777"),
    ]
    static let shapes = [
        "blob", "pebble", "squircle", "tablet", "wedge", "hex", "cloud", "teardrop",
    ]

    static func sourceSizeError(byteLength: Int) -> String? {
        byteLength > sourceMaxBytes ? "Choose an image smaller than 25 MB." : nil
    }

    static let extensionMime: [String: String] = [
        ".png": "image/png", ".jpg": "image/jpeg", ".jpeg": "image/jpeg", ".webp": "image/webp",
        ".gif": "image/gif", ".bmp": "image/bmp", ".avif": "image/avif", ".svg": "image/svg+xml",
    ]

    static func mimeHint(forExtension ext: String) -> String {
        extensionMime[ext.lowercased()] ?? "application/octet-stream"
    }

    static func clampZoom(_ value: Double) -> Double {
        guard value.isFinite else { return minZoom }
        return min(max(value, minZoom), maxZoom)
    }

    static func initialCrop(width: Double, height: Double) -> MobileAvatarCrop {
        .init(zoom: minZoom, centerX: width / 2, centerY: height / 2)
    }

    static func visibleSide(width: Double, height: Double, zoom: Double) -> Double {
        min(width, height) / clampZoom(zoom)
    }

    static func clampCrop(
        width: Double,
        height: Double,
        crop: MobileAvatarCrop
    ) -> MobileAvatarCrop {
        let zoom = clampZoom(crop.zoom)
        let half = visibleSide(width: width, height: height, zoom: zoom) / 2
        return .init(
            zoom: zoom,
            centerX: min(max(crop.centerX, half), width - half),
            centerY: min(max(crop.centerY, half), height - half)
        )
    }

    static func pan(
        width: Double,
        height: Double,
        crop: MobileAvatarCrop,
        deltaX: Double,
        deltaY: Double
    ) -> MobileAvatarCrop {
        let zoom = clampZoom(crop.zoom)
        let stageScale = (stageSize / min(width, height)) * zoom
        return clampCrop(
            width: width,
            height: height,
            crop: .init(
                zoom: zoom,
                centerX: crop.centerX - deltaX / stageScale,
                centerY: crop.centerY - deltaY / stageScale
            )
        )
    }

    static func cropRect(
        width: Double,
        height: Double,
        crop: MobileAvatarCrop
    ) -> CGRect {
        let bounded = clampCrop(width: width, height: height, crop: crop)
        let side = visibleSide(width: width, height: height, zoom: bounded.zoom)
        return CGRect(
            x: bounded.centerX - side / 2,
            y: bounded.centerY - side / 2,
            width: side,
            height: side
        )
    }

    @MainActor
    static func normalizeSource(data: Data) throws -> UIImage {
        if data.count > sourceMaxBytes { throw AvatarImagePolicyError.sourceTooLarge }
        guard let raw = UIImage(data: data), raw.size.width > 0, raw.size.height > 0 else {
            throw AvatarImagePolicyError.invalidImage
        }
        let longest = max(raw.size.width, raw.size.height)
        let ratio = min(1, CGFloat(sourceMaxDimension) / longest)
        let target = CGSize(
            width: max(1, (raw.size.width * ratio).rounded()),
            height: max(1, (raw.size.height * ratio).rounded())
        )
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        format.opaque = false
        return UIGraphicsImageRenderer(size: target, format: format).image { _ in
            raw.draw(in: CGRect(origin: .zero, size: target))
        }
    }

    @MainActor
    static func pngDataURL(image: UIImage, crop: MobileAvatarCrop) throws -> String {
        guard let cgImage = image.cgImage else { throw AvatarImagePolicyError.invalidImage }
        let rect = cropRect(
            width: Double(cgImage.width),
            height: Double(cgImage.height),
            crop: crop
        ).integral
        guard rect.width > 0, rect.height > 0,
              let cropped = cgImage.cropping(to: rect)
        else { throw AvatarImagePolicyError.exportFailed }

        let source = UIImage(cgImage: cropped, scale: 1, orientation: .up)
        let size = CGSize(width: outputSize, height: outputSize)
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        format.opaque = false
        let rendered = UIGraphicsImageRenderer(size: size, format: format).image { _ in
            source.draw(in: CGRect(origin: .zero, size: size))
        }
        guard let png = rendered.pngData(), !png.isEmpty else {
            throw AvatarImagePolicyError.exportFailed
        }
        return "data:image/png;base64,\(png.base64EncodedString())"
    }
}
