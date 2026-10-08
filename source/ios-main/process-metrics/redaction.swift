import CryptoKit
import Foundation

struct IOSProcessNameSanitization: Equatable, Sendable {
    let name: String
    let nameHash: String
}

private let iosHelperProcessNameRegex = try! NSRegularExpression(
    pattern: #"^(?:Fabushi(?: Lab)?|Grok Bot(?: Lab)?) Helper(?: \((?:GPU|Plugin|Renderer)\))?$"#
)

func hashProcessName(_ name: String) -> String {
    SHA256.hash(data: Data(name.utf8))
        .map { String(format: "%02x", $0) }
        .joined()
}

func sanitizeProcessName(_ name: String) -> IOSProcessNameSanitization {
    let nameHash = hashProcessName(name)
    let slash = max(
        name.lastIndex(of: "/").map { name.distance(from: name.startIndex, to: $0) } ?? -1,
        name.lastIndex(of: "\\").map { name.distance(from: name.startIndex, to: $0) } ?? -1
    )
    let label: String
    if slash >= 0 {
        let index = name.index(name.startIndex, offsetBy: slash + 1)
        label = String(name[index...]).trimmingCharacters(in: .whitespacesAndNewlines)
    } else {
        label = name.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    let range = NSRange(label.startIndex..., in: label)
    if iosHelperProcessNameRegex.firstMatch(in: label, range: range) != nil {
        return .init(name: label, nameHash: nameHash)
    }

    let firstToken = label.split(whereSeparator: { $0.isWhitespace }).first.map(String.init) ?? ""
    return .init(name: firstToken, nameHash: nameHash)
}
