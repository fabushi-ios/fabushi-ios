import Foundation
import Darwin

struct AtomicWriteError: Error, Equatable, Sendable {
    let operation: String
    let errnoCode: Int32
}

func writeFileAtomic(
    targetPath: String,
    data: Data,
    mode: Int32? = nil
) throws {
    let targetURL = URL(fileURLWithPath: targetPath)
    let directory = targetURL.deletingLastPathComponent().path
    try FileManager.default.createDirectory(
        atPath: directory,
        withIntermediateDirectories: true
    )

    let temporaryPath = URL(fileURLWithPath: directory)
        .appendingPathComponent(".\(targetURL.lastPathComponent).\(UUID().uuidString.replacingOccurrences(of: "-", with: "").lowercased()).part")
        .path

    let permissions = mode_t(mode ?? 0o600)
    let fd = Darwin.open(temporaryPath, O_WRONLY | O_CREAT | O_EXCL, permissions)
    guard fd >= 0 else { throw AtomicWriteError(operation: "open", errnoCode: errno) }

    var closeNeeded = true
    defer {
        if closeNeeded { _ = Darwin.close(fd) }
    }

    do {
        try data.withUnsafeBytes { rawBuffer in
            guard var base = rawBuffer.baseAddress?.assumingMemoryBound(to: UInt8.self) else { return }
            var remaining = rawBuffer.count
            while remaining > 0 {
                let written = Darwin.write(fd, base, remaining)
                if written < 0 {
                    if errno == EINTR { continue }
                    throw AtomicWriteError(operation: "write", errnoCode: errno)
                }
                remaining -= written
                base = base.advanced(by: written)
            }
        }
        if Darwin.fsync(fd) != 0 {
            throw AtomicWriteError(operation: "fsync", errnoCode: errno)
        }
        if Darwin.close(fd) != 0 {
            closeNeeded = false
            throw AtomicWriteError(operation: "close", errnoCode: errno)
        }
        closeNeeded = false
    } catch {
        _ = Darwin.unlink(temporaryPath)
        throw error
    }

    if Darwin.rename(temporaryPath, targetPath) != 0 {
        let code = errno
        _ = Darwin.unlink(temporaryPath)
        throw AtomicWriteError(operation: "rename", errnoCode: code)
    }
}

func writeFileAtomic(
    targetPath: String,
    string: String,
    mode: Int32? = nil
) throws {
    try writeFileAtomic(
        targetPath: targetPath,
        data: Data(string.utf8),
        mode: mode
    )
}
