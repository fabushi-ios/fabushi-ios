import Foundation

enum CoordinatorExitClass: String, Equatable, Sendable {
    case clean
    case breach
    case bootstrap
    case suspended
    case other
}

struct CoordinatorLifecycleTelemetry: Equatable, Sendable {
    let outcome: String
    let exitClass: CoordinatorExitClass?
    let uptimeMilliseconds: Int?
    let relaunchSequence: Int
    let delayMilliseconds: Int?
}

enum CoordinatorTelemetry {
    static let recentFailureWindowMilliseconds = 30_000
    static let healthyUptimeMilliseconds = 30_000

    static func classify(exitCode: Int?) -> CoordinatorExitClass {
        switch exitCode {
        case 0: .clean
        case 1: .breach
        case 2: .bootstrap
        case nil: .suspended
        default: .other
        }
    }

    static func relaunchDelayMilliseconds(attempt: Int) -> Int {
        let exponent = max(0, min(attempt, 8))
        return min(10_000, 250 * (1 << exponent))
    }
}


enum IOSAuthTelemetryStream: String, Equatable, Sendable {
    case session
    case signin
}

struct IOSAuthTelemetryProjection: Equatable, Sendable {
    let stream: IOSAuthTelemetryStream
    let level: IOSLifecycleTelemetryLevel
    let metadata: [String: String]
}

enum IOSCursorSessionRefreshFailure: Equatable, Sendable {
    case httpStatus(Int)
    case network(String)
    case badPayload
}

enum IOSCursorSessionSignoutCause: String, Equatable, Sendable {
    case policy
    case sessionRevoked = "session_revoked"
    case unparseable
    case userAction = "user_action"
    case loginCancelled = "login_cancelled"
    case accountRefused = "account_refused"
}

enum IOSCursorSessionReport: Equatable, Sendable {
    case refreshFailed(IOSCursorSessionRefreshFailure)
    case refreshRecovered(consecutiveFailures: Int, degradedMs: Int)
    case rotationRescued
    case signedOut(cause: IOSCursorSessionSignoutCause, durable: Bool)
    case keychainUnavailable
}

enum IOSCursorSigninReport: Equatable, Sendable {
    case loginStarted
    case loginCompleted
    case loginFailed(cause: String)
    case signedOut(cause: String)
    case gate(String)
    case consult(String)
}

private let IOS_AUTH_CONSECUTIVE_FAILURES_CAP = 10_000
private let IOS_AUTH_DEGRADED_MS_BUCKET_CAP = 86_400_000
private let IOS_AUTH_DEGRADED_MS_BUCKET_CEILINGS = [
    5_000, 30_000, 60_000, 300_000, 1_800_000, 21_600_000, IOS_AUTH_DEGRADED_MS_BUCKET_CAP,
]

func iosAuthDegradedMsBucket(_ degradedMs: Int) -> Int {
    IOS_AUTH_DEGRADED_MS_BUCKET_CEILINGS.first(where: { degradedMs < $0 })
        ?? IOS_AUTH_DEGRADED_MS_BUCKET_CAP
}

private func iosAuthErrorTags(
    code: String,
    retryable: Bool,
    extra: [String: String] = [:]
) -> [String: String] {
    var tags = [
        "error_code": code,
        "error_domain": "auth",
        "error_retryable": String(retryable),
    ]
    tags.merge(extra) { _, incoming in incoming }
    return tags
}

func iosCursorSessionTelemetry(_ report: IOSCursorSessionReport) -> IOSAuthTelemetryProjection {
    switch report {
    case .refreshFailed(let failure):
        let tags: [String: String]
        switch failure {
        case .httpStatus(let status):
            tags = iosAuthErrorTags(
                code: "SAND-E0214",
                retryable: true,
                extra: ["http_status": String(status)]
            )
        case .network(let errno):
            tags = iosAuthErrorTags(
                code: "SAND-E0215",
                retryable: true,
                extra: ["errno": String(errno.prefix(64))]
            )
        case .badPayload:
            tags = iosAuthErrorTags(code: "SAND-E0216", retryable: true)
        }
        return .init(
            stream: .session,
            level: .warn,
            metadata: ["phase": "refresh_failed"].merging(tags) { _, incoming in incoming }
        )
    case .refreshRecovered(let consecutiveFailures, let degradedMs):
        return .init(
            stream: .session,
            level: .info,
            metadata: [
                "phase": "refresh_recovered",
                "consecutive_failures": String(min(consecutiveFailures, IOS_AUTH_CONSECUTIVE_FAILURES_CAP)),
                "degraded_ms": String(iosAuthDegradedMsBucket(degradedMs)),
            ]
        )
    case .rotationRescued:
        return .init(stream: .session, level: .info, metadata: ["phase": "rotation_rescued"])
    case .signedOut(let cause, let durable):
        var metadata = [
            "phase": "signed_out",
            "cause": cause.rawValue,
            "durable": String(durable),
        ]
        if cause == .policy {
            metadata.merge(iosAuthErrorTags(code: "SAND-E0218", retryable: false)) { _, incoming in incoming }
        } else if cause == .sessionRevoked || cause == .unparseable {
            metadata.merge(iosAuthErrorTags(code: "SAND-E0217", retryable: false)) { _, incoming in incoming }
        }
        return .init(
            stream: .session,
            level: (!durable || metadata["error_code"] != nil) ? .warn : .info,
            metadata: metadata
        )
    case .keychainUnavailable:
        return .init(
            stream: .session,
            level: .warn,
            metadata: ["phase": "keychain_unavailable"].merging(
                iosAuthErrorTags(code: "SAND-E0219", retryable: false)
            ) { _, incoming in incoming }
        )
    }
}

func iosCursorSigninTelemetry(_ report: IOSCursorSigninReport) -> IOSAuthTelemetryProjection {
    switch report {
    case .loginStarted:
        return .init(stream: .signin, level: .warn, metadata: ["phase": "login_started"])
    case .loginCompleted:
        return .init(stream: .signin, level: .warn, metadata: ["phase": "login_completed"])
    case .loginFailed(let cause):
        return .init(
            stream: .signin,
            level: .warn,
            metadata: ["phase": "login_failed", "cause": cause]
        )
    case .signedOut(let cause):
        return .init(
            stream: .signin,
            level: .warn,
            metadata: ["phase": "signed_out", "cause": cause]
        )
    case .gate(let gate):
        return .init(
            stream: .signin,
            level: .warn,
            metadata: ["phase": "boot_gate", "gate": gate]
        )
    case .consult(let outcome):
        return .init(
            stream: .signin,
            level: .warn,
            metadata: ["phase": "account_consult", "outcome": outcome]
        )
    }
}

@MainActor
final class IOSAuthTelemetryRelay {
    static let preAttachBufferLimit = 16
    typealias Sink = @MainActor (IOSAuthTelemetryProjection) -> Void

    private var sink: Sink?
    private var pending: [IOSAuthTelemetryProjection] = []

    func attach(_ sink: @escaping Sink) {
        self.sink = sink
        let buffered = pending
        pending.removeAll(keepingCapacity: true)
        buffered.forEach(sink)
    }

    func report(_ projection: IOSAuthTelemetryProjection) {
        guard let sink else {
            if pending.count < Self.preAttachBufferLimit {
                pending.append(projection)
            }
            return
        }
        sink(projection)
    }

    var pendingCount: Int { pending.count }
}
