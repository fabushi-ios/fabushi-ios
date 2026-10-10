import Foundation
import Darwin

protocol SystemErrnoCodeProviding: Error {
    var systemErrnoCode: String? { get }
    var underlyingSystemError: Error? { get }
}
struct SystemErrnoError: SystemErrnoCodeProviding {
    let systemErrnoCode: String?
    let underlyingSystemError: Error?
}

enum SystemErrno {
    static func find(in error: Error?) -> String? {
        var current = error, depth = 0
        while let value = current, depth < 32 {
            depth += 1
            if let provided = value as? SystemErrnoCodeProviding {
                if let code = provided.systemErrnoCode, isSymbolicCode(code) { return code }
                current = provided.underlyingSystemError
                continue
            }
            let ns = value as NSError
            if ns.domain == NSPOSIXErrorDomain, let code = symbolicPOSIXCode(Int32(ns.code)) { return code }
            current = ns.userInfo[NSUnderlyingErrorKey] as? Error
        }
        return nil
    }

    private static func isSymbolicCode(_ value: String) -> Bool {
        guard value.first == "E", value.count > 1 else { return false }
        return value.dropFirst().allSatisfy { $0.isUppercase || $0 == "_" }
    }

    private static func symbolicPOSIXCode(_ code: Int32) -> String? {
        switch code {
        case EACCES: "EACCES"
        case ECONNREFUSED: "ECONNREFUSED"
        case ECONNRESET: "ECONNRESET"
        case EHOSTUNREACH: "EHOSTUNREACH"
        case ENETUNREACH: "ENETUNREACH"
        case ETIMEDOUT: "ETIMEDOUT"
        case ENOENT: "ENOENT"
        case EPIPE: "EPIPE"
        case EINTR: "EINTR"
        case EINVAL: "EINVAL"
        default: nil
        }
    }
}
