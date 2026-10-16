import Foundation

let LEGACY_MCP_AUTH_FILENAME = "mcp-auth.json"

func isLegacyMcpAuthFile(_ name: String) -> Bool {
    name == LEGACY_MCP_AUTH_FILENAME || name.hasPrefix(LEGACY_MCP_AUTH_FILENAME + ".")
}

struct LegacyMcpAuthCleanupResult: Equatable, Sendable {
    let outcome: String
    let removedCount: Int
}

func cleanupLegacyMcpAuthCredentials(_ rootDir: String) async -> LegacyMcpAuthCleanupResult {
    let fm = FileManager.default
    var isDirectory: ObjCBool = false
    guard fm.fileExists(atPath: rootDir, isDirectory: &isDirectory), isDirectory.boolValue else {
        return .init(outcome: "not_found", removedCount: 0)
    }
    let entries: [String]
    do {
        entries = try fm.contentsOfDirectory(atPath: rootDir)
    } catch {
        return .init(outcome: "error", removedCount: 0)
    }
    let files = entries.filter(isLegacyMcpAuthFile)
    guard !files.isEmpty else { return .init(outcome: "not_found", removedCount: 0) }

    var removedCount = 0
    var sawError = false
    for name in files {
        do {
            try fm.removeItem(at: URL(fileURLWithPath: rootDir).appendingPathComponent(name))
            removedCount += 1
        } catch {
            sawError = true
        }
    }
    return .init(outcome: sawError ? "error" : "deleted", removedCount: removedCount)
}
