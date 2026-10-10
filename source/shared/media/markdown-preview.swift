import Foundation

enum MarkdownPreview {
    private static let transforms: [(String, String)] = [
        (#"\x60+"#, ""),
        (#"!\[([^\]]*)\]\([^)]*\)"#, "$1"),
        (#"\[([^\]]+)\]\([^)]*\)"#, "$1"),
        (#"\$\$((?:[^$\\]|\\[\s\S])+)\$\$"#, "$1"),
        (#"\\\(([\s\S]+?)\\\)"#, "$1"),
        (#"\\\[([\s\S]+?)\\\]"#, "$1"),
        (#"\\\$"#, "$"),
        (#"(?m)^\s{0,3}#{1,6}\s+"#, ""),
        (#"(?m)^\s{0,3}>\s?"#, ""),
        (#"(?m)^\s{0,3}(?:[-*+]|\d+[.)])\s+"#, ""),
        (#"\*\*([^*]+)\*\*"#, "$1"),
        (#"__([^_]+)__"#, "$1"),
        (#"~~([^~]+)~~"#, "$1"),
        (#"\*([^*\n]+)\*"#, "$1"),
        (#"(?<!\w)_([^_\n]+)_(?!\w)"#, "$1"),
        (#"\|"#, " "),
    ]

    static func toPreviewText(_ input: String) -> String {
        var text=input
        for (pattern,replacement) in transforms {
            guard let regex=try? NSRegularExpression(pattern:pattern) else { continue }
            text=regex.stringByReplacingMatches(in:text,range:NSRange(text.startIndex...,in:text),withTemplate:replacement)
        }
        guard let whitespace=try? NSRegularExpression(pattern:#"\s+"#) else { return text.trimmingCharacters(in:.whitespacesAndNewlines) }
        return whitespace.stringByReplacingMatches(in:text,range:NSRange(text.startIndex...,in:text),withTemplate:" ")
            .trimmingCharacters(in:.whitespacesAndNewlines)
    }
}
