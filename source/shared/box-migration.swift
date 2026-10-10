import Foundation

struct SandBoxMigrationOperationId: Codable, Equatable, Sendable {
    let value: String
}

func parseSandBoxMigrationOperationId(_ value: Any) -> SandBoxMigrationOperationId? {
    guard let value = value as? String, !value.isEmpty else { return nil }
    return .init(value: value)
}

func isSameSandBoxMigrationOperation(
    _ left: SandBoxMigrationOperationId?,
    _ right: SandBoxMigrationOperationId?
) -> Bool {
    guard let left, let right else { return false }
    return left.value == right.value
}
