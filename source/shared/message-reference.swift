import Foundation

enum MessageReference {
    private static let exact = try! NSRegularExpression(pattern: #"^t(?:\d+u(?:a\d+)?|(?:\d+|b)[as]\d+)$"#)

    static func isMessageAddress(_ value: String) -> Bool {
        exact.firstMatch(in: value, range: NSRange(value.startIndex..., in: value)) != nil
    }
}
