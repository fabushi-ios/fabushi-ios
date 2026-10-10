import Foundation

enum MP4Dimensions {
    private struct ByteRange { let start:Int; let end:Int }
    private struct ClampedBox { let body:ByteRange; let next:Int }

    static func read(_ data:Data) -> MediaDimensions? {
        let bytes=[UInt8](data); var searchFrom=0
        while searchFrom+4<=bytes.count {
            let moov=indexOfTag(bytes,tag:"moov",from:searchFrom)
            if moov<0 { return nil }
            if let dimensions=readMoov(bytes,moovTag:moov) { return dimensions }
            searchFrom=moov+4
        }
        return nil
    }

    private static func readU32BE(_ bytes:[UInt8],_ offset:Int)->UInt32? {
        guard offset>=0,offset+4<=bytes.count else { return nil }
        return UInt32(bytes[offset])<<24 | UInt32(bytes[offset+1])<<16 | UInt32(bytes[offset+2])<<8 | UInt32(bytes[offset+3])
    }
    private static func tag(_ bytes:[UInt8],at offset:Int)->String? {
        guard offset>=0,offset+4<=bytes.count else { return nil }
        return String(bytes:bytes[offset..<(offset+4)],encoding:.ascii)
    }
    private static func indexOfTag(_ bytes:[UInt8],tag wanted:String,from:Int)->Int {
        let target=Array(wanted.utf8); guard target.count==4 else { return -1 }
        var at=max(from,4)
        while at+4<=bytes.count { if Array(bytes[at..<(at+4)])==target { return at }; at+=1 }
        return -1
    }
    private static func readMoov(_ bytes:[UInt8],moovTag:Int)->MediaDimensions? {
        guard let box=clampedBoxAt(bytes,at:moovTag-4,end:bytes.count) else { return nil }
        for trak in childBoxes(bytes,range:box.body,type:"trak") {
            for tkhd in childBoxes(bytes,range:trak,type:"tkhd") {
                if let dimensions=readTkhd(bytes,box:tkhd) { return dimensions }
            }
        }
        return nil
    }
    private static func clampedBoxAt(_ bytes:[UInt8],at:Int,end:Int)->ClampedBox? {
        guard at>=0,at+8<=end,let initial=readU32BE(bytes,at) else { return nil }
        var size=Int(initial),header=8
        if size==1 { guard at+16<=end,let low=readU32BE(bytes,at+12) else { return nil }; size=Int(low); header=16 }
        else if size==0 { size=end-at }
        guard size>=header else { return nil }
        return .init(body:.init(start:at+header,end:min(at+size,end)),next:at+size)
    }
    private static func childBoxes(_ bytes:[UInt8],range:ByteRange,type:String)->[ByteRange] {
        var result:[ByteRange]=[],offset=range.start
        while offset+8<=range.end {
            guard let box=clampedBoxAt(bytes,at:offset,end:range.end) else { break }
            if tag(bytes,at:offset+4)==type { result.append(box.body) }
            if box.next>range.end || box.next<=offset { break }; offset=box.next
        }
        return result
    }
    private static func readTkhd(_ bytes:[UInt8],box:ByteRange)->MediaDimensions? {
        guard box.start<box.end else { return nil }
        let widened=bytes[box.start]==1 ? 12 : 0, matrixAt=box.start+40+widened, widthAt=matrixAt+36
        guard widthAt+8<=box.end,let rawW=readU32BE(bytes,widthAt),let rawH=readU32BE(bytes,widthAt+4) else { return nil }
        let w=Int(rawW>>16),h=Int(rawH>>16); guard w>0,h>0 else { return nil }
        return matrixIsQuarterTurn(bytes,matrixAt:matrixAt) ? .init(width:h,height:w) : .init(width:w,height:h)
    }
    private static func matrixIsQuarterTurn(_ bytes:[UInt8],matrixAt:Int)->Bool {
        guard let a=readU32BE(bytes,matrixAt),let b=readU32BE(bytes,matrixAt+4),let c=readU32BE(bytes,matrixAt+12),let d=readU32BE(bytes,matrixAt+16) else { return false }
        return a==0 && d==0 && b != 0 && c != 0
    }
}
