import Foundation

enum AttachmentPreviewPolicy {
    static let textPreviewableExtensions: Set<String> = [
        "txt", "text", "log", "md", "markdown", "mdx", "rst", "adoc", "tex", "json", "jsonc",
        "json5", "ndjson", "csv", "tsv", "xml", "yaml", "yml", "toml", "ini", "cfg", "conf",
        "env", "properties", "plist", "gradle", "html", "htm", "css", "scss", "sass", "less", "svg",
        "js", "jsx", "mjs", "cjs", "ts", "tsx", "mts", "cts", "py", "pyi", "rb", "go", "rs",
        "java", "kt", "kts", "c", "h", "cc", "cpp", "cxx", "hpp", "hh", "cs", "php", "swift",
        "scala", "dart", "lua", "pl", "pm", "r", "sql", "graphql", "gql", "proto", "vue", "svelte",
        "astro", "sh", "bash", "zsh", "fish", "bat", "ps1", "tf", "tfvars", "dockerfile", "diff", "patch",
    ]
    static let binarySniffByteWindow = 8 * 1024
    static let binaryControlByteRatio = 0.3

    static func isTextPreviewableName(_ nameOrPath: String) -> Bool {
        guard let ext = AttachmentOpenPolicy.attachmentExtension(nameOrPath) else { return false }
        return textPreviewableExtensions.contains(ext)
    }

    static func looksLikeBinary(_ data: Data) -> Bool {
        let bytes = [UInt8](data.prefix(binarySniffByteWindow))
        guard !bytes.isEmpty else { return false }
        var controlBytes = 0
        for byte in bytes {
            if byte == 0 { return true }
            let isTextWhitespace = byte >= 9 && byte <= 13
            if byte < 32 && !isTextWhitespace { controlBytes += 1 }
        }
        return Double(controlBytes) / Double(bytes.count) > binaryControlByteRatio
    }
}
