import Foundation

private let SAND_OPAQUE_ID = try! NSRegularExpression(pattern: #"^[0-9A-Za-z._:|\-]{1,128}$"#)

let SAND_ERRNO_TAGS = [
    "ECONNREFUSED","ECONNRESET","ECONNABORTED","ETIMEDOUT","EPIPE","ENETRESET","ENETDOWN",
    "ENETUNREACH","EHOSTUNREACH","EHOSTDOWN","EAI_AGAIN","ENOTFOUND","EADDRINUSE","EACCES",
    "EPERM","ENOENT","ENOSPC","EDQUOT","EROFS","EBUSY","EMFILE","EIO",
]
let SAND_ERRNO_FALLBACK = "E_OTHER"

func brandedEnumOf(_ values: [String], fallback: String, value: String?) -> String? {
    guard let value else { return nil }
    return values.contains(value) ? value : fallback
}

func brandLiteralEnum(_ value: String) -> String { value }

func brandedId(_ value: String?) -> String? {
    guard let value else { return nil }
    let range = NSRange(value.startIndex..<value.endIndex, in: value)
    return SAND_OPAQUE_ID.firstMatch(in: value, range: range) != nil ? value : nil
}

func brandedErrno(_ value: String?) -> String? {
    brandedEnumOf(SAND_ERRNO_TAGS, fallback: SAND_ERRNO_FALLBACK, value: value)
}
