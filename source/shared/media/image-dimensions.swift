import Foundation
import ImageIO

struct MediaDimensions: Equatable, Sendable { let width: Int; let height: Int }

enum ImageFileDimensions {
    static func read(_ data: Data) -> MediaDimensions? {
        guard !data.isEmpty,
              let source=CGImageSourceCreateWithData(data as CFData,nil),
              let props=CGImageSourceCopyPropertiesAtIndex(source,0,nil) as? [CFString:Any],
              let w=props[kCGImagePropertyPixelWidth] as? NSNumber,
              let h=props[kCGImagePropertyPixelHeight] as? NSNumber
        else { return nil }
        var width=w.intValue, height=h.intValue
        guard width>0,height>0 else { return nil }
        if let orientation=props[kCGImagePropertyOrientation] as? NSNumber, [5,6,7,8].contains(orientation.intValue) { swap(&width,&height) }
        return .init(width:width,height:height)
    }
    static func readWebpOrHeic(_ data: Data) -> MediaDimensions? { read(data) }
    static func readPng(_ data: Data) -> MediaDimensions? {
        guard [UInt8](data.prefix(8)) == [137,80,78,71,13,10,26,10] else { return nil }; return read(data)
    }
    static func readGif(_ data: Data) -> MediaDimensions? {
        guard let header=String(data:data.prefix(6),encoding:.ascii),header=="GIF87a" || header=="GIF89a" else { return nil }; return read(data)
    }
    static func readJpeg(_ data: Data) -> MediaDimensions? {
        guard [UInt8](data.prefix(2)) == [0xff,0xd8] else { return nil }; return read(data)
    }
}
