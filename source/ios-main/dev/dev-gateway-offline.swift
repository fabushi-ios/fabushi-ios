import Foundation

enum IOSDevGatewayOfflineError: LocalizedError, Equatable, Sendable {
    case induced

    var errorDescription: String? {
        "Gateway access is intentionally offline for iOS developer controls"
    }
}

/// MainActor serialization is the iOS-native replacement for Grok's promise
/// chain. The state is owned by iOS-main and therefore survives coordinator
/// port relaunches without inventing a desktop process boundary.
@MainActor
final class IOSDevGatewayOfflineControl {
    private(set) var isInduced = false

    @discardableResult
    func apply(_ next: Bool) -> Bool {
        isInduced = next
        return isInduced
    }

    @discardableResult
    func reapplyAfterCoordinatorLaunch() -> Bool {
        isInduced
    }

    func requireOnline() throws {
        if isInduced {
            throw IOSDevGatewayOfflineError.induced
        }
    }
}
