import Foundation

enum SandText {
    private static func prefixByUTF16Units(_ value: String, maxLength: Int) -> String {
        guard maxLength > 0 else { return "" }
        var result = ""
        var units = 0
        for character in value {
            let part = String(character)
            let next = part.utf16.count
            guard units + next <= maxLength else { break }
            result.append(character)
            units += next
        }
        return result
    }

    static func clampLine(_ raw: String, maxLength: Int) -> String {
        let collapsed = raw.replacingOccurrences(of: #"\s+"#, with: " ", options: .regularExpression)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return prefixByUTF16Units(collapsed, maxLength: maxLength)
    }

    static func clampBlock(_ raw: String, maxLength: Int) -> String {
        prefixByUTF16Units(raw.trimmingCharacters(in: .whitespacesAndNewlines), maxLength: maxLength)
    }

    static func decapitalize(_ phrase: String) -> String {
        guard let first = phrase.first else { return phrase }
        return first.lowercased() + phrase.dropFirst()
    }

    static func slugifyName(_ name: String, fallbackPrefix: String, nowMilliseconds: Int64) -> String {
        let lower = name.lowercased()
        let replaced = lower.replacingOccurrences(of: #"[^a-z0-9]+"#, with: "-", options: .regularExpression)
        let trimmed = replaced.trimmingCharacters(in: CharacterSet(charactersIn: "-"))
        let slug = String(trimmed.prefix(48))
        return slug.isEmpty ? "\(fallbackPrefix)-\(nowMilliseconds)" : slug
    }
}
