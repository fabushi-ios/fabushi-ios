import Foundation

enum GatewayFailureCode: String, Sendable {
    case commandFailed = "gateway-command-failed"
    case unreachable = "gateway-unreachable"
    case transportFailed = "gateway-transport-failed"
}

struct GatewayCommandError: LocalizedError, Sendable {
    let message: String
    var errorDescription: String? { message }
}

struct GatewayUnreachableError: LocalizedError, Sendable {
    let transportKind: String
    let message: String
    var errorDescription: String? { message }
}

enum GatewayFailureMapper {
    static func failure(for error: Error) -> CoordinatorFailure {
        if let value = error as? GatewayCommandError {
            return .init(
                code: GatewayFailureCode.commandFailed.rawValue,
                message: value.message,
                details: ["boundary": .string("gateway")]
            )
        }
        if let value = error as? GatewayUnreachableError {
            return .init(
                code: GatewayFailureCode.unreachable.rawValue,
                message: value.message,
                transportKind: value.transportKind,
                details: [
                    "boundary": .string("gateway"),
                    "transportKind": .string(value.transportKind),
                ]
            )
        }
        if let outcome = GatewayReachability.classifyOutcome(error) {
            return .init(
                code: GatewayFailureCode.unreachable.rawValue,
                message: error.localizedDescription,
                transportKind: outcome.transportKind,
                details: [
                    "boundary": .string("gateway"),
                    "transportKind": .string(outcome.transportKind),
                ]
            )
        }
        return .init(
            code: GatewayFailureCode.transportFailed.rawValue,
            message: error.localizedDescription,
            details: ["boundary": .string("gateway")]
        )
    }
}
