import Foundation

let CONNECT_CODE_INVALID_ARGUMENT = 3
let CONNECT_CODE_DEADLINE_EXCEEDED = 4
let CONNECT_CODE_RESOURCE_EXHAUSTED = 8
let CONNECT_CODE_UNAVAILABLE = 14

protocol ConnectErrorLike: Error {
    var connectCode: Int { get }
    var connectMetadata: [String: String] { get }
}

func isRateLimitConnectError(_ error: Error) -> Bool {
    (error as? any ConnectErrorLike)?.connectCode == CONNECT_CODE_RESOURCE_EXHAUSTED
}

func isInvalidArgumentConnectError(_ error: Error) -> Bool {
    (error as? any ConnectErrorLike)?.connectCode == CONNECT_CODE_INVALID_ARGUMENT
}

func isDeadlineExceededConnectError(_ error: Error) -> Bool {
    (error as? any ConnectErrorLike)?.connectCode == CONNECT_CODE_DEADLINE_EXCEEDED
}

func isTransientConnectError(_ error: Error) -> Bool {
    guard let code = (error as? any ConnectErrorLike)?.connectCode else { return false }
    return code == CONNECT_CODE_UNAVAILABLE
        || code == CONNECT_CODE_DEADLINE_EXCEEDED
        || code == CONNECT_CODE_RESOURCE_EXHAUSTED
}

func getConnectRetryAfterMs(
    _ error: Error,
    nowMs: Int64 = Int64(Date().timeIntervalSince1970 * 1_000)
) -> Int? {
    guard let error = error as? any ConnectErrorLike else { return nil }
    return RetryAfter.parseMilliseconds(
        error.connectMetadata["retry-after"],
        now: Date(timeIntervalSince1970: TimeInterval(nowMs) / 1_000)
    )
}
