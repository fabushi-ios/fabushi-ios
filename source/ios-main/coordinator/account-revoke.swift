import Foundation

struct CoordinatorAccountRevocationResult: Equatable, Sendable {
    let kind: String
    let status: IOSCursorAuthStatus
    let error: String?
}

@MainActor
final class ProductionCoordinatorAccountRevoker {
    typealias Revoke = @MainActor () async -> CoordinatorAccountRevocationResult

    private let revoke: Revoke

    init(revoke: @escaping Revoke) {
        self.revoke = revoke
    }

    func revokeRefusedAccount() async -> CoordinatorAccountRevocationResult {
        await revoke()
    }
}
