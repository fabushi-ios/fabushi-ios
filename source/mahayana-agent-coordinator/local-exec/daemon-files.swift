import Foundation

enum IOSLocalExecFiles {
    static func directory(in applicationSupport: URL) -> URL {
        applicationSupport.appendingPathComponent("local-exec", isDirectory: true)
    }

    static func traceFile(in applicationSupport: URL) -> URL {
        directory(in: applicationSupport).appendingPathComponent("trace.jsonl")
    }

    static func ensureDirectory(in applicationSupport: URL) throws {
        try FileManager.default.createDirectory(
            at: directory(in: applicationSupport),
            withIntermediateDirectories: true
        )
    }
}
