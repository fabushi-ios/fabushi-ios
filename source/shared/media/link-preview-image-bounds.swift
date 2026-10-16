import Foundation

struct LinkPreviewImageSize: Equatable, Sendable {
    let width: Int
    let height: Int
}

struct LinkPreviewImageBounds<Encoding: Sendable>: Sendable {
    let maxDimension: Int
    let encoding: Encoding
}

enum LinkPreviewResizeTarget: Equatable, Sendable {
    case width(Int)
    case height(Int)
}

enum LinkPreviewImagePolicy {
    static let maxDecodePixels = 24_000_000
    private static let dibHeaderSizes: Set<UInt32> = [40, 52, 56, 108, 124]

    private static func readU16LE(_ bytes: [UInt8], _ offset: Int) -> UInt16? {
        guard offset >= 0, offset + 2 <= bytes.count else { return nil }
        return UInt16(bytes[offset]) | (UInt16(bytes[offset + 1]) << 8)
    }

    private static func readU32LE(_ bytes: [UInt8], _ offset: Int) -> UInt32? {
        guard offset >= 0, offset + 4 <= bytes.count else { return nil }
        return UInt32(bytes[offset])
            | (UInt32(bytes[offset + 1]) << 8)
            | (UInt32(bytes[offset + 2]) << 16)
            | (UInt32(bytes[offset + 3]) << 24)
    }

    private static func readI32LE(_ bytes: [UInt8], _ offset: Int) -> Int32? {
        readU32LE(bytes, offset).map { Int32(bitPattern: $0) }
    }

    private static func readIcoBitmapSize(_ bytes: [UInt8], offset: Int) -> LinkPreviewImageSize? {
        guard offset >= 0,
              offset + 12 <= bytes.count,
              let headerSize = readU32LE(bytes, offset),
              dibHeaderSizes.contains(headerSize),
              let rawWidth = readI32LE(bytes, offset + 4),
              let rawHeight = readI32LE(bytes, offset + 8)
        else { return nil }

        let width = Int(rawWidth)
        let height64 = Int64(rawHeight)
        let absoluteHeight = height64 < 0 ? -height64 : height64
        let height = Int((absoluteHeight + 1) / 2)
        guard width > 0, height > 0 else { return nil }
        return .init(width: width, height: height)
    }

    private static func readIcoSize(_ data: Data) -> LinkPreviewImageSize? {
        let bytes = [UInt8](data)
        guard bytes.count >= 6,
              readU16LE(bytes, 0) == 0,
              let kind = readU16LE(bytes, 2),
              kind == 1 || kind == 2,
              let entryCountRaw = readU16LE(bytes, 4)
        else { return nil }

        let entryCount = Int(entryCountRaw)
        guard entryCount > 0, bytes.count >= 6 + entryCount * 16 else { return nil }

        var largest: LinkPreviewImageSize?
        var largestPixels = 0
        for index in 0..<entryCount {
            let entry = 6 + index * 16
            guard let payloadRaw = readU32LE(bytes, entry + 12) else { continue }
            let payload = Int(payloadRaw)
            let entryWidth = bytes[entry] == 0 ? 256 : Int(bytes[entry])
            let entryHeight = bytes[entry + 1] == 0 ? 256 : Int(bytes[entry + 1])
            let candidates: [LinkPreviewImageSize?] = [
                LinkPreviewImageSize(width: entryWidth, height: entryHeight),
                payload < bytes.count
                    ? ImageFileDimensions.readPng(Data(bytes[payload...])).map {
                        LinkPreviewImageSize(width: $0.width, height: $0.height)
                    }
                    : nil,
                readIcoBitmapSize(bytes, offset: payload),
            ]
            for candidate in candidates.compactMap({ $0 }) {
                let pixels = candidate.width.multipliedReportingOverflow(by: candidate.height)
                guard !pixels.overflow, pixels.partialValue > largestPixels else { continue }
                largestPixels = pixels.partialValue
                largest = candidate
            }
        }
        return largest
    }

    private static func decodeBase64DataURL(_ dataURL: String) -> Data? {
        guard dataURL.hasPrefix("data:"),
              let comma = dataURL.firstIndex(of: ","),
              dataURL[..<comma].hasSuffix(";base64")
        else { return nil }
        let payload = String(dataURL[dataURL.index(after: comma)...])
        return Data(base64Encoded: payload, options: [.ignoreUnknownCharacters])
    }

    static func readEncodedImageSize(_ dataURL: String) -> LinkPreviewImageSize? {
        guard let data = decodeBase64DataURL(dataURL) else { return nil }
        let readers: [(Data) -> MediaDimensions?] = [
            ImageFileDimensions.readPng,
            ImageFileDimensions.readJpeg,
            ImageFileDimensions.readGif,
            ImageFileDimensions.readWebpOrHeic,
        ]
        for read in readers {
            if let size = read(data), size.width > 0, size.height > 0 {
                return .init(width: size.width, height: size.height)
            }
        }
        return readIcoSize(data)
    }

    static func bound<Encoding: Sendable>(
        _ dataURL: String?,
        bounds: LinkPreviewImageBounds<Encoding>,
        resize: (String, LinkPreviewResizeTarget, Encoding) -> String?
    ) -> String? {
        guard let dataURL,
              bounds.maxDimension > 0,
              let size = readEncodedImageSize(dataURL)
        else { return nil }

        let pixels = size.width.multipliedReportingOverflow(by: size.height)
        guard !pixels.overflow, pixels.partialValue <= maxDecodePixels else { return nil }

        if max(size.width, size.height) <= bounds.maxDimension {
            return dataURL
        }
        let target: LinkPreviewResizeTarget = size.width >= size.height
            ? .width(bounds.maxDimension)
            : .height(bounds.maxDimension)
        return resize(dataURL, target, bounds.encoding) ?? dataURL
    }
}
